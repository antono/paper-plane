# End-to-end tests

The application is run for real (the Devel build, under Xvfb and a private
D-Bus session), with TDLib replaced by a scripted fake, and driven through
automation actions over D-Bus. The tests check both what the UI shows and what
the application asked "TDLib" for.

```sh
nix build .#checks.x86_64-linux.e2e    # or: nix flake check
```

The result holds, per test, the profile, `app.log`, the request log of the
fake (`requests.jsonl`) and the screenshots (`automation/*.png`).

## Pieces

| Path | What |
| --- | --- |
| `fake-tdjson/` | `libtdjson.so.<version>` that implements `td_create_client_id`, `td_send`, `td_receive` on top of a scenario. Built from the `tdlib-rs` types, so its objects follow the TL schema the app is built against. |
| `scenarios/*.json` | Users, chats, messages, the login mode and injected failures (`"fail": {"viewMessages": {...}}`). |
| `../../src/automation.rs` | GActions of the Devel profile: `auto-state`, `auto-list-chats`, `auto-open-chat`, `auto-send-text`, `auto-screenshot`, `auto-submit-phone`, `auto-submit-code`. |
| `run.py` | Starts the app with the fake through `LD_LIBRARY_PATH`, drives it with `gdbus` and asserts. |

Running it outside Nix, against a Devel build installed into a prefix:

```sh
meson setup _build -Dprofile=development -Dprefix=$PWD/_inst && meson install -C _build
glib-compile-schemas _inst/share/glib-2.0/schemas
(cd tests/e2e/fake-tdjson && cargo build)
GDK_BACKEND=x11 GSK_RENDERER=cairo dbus-run-session -- \
  xvfb-run -a -s "-screen 0 1280x800x24" python3 tests/e2e/run.py \
  --app _inst/bin/paper-plane \
  --fake-lib tests/e2e/fake-tdjson/target/debug/libtdjson.so --artifacts /tmp/e2e
```

`xvfb-run`'s default screen (8-bit) is not enough for GTK to draw; use 24-bit.

## The current tests

- **chat-list-open-and-send**: the chat list of the session, opening a chat
  loads its history, a screenshot is rendered, text typed into the composer is
  sent (`sendMessage`), and the pending message is replaced by the sent one;
  quitting closes TDLib.
- **login-with-phone-number**: phone number, a wrong code, the right code.
- **survives-unsupported-auth-state**: TDLib asks for a login email address,
  which the app does not implement (it aborted before vitlav's fix 74f246d0).
- **survives-failing-view-messages**: TDLib rejects `viewMessages`. This test
  found a crash (the result was unwrapped), fixed in consolidated@a8e73279.

## Why a fake libtdjson (investigation, 2026-11)

Options that were considered:

1. **Telegram's test data centers.** Real servers that accept synthetic
   accounts: phone numbers `99966XYYYY` (X = DC 1–3), the login code is the DC
   digit five times; the app supports them with `--test-dc`. Real protocol and
   TDLib, but needs the network (not in the Nix sandbox), accounts are shared
   and reset, and timing is not deterministic. Good for a manual or nightly
   smoke test, not for CI. The automation actions work against them too.
2. **An open-source MTProto server** posing as Telegram:
   teamgram-server (Go, a full server with its own service dependencies),
   [gramsrv](https://github.com/iamxvbaba/gramsrv) (says it supports a TDLib
   fork), [telesrv](https://telesrv.net/) (experimental). TDLib only talks to
   Telegram's DC addresses and RSA keys, so these need a patched TDLib, and the
   server has to keep up with TDLib's layer (227 for TDLib 1.8.65). Not tried,
   so whether stock or patched TDLib connects to any of them is unverified; a
   full server is also more than UI tests need.
3. **A fake libtdjson** (chosen). The app only uses three C functions of
   TDLib's JSON interface, so a library with the same SONAME replaces it
   through `LD_LIBRARY_PATH`. Deterministic, offline, runs in the Nix sandbox,
   and failures can be injected per request. It does not test TDLib itself,
   and the fake has to grow with the requests the app makes (unknown requests
   are answered with an error and marked `"unsupported"` in the request log).

For driving the UI, the automation bridge (GActions, adapted from Yacha's
modernization branch) acts on the application's objects. Input-level tests
(clicks, keys) would need the accessibility tree:
[WayDriver](https://discourse.gnome.org/t/waydriver-playwright-style-functional-testing-for-gtk-apps/34836)
(Rust, headless Mutter, Playwright-style) or
Dogtail (AT-SPI, Wayland support since 1.0).
Both run headless; GTK 4 registers its accessibility tree only on the session
bus, which WayDriver works around. A next step could combine them with this
fake.

## Next steps

- Scenarios for groups and channels, media (photos, albums, voice notes,
  stickers), reactions, edits and deletions.
- Replay a recorded `requests.jsonl` to check that a change does not alter
  what the app asks TDLib for.
- Input-level tests with WayDriver or Dogtail on top of the fake.
