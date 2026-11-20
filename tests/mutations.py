#!/usr/bin/env python3
"""Mutation check: do the tests catch it when a merged feature breaks?

For every feature merged from the forks, one mutation undoes its key code
(a small, compiling change that reverts part of the commit), and the test
listed for it in tests/COVERAGE.md must then fail. A mutation that leaves
all tests green means the test does not really exercise the feature.

    python3 tests/mutations.py --build _build [--e2e --app PREFIX/bin/paper-plane
        --fake-lib PATH/libtdjson.so] [-k FILTER]

Run it in the development shell (meson, cargo, and for --e2e: xvfb-run,
dbus-run-session, gdbus). Every file is restored afterwards.
"""

import argparse
import os
import re
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

# (commit, feature, file, original, mutated, expected failure)
# The expected failure is "unit:<test path>", "gtk" (ui::tests::gtk),
# "pack:<check>" (build-aux/check-packaging.py), "meson:<test>", "build"
# (the build must refuse it) or "e2e:<test name>". The commit field may
# name several commits that the mutation covers.
MUTATIONS = [
    ("f966e155", "voice notes: waveform", "src/ui/components/waveform.rs",
     "let decoded = glib::base64_decode(waveform);",
     "let decoded = waveform.as_bytes().to_vec();",
     "unit:ui::components::waveform::tests::decodes_base64_packed_samples"),
    ("f966e155", "voice notes: duration", "src/ui/session/content/message_row/voice.rs",
     'format!("{minutes:02}:{seconds:02}")', 'format!("{minutes}:{seconds}")',
     "unit:ui::session::content::message_row::voice::tests::formats_minutes_and_seconds"),
    ("18fb29ce", "reactions: model", "src/model/message_interaction_info.rs",
     ".map(|reactions| reactions.reactions.clone())", ".map(|_| Vec::new())",
     "unit:model::message_interaction_info::tests::reactions_of_new_messages_and_updates"),
    ("18fb29ce", "reactions: chip layout", "src/ui/session/content/message_row/reactions.rs",
     "height += row_height + SPACING;", "height += row_height;",
     "unit:ui::session::content::message_row::reactions::imp::tests::chips_wrap_to_new_rows"),
    ("18fb29ce", "reactions: picker", "src/ui/session/content/message_row/reactions_chooser.rs",
     "imp.flow_box.remove_all();", "",
     "gtk"),
    ("743880f7", "select and copy text: plain text", "src/ui/session/content/message_row/label.rs",
     '            .replace(OBJECT_REPLACEMENT_CHARACTER, "")\n',
     "            .replace('\\u{0}', \"\")\n",
     "gtk"),
    ("743880f7", "select and copy text: label lookup", "src/ui/session/content/message_row/mod.rs",
     "if let Some(label) = widget.downcast_ref::<ui::MessageLabel>() {",
     "if let Some(label) = widget.downcast_ref::<ui::MessageLabel>().filter(|_| false) {",
     "gtk"),
    ("16f612cb", "media viewer: entries", "src/ui/session/content/media_viewer.rs",
     "            if message.id() <= anchor_newest_id {\n                continue;",
     "            if message.id() <= anchor_newest_id || message.id() > 0 {\n                continue;",
     "unit:ui::session::content::media_viewer::tests::newer_entries_include_album_members"),
    ("2bfb174c", "video offloading", "src/ui/session/content/media_viewer_page.rs",
     ".set_graphics_offload(gtk::GraphicsOffloadEnabled::Enabled);",
     ".set_graphics_offload(gtk::GraphicsOffloadEnabled::Disabled);",
     "gtk"),
    ("3ddbaf12", "playback manager", "src/ui/session/playback_manager.rs",
     "        *self.0.current_path.borrow_mut() = None;\n\n        self.0.media_file.pause();",
     "\n        self.0.media_file.pause();",
     "gtk"),
    ("6a1fc081", "WebM stickers", "src/ui/components/webm_animation.rs",
     '"V_VP8" => Some(Self::Vp8),', '"V_VP8" => Some(Self::Vp9),',
     "unit:ui::components::webm_animation::tests::decodes_vp8"),
    ("7679e134", "media download settings", "src/model/media_manager.rs",
     "MediaType::Video => self.videos && size <= self.videos_max_size,",
     "MediaType::Video => self.videos,",
     "unit:model::media_manager::tests::size_limits_are_inclusive"),
    ("5418fc72", "running in the background", "src/application.rs",
     "            *hold = Some(self.hold());\n", "",
     "gtk"),
    ("eaa45b1e", "media handling: albums", "src/model/chat_history_model.rs",
     "if album_id == 0 || !is_groupable {", "if album_id == 0 || is_groupable {",
     "unit:model::chat_history_model::tests::album_members_share_one_row"),
    ("eaa45b1e", "media handling: photo sizes", "src/utils.rs",
     'let type_ = if scale_factor > 1 { "y" } else { "x" };',
     'let type_ = if scale_factor > 1 { "x" } else { "y" };',
     "unit:utils::tests::photo_sizes_for_scale_factors"),
    ("eaa45b1e", "media handling: video rows", "src/ui/session/content/message_row/video.rs",
     '.bind(&*imp.message_bubble, "label", Some(message));',
     '.bind(&*imp.message_bubble, "label", None::<&model::Message>);',
     "gtk"),
    ("78359245", "message loading around the anchor", "src/model/chat_history_model.rs",
     "let anchor = if chat.unread_count() > 0 {", "let anchor = if chat.unread_count() < 0 {",
     "unit:model::chat_history_model::tests::unread_chats_are_anchored_at_the_last_read_message"),
    ("7c7afe80", "chat history without a patched GTK", "src/model/reversed_list_model.rs",
     "model.item(n_items - 1 - position)", "model.item(position)",
     "unit:model::reversed_list_model::tests::mirrors_changes_of_the_inner_model"),
    ("73138846", "test DC sessions apart", "src/model/client_manager.rs",
     ".filter(|database_info| database_info.use_test_dc == test_dc)",
     ".filter(|_| true)",
     "unit:model::client_manager::tests::test_dc_sessions_are_kept_apart"),
    ("74f246d0", "unsupported login states", "src/model/client_state_auth.rs",
     'log::error!("Unsupported state during auth: {other:?}");',
     'unreachable!("Unsupported state during auth: {other:?}");',
     "unit:model::client_state_auth::tests::unsupported_states_keep_the_current_page"),
    ("e37f1b81", "client view on closing", "src/ui/client_view.rs",
     "adw::Spinner::new().upcast::<gtk::Widget>()", "panic!()",
     "gtk"),
    ("91b84d0e", "Replies chat", "src/expressions.rs",
     '} else if chat.is_replies_chat() {\n                gettext("Replies")',
     '} else if false {\n                gettext("Replies")',
     "gtk"),
    ("6c79d201", "secret chat style", "src/ui/session/sidebar/row.rs",
     "        self.update_secret_chat_style();\n", "",
     "gtk"),
    ("438e5f0c", "MediaDownloadButton registered", "src/ui/mod.rs",
     "    MediaDownloadButton::static_type();\n", "",
     "gtk"),
    # Packaging
    ("d61f344a", "doap syntax", "PaperPlane.doap",
     "</Project>", "", "pack:doap is well formed"),
    ("d7cfd284", "old GL renderer in the Flatpak", "build-aux/app.drey.PaperPlane.Devel.json",
     '"--env=GSK_RENDERER=gl",', '', "pack:flatpak uses the old GL renderer"),
    ("55153962", "GNOME 50 runtime", "build-aux/app.drey.PaperPlane.Devel.json",
     '"runtime-version": "50",', '"runtime-version": "49",', "pack:flatpak runtime is GNOME 50"),
    ("8261f0c9", "Flatpak uses the GTK of the runtime", "build-aux/app.drey.PaperPlane.Devel.json",
     '"modules": [', '"modules": [{"name": "gtk", "buildsystem": "meson", "sources": []},',
     "pack:flatpak uses the GTK of the runtime"),
    ("bf97c861", "TDLib built in the Flatpak", "build-aux/app.drey.PaperPlane.Devel.json",
     '"name": "tdlib",', '"name": "tdlib-gone",',
     "pack:flatpak builds TDLib from source, found with pkg-config"),
    ("aef472b4", "CI image of the runtime", ".github/workflows/ci.yml",
     "flatpak-github-actions:gnome-50", "flatpak-github-actions:gnome-47",
     "pack:CI builds with the image of the Flatpak runtime"),
    ("6f2be144", "dependency minimums", "meson.build",
     "dependency('gtk4', version: '>= 4.16')", "dependency('gtk4', version: '>= 4.12')",
     "pack:meson requires the library versions the Rust bindings use"),
    ("2a3fff37", "new clone! syntax", "src/model/message.rs",
     "    pub(crate) fn toggle_reaction(", "    // clone!(@weak obj => move || {})\n    pub(crate) fn toggle_reaction(",
     "pack:no clone! of the old syntax"),
    ("b6522213", "removed autoplay setting", "data/app.drey.PaperPlane.gschema.xml.in",
     '    <key name="run-in-background" type="b">',
     '    <key name="autoplay-videos" type="b"><default>false</default></key>\n    <key name="run-in-background" type="b">',
     "pack:the removed autoplay setting left nothing behind"),
    ("c262fb70 7d2956af 3e9f575c 1f2840ca", "Session is an Adw.Bin (caught by the template test)",
     "src/ui/session/mod.blp", "template $Session : Adw.Bin {", "template $Session : Gtk.Box {",
     "gtk"),
    ("c1eecd89", "video playback in the media viewer", "src/ui/session/playback_manager.rs",
     "        media.set_filename(Some(path));\n", "",
     "gtk"),
    ("18721f28 788ffd24 b9295d01 e29b724f", "login links are buttons", "src/ui/login/phone_number.blp",
     '          Button {\n            margin-top: 12;\n            action-name: "login.phone-number.use-qr-code";\n'
     '            label: _("Quick login using QR code");\n            styles ["link"]\n',
     '          LinkButton {\n            margin-top: 12;\n            action-name: "login.phone-number.use-qr-code";\n'
     '            label: _("Quick login using QR code");\n',
     "meson:validate-ui"),
    ("52b9fba2", "merge: secret chat property in the history", "src/ui/session/content/chat_history.rs",
     "self.set_is_secret_chat(matches!(chat.chat_type(), model::ChatType::Secret(_)));",
     "self.set_is_secret_chat(false);",
     "gtk"),
    ("ffae468a dc8fb1cb", "API generated from the schema of the system TDLib", "tdlib-rs/build.rs",
     'let file = format!("tl/api_{version}.tl");', 'let file = format!("tl/api_{version}.missing.tl");',
     "build"),
    ("c7e6d966 6a322b1f 36d4c5ef 4bbbf7ec", "built against TDLib 1.8.65", "meson.build",
     "dependency('tdjson', version: '>= 1.8.65')", "dependency('tdjson', version: '>= 1.8.66')",
     "build"),
    # End to end
    ("1e1e3ad7 eabdee5b ecdbe0b9 3430fb73 71583b69", "tdlib-rs requests and responses",
     "tdlib-rs/src/lib.rs",
     '    request["@extra"] = serde_json::to_value(extra).unwrap();\n', "",
     "e2e:chat-list-open-and-send"),
    ("78359245", "message loading around the anchor (e2e)", "src/model/chat_history_model.rs",
     "let anchor = if chat.unread_count() > 0 {", "let anchor = if chat.unread_count() < 0 {",
     "e2e:chat-list-open-and-send"),
    ("38bee205", "close TDLib before quitting", "src/application.rs",
     "                app.main_window()\n                    .client_manager_view()\n"
     "                    .client_manager()\n                    .close_all();\n",
     "",
     "e2e:chat-list-open-and-send"),
    ("74f246d0", "unsupported login states (e2e)", "src/model/client_state_auth.rs",
     'log::error!("Unsupported state during auth: {other:?}");',
     'unreachable!("Unsupported state during auth: {other:?}");',
     "e2e:survives-unsupported-auth-state"),
    ("a8e73279", "failing viewMessages (e2e)", "src/ui/session/content/chat_history.rs",
     'log::warn!("Could not mark messages of chat {chat_id} as viewed: {e:?}");',
     'panic!("Could not mark messages of chat {chat_id} as viewed: {e:?}");',
     "e2e:survives-failing-view-messages"),
]


def run(cmd, **kwargs):
    return subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, **kwargs)


def pack_failure(build, check):
    run(["meson", "test", "-C", build, "check-packaging"])
    log = (pathlib.Path(build) / "meson-logs/testlog.txt").read_text(errors="replace")
    if f"ok {check}" in log:
        return False, log
    if f"FAIL {check}" in log:
        return True, log
    return None, log


def meson_failure(build, test):
    run(["meson", "test", "-C", build, test])
    log = (pathlib.Path(build) / "meson-logs/testlog.txt").read_text(errors="replace")
    if f":{test}\n" not in log:
        return None, log
    passed = re.search(rf"^test:\s+.*:{re.escape(test)}\n(?:.*\n){{0,3}}?result:\s+exit status 0$", log, re.M)
    return passed is None, log


def build_refused(build):
    result = run(["meson", "compile", "-C", build])
    return result.returncode != 0, result.stdout + result.stderr


def unit_failures(build):
    result = run(["meson", "test", "-C", build, "cargo-test"])
    log = (pathlib.Path(build) / "meson-logs/testlog.txt").read_text(errors="replace")
    if "error[E" in log or "could not compile" in log:
        return None, log
    return result.returncode != 0, log


def e2e_failure(args, name):
    install = run(["meson", "install", "-C", args.build])
    if install.returncode != 0:
        return None, install.stdout + install.stderr
    env = dict(os.environ, GDK_BACKEND="x11", GSK_RENDERER="cairo")
    result = run(
        ["dbus-run-session", "--", "xvfb-run", "-a", "-s", "-screen 0 1280x800x24",
         sys.executable, "tests/e2e/run.py", "-k", name,
         "--app", args.app, "--fake-lib", args.fake_lib],
        env=env,
    )
    if f"FAIL {name}" in result.stdout:
        return True, result.stdout
    if f"ok   {name}" in result.stdout:
        return False, result.stdout
    # Neither: the test harness itself broke, which proves nothing.
    print(result.stdout[-2000:], result.stderr[-2000:])
    return None, result.stdout


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--build", required=True, help="the Meson build directory")
    parser.add_argument("--e2e", action="store_true", help="also run the end-to-end mutations")
    parser.add_argument("--app", help="the installed Devel binary (for --e2e)")
    parser.add_argument("--fake-lib", help="the fake libtdjson.so (for --e2e)")
    parser.add_argument("-k", help="only mutations whose feature contains this")
    args = parser.parse_args()

    survived = []
    for commit, feature, path, original, mutated, expected in MUTATIONS:
        if args.k and args.k not in feature:
            continue
        is_e2e = expected.startswith("e2e:")
        if is_e2e and not args.e2e:
            continue
        file = ROOT / path
        source = file.read_text()
        if source.count(original) != 1:
            survived.append(f"{commit} {feature}: the code to mutate is not in {path}")
            print(f"STALE  {commit} {feature}")
            continue
        try:
            file.write_text(source.replace(original, mutated))
            if is_e2e:
                failed, log = e2e_failure(args, expected[4:])
                detail = expected
            elif expected.startswith("meson:"):
                failed, log = meson_failure(args.build, expected[6:])
                detail = expected
            elif expected == "build":
                failed, log = build_refused(args.build)
                detail = "the build"
            elif expected.startswith("pack:"):
                failed, log = pack_failure(args.build, expected[5:])
                detail = expected
            else:
                failed, log = unit_failures(args.build)
                # The expected test must not have passed. A panic inside a
                # GTK callback aborts the whole test process, so there is no
                # "FAILED" line then, only the missing "ok".
                test = "ui::tests::gtk" if expected == "gtk" else expected[5:]
                if failed:
                    failed = f"test {test} ... ok" not in log
                detail = expected
        finally:
            file.write_text(source)
        if failed is None:
            survived.append(f"{commit} {feature}: the mutation does not build or the tests broke")
            print(f"BROKEN {commit} {feature}")
        elif failed:
            print(f"caught {commit} {feature}: {detail} failed")
        else:
            survived.append(f"{commit} {feature}: {detail} still passes")
            print(f"MISSED {commit} {feature}: {detail} still passes")

    # Leave the build as the sources are.
    if args.e2e:
        run(["meson", "install", "-C", args.build])
    print()
    if survived:
        print("\n".join(survived))
        return 1
    print("every mutation was caught")
    return 0


if __name__ == "__main__":
    sys.exit(main())
