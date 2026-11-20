# What tests what

Every feature and fix merged into `consolidated` from the forks, and the tests
that exercise it. `tests/check_coverage.py` checks this table against the
logs of a test run: every listed test must have run and passed, and every
merged commit (except translations, CI and docs) must be listed.

That shows the tests ran, not that they test the feature. `tests/mutations.py`
does: for every feature it breaks the merged code (a small, compiling partial
revert of the commit) and requires the listed test to fail; every row of the
table has at least one mutation (check_coverage.py checks that too). On
2026-11-20 all 44 mutations (27 unit/GTK, 9 packaging, 1 validate-ui, 2 the
build refuses, 5 end-to-end) were caught.

Coverage is per behaviour, not per commit: an independent review on
2026-11-20 found behaviours of large commits that the first map missed. They
are covered now as far as listed below, and the rest is under "Known gaps".

glib catches panics inside futures spawned on a main context and only logs
them, so a GTK check does not fail on a panic in a spawned task. The
end-to-end tests do (they fail on "panicked" in the application log); GTK
checks call such code directly where they can.

Test kinds:

- `unit:<path>`: a Rust unit test (`cargo test` output, `test <path> ... ok`).
- `gtk:<line>`: a check inside the `ui::tests::gtk` test, which prints that line.
- `meson:<name>`: a Meson test (`<name> OK`).
- `e2e:<name>`: an end-to-end test of `tests/e2e/run.py` (`ok   <name>`).
- `fake:<name>`: a unit test of the fake TDLib (`tests/e2e/fake-tdjson`).
- `pack:<check>`: a check of `build-aux/check-packaging.py` (the Meson test
  `check-packaging`): the doap file, the Flatpak manifest, the CI image, the
  dependency minimums, leftovers of removed code.
- `removed:<pattern>`: not in the merged code any more (`git grep` finds nothing).

| Commits | Feature or fix | Tests |
| --- | --- | --- |
| `91b84d0e` | Replies chat (#467) | `gtk:chat names and senders in the Replies chat`, `unit:ui::components::avatar::tests::appearance_of_special_chats`, `gtk:message rows: live locations, albums, Replies avatars, Copy Text` |
| `afc9555f` `6c79d201` `9c1153d0` | Secret chat style (#625) | `gtk:secret chat style in the chat list`, `gtk:secret chat style in the chat history` |
| `d61f344a` | doap syntax (#572) | `pack:doap is well formed` |
| `d7cfd284` | Old GL renderer in the Flatpak (#631) | `pack:flatpak uses the old GL renderer` |
| `6f2be144` | Dependency updates (#644) | `pack:meson requires the library versions the Rust bindings use`, `meson:clippy` |
| `1e1e3ad7` `eabdee5b` | tdlib-rs and TDLib 1.8.61 | `fake:tests::every_object_parses_as_tdlib_types`, `e2e:chat-list-open-and-send` |
| `aef472b4` | Flatpak builder image in CI | `pack:CI builds with the image of the Flatpak runtime` |
| `c262fb70` `7d2956af` | Session inherits from Adw.Bin | `gtk:/app/drey/paper-plane/ui/session/mod.ui: Session` |
| `55153962` | GNOME 50 runtime | `pack:flatpak runtime is GNOME 50` |
| `7679e134` | Media download settings | `unit:model::media_manager::tests::per_kind_switches`, `unit:model::media_manager::tests::size_limits_are_inclusive`, `unit:model::media_manager::tests::master_switch_disables_everything`, `unit:model::media_manager::tests::settings_schema_has_the_auto_download_keys`, `unit:ui::session::content::message_row::file_status::tests::statuses_of_files`, `e2e:media-replies-reactions-and-permissions` |
| `5418fc72` | Running in the background | `gtk:running in the background`, `e2e:runs-in-the-background` |
| `b6522213` | Autoplay videos setting (removed again in `eaa45b1e`) | `removed:should_autoplay_videos`, `pack:the removed autoplay setting left nothing behind` |
| `78359245` | Message loading around the anchor | `unit:model::chat_history_model::tests::unread_chats_are_anchored_at_the_last_read_message`, `unit:model::chat_history_model::tests::new_messages_wait_while_not_at_the_newest`, `unit:model::chat_history_model::tests::new_messages_are_pushed_to_the_front`, `e2e:chat-list-open-and-send`, `e2e:sending-reveals-the-message`, `e2e:media-replies-reactions-and-permissions`, `gtk:delayed view report after closing the chat`, `gtk:WebM stickers in the sticker widget` |
| `eaa45b1e` | Media handling in chat (video rows, photo sizes, minithumbnails, albums) | `gtk:video messages`, `unit:utils::tests::photo_sizes_for_scale_factors`, `gtk:minithumbnail textures`, `unit:model::chat_history_model::tests::album_members_share_one_row`, `unit:ui::session::content::message_row::album_layout::tests::rows_fill_the_width_and_do_not_overlap`, `unit:model::media_album::tests::keeps_members_sorted_and_unique`, `gtk:message rows: live locations, albums, Replies avatars, Copy Text`, `gtk:chat list selection without chats` |
| `bf97c861` | Meson build fix (TDLib backend, history loading) | `pack:flatpak builds TDLib from source, found with pkg-config`, `unit:model::chat_history_model::tests::new_messages_wait_while_not_at_the_newest` |
| `16f612cb` | Media viewer | `gtk:media viewer: open an album, close`, `unit:ui::session::content::media_viewer::tests::newer_entries_include_album_members`, `unit:ui::session::content::media_viewer::tests::viewer_items_of_messages` |
| `3ddbaf12` | Playback manager | `gtk:playback manager` |
| `c1eecd89` | Video playback in the media viewer | `gtk:media viewer: open an album, close`, `gtk:media viewer page: video graphics offload`, `gtk:playback manager` |
| `f966e155` | Voice notes | `unit:ui::components::waveform::tests::decodes_base64_packed_samples`, `gtk:waveform widget`, `unit:ui::session::content::message_row::voice::tests::formats_minutes_and_seconds` |
| `2bfb174c` | Video offloading | `gtk:media viewer page: video graphics offload` |
| `6a1fc081` | WebM stickers with libvpx | `unit:ui::components::webm_animation::tests::decodes_vp9_with_alpha`, `unit:ui::components::webm_animation::tests::decodes_vp8`, `unit:ui::components::webm_animation::tests::non_webm_file_is_reported`, `gtk:WebM stickers in the sticker widget` |
| `18fb29ce` | Message reactions | `unit:model::message_interaction_info::tests::reactions_of_new_messages_and_updates`, `unit:ui::session::content::message_row::reactions::imp::tests::chips_wrap_to_new_rows`, `gtk:reaction picker`, `e2e:media-replies-reactions-and-permissions` |
| `743880f7` | Select and copy text | `gtk:message label text selection`, `gtk:message label lookup for Copy Text`, `gtk:message rows: live locations, albums, Replies avatars, Copy Text`, `unit:utils::tests::emoji_are_shown_in_colour` |
| `ecdbe0b9` `3430fb73` `ffae468a` `dc8fb1cb` `71583b69` | In-tree tdlib-rs, API from the system TDLib schema | `fake:tests::every_object_parses_as_tdlib_types`, `e2e:chat-list-open-and-send` |
| `c7e6d966` `6a322b1f` `36d4c5ef` `4bbbf7ec` | Port to TDLib 1.8.65 | `e2e:chat-list-open-and-send`, `e2e:login-with-phone-number`, `gtk:drafts in the composer`, `unit:strings::tests::live_locations_are_named` |
| `74f246d0` `345a4e1a` | Unsupported login states do not abort | `unit:model::client_state_auth::tests::unsupported_states_keep_the_current_page`, `e2e:survives-unsupported-auth-state` |
| `2a3fff37` | New clone! syntax | `pack:no clone! of the old syntax`, `meson:clippy` |
| `38bee205` `a32f9599` | Close the TDLib clients before quitting | `e2e:chat-list-open-and-send`, `e2e:login-with-phone-number` |
| `b62f8d7a` `e37f1b81` | Client view on logging out and closing | `gtk:client view while logging out and closing` |
| `73138846` `03905edf` | Test DC sessions apart | `unit:model::client_manager::tests::test_dc_sessions_are_kept_apart` |
| `9baeae5c` `7c7afe80` | Chat history without a patched GTK | `unit:model::reversed_list_model::tests::mirrors_changes_of_the_inner_model`, `e2e:chat-list-open-and-send` |
| `8261f0c9` `1217192b` | Flatpak uses the GTK of the runtime | `pack:flatpak uses the GTK of the runtime` |
| `3e9f575c` `1f2840ca` | Template test | `unit:ui::tests::gtk` |
| `18721f28` `788ffd24` | Login links as buttons | `meson:validate-ui` |
| `b9295d01` `e29b724f` | validate-ui reports link buttons | `meson:validate-ui` |
| `438e5f0c` | MediaDownloadButton registered | `gtk:/app/drey/paper-plane/ui/session/content/message_row/media_download_button.ui: PaplMediaDownloadButton` |
| `52b9fba2` | Merge resolution: secret chat property in next's history, Replies avatar, background vs. close | `gtk:secret chat style in the chat history`, `unit:ui::components::avatar::tests::appearance_of_special_chats`, `gtk:running in the background`, `e2e:chat-list-open-and-send` |

## Known gaps

Behaviour of the merged commits that no test exercises yet (from the
independent review, 2026-11-20):

- Notifications: sent at once, also for chats without a photo (5418fc72).
- The scroll-down button (`handle_scroll_down`, `jump_to_first_unread`,
  `go_to_newest`), jumping to a replied message (`jump-to-message`,
  `new_for_message`) and loading older messages on scroll (78359245).
- The media viewer's paging (`search_media_messages`, `maybe_fetch_older`),
  navigation, opening it from a tile, and video playback controls in a page
  (16f612cb, c1eecd89); inline playback in `media_video_tile` (eaa45b1e).
- Downloading on click when auto-download is off (7679e134); playing a voice
  note from its row (f966e155).
- `chat_history_row` marking every album member as viewed (eaa45b1e); the
  message cache's weak references and `from_cache` deletions (78359245).
- `libc::_exit` after the application run (7679e134): the fake TDLib cannot
  reproduce the hang it avoids.
- Ported TDLib 1.8.65 calls without a scenario: search_chats and
  search_public_chats, sending photos and documents, `reply_to` on sending,
  the sponsored message label, `register_user` (c7e6d966, 1e1e3ad7).
- The Replies chat branches of the bubble, the sidebar sender label and the
  mute bar, and its test-DC id 708513 (91b84d0e).
- Shutdown races: futures that outlive the session and call `session_()`
  (an `unwrap`) crash on quit. The ones the end-to-end tests hit are fixed
  (chat history loading, chat list paging); about 15 other calls in
  src/model are not checked.
