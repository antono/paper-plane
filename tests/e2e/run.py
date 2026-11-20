#!/usr/bin/env python3
"""End-to-end tests of Paper Plane against the fake TDLib.

Runs the Devel build of the application with libtdjson replaced by
tests/e2e/fake-tdjson (through LD_LIBRARY_PATH), drives it through the
automation actions (src/automation.rs) over D-Bus and checks both what the
UI shows (state.txt, chats.txt, screenshots) and what the application asked
"TDLib" for (the request log of the fake).

It needs a display and a session bus, so run it through run.sh, or as

    dbus-run-session -- xvfb-run -a python3 tests/e2e/run.py \
        --app PREFIX/bin/paper-plane --fake-lib PATH/libtdjson.so [--artifacts DIR]
"""

import argparse
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time

APP_ID = "app.drey.PaperPlane.Devel"
OBJECT_PATH = "/app/drey/PaperPlane/Devel"
SCENARIOS = pathlib.Path(__file__).resolve().parent / "scenarios"


class Failure(Exception):
    pass


class App:
    """A running instance of the application with a fresh profile."""

    def __init__(self, args, scenario, name):
        self.args = args
        self.name = name
        self.root = pathlib.Path(tempfile.mkdtemp(prefix=f"pp-e2e-{name}-"))
        self.out = self.root / "automation"
        self.request_log = self.root / "requests.jsonl"
        # The dynamic loader looks the library up by the file name the
        # application was linked with, libtdjson.so.<TDLib version>.
        lib_dir = self.root / "lib"
        lib_dir.mkdir()
        shutil.copy(args.fake_lib, lib_dir / f"libtdjson.so.{args.tdlib_version}")
        env = dict(os.environ)
        for var, sub in [
            ("HOME", "home"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_CACHE_HOME", "cache"),
        ]:
            (self.root / sub).mkdir()
            env[var] = str(self.root / sub)
        env.update(
            {
                "PAPER_PLANE_AUTOMATION_DIR": str(self.out),
                "FAKE_TDJSON_SCENARIO": str(scenario),
                "FAKE_TDJSON_LOG": str(self.request_log),
                "LD_LIBRARY_PATH": os.pathsep.join(
                    filter(None, [str(lib_dir), env.get("LD_LIBRARY_PATH")])
                ),
                "GSETTINGS_BACKEND": "memory",
                "GTK_A11Y": "none",
                "RUST_BACKTRACE": "1",
                "RUST_LOG": env.get("RUST_LOG", "info"),
            }
        )
        # A Devel build installed into a prefix (not wrapped by Nix) finds
        # its compiled schema there.
        schemas = pathlib.Path(args.app).resolve().parent.parent / "share/glib-2.0/schemas"
        if (schemas / "gschemas.compiled").exists():
            env.setdefault("GSETTINGS_SCHEMA_DIR", str(schemas))
        self.stderr_path = self.root / "app.log"
        self.stderr = open(self.stderr_path, "w")
        self.process = subprocess.Popen(
            [args.app], env=env, stdout=self.stderr, stderr=subprocess.STDOUT
        )

    # --- driving -----------------------------------------------------------

    def action(self, name, parameter=None):
        params = "[]" if parameter is None else f"[<{parameter}>]"
        result = subprocess.run(
            [
                "gdbus", "call", "--session", "--dest", APP_ID,
                "--object-path", OBJECT_PATH,
                "--method", "org.gtk.Actions.Activate", name, params, "{}",
            ],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise Failure(f"activating {name} failed: {result.stderr.strip()}")

    def wait_for(self, what, check, timeout=20):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            self.assert_alive()
            try:
                last = check()
                if last:
                    return last
            except (Failure, FileNotFoundError):
                pass
            time.sleep(0.2)
        raise Failure(f"timed out waiting for {what} (last: {last!r})")

    def artifact(self, name):
        return (self.out / name).read_text()

    def state(self):
        """Refreshes and parses state.txt."""
        path = self.out / "state.txt"
        path.unlink(missing_ok=True)
        self.action("auto-state")
        text = self.wait_for("state.txt", lambda: path.exists() and path.read_text(), 5)
        state = {"messages": []}
        for line in text.splitlines():
            fields = line.split("\t")
            if fields[0] == "message":
                state["messages"].append((int(fields[1]), fields[2]))
            else:
                state[fields[0]] = fields[1:]
        return state

    def screenshot(self, name):
        """Renders the window to <name>.png; retried, since a window that
        has not drawn a frame yet renders nothing."""
        path = self.out / f"{name}.png"

        def take():
            self.action("auto-screenshot", f"'{name}'")
            time.sleep(0.3)
            return path.exists() and path.read_bytes()

        return self.wait_for(f"the screenshot {name}", take)

    def assert_all_requests_supported(self):
        """Every request was handled by the fake, so the test exercised the
        normal path and not error handling by accident."""
        unsupported = sorted({r["request"]["@type"] for r in self.requests() if r.get("unsupported")})
        if unsupported:
            raise Failure(f"the fake does not implement {', '.join(unsupported)}")

    def requests(self, type_=None):
        if not self.request_log.exists():
            return []
        lines = [json.loads(l) for l in self.request_log.read_text().splitlines()]
        return [l for l in lines if type_ is None or l["request"].get("@type") == type_]

    # --- lifecycle ---------------------------------------------------------

    def assert_alive(self):
        if self.process.poll() is not None:
            raise Failure(f"the application exited with {self.process.returncode}")

    def wait_until_logged_in(self):
        # The D-Bus name shows up once the application is registered.
        self.wait_for("the application on the bus", self._registered)
        self.wait_for(
            "the session",
            lambda: self.state().get("client-state", [""])[0] == "ClientStateSession",
        )

    def _registered(self):
        try:
            self.action("auto-state")
            return True
        except Failure:
            return False

    def quit(self):
        """Quits through the application's own action and checks the exit."""
        self.assert_alive()
        self.action("quit")
        try:
            code = self.process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.process.send_signal(signal.SIGKILL)
            raise Failure("the application did not quit")
        if code != 0:
            raise Failure(f"the application quit with {code}")

    def close(self, keep_to=None):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        self.stderr.close()
        log = self.stderr_path.read_text(errors="replace")
        if keep_to:
            dest = pathlib.Path(keep_to) / self.name
            shutil.copytree(self.root, dest, dirs_exist_ok=True)
        shutil.rmtree(self.root, ignore_errors=True)
        return log


# --- the tests -------------------------------------------------------------


def test_chat_list_open_and_send(app):
    app.wait_until_logged_in()

    app.action("auto-list-chats")
    chats = app.wait_for("chats.txt", lambda: app.artifact("chats.txt"))
    rows = [line.split("\t") for line in chats.splitlines()]
    assert [r[:2] for r in rows] == [["1001", "Alice Example"], ["1002", "Bob Example"]], rows
    assert rows[0][2] == "1", f"Alice's chat has one unread message: {rows}"

    app.action("auto-open-chat", "int64 1001")
    state = app.wait_for(
        "the history of Alice's chat",
        lambda: (s := app.state()).get("open-chat", [None])[0] == "1001"
        and len(s["messages"]) == 3
        and s,
    )
    assert [m[0] for m in state["messages"]] == [3, 2, 1], state["messages"]
    assert app.requests("openChat"), "openChat was not requested"
    assert app.requests("getChatHistory"), "getChatHistory was not requested"

    png = app.screenshot("chat")
    assert png.startswith(b"\x89PNG"), "the screenshot is not a PNG"

    app.action("auto-send-text", "'Hello from the e2e test'")
    state = app.wait_for(
        "the sent message in the history",
        lambda: (s := app.state())
        and any(text == "Hello from the e2e test" and id_ >= 1 << 30 for id_, text in s["messages"])
        and s,
    )
    sent = app.requests("sendMessage")
    assert len(sent) == 1, sent
    assert sent[0]["request"]["input_message_content"]["text"]["text"] == "Hello from the e2e test"
    # The pending copy was replaced by the sent message.
    assert not any(id_ >= 1 << 40 for id_, _ in state["messages"]), state["messages"]

    app.quit()
    assert app.requests("close"), "TDLib was not closed on quit"
    app.assert_all_requests_supported()


def test_login_with_phone_number(app):
    app.wait_for("the application on the bus", app._registered)
    app.wait_for(
        "the login screen",
        lambda: app.state().get("client-state", [""])[0] == "ClientStateAuth",
    )
    app.action("auto-submit-phone", "'9996612345'")
    app.wait_for("the code request", lambda: app.requests("setAuthenticationPhoneNumber"))
    app.action("auto-submit-code", "'00000'")
    app.wait_for("the rejected code", lambda: app.requests("checkAuthenticationCode"))
    app.assert_alive()
    app.action("auto-submit-code", "'12345'")
    app.wait_for(
        "the session",
        lambda: app.state().get("client-state", [""])[0] == "ClientStateSession",
    )
    app.quit()
    app.assert_all_requests_supported()


def test_survives_failing_view_messages(app):
    """TDLib may reject viewMessages; opening an unread chat must not crash."""
    app.wait_until_logged_in()
    app.action("auto-open-chat", "int64 1001")
    app.wait_for("the viewMessages request", lambda: app.requests("viewMessages"), 30)
    time.sleep(1)
    app.assert_alive()
    app.quit()


TESTS = [
    ("chat-list-open-and-send", "two-chats.json", test_chat_list_open_and_send),
    ("login-with-phone-number", "login.json", test_login_with_phone_number),
    ("survives-failing-view-messages", "view-messages-fails.json", test_survives_failing_view_messages),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--app", required=True, help="the Devel paper-plane binary")
    parser.add_argument("--fake-lib", required=True, help="the fake libtdjson.so")
    parser.add_argument("--tdlib-version", default="1.8.65", help="the TDLib the app links")
    parser.add_argument("--artifacts", help="keep profiles, logs and screenshots here")
    parser.add_argument("-k", help="only run tests whose name contains this")
    args = parser.parse_args()

    failed = []
    for name, scenario, test in TESTS:
        if args.k and args.k not in name:
            continue
        print(f"=== {name}", flush=True)
        app = App(args, SCENARIOS / scenario, name)
        error = None
        try:
            test(app)
        except (Failure, AssertionError) as e:
            error = e or "assertion failed"
        log = app.close(args.artifacts)
        panics = [l for l in log.splitlines() if "panicked" in l]
        if panics and not error:
            error = f"the application panicked: {panics[0]}"
        if error:
            failed.append(name)
            print(f"FAIL {name}: {error}")
            print("--- application log (tail)")
            print("\n".join(log.splitlines()[-40:]))
        else:
            print(f"ok   {name}")

    total = sum(1 for n, _, _ in TESTS if not args.k or args.k in n)
    print(f"\n{total - len(failed)} passed, {len(failed)} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
