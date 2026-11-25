//! Development-only UI automation bridge, used by the end-to-end tests.
//!
//! Registers `GAction`s on the [`Application`] on the `Devel` profile only.
//! `GApplication` exports its actions on the session bus (`org.gtk.Actions`),
//! so a driver activates them with `gdbus` at the object path of the
//! application id, `/app/drey/PaperPlane/Devel`:
//!
//! ```sh
//! gdbus call --session --dest app.drey.PaperPlane.Devel \
//!   --object-path /app/drey/PaperPlane/Devel \
//!   --method org.gtk.Actions.Activate auto-open-chat "[<int64 1001>]" '{}'
//! ```
//!
//! Actions that report something write a file under [`output_dir`]
//! (`$XDG_CACHE_HOME/paper-plane/automation`, or `PAPER_PLANE_AUTOMATION_DIR`).
//!
//! Adapted from the automation bridge of Yacha's modernization branch
//! (Yach0/paper-plane@58bc497a). Unlike there, sending text goes through the
//! composer of the open chat, so the UI path is what gets tested.

use adw::prelude::*;
use glib::clone;
use gtk::gio;
use gtk::glib;

use crate::application::Application;
use crate::config;
use crate::model;
use crate::types::ChatId;
use crate::types::ClientId;
use crate::ui;
use crate::utils;

/// Directory where automation artifacts are written.
pub(crate) fn output_dir() -> std::path::PathBuf {
    let dir = std::env::var_os("PAPER_PLANE_AUTOMATION_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            glib::user_cache_dir()
                .join("paper-plane")
                .join("automation")
        });
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("Could not create {}: {e:?}", dir.display());
    }
    dir
}

fn write_artifact(name: &str, contents: &[u8]) {
    let path = output_dir().join(name);
    // Written to a temporary file first, so a driver polling for the file
    // never reads it half-written.
    let tmp = path.with_extension("part");
    match std::fs::write(&tmp, contents).and_then(|_| std::fs::rename(&tmp, &path)) {
        Ok(()) => log::info!("automation: wrote {}", path.display()),
        Err(e) => log::warn!("automation: failed to write {}: {e:?}", path.display()),
    }
}

/// Calls `f` for `widget` and all its descendants.
fn walk(widget: &gtk::Widget, f: &mut dyn FnMut(&gtk::Widget)) {
    f(widget);
    let mut child = widget.first_child();
    while let Some(c) = child {
        walk(&c, f);
        child = c.next_sibling();
    }
}

/// The first descendant of `widget` of type `T`, depth first.
fn descendant<T: IsA<gtk::Widget>>(widget: &gtk::Widget) -> Option<T> {
    if let Some(found) = widget.downcast_ref::<T>() {
        return Some(found.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(found) = descendant::<T>(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// Every descendant of `widget` of type `T`.
fn descendants<T: IsA<gtk::Widget>>(widget: &gtk::Widget) -> Vec<T> {
    let mut found = Vec::new();
    if let Some(widget) = widget.downcast_ref::<T>() {
        found.push(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        found.extend(descendants::<T>(&c));
        child = c.next_sibling();
    }
    found
}

/// Renders a shown native widget (a popover) to `<output_dir>/<name>.png`.
fn render_widget(widget: &gtk::Widget, name: &str) {
    let Some(renderer) = widget.native().and_then(|native| native.renderer()) else {
        log::warn!("render: {name} is not realized");
        return;
    };
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, widget.width() as f64, widget.height() as f64);
    let Some(node) = snapshot.to_node() else {
        log::warn!("render: {name} draws nothing");
        return;
    };
    let texture = renderer.render_texture(&node, None);
    write_artifact(&format!("{name}.png"), &texture.save_to_png_bytes());
}

impl Application {
    /// Registers the automation actions. No-op on non-`Devel` builds.
    pub(crate) fn setup_automation(&self) {
        if config::PROFILE != "Devel" {
            return;
        }

        log::info!(
            "Automation bridge enabled; artifacts are written to {}",
            output_dir().display()
        );

        self.add_auto_action(
            "auto-screenshot",
            Some(glib::VariantTy::STRING),
            |app, p| {
                let name = p
                    .and_then(|p| p.get::<String>())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "screenshot".to_owned());
                app.auto_screenshot(&name);
            },
        );
        self.add_auto_action("auto-state", None, |app, _| app.auto_state());
        self.add_auto_action("auto-list-chats", None, |app, _| app.auto_list_chats());
        self.add_auto_action(
            "auto-open-chat",
            Some(glib::VariantTy::INT64),
            |app, p| match p.and_then(|p| p.get::<i64>()) {
                Some(chat_id) => app.auto_open_chat(chat_id),
                None => log::warn!("auto-open-chat: missing chat id"),
            },
        );
        self.add_auto_action(
            "auto-send-text",
            Some(glib::VariantTy::STRING),
            |app, p| match p.and_then(|p| p.get::<String>()) {
                Some(text) => app.auto_send_text(text),
                None => log::warn!("auto-send-text: missing text"),
            },
        );
        self.add_auto_action(
            "auto-submit-phone",
            Some(glib::VariantTy::STRING),
            |app, p| {
                if let Some(phone) = p.and_then(|p| p.get::<String>()) {
                    app.auto_submit_phone(phone);
                }
            },
        );
        self.add_auto_action(
            "auto-set-boolean",
            Some(glib::VariantTy::new("(sb)").unwrap()),
            |_, p| match p.and_then(|p| p.get::<(String, bool)>()) {
                Some((key, value)) => {
                    let settings = gio::Settings::new(config::APP_ID);
                    if let Err(e) = settings.set_boolean(&key, value) {
                        log::warn!("auto-set-boolean: {e}");
                    }
                }
                None => log::warn!("auto-set-boolean: missing parameters"),
            },
        );
        self.add_auto_action("auto-close-window", None, |app, _| {
            // As the window's close button does
            if let Some(window) = app.auto_window() {
                window.close();
            }
        });
        self.add_auto_action(
            "auto-message-menu",
            Some(glib::VariantTy::INT64),
            |app, p| match p.and_then(|p| p.get::<i64>()) {
                Some(id) => app.auto_message_menu(id),
                None => log::warn!("auto-message-menu: missing message id"),
            },
        );
        self.add_auto_action(
            "auto-react",
            Some(glib::VariantTy::new("(xs)").unwrap()),
            |app, p| match p.and_then(|p| p.get::<(i64, String)>()) {
                Some((message_id, emoji)) => match app.open_chat_message(message_id) {
                    Some(message) => message.toggle_reaction(model::reaction_type_from_key(&emoji)),
                    None => log::warn!("auto-react: message {message_id} is not loaded"),
                },
                None => log::warn!("auto-react: missing parameters"),
            },
        );
        self.add_auto_action(
            "auto-fetch-properties",
            Some(glib::VariantTy::INT64),
            |app, p| {
                match p
                    .and_then(|p| p.get::<i64>())
                    .and_then(|id| app.open_chat_message(id))
                {
                    // What opening the context menu of the message does
                    Some(message) => message.fetch_properties(),
                    None => log::warn!("auto-fetch-properties: message not loaded"),
                }
            },
        );
        self.add_auto_action(
            "auto-scroll",
            Some(glib::VariantTy::DOUBLE),
            |app, p| match p.and_then(|p| p.get::<f64>()) {
                Some(fraction) => app.auto_scroll(fraction),
                None => log::warn!("auto-scroll: missing fraction"),
            },
        );
        self.add_auto_action(
            "auto-history-action",
            Some(glib::VariantTy::new("(sx)").unwrap()),
            |app, p| match p.and_then(|p| p.get::<(String, i64)>()) {
                Some((name, message_id)) => app.auto_history_action(&name, message_id),
                None => log::warn!("auto-history-action: missing parameters"),
            },
        );
        self.add_auto_action(
            "auto-window-size",
            Some(glib::VariantTy::new("(ii)").unwrap()),
            |app, p| match p.and_then(|p| p.get::<(i32, i32)>()) {
                Some((width, height)) => app.auto_window_size(width, height),
                None => log::warn!("auto-window-size: missing parameters"),
            },
        );
        self.add_auto_action(
            "auto-info-activate",
            Some(glib::VariantTy::new("(si)").unwrap()),
            |app, p| match p.and_then(|p| p.get::<(String, i32)>()) {
                Some((page, index)) => app.auto_info_activate(&page, index),
                None => log::warn!("auto-info-activate: missing parameters"),
            },
        );
        self.add_auto_action("auto-copy-selected", None, |app, _| {
            app.auto_copy_selected()
        });
        self.add_auto_action(
            "auto-delete-selected",
            Some(glib::VariantTy::BOOLEAN),
            |app, p| match p.and_then(|p| p.get::<bool>()) {
                Some(revoke) => app.auto_delete_selected(revoke),
                None => log::warn!("auto-delete-selected: missing revoke flag"),
            },
        );
        self.add_auto_action(
            "auto-forward-selected",
            Some(glib::VariantTy::INT64),
            |app, p| match p.and_then(|p| p.get::<i64>()) {
                Some(chat_id) => app.auto_forward_selected(chat_id),
                None => log::warn!("auto-forward-selected: missing chat id"),
            },
        );
        self.add_auto_action("auto-delete-options", None, |app, _| {
            app.auto_delete_options()
        });
        self.add_auto_action("auto-download-media", None, |app, _| {
            app.auto_download_media()
        });
        self.add_auto_action("auto-search", Some(glib::VariantTy::STRING), |app, p| {
            if let Some(query) = p.and_then(|p| p.get::<String>()) {
                app.auto_search(&query);
            }
        });
        self.add_auto_action(
            "auto-send-file",
            Some(glib::VariantTy::new("(sb)").unwrap()),
            |app, p| match p.and_then(|p| p.get::<(String, bool)>()) {
                Some((path, as_file)) => app.auto_send_file(path, as_file),
                None => log::warn!("auto-send-file: missing parameters"),
            },
        );
        self.add_auto_action(
            "auto-submit-registration",
            Some(glib::VariantTy::new("(ss)").unwrap()),
            |app, p| match p.and_then(|p| p.get::<(String, String)>()) {
                Some((first, last)) => app.auto_submit_registration(first, last),
                None => log::warn!("auto-submit-registration: missing parameters"),
            },
        );
        self.add_auto_action(
            "auto-submit-code",
            Some(glib::VariantTy::STRING),
            |app, p| {
                if let Some(code) = p.and_then(|p| p.get::<String>()) {
                    app.auto_submit_code(code);
                }
            },
        );
    }

    fn add_auto_action(
        &self,
        name: &str,
        parameter_type: Option<&glib::VariantTy>,
        handler: impl Fn(&Application, Option<&glib::Variant>) + 'static,
    ) {
        let action = gio::SimpleAction::new(name, parameter_type);
        action.connect_activate(clone!(
            #[weak(rename_to = app)]
            self,
            move |_, param| handler(&app, param)
        ));
        self.add_action(&action);
    }

    /// The main window, also while it is hidden (running in the background).
    /// Resizes the window, e.g. to the size of a phone to check the
    /// adaptive layouts.
    fn auto_window_size(&self, width: i32, height: i32) {
        let Some(window) = self.auto_window() else {
            log::warn!("auto-window-size: no window");
            return;
        };
        window.set_default_size(width, height);
    }

    /// Activates the `index`th item of a tab (`media`, `files`, `links`,
    /// `voice`, `music`) of the open chat info dialog, as a click does.
    fn auto_info_activate(&self, page: &str, index: i32) {
        let Some(dialog) = self
            .auto_window()
            .and_then(|w| descendant::<ui::ChatInfoWindow>(w.upcast_ref()))
        else {
            log::warn!("auto-info-activate: no chat info dialog");
            return;
        };
        if !dialog.activate_shared_item(page, index) {
            log::warn!("auto-info-activate: no item {index} in {page}");
        }
    }

    fn auto_window(&self) -> Option<ui::Window> {
        self.windows()
            .into_iter()
            .find_map(|w| w.downcast::<ui::Window>().ok())
    }

    fn active_client(&self) -> Option<model::Client> {
        self.auto_window()?.client_manager_view().active_client()
    }

    fn active_client_id(&self) -> Option<ClientId> {
        self.active_client().map(|client| client.id())
    }

    /// The chat shown in the chat history, if any.
    fn open_chat(&self) -> Option<model::Chat> {
        self.auto_window()
            .and_then(|w| descendant::<ui::ChatHistory>(w.upcast_ref()))
            .and_then(|history| history.chat())
    }

    fn chat_history(&self) -> Option<ui::ChatHistory> {
        self.auto_window()
            .and_then(|w| descendant::<ui::ChatHistory>(w.upcast_ref()))
    }

    /// A loaded message of the open chat.
    fn open_chat_message(&self, id: i64) -> Option<model::Message> {
        self.open_chat()?.message(id)
    }

    fn active_session(&self) -> Option<model::ClientStateSession> {
        self.active_client()?
            .state()
            .and_downcast::<model::ClientStateSession>()
    }

    /// Opens the menu of a shown message of the open chat, as a tap does, and
    /// renders it to `<output_dir>/message-menu.png` once its reactions are
    /// in.
    fn auto_message_menu(&self, id: i64) {
        let Some(history) = self.chat_history() else {
            log::warn!("auto-message-menu: no chat history");
            return;
        };
        let row = descendants::<ui::MessageRow>(history.upcast_ref())
            .into_iter()
            .find(|row| {
                row.message()
                    .downcast_ref::<model::Message>()
                    .is_some_and(|message| message.id() == id)
            });
        let Some(base) = row.and_then(|row| descendant::<ui::MessageBase>(row.upcast_ref()))
        else {
            log::warn!("auto-message-menu: message {id} is not shown");
            return;
        };
        base.open_menu();

        let menu = history.message_menu().clone();
        let reactions = history.quick_reactions().clone();
        let mut tries = 0;
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            tries += 1;
            if !reactions.is_visible() && tries < 30 {
                return glib::ControlFlow::Continue;
            }
            render_widget(menu.upcast_ref(), "message-menu");
            menu.popdown();
            glib::ControlFlow::Break
        });
    }

    /// Renders the main window to `<output_dir>/<name>.png`, through the
    /// renderer of the window, so no compositor is needed.
    fn auto_screenshot(&self, name: &str) {
        let Some(window) = self.auto_window() else {
            log::warn!("auto-screenshot: no window");
            return;
        };
        let (width, height) = (window.width(), window.height());
        let Some(renderer) = window.native().and_then(|native| native.renderer()) else {
            log::warn!("auto-screenshot: window not realized");
            return;
        };

        // The paintable of a toplevel draws nothing, so its child is drawn.
        let Some(content) = window.child() else {
            log::warn!("auto-screenshot: the window is empty");
            return;
        };
        let paintable = gtk::WidgetPaintable::new(Some(&content));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, width as f64, height as f64);
        let Some(node) = snapshot.to_node() else {
            log::warn!("auto-screenshot: nothing to render");
            return;
        };
        let texture = renderer.render_texture(&node, None);
        write_artifact(&format!("{name}.png"), &texture.save_to_png_bytes());
    }

    /// Writes `<output_dir>/state.txt`: the state of the active client
    /// (`auth`, `session`, …), and the open chat with its loaded messages.
    fn auto_state(&self) {
        let mut out = String::new();
        let state = self
            .active_client()
            .and_then(|client| client.state())
            .map(|state| state.type_().name().to_owned())
            .unwrap_or_else(|| "none".to_owned());
        out.push_str(&format!("client-state\t{state}\n"));
        if let Some(window) = self.auto_window() {
            out.push_str(&format!(
                "window\t{}\t{}\t{}x{}\n",
                window.is_mapped(),
                window.child().is_some_and(|c| c.is_drawable()),
                window.width(),
                window.height()
            ));
        }

        let history = self
            .auto_window()
            .and_then(|w| descendant::<ui::ChatHistory>(w.upcast_ref()));
        if let Some(chat) = history.and_then(|h| h.chat()) {
            out.push_str(&format!("open-chat\t{}\t{}\n", chat.id(), chat.title()));
            if let Some(model) = chat.history() {
                out.push_str(&format!(
                    "history\t{}\t{}\n",
                    model.at_newest(),
                    model.anchor()
                ));
                for i in 0..model.n_items() {
                    let Some(item) = model.item(i).and_downcast::<model::ChatHistoryItem>() else {
                        continue;
                    };
                    if let Some(message) = item.message() {
                        out.push_str(&format!(
                            "message\t{}\t{}\n",
                            message.id(),
                            crate::strings::message_content(message)
                        ));
                        let reactions: Vec<String> = message
                            .interaction_info()
                            .reactions()
                            .0
                            .iter()
                            .map(|r| {
                                format!(
                                    "{}{}{}",
                                    model::reaction_key(&r.r#type),
                                    r.total_count,
                                    if r.is_chosen { "*" } else { "" }
                                )
                            })
                            .collect();
                        out.push_str(&format!(
                            "reactions\t{}\t{}\n",
                            message.id(),
                            reactions.join(",")
                        ));
                        out.push_str(&format!(
                            "can-edit\t{}\t{}\n",
                            message.id(),
                            message.can_be_edited()
                        ));
                    }
                }
            }
        }

        if let Some(history) = self.chat_history() {
            let ids: Vec<String> = history
                .selection()
                .ids()
                .iter()
                .map(|id| id.to_string())
                .collect();
            out.push_str(&format!(
                "selection\t{}\t{}\n",
                history.selection().active(),
                ids.join(",")
            ));
            out.push_str(&format!(
                "selection-title\t{}\n",
                history.selection_title_text()
            ));
            out.push_str(&format!(
                "bottom-bar\t{}\n",
                history.bottom_page().unwrap_or_default()
            ));
            let mut text_views = 0;
            walk(history.upcast_ref(), &mut |widget| {
                // The composer is an editable text view as well
                if widget
                    .downcast_ref::<gtk::TextView>()
                    .is_some_and(|view| !view.is_editable())
                {
                    text_views += 1;
                }
            });
            out.push_str(&format!("selected-text-view\t{text_views}\n"));
        }

        // What the chat history shows: the pictures with content and the
        // replies with the text of their target.
        if let Some(history) = self
            .auto_window()
            .and_then(|w| descendant::<ui::ChatHistory>(w.upcast_ref()))
        {
            let mut pictures = 0;
            let mut downloads = 0;
            let mut replies = vec![];
            walk(history.upcast_ref(), &mut |widget| {
                if widget
                    .downcast_ref::<ui::MediaPhotoTile>()
                    .is_some_and(|tile| tile.offers_download())
                {
                    downloads += 1;
                }
                if let Some(picture) = widget.downcast_ref::<gtk::Picture>() {
                    if picture.paintable().is_some() {
                        pictures += 1;
                    }
                }
                if widget.is::<ui::MessageReply>() {
                    let mut texts = vec![];
                    walk(widget, &mut |w| {
                        if let Some(label) = w.downcast_ref::<gtk::Label>() {
                            if !label.text().is_empty() {
                                texts.push(label.text().to_string());
                            }
                        }
                    });
                    replies.push(texts.join(" | "));
                }
            });
            out.push_str(&format!("pictures\t{pictures}\n"));
            out.push_str(&format!("download-offers\t{downloads}\n"));
            if let Some(scrolled) = descendant::<gtk::ScrolledWindow>(history.upcast_ref()) {
                let adj = scrolled.vadjustment();
                out.push_str(&format!(
                    "scroll\t{}\t{}\t{}\n",
                    adj.value(),
                    adj.page_size(),
                    adj.upper()
                ));
            }
            for reply in replies {
                out.push_str(&format!("reply\t{reply}\n"));
            }
        }
        if let Some(search) = self
            .auto_window()
            .and_then(|w| descendant::<ui::SidebarSearch>(w.upcast_ref()))
        {
            for name in search.result_names() {
                out.push_str(&format!("search\t{name}\n"));
            }
        }
        write_artifact("state.txt", out.as_bytes());
    }

    /// Scrolls the chat history to `fraction` of its range: 0 is the
    /// oldest loaded message, 1 the newest, as a user dragging it would.
    fn auto_scroll(&self, fraction: f64) {
        let Some(scrolled) = self
            .auto_window()
            .and_then(|w| descendant::<ui::ChatHistory>(w.upcast_ref()))
            .and_then(|history| descendant::<gtk::ScrolledWindow>(history.upcast_ref()))
        else {
            log::warn!("auto-scroll: no chat history");
            return;
        };
        let adj = scrolled.vadjustment();
        adj.set_value((adj.upper() - adj.page_size()) * fraction.clamp(0.0, 1.0));
    }

    /// Activates an action of the chat history, as its buttons do:
    /// `scroll-down` (the message id is ignored), `jump-to-message` or
    /// `reply`.
    fn auto_history_action(&self, name: &str, message_id: i64) {
        let Some(history) = self
            .auto_window()
            .and_then(|w| descendant::<ui::ChatHistory>(w.upcast_ref()))
        else {
            log::warn!("auto-history-action: no chat history");
            return;
        };
        let parameter = matches!(name, "jump-to-message" | "reply" | "toggle-selection")
            .then(|| message_id.to_variant());
        if let Err(e) = history.activate_action(&format!("chat-history.{name}"), parameter.as_ref())
        {
            log::warn!("auto-history-action: {e}");
        }
    }

    /// Copies the selection like the Copy button of the selection bar and
    /// writes the copied text to `copied.txt`.
    fn auto_copy_selected(&self) {
        match self.chat_history() {
            Some(history) => write_artifact("copied.txt", history.copy_selected().as_bytes()),
            None => log::warn!("auto-copy-selected: no chat history"),
        }
    }

    /// Deletes the selection as the delete dialog does after the choice.
    fn auto_delete_selected(&self, revoke: bool) {
        match self.chat_history() {
            Some(history) => history.delete_selected(revoke),
            None => log::warn!("auto-delete-selected: no chat history"),
        }
    }

    /// Forwards the selection to the chat, without the chat picker.
    fn auto_forward_selected(&self, chat_id: ChatId) {
        let (Some(history), Some(session)) = (self.chat_history(), self.active_session()) else {
            log::warn!("auto-forward-selected: no chat history");
            return;
        };
        let Some(chat) = session.try_chat(chat_id) else {
            log::warn!("auto-forward-selected: unknown chat {chat_id}");
            return;
        };
        utils::spawn(async move { history.forward_selected_to(&chat).await });
    }

    /// Writes what the delete dialog would offer for the selection to
    /// `delete-options.txt`.
    fn auto_delete_options(&self) {
        let Some(history) = self.chat_history() else {
            log::warn!("auto-delete-options: no chat history");
            return;
        };
        utils::spawn(async move {
            let (me, everyone) = history.delete_options().await;
            write_artifact(
                "delete-options.txt",
                format!("me\t{me}\neveryone\t{everyone}\n").as_bytes(),
            );
        });
    }

    /// Searches for `query` in the sidebar; the results are in state.txt.
    fn auto_search(&self, query: &str) {
        match self
            .auto_window()
            .and_then(|w| descendant::<ui::SidebarSearch>(w.upcast_ref()))
        {
            Some(search) => search.set_query(query),
            None => log::warn!("auto-search: no search"),
        }
    }

    /// Sends the file at `path` to the open chat as a photo, or as a file,
    /// through the window that the attach button opens.
    fn auto_send_file(&self, path: String, as_file: bool) {
        let (Some(window), Some(chat)) = (self.auto_window(), self.open_chat()) else {
            log::warn!("auto-send-file: no open chat");
            return;
        };
        let send_window = ui::SendMediaWindow::new(window.upcast_ref(), &chat, path, 0);
        utils::spawn(async move { send_window.send(as_file).await });
    }

    /// Registers the new account of the login with the given name.
    fn auto_submit_registration(&self, first_name: String, last_name: String) {
        let Some(registration) = self
            .active_client()
            .and_then(|client| client.state())
            .and_downcast::<model::ClientStateAuth>()
            .and_then(|auth| auth.state())
            .and_downcast::<model::ClientStateAuthRegistration>()
        else {
            log::warn!("auto-submit-registration: no registration");
            return;
        };
        utils::spawn(async move {
            if let Err(e) = registration.send_registration(first_name, last_name).await {
                log::warn!("auto-submit-registration failed: {e:?}");
            }
        });
    }

    /// Clicks the photos of the chat history that offer a download.
    fn auto_download_media(&self) {
        let Some(history) = self
            .auto_window()
            .and_then(|w| descendant::<ui::ChatHistory>(w.upcast_ref()))
        else {
            log::warn!("auto-download-media: no chat history");
            return;
        };
        walk(history.upcast_ref(), &mut |widget| {
            if let Some(tile) = widget.downcast_ref::<ui::MediaPhotoTile>() {
                if tile.offers_download() {
                    tile.click_download();
                }
            }
        });
    }

    /// Writes the main chat list (`<id>\t<title>\t<unread>`) to
    /// `<output_dir>/chats.txt`.
    fn auto_list_chats(&self) {
        let Some(session) = self.active_session() else {
            log::warn!("auto-list-chats: no active session");
            return;
        };
        let list = session.main_chat_list();
        let mut out = String::new();
        for i in 0..list.n_items() {
            if let Some(item) = list.item(i).and_downcast::<model::ChatListItem>() {
                let chat = item.chat_();
                out.push_str(&format!(
                    "{}\t{}\t{}\n",
                    chat.id(),
                    chat.title(),
                    chat.unread_count()
                ));
            }
        }
        write_artifact("chats.txt", out.as_bytes());
    }

    fn auto_open_chat(&self, chat_id: ChatId) {
        let (Some(window), Some(client_id)) = (self.auto_window(), self.active_client_id()) else {
            log::warn!("auto-open-chat: no active client");
            return;
        };
        window.select_chat(client_id, chat_id);
        // As a tap on the row of the chat does, which also shows the chat
        // when the window is as narrow as a phone.
        if let Some(split_view) = descendant::<adw::NavigationSplitView>(window.upcast_ref()) {
            split_view.set_show_content(true);
        }
    }

    /// Types `text` into the composer of the open chat and sends it.
    fn auto_send_text(&self, text: String) {
        let Some(window) = self.auto_window() else {
            return;
        };
        let Some(bar) = descendant::<ui::ChatActionBar>(window.upcast_ref()) else {
            log::warn!("auto-send-text: no chat is open");
            return;
        };
        let Some(entry) = descendant::<ui::MessageEntry>(bar.upcast_ref()) else {
            log::warn!("auto-send-text: no composer");
            return;
        };
        entry.set_formatted_text(Some(model::BoxedFormattedText(
            tdlib::types::FormattedText {
                text,
                entities: vec![],
            },
        )));
        if let Err(e) = bar.activate_action("chat-action-bar.send-message", None) {
            log::warn!("auto-send-text: {e}");
        }
    }

    fn auto_submit_phone(&self, phone_number: String) {
        let Some(client_id) = self.active_client_id() else {
            return;
        };
        utils::spawn(async move {
            if let Err(e) =
                tdlib::functions::set_authentication_phone_number(phone_number, None, client_id)
                    .await
            {
                log::warn!("auto-submit-phone failed: {e:?}");
            }
        });
    }

    fn auto_submit_code(&self, code: String) {
        let Some(client_id) = self.active_client_id() else {
            return;
        };
        utils::spawn(async move {
            if let Err(e) = tdlib::functions::check_authentication_code(code, client_id).await {
                log::warn!("auto-submit-code failed: {e:?}");
            }
        });
    }
}
