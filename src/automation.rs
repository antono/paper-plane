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
        .unwrap_or_else(|| glib::user_cache_dir().join("paper-plane").join("automation"));
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

        self.add_auto_action("auto-screenshot", Some(glib::VariantTy::STRING), |app, p| {
            let name = p
                .and_then(|p| p.get::<String>())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "screenshot".to_owned());
            app.auto_screenshot(&name);
        });
        self.add_auto_action("auto-state", None, |app, _| app.auto_state());
        self.add_auto_action("auto-list-chats", None, |app, _| app.auto_list_chats());
        self.add_auto_action("auto-open-chat", Some(glib::VariantTy::INT64), |app, p| {
            match p.and_then(|p| p.get::<i64>()) {
                Some(chat_id) => app.auto_open_chat(chat_id),
                None => log::warn!("auto-open-chat: missing chat id"),
            }
        });
        self.add_auto_action("auto-send-text", Some(glib::VariantTy::STRING), |app, p| {
            match p.and_then(|p| p.get::<String>()) {
                Some(text) => app.auto_send_text(text),
                None => log::warn!("auto-send-text: missing text"),
            }
        });
        self.add_auto_action("auto-submit-phone", Some(glib::VariantTy::STRING), |app, p| {
            if let Some(phone) = p.and_then(|p| p.get::<String>()) {
                app.auto_submit_phone(phone);
            }
        });
        self.add_auto_action("auto-submit-code", Some(glib::VariantTy::STRING), |app, p| {
            if let Some(code) = p.and_then(|p| p.get::<String>()) {
                app.auto_submit_code(code);
            }
        });
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

    fn auto_window(&self) -> Option<ui::Window> {
        self.active_window().and_downcast::<ui::Window>()
    }

    fn active_client(&self) -> Option<model::Client> {
        self.auto_window()?.client_manager_view().active_client()
    }

    fn active_client_id(&self) -> Option<ClientId> {
        self.active_client().map(|client| client.id())
    }

    fn active_session(&self) -> Option<model::ClientStateSession> {
        self.active_client()?
            .state()
            .and_downcast::<model::ClientStateSession>()
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
                for i in 0..model.n_items() {
                    let Some(item) = model.item(i).and_downcast::<model::ChatHistoryItem>()
                    else {
                        continue;
                    };
                    if let Some(message) = item.message() {
                        out.push_str(&format!(
                            "message\t{}\t{}\n",
                            message.id(),
                            crate::strings::message_content(message)
                        ));
                    }
                }
            }
        }
        write_artifact("state.txt", out.as_bytes());
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
