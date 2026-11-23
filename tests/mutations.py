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
    ("78359245", "history loads survive a dropped chat", "src/model/chat_history_model.rs",
     "self.imp().chat.upgrade().ok_or_else(|| {",
     "Some(self.imp().chat.upgrade().unwrap()).ok_or_else(|| {",
     "unit:model::chat_history_model::tests::loads_fail_without_a_panic_once_the_chat_is_gone"),
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
    # Found untested by an independent review, then covered
    ("6c79d201", "secret chat style on recycled rows", "src/ui/session/sidebar/row.rs",
     '                    title_label.remove_css_class("accent");\n', "",
     "gtk"),
    ("6a322b1f", "drafts restored in the composer", "src/ui/session/content/chat_action_bar.rs",
     ".set_formatted_text(Some(model::BoxedFormattedText(content.text)));",
     ".set_formatted_text(None);",
     "gtk"),
    ("eaa45b1e", "selection of an empty chat list", "src/ui/session/sidebar/selection.rs",
     "if n_items == 0 {", "if n_items == u32::MAX {",
     "gtk"),
    ("6a1fc081", "WebM stickers in the sticker widget", "src/ui/components/sticker.rs",
     "let animation = WebmAnimation::new(&path, looped);",
     "let animation: WebmAnimation = unimplemented!();",
     "gtk"),
    ("78359245", "stickers download when shown", "src/ui/components/sticker.rs",
     "        if !self.is_mapped() {\n            return;\n        }\n", "",
     "gtk"),
    ("6a322b1f", "live locations in message rows", "src/ui/session/content/message_row/mod.rs",
     "MessageLocation(_) | MessageLiveLocation(_) => {", "MessageLocation(_) => {",
     "gtk"),
    ("6a322b1f", "live location name", "src/strings.rs",
     'MessageLiveLocation(_) => gettext("Live Location"),',
     'MessageLiveLocation(_) => gettext("Location"),',
     "unit:strings::tests::live_locations_are_named"),
    ("eaa45b1e", "album rows", "src/ui/session/content/message_row/mod.rs",
     "if message_.media_album().is_some_and(|album| album.len() >= 2)",
     "if message_.media_album().is_some_and(|album| album.len() >= 3)",
     "gtk"),
    ("91b84d0e", "Replies chat avatars in rows", "src/ui/session/content/message_row/mod.rs",
     "            } else if message.chat_().is_own_chat() || message.chat_().is_replies_chat() {\n"
     "                message.forward_info().is_some()",
     "            } else if message.chat_().is_own_chat() {\n"
     "                message.forward_info().is_some()",
     "gtk"),
    ("743880f7", "Copy Text action", "src/ui/session/content/message_row/mod.rs",
     "        self.clipboard().set_text(&text);\n", "",
     "gtk"),
    ("743880f7", "emoji in colour", "src/utils.rs",
     'format!("{emoji}\\u{FE0F}")', 'emoji.to_string()',
     "unit:utils::tests::emoji_are_shown_in_colour"),
    # Bugs found by the end-to-end tests
    ("78359245", "the newest loaded message ends the newer loading",
     "src/model/chat_history_model.rs",
     "last_message_id.is_some_and(|last| loaded.into_iter().any(|id| id >= last))",
     "last_message_id.is_some_and(|last| loaded.into_iter().any(|id| id > last))",
     "unit:model::chat_history_model::tests::loading_the_last_message_reaches_the_newest"),
    ("78359245", "delayed view report without a chat", "src/ui/session/content/chat_history.rs",
     "            let Some(chat) = self.chat() else {\n                return;\n            };",
     "            let chat = self.chat().unwrap();",
     "gtk"),
    ("78359245", "sending while not at the newest shows the newest", "src/ui/session/content/chat_history.rs",
     "                            obj.go_to_newest();\n", "",
     "gtk"),
    ("78359245", "go_to_newest goes to the newest", "src/ui/session/content/chat_history.rs",
     "let model = model::ChatHistoryModel::new_at_newest(&chat);",
     "let model = model::ChatHistoryModel::new(&chat);",
     "gtk"),
    ("78359245", "chat requests after the session is gone", "src/model/chat.rs",
     "        self.session()\n            .and_then(|session| session.client())",
     "        Some(self.session_())\n            .and_then(|session| session.client())",
     "unit:model::chat::tests::requests_after_the_session_is_gone_fail"),
    ("9baeae5c", "a model row is not added twice", "src/model/chat_history_model.rs",
     "        if is_row {\n            return;\n        }\n", "",
     "unit:model::chat_history_model::tests::a_message_already_in_the_list_is_not_added_again"),
    # End to end, found untested by the review
    ("9baeae5c", "opening loads a page, not the whole history (e2e)", "src/ui/session/content/chat_history.rs",
     "                                if adj.upper() > adj.page_size() * 1.5 {\n                                    break;",
     "                                if adj.upper() < 0.0 {\n                                    break;",
     "e2e:long-chat-opens-at-the-bottom"),
    ("7679e134", "downloads reach the waiting widgets (e2e)", "src/model/client_state_session.rs",
     ".retain(|sender| sender.send_blocking(file.clone()).is_ok());",
     ".retain(|_| false);",
     "e2e:media-replies-reactions-and-permissions"),
    ("78359245", "reply targets (e2e)", "src/model/chat.rs",
     "                replied_messages.insert((chat_id, message_id), message.clone());\n                Ok(message)\n",
     "                replied_messages.insert((chat_id, message_id), message.clone());\n                drop(message);\n                Ok(None)\n",
     "e2e:media-replies-reactions-and-permissions"),
    ("18fb29ce", "toggling a reaction (e2e)", "src/model/message.rs",
     "let result = if is_chosen {", "let result = if false {",
     "e2e:media-replies-reactions-and-permissions"),
    ("78359245", "message properties fetched for the menu (e2e)", "src/model/message.rs",
     "        self.imp().properties_fetched.set(true);\n",
     "        self.imp().properties_fetched.set(true);\n        if self.id() != 0 {\n            return;\n        }\n",
     "e2e:media-replies-reactions-and-permissions"),
    ("5418fc72", "closing the window runs in the background (e2e)", "src/ui/window.rs",
     'if self.settings.boolean("run-in-background")', "if false",
     "e2e:runs-in-the-background"),
    ("5418fc72", "presenting the window reopens the chat (e2e)", "src/application.rs",
     "        window.client_manager_view().set_chats_open(true);\n", "",
     "e2e:runs-in-the-background"),
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
    # Known gaps of the review, closed on 2026-11-22
    ("5418fc72", "notifications are sent before the avatar is downloaded", "src/ui/window.rs",
     "        send(&notification_id, &notification);\n\n        let file_id = avatar_file.id;",
     "\n        let file_id = avatar_file.id;",
     "gtk"),
    ("91b84d0e", "Replies chat on the test DC", "src/model/chat.rs",
     "self.id() == 1271266957 || self.id() == 708513", "self.id() == 1271266957",
     "unit:model::chat::tests::replies_chat_on_both_data_centers"),
    ("91b84d0e", "mute bar of the Replies chat", "src/ui/session/content/chat_action_bar.rs",
     "                    } else if chat.is_replies_chat() {", "                    } else if false {",
     "gtk"),
    ("91b84d0e", "sender labels in the Replies chat", "src/ui/session/sidebar/row.rs",
     "    if message.chat_().is_own_chat() || message.chat_().is_replies_chat() {",
     "    if message.chat_().is_own_chat() {",
     "gtk"),
    ("91b84d0e", "bubble senders in the Replies chat", "src/ui/session/content/message_row/bubble.rs",
     "        } else if message.chat_().is_own_chat() || message.chat_().is_replies_chat() {",
     "        } else if message.chat_().is_own_chat() {",
     "gtk"),
    ("78359245", "scroll-down re-opens the history at the newest message",
     "src/ui/session/content/chat_history.rs",
     "        } else if !model.at_newest() {\n            self.go_to_newest();",
     "        } else if false {\n            self.go_to_newest();",
     "e2e:jump-to-message-and-back-to-the-newest"),
    ("78359245", "scroll-down returns to the first unread message",
     "src/ui/session/content/chat_history.rs",
     "if chat.unread_count() > 0 && chat.last_read_inbox_message_id() != 0 && !model.at_newest() {",
     "if false {",
     "e2e:scroll-down-jumps-to-the-first-unread"),
    ("78359245", "older messages are loaded on scroll", "src/ui/session/content/chat_history.rs",
     "let near_oldest = adj.value() < adj.page_size() * 2.0\n                        || adj.upper() <= adj.page_size() * 2.0;",
     "let near_oldest = false;",
     "e2e:scrolling-loads-older-messages"),
    ("78359245", "messages dropped from the TDLib cache are not deleted", "src/model/chat.rs",
     "                if data.from_cache {", "                if false {",
     "unit:model::chat::tests::messages_dropped_from_the_tdlib_cache_are_not_deleted"),
    ("78359245", "requests after quitting fail instead of panicking", "src/model/chat.rs",
     "toggle_chat_is_marked_as_unread(self.id(), true, self.client_id()?)",
     "toggle_chat_is_marked_as_unread(self.id(), true, self.session_().client_().id())",
     "unit:model::chat::tests::requests_after_the_session_is_gone_fail"),
    ("eaa45b1e", "album rows view all members", "src/ui/session/content/chat_history_row.rs",
     "Some(album) => album.messages().iter().map(|m| m.id()).collect(),",
     "Some(_) => vec![message.id()],",
     "unit:ui::session::content::chat_history_row::tests::album_rows_view_all_members"),
    ("eaa45b1e", "the first click plays an animation", "src/ui/session/content/message_row/media_video_tile.rs",
     "            self.maybe_prepare_media();\n            imp.pending_play.set(true);",
     "            imp.pending_play.set(true);\n            self.maybe_prepare_media();",
     "gtk"),
    ("16f612cb", "older media are prepended once", "src/ui/session/content/media_viewer.rs",
     ".filter(|message| message.id() < oldest_known_id)", ".filter(|_| true)",
     "gtk"),
    ("16f612cb", "navigation buttons of the settled page", "src/ui/session/content/media_viewer.rs",
     "        // page.\n        self.update_navigation(entry_index);\n", "        // page.\n",
     "gtk"),
    ("c1eecd89", "animations loop in the viewer", "src/ui/session/content/media_viewer_page.rs",
     "                    media.set_loop(is_animation);", "                    media.set_loop(false);",
     "gtk"),
    ("7679e134", "photos wait for a click without auto-download",
     "src/ui/session/content/message_row/media_loader.rs",
     "                if session\n                    .media_manager()\n                    .should_auto_download(media_type, size)",
     "                if true",
     "e2e:photos-download-on-click-without-auto-download"),
    ("f966e155", "a downloaded voice note plays", "src/ui/session/content/message_row/voice.rs",
     "                imp.play_when_downloaded.set(true);\n", "",
     "gtk"),
    ("c7e6d966", "public chats in the search", "src/ui/session/sidebar/search/mod.rs",
     "                list.append(&Section::new(SectionType::Global));\n", "",
     "e2e:search-finds-chats-contacts-and-public-chats"),
    ("c7e6d966", "registering with the typed name", "src/model/client_state_auth_registration.rs",
     "            first_name,\n            last_name,", "            last_name,\n            first_name,",
     "e2e:register-a-new-account"),
    ("c7e6d966", "replies are sent with their target", "src/ui/session/content/chat_action_bar.rs",
     "                    ChatActionBarState::Replying(id) => id,",
     "                    ChatActionBarState::Replying(_) => 0,",
     "e2e:replies-photos-and-files-are-sent"),
    ("1e1e3ad7", "photos and files are sent as such", "src/ui/session/content/send_media_window.rs",
     "        let content = if send_as_file {", "        let content = if !send_as_file {",
     "e2e:replies-photos-and-files-are-sent"),
    ("c7e6d966", "the sponsor names a sponsored message", "src/model/sponsored_message.rs",
     "let sponsor_label = if sponsored_message.sponsor.info.is_empty() {",
     "let sponsor_label = if true {",
     "gtk"),
    ("7679e134", "quitting skips the hanging teardown of libtdjson", "src/main.rs",
     "    unsafe { libc::_exit(exit_code.value()) }",
     "    std::process::exit(exit_code.value())",
     "e2e:quits-although-tdlib-hangs-at-exit"),
    # Zooming photos in the media viewer (new in consolidated)
    ("zoom", "the zoomed point stays under the pointer", "src/ui/session/content/media_viewer_page.rs",
     "imp.pending_scroll.set(Some((px * zoom - x, py * zoom - y)));",
     "imp.pending_scroll.set(Some((px * zoom, py * zoom)));",
     "gtk"),
    ("zoom", "the zoom is limited", "src/ui/session/content/media_viewer_page.rs",
     "zoom.clamp(1.0, MAX_ZOOM)", "zoom.max(1.0)",
     "gtk"),
    ("zoom", "a zoomed photo does not swipe", "src/ui/session/content/media_viewer.rs",
     "imp.carousel.set_interactive(zoom <= 1.0);", "imp.carousel.set_interactive(true);",
     "gtk"),
    ("zoom", "leaving a page resets its zoom", "src/ui/session/content/media_viewer_page.rs",
     "        imp.displayed.set(false);\n        self.set_zoom(1.0, None);\n\n",
     "        imp.displayed.set(false);\n\n",
     "gtk"),
    ("selection", "selection text: text view focus", "src/ui/session/content/message_row/label.rs",
     "text_view.set_focusable(false);", "text_view.set_focusable(true);",
     "gtk"),
    ("selection", "selection text: links of the text view", "src/ui/session/content/message_row/label.rs",
     'language.strip_prefix("x-link-")?', 'language.strip_prefix("x-lonk-")?',
     "gtk"),
    ("selection", "selection text: link lookup hits the first character", "src/ui/session/content/message_row/label.rs",
     "*start <= offset && offset < *end", "*start < offset && offset < *end",
     "gtk"),
    ("selection", "selection text: spacer line keeps the label height", "src/ui/session/content/message_row/label.rs",
     "buffer.insert(&mut buffer.end_iter(), &WORD_JOINER.to_string());", "let _ = WORD_JOINER;",
     "gtk"),
    ("selection", "selection text: only a single message", "src/ui/session/content/chat_history_row.rs",
     "selected && selection.is_exactly(&ids),", "selected,",
     "gtk"),
    ("selection", "forward: chunks of a request", "src/model/chat.rs",
     "ids.chunks(FORWARD_LIMIT)", "ids.chunks(FORWARD_LIMIT + 1)",
     "unit:model::chat::tests::forwarding_is_split_into_sorted_chunks"),
    ("selection", "forward: order of the forwarded messages", "src/model/chat.rs",
     "    ids.sort_unstable();\n    ids.chunks", "    ids.chunks",
     "unit:model::chat::tests::forwarding_is_split_into_sorted_chunks"),
    ("selection", "forward: chat picker search ignores case", "src/ui/session/content/chat_picker_dialog.rs",
     ".ignore_case(true)", ".ignore_case(false)",
     "gtk"),
    ("selection", "forward: chat picker pick emits the chat", "src/ui/session/content/chat_picker_dialog.rs",
     'self.emit_by_name::<()>("chat-picked", &[&chat]);', "let _ = &chat;",
     "gtk"),
    ("selection", "forward: action follows the selection", "src/ui/session/content/chat_history.rs",
     'self.action_set_enabled("chat-history.forward-selected", active);', 'self.action_set_enabled("chat-history.forward-selected", false);',
     "gtk"),
    ("selection", 'selection: toggle groups', 'src/ui/session/content/message_selection.rs',
     '                if all {\n                    set.remove(id);\n                } else {\n                    set.insert(*id);\n                }', '                let _ = all;\n                set.insert(*id);',
     'unit:ui::session::content::message_selection::tests::toggle_adds_and_removes_groups'),
    ("selection", 'selection: copy format', 'src/ui/session/content/message_selection.rs',
     'format!("[{time}] {sender}: {text}")', 'format!("{sender}: {text} [{time}]")',
     'unit:ui::session::content::message_selection::tests::formats_copied_lines'),
    ("selection", 'selection: album members', 'src/ui/session/content/chat_history.rs',
     'Some(album) if album.len() >= 2 => album.messages().iter().map(|m| m.id()).collect(),', 'Some(album) if album.len() >= 2 => vec![message_id],',
     'gtk'),
    ("selection", 'selection: header switch', 'src/ui/session/content/chat_history.rs',
     '.set_visible_child_name(if active { "selection" } else { "normal" });', '.set_visible_child_name("normal");',
     'gtk'),
    ("selection", 'selection: bottom bar switch', 'src/ui/session/content/chat_history.rs',
     '.set_visible_child_name(if active { "selection" } else { "composer" });', '.set_visible_child_name("composer");',
     'gtk'),
    ("selection", 'selection: reply only with one', 'src/ui/session/content/chat_history.rs',
     'imp.reply_selected_button.set_visible(n == 1);', 'imp.reply_selected_button.set_visible(n >= 1);',
     'gtk'),
    ("selection", 'selection: labels not selectable', 'src/ui/session/content/message_row/label.blp',
     '    selectable: false;', '    selectable: true;',
     'gtk'),
    ("selection", 'selection: deleted messages leave', 'src/ui/session/content/message_selection.rs',
     'move |_, message| obj.remove(message.id())', 'move |_, _| {}',
     'gtk'),
    ("selection", 'selection: links of the text view', 'src/ui/session/content/message_row/label.rs',
     'output.push_str(&format!("<span lang=\\"x-link-{}\\">", uris.len()));', 'output.push_str(&format!("<span lang=\\"x-lonk-{}\\">", uris.len()));',
     'unit:ui::session::content::message_row::label::tests::links_to_spans'),
    ("selection", 'selection: delete revoke flag', 'src/ui/session/content/chat_history.rs',
     'chat.delete_messages(ids, revoke)', 'chat.delete_messages(ids, !revoke)',
     'e2e:selection-delete'),
    ("selection", 'selection: delete for everyone needs all', 'src/ui/session/content/chat_history.rs',
     'messages.iter().all(|m| m.can_be_deleted_for_all_users()),', 'messages.iter().any(|m| m.can_be_deleted_for_all_users()),',
     'e2e:selection-delete'),
    ("selection", 'selection: forward from and to chats', 'src/model/chat.rs',
     '                self.id(),\n                None,\n                from.id(),\n', '                from.id(),\n                None,\n                self.id(),\n',
     'e2e:selection-forward'),
    ("selection", 'selection: forward as a copy', 'src/model/chat.rs',
     '                chunk,\n                None,\n                false,\n', '                chunk,\n                None,\n                true,\n',
     'e2e:selection-forward'),
    ("selection", 'Escape shortcut', 'src/ui/session/content/chat_history.rs',
     'gtk::ShortcutTrigger::parse_string("Escape"),', 'gtk::ShortcutTrigger::parse_string("Return"),',
     'gtk'),
    ("selection", 'forward refuses unforwardable messages', 'src/ui/session/content/chat_history.rs',
     '!messages.iter().all(|m| m.can_be_forwarded())', '!messages.iter().any(|m| m.can_be_forwarded())',
     'gtk'),
    ("selection", 'forward guard before the await', 'src/ui/session/content/chat_history.rs',
     '        imp.is_picking_chat.set(true);\n        self.update_selection_ui();\n\n        // The selection and the chat can change', '        // The selection and the chat can change',
     'gtk'),
    ("selection", 'reply needs a writable chat', 'src/ui/session/content/chat_history.rs',
     '        let can_reply = n == 1\n            && imp.selection.messages().first().is_some_and(|message| {\n                super::message_row::can_send_messages_in_chat(&message.chat_())\n            });', '        let can_reply = n == 1;',
     'gtk'),
    ("selection", 'delete dialog: for everyone needs all', 'src/ui/session/content/chat_history.rs',
     '    if for_all {\n        responses.push', '    if true {\n        responses.push',
     'unit:ui::session::content::chat_history::tests::delete_dialog_offers_what_all_messages_allow'),
    ("selection", 'delete dialog: everyone revokes', 'src/ui/session/content/chat_history.rs',
     '"everyone" => Some(true),', '"everyone" => Some(false),',
     'unit:ui::session::content::chat_history::tests::delete_responses_revoke_only_for_everyone'),
    ("selection", 'delete dialog: me does not revoke', 'src/ui/session/content/chat_history.rs',
     '"me" => Some(false),', '"me" => Some(true),',
     'unit:ui::session::content::chat_history::tests::delete_responses_revoke_only_for_everyone'),
    ("selection", 'delete dialog: snapshot guard', 'src/ui/session/content/chat_history.rs',
     '        if self.imp().selection.message_ids() != ids {', '        if false {',
     'gtk'),
    ("selection", 'row gestures: tap selects', 'src/ui/session/content/message_row/mod.rs',
     '        self.claim_selection_click();\n        self.select();', '        self.claim_selection_click();',
     'gtk'),
    ("selection", 'row gestures: tap claims on release', 'src/ui/session/content/message_row/mod.rs',
     '        // release.\n        self.claim_selection_click();', '        // release.',
     'gtk'),
    ("selection", 'row gestures: every touch tap toggles', 'src/ui/session/content/message_row/mod.rs',
     '    is_touch || n_press == 1', '    n_press == 1',
     'unit:ui::session::content::message_row::tests::taps_toggle_every_time'),
    ("selection", 'row gestures: no Select for a selected message', 'src/ui/session/content/message_row/mod.rs',
     '            self.message().is::<model::Message>()\n                && !(self.selection_active() && self.is_selected()),', '            self.message().is::<model::Message>(),',
     'gtk'),
    ("selection", 'row gestures: long press selects', 'src/ui/session/content/message_row/base.rs',
     'row.activate_action("message-row.select", None).unwrap();', 'self.show_message_menu(x as i32, y as i32);',
     'gtk'),
    ("selection", 'selection: links out of the way', 'src/ui/session/content/message_row/label.rs',
     'imp.label.set_can_target(!selecting);', 'imp.label.set_can_target(true);',
     'gtk'),
    ("selection", 'selection: links do not open', 'src/ui/session/content/message_row/label.rs',
     'if weak.upgrade().is_some_and(|obj| obj.is_selecting()) {', 'if false {',
     'gtk'),
    ("selection", 'selection: acts on known messages', 'src/ui/session/content/message_selection.rs',
     'self.messages().iter().map(|m| m.id()).collect()', 'self.ids()',
     'gtk'),
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
