use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use futures::Future;
use gettextrs::gettext;
use glib::clone;
use gtk::gio;
use gtk::glib;
use gtk::CompositeTemplate;

use super::message_selection;
use super::message_selection::MessageSelection;
use crate::expressions;
use crate::i18n::gettext_f;
use crate::i18n::ngettext_f;
use crate::model;
use crate::ui;
use crate::utils;

/// The number of messages requested when the viewport is first filled with
/// messages. If it is not enough, smaller chunks are requested until it is.
const INITIAL_LOAD_LIMIT: i32 = 30;

/// The delay, in milliseconds, before the viewed messages are sent to TDLib.
/// Messages are accumulated in the meantime, so that scrolling doesn't issue
/// a TDLib request on every change of the visible messages.
const VIEW_MESSAGES_DELAY_MS: u64 = 150;

/// How long the message that was jumped to stays tinted once it is shown.
const HIGHLIGHT_DURATION: std::time::Duration = std::time::Duration::from_millis(1500);

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/chat_history.ui")]
    pub(crate) struct ChatHistory {
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) chat_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) model: RefCell<Option<model::ChatHistoryModel>>,
        pub(super) message_menu: OnceCell<gtk::PopoverMenu>,
        pub(super) quick_reactions: OnceCell<ui::QuickReactions>,
        pub(super) is_auto_scrolling: Cell<bool>,
        pub(super) is_loading_messages: Cell<bool>,
        pub(super) sticky: Cell<bool>,
        /// The message that was jumped to, which is tinted for a moment, or 0.
        pub(super) highlighted_message_id: Cell<i64>,
        /// Counts the highlights, so that the end of an older one does not
        /// end a newer one.
        pub(super) highlight_generation: Cell<u32>,
        pub(super) viewed_message_ids: RefCell<HashSet<i64>>,
        pub(super) viewed_message_ids_changed: Cell<bool>,
        pub(super) view_messages_scheduled: Cell<bool>,
        pub(super) is_secret_chat: Cell<bool>,
        /// The selected messages. It lives as long as the widget, so that the
        /// recycled rows of the list view always talk to the same object.
        pub(super) selection: MessageSelection,
        /// Whether the chat picker of the forwarding is open. Escape then
        /// belongs to the dialog, not to the selection behind it.
        pub(super) is_picking_chat: Cell<bool>,
        #[template_child]
        pub(super) header_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) selection_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub(super) bottom_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) reply_selected_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) forward_selected_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) window_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub(super) title_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) background: TemplateChild<ui::Background>,
        #[template_child]
        pub(super) scrolled_window: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub(super) list_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) chat_action_bar: TemplateChild<ui::ChatActionBar>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatHistory {
        const NAME: &'static str = "PaplChatHistory";
        type Type = super::ChatHistory;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();

            klass.install_action("chat-history.view-info", None, move |widget, _, _| {
                widget.open_info_dialog();
            });
            klass.install_action("chat-history.scroll-down", None, move |widget, _, _| {
                widget.handle_scroll_down();
            });
            klass.install_action(
                "chat-history.reply",
                Some(glib::VariantTy::INT64),
                move |widget, _, variant| {
                    let message_id = variant.and_then(|v| v.get()).unwrap();
                    widget.imp().chat_action_bar.reply_to_message_id(message_id);
                },
            );
            klass.install_action(
                "chat-history.edit",
                Some(glib::VariantTy::INT64),
                move |widget, _, variant| {
                    let message_id = variant.and_then(|v| v.get()).unwrap();
                    widget.imp().chat_action_bar.edit_message_id(message_id);
                },
            );
            klass.install_action(
                "chat-history.jump-to-message",
                Some(glib::VariantTy::INT64),
                move |widget, _, variant| {
                    let message_id = variant.and_then(|v| v.get()).unwrap();
                    widget.jump_to_message(message_id);
                },
            );
            klass.install_action(
                "chat-history.toggle-selection",
                Some(glib::VariantTy::INT64),
                move |widget, _, variant| {
                    let message_id = variant.and_then(|v| v.get()).unwrap();
                    widget.toggle_selection(message_id);
                },
            );
            klass.install_action(
                "chat-history.cancel-selection",
                None,
                move |widget, _, _| {
                    widget.imp().selection.clear();
                },
            );
            klass.install_action("chat-history.reply-selected", None, move |widget, _, _| {
                widget.reply_selected();
            });
            klass.install_action("chat-history.copy-selected", None, move |widget, _, _| {
                widget.copy_selected();
            });
            klass.install_action_async(
                "chat-history.forward-selected",
                None,
                |widget, _, _| async move {
                    widget.forward_selected().await;
                },
            );
            klass.install_action_async(
                "chat-history.delete-selected",
                None,
                |widget, _, _| async move {
                    widget.show_delete_selected_dialog().await;
                },
            );
            klass.install_action_async(
                "chat-history.leave-chat",
                None,
                |widget, _, _| async move {
                    widget.show_leave_chat_dialog().await;
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatHistory {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::Chat>("chat")
                        .explicit_notify()
                        .build(),
                    glib::ParamSpecBoolean::builder("sticky")
                        .read_only()
                        .build(),
                    glib::ParamSpecBoolean::builder("is-secret-chat")
                        .read_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "chat" => {
                    let chat = value.get().unwrap();
                    obj.set_chat(chat);
                }
                "sticky" => obj.set_sticky(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "chat" => obj.chat().to_value(),
                "sticky" => obj.sticky().to_value(),
                "is-secret-chat" => obj.is_secret_chat().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.setup_expressions();

            // Clicking or tapping the title opens the profile, as in Telegram.
            // On phones the menu is otherwise the only way to it.
            self.title_box.set_cursor_from_name(Some("pointer"));
            let title_click = gtk::GestureClick::new();
            title_click.connect_released(clone!(
                #[weak]
                obj,
                move |gesture, _, _, _| {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    obj.open_info_dialog();
                }
            ));
            self.title_box.add_controller(title_click);

            // Without a selection the actions are disabled, so that the
            // Escape shortcut is not consumed and falls through.
            for action in [
                "chat-history.cancel-selection",
                "chat-history.reply-selected",
                "chat-history.copy-selected",
                "chat-history.forward-selected",
                "chat-history.delete-selected",
            ] {
                obj.action_set_enabled(action, false);
            }
            self.selection.connect_changed(clone!(
                #[weak]
                obj,
                move |_| obj.update_selection_ui()
            ));

            let shortcut_controller = gtk::ShortcutController::new();
            shortcut_controller.set_scope(gtk::ShortcutScope::Global);
            shortcut_controller.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("Escape"),
                Some(gtk::NamedAction::new("chat-history.cancel-selection")),
            ));
            obj.add_controller(shortcut_controller);

            let adj = self.list_view.vadjustment().unwrap();
            adj.connect_value_changed(clone!(
                #[weak]
                obj,
                move |adj| {
                    obj.schedule_view_messages();

                    let imp = obj.imp();

                    if imp.is_loading_messages.get() {
                        return;
                    }

                    if imp.is_auto_scrolling.get() {
                        if adj.value() + adj.page_size() >= adj.upper() {
                            imp.is_auto_scrolling.set(false);
                            obj.set_sticky(true);
                        }
                } else {
                    obj.set_sticky(adj.value() + adj.page_size() >= adj.upper());

                    // The list is reversed: a small value means that the oldest
                    // loaded message is visible, while a value close to the upper
                    // limit means that the newest loaded message is visible.
                    let near_oldest = adj.value() < adj.page_size() * 2.0
                        || adj.upper() <= adj.page_size() * 2.0;
                    let near_newest = adj.upper() > adj.page_size() * 2.0
                        && adj.value() + adj.page_size() * 2.0 >= adj.upper();

                    log::debug!(
                        "Scrolled (near_oldest = {near_oldest}, near_newest = {near_newest}, adj.value = {}, adj.upper = {}, adj.page_size = {})",
                        adj.value(),
                        adj.upper(),
                        adj.page_size(),
                    );

                    if !near_oldest && !near_newest {
                        return;
                    }

                    if let Some(model) = imp.model.borrow().as_ref() {
                        imp.is_loading_messages.set(true);

                        utils::spawn(clone!(
                            #[weak]
                            obj,
                            #[weak]
                            model,
                            async move {
                                let result = if near_oldest {
                                    log::debug!("Loading older messages on scroll (limit = 30)");
                                    model.load_older_messages(30).await
                                } else {
                                    log::debug!("Loading newer messages on scroll (limit = 50)");
                                    model.load_newer_messages(50).await
                                };

                                obj.imp().is_loading_messages.set(false);

                                if let Err(model::ChatHistoryError::AlreadyLoading) = result {
                                    log::debug!("Scroll-triggered load already in flight");
                                }
                                if let Err(model::ChatHistoryError::Tdlib(e)) = result {
                                    log::warn!("Couldn't load more chat messages: {:?}", e);
                                }
                            }
                        ));
                    }
                }
                }
            ));

            adj.connect_upper_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    if obj.sticky() || obj.imp().is_auto_scrolling.get() {
                        obj.scroll_down();
                    }
                }
            ));
        }

        fn dispose(&self) {
            if let Some(chat) = self.obj().chat() {
                perform_chat_action(&chat, tdlib::functions::close_chat);
            }
        }
    }

    impl WidgetImpl for ChatHistory {
        fn direction_changed(&self, previous_direction: gtk::TextDirection) {
            let obj = self.obj();

            if obj.direction() == previous_direction {
                return;
            }

            if let Some(menu) = self.message_menu.get() {
                menu.set_halign(if obj.direction() == gtk::TextDirection::Rtl {
                    gtk::Align::End
                } else {
                    gtk::Align::Start
                });
            }
        }
    }

    impl BinImpl for ChatHistory {}
}

glib::wrapper! {
    pub(crate) struct ChatHistory(ObjectSubclass<imp::ChatHistory>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ChatHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatHistory {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    fn setup_expressions(&self) {
        let chat_expression = Self::this_expression("chat");

        // Chat title
        expressions::chat_display_name(&chat_expression).bind(
            &*self.imp().window_title,
            "title",
            Some(self),
        );
    }

    pub(crate) fn selection(&self) -> &MessageSelection {
        &self.imp().selection
    }

    /// Selects or deselects the message, a whole media album if it is one.
    fn toggle_selection(&self, message_id: i64) {
        let Some(message) = self.chat().and_then(|chat| chat.message(message_id)) else {
            return;
        };

        let ids = match message.media_album() {
            Some(album) if album.len() >= 2 => album.messages().iter().map(|m| m.id()).collect(),
            _ => vec![message_id],
        };
        self.imp().selection.toggle(&ids);
    }

    /// Shows the selection header and action bar while messages are selected.
    fn update_selection_ui(&self) {
        let imp = self.imp();
        let n = imp.selection.n_selected();
        let active = imp.selection.active();

        imp.header_stack
            .set_visible_child_name(if active { "selection" } else { "normal" });
        imp.bottom_stack
            .set_visible_child_name(if active { "selection" } else { "composer" });
        imp.selection_title.set_title(&ngettext_f(
            "{n} selected",
            "{n} selected",
            n,
            &[("n", &n.to_string())],
        ));

        // Replying is possible to one message, in a chat that can be written to.
        let can_reply = n == 1
            && imp.selection.messages().first().is_some_and(|message| {
                super::message_row::can_send_messages_in_chat(&message.chat_())
            });
        imp.reply_selected_button.set_visible(n == 1);
        self.action_set_enabled("chat-history.reply-selected", can_reply);

        self.action_set_enabled(
            "chat-history.cancel-selection",
            active && !imp.is_picking_chat.get(),
        );
        self.action_set_enabled("chat-history.copy-selected", active);
        self.action_set_enabled("chat-history.forward-selected", active);
        self.action_set_enabled("chat-history.delete-selected", active);
    }

    fn reply_selected(&self) {
        let ids = self.imp().selection.ids();
        if let [id] = ids[..] {
            self.activate_action("chat-history.reply", Some(&id.to_variant()))
                .unwrap();
            self.imp().selection.clear();
        }
    }

    /// Puts the selected messages on the clipboard, Telegram-style, and leaves
    /// the selection mode. Returns the copied text.
    pub(crate) fn copy_selected(&self) -> String {
        // The selected part of the text of a single message wins over the
        // whole text
        let selected_part = selected_text_in(self.upcast_ref());
        let text = selected_part
            .unwrap_or_else(|| message_selection::copy_text_of(&self.imp().selection.messages()));

        if !text.is_empty() {
            self.clipboard().set_text(&text);
            if self.ancestor(adw::ToastOverlay::static_type()).is_some() {
                utils::show_toast(self, gettext("Copied to clipboard"));
            }
        }
        self.imp().selection.clear();

        text
    }

    /// Asks for the chat to forward the selected messages to.
    async fn forward_selected(&self) {
        let imp = self.imp();
        let messages = imp.selection.messages();
        let Some(chat) = self.chat() else {
            return;
        };
        if messages.is_empty() || imp.is_picking_chat.get() {
            return;
        }

        // The action is not disabled while it awaits, so a second activation
        // must find the guard set already. It also takes Escape and Cancel
        // away, which would leave the mode behind a dialog.
        imp.is_picking_chat.set(true);
        self.update_selection_ui();

        // The selection and the chat can change while TDLib is asked, so the
        // picker and the forwarding use this snapshot only.
        let ids: Vec<i64> = messages.iter().map(|m| m.id()).collect();

        // Whether a message can be forwarded is only known after TDLib was
        // asked, which is normally done lazily.
        futures::future::join_all(messages.iter().map(|m| m.load_properties())).await;

        self.offer_forwarding(chat, messages, ids);
    }

    /// Opens the chat picker if the selection is still what was asked about
    /// and all of it can be forwarded. `is_picking_chat` is set by the caller
    /// and reset here unless the picker opens.
    fn offer_forwarding(&self, chat: model::Chat, messages: Vec<model::Message>, ids: Vec<i64>) {
        let imp = self.imp();
        let unchanged = self.chat().as_ref() == Some(&chat) && imp.selection.message_ids() == ids;
        if !unchanged || !messages.iter().all(|m| m.can_be_forwarded()) {
            imp.is_picking_chat.set(false);
            self.update_selection_ui();
            if unchanged && self.ancestor(adw::ToastOverlay::static_type()).is_some() {
                utils::show_toast(self, gettext("Some of these messages can't be forwarded"));
            }
            return;
        }

        let dialog = ui::ChatPickerDialog::new(&chat.session_());
        dialog.connect_chat_picked(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, target| {
                let (chat, ids) = (chat.clone(), ids.clone());
                utils::spawn(async move { obj.forward_to(&target, &chat, ids).await });
            }
        ));
        dialog.connect_closed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                obj.imp().is_picking_chat.set(false);
                obj.update_selection_ui();
            }
        ));

        dialog.present(Some(self));
    }

    /// Forwards the selected messages to the chat and leaves the selection
    /// mode. The selection stays if TDLib refuses.
    pub(crate) async fn forward_selected_to(&self, target: &model::Chat) {
        let Some(chat) = self.chat() else {
            return;
        };
        let ids = self.imp().selection.message_ids();
        self.forward_to(target, &chat, ids).await;
    }

    async fn forward_to(&self, target: &model::Chat, chat: &model::Chat, ids: Vec<i64>) {
        if ids.is_empty() {
            return;
        }

        let toast = match target.forward_messages(chat, ids.clone()).await {
            Ok(()) => {
                // Another chat has its own selection by now
                if self.chat().as_ref() == Some(chat) {
                    self.imp().selection.clear();
                }
                gettext_f("Forwarded to {chat}", &[("chat", &target.title())])
            }
            Err(e) => {
                log::warn!("Error forwarding messages: {e:?}");
                gettext("Could not forward the messages")
            }
        };
        if self.ancestor(adw::ToastOverlay::static_type()).is_some() {
            utils::show_toast(self, toast);
        }
    }

    /// Whether all selected messages can be deleted for the user only, and
    /// whether they can be deleted for everyone.
    pub(crate) async fn delete_options(&self) -> (bool, bool) {
        Self::options_for(&self.imp().selection.messages()).await
    }

    async fn options_for(messages: &[model::Message]) -> (bool, bool) {
        // Whether a message can be deleted for everyone is only known after
        // TDLib was asked, which is normally done lazily.
        futures::future::join_all(messages.iter().map(|m| m.load_properties())).await;

        (
            messages.iter().all(|m| m.can_be_deleted_only_for_self()),
            messages.iter().all(|m| m.can_be_deleted_for_all_users()),
        )
    }

    /// The title of the selection header, for the automation bridge.
    pub(crate) fn selection_title_text(&self) -> String {
        self.imp().selection_title.title().to_string()
    }

    /// The name of the page shown instead of the message list's bottom bar.
    pub(crate) fn bottom_page(&self) -> Option<String> {
        self.imp()
            .bottom_stack
            .visible_child_name()
            .map(|name| name.to_string())
    }

    async fn show_delete_selected_dialog(&self) {
        if self.imp().selection.n_selected() == 0 {
            return;
        }
        // The selection can change while the properties load or the dialog is
        // open, so the dialog describes and deletes this snapshot only.
        let messages = self.imp().selection.messages();
        let ids: Vec<i64> = messages.iter().map(|m| m.id()).collect();
        let n = ids.len() as u32;
        let (for_me, for_all) = Self::options_for(&messages).await;

        let Some(window) = self.parent_window() else {
            return;
        };
        if !for_me && !for_all {
            if self.ancestor(adw::ToastOverlay::static_type()).is_some() {
                utils::show_toast(self, gettext("These messages can't be deleted"));
            }
            return;
        }

        let dialog = adw::AlertDialog::builder()
            .heading(ngettext_f(
                "Delete {n} message?",
                "Delete {n} messages?",
                n,
                &[("n", &n.to_string())],
            ))
            .build();
        dialog.add_response("cancel", &gettext("_Cancel"));
        for response in delete_responses(for_me, for_all) {
            let label = match response {
                "me" => gettext("Delete for _Me"),
                _ => gettext("Delete for Ever_yone"),
            };
            dialog.add_response(response, &label);
            dialog.set_response_appearance(response, adw::ResponseAppearance::Destructive);
        }
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let response = dialog.choose_future(Some(&window)).await;
        self.delete_after_response(&response, &ids);
    }

    /// Deletes the snapshot of the selection that the dialog asked about, as
    /// the response says.
    fn delete_after_response(&self, response: &str, ids: &[i64]) {
        let Some(revoke) = revoke_of_response(response) else {
            return;
        };
        if self.imp().selection.message_ids() != ids {
            // The options above may not hold for the current selection
            return;
        }
        self.delete_selected(revoke);
    }

    /// Deletes the selected messages with one request and leaves the
    /// selection mode.
    pub(crate) fn delete_selected(&self, revoke: bool) {
        let selection = &self.imp().selection;
        // Ids of messages that are gone are not worth a request, and the
        // options that were checked covered only the messages that are known.
        let ids = selection.message_ids();
        selection.clear();

        let Some(chat) = self.chat() else {
            return;
        };
        if ids.is_empty() {
            return;
        }

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                if let Err(e) = chat.delete_messages(ids, revoke).await {
                    log::warn!("Error deleting messages (revoke = {revoke}): {e:?}");
                    if obj.ancestor(adw::ToastOverlay::static_type()).is_some() {
                        utils::show_toast(&obj, gettext("Couldn't delete the messages"));
                    }
                }
            }
        ));
    }

    fn open_info_dialog(&self) {
        if let Some(chat) = self.chat() {
            let dialog = ui::ChatInfoWindow::new(&chat);
            // A shared media, file or link activated in the profile is shown
            // in the history, as long as this chat is still the open one.
            dialog.connect_jump_to_message(clone!(
                #[weak(rename_to = obj)]
                self,
                move |dialog, message_id| {
                    if obj.chat().as_ref() == dialog.chat() {
                        obj.jump_to_message(message_id);
                    }
                }
            ));
            dialog.present(Some(self));
        }
    }

    async fn show_leave_chat_dialog(&self) {
        if let Some(chat) = self.chat() {
            let dialog = adw::AlertDialog::new(
                Some(&gettext("Leave chat?")),
                Some(&gettext("Do you want to leave this chat?")),
            );
            dialog.add_responses(&[("no", &gettext("_No")), ("yes", &gettext("_Yes"))]);
            dialog.set_default_response(Some("no"));
            dialog.set_close_response("no");
            dialog.set_response_appearance("yes", adw::ResponseAppearance::Destructive);

            if dialog
                .choose_future(Some(&self.parent_window().unwrap()))
                .await
                == "yes"
            {
                match tdlib::functions::leave_chat(chat.id(), chat.session_().client_().id()).await
                {
                    Ok(_) => {
                        // Unselect recently left chat
                        utils::ancestor::<_, ui::Sidebar>(self)
                            .set_selected_chat(Option::<model::Chat>::None);
                    }
                    Err(e) => log::warn!("Failed to leave chat: {:?}", e),
                }
            }
        }
    }

    fn parent_window(&self) -> Option<gtk::Window> {
        self.root()?.downcast().ok()
    }

    fn request_sponsored_message(&self, chat: &model::Chat, list: &gio::ListStore) {
        utils::spawn(clone!(
            #[weak]
            chat,
            #[weak]
            list,
            async move {
                match model::SponsoredMessage::request(&chat).await {
                    Ok(sponsored_message) => {
                        if let Some(sponsored_message) = sponsored_message {
                            list.append(&sponsored_message);
                        }
                    }
                    Err(e) => {
                        if e.code != 404 {
                            log::warn!("Failed to request a SponsoredMessage: {:?}", e);
                        }
                    }
                }
            }
        ));
    }

    pub(crate) fn add_to_viewed_message_ids(&self, message_id: i64) {
        let imp = self.imp();
        if imp.viewed_message_ids.borrow_mut().insert(message_id) {
            imp.viewed_message_ids_changed.set(true);
        }
    }

    pub(crate) fn remove_from_viewed_message_ids(&self, message_id: i64) {
        let imp = self.imp();
        if imp.viewed_message_ids.borrow_mut().remove(&message_id) {
            imp.viewed_message_ids_changed.set(true);
        }
    }

    pub(crate) fn message_menu(&self) -> &gtk::PopoverMenu {
        self.imp().message_menu.get_or_init(|| {
            let menu = gtk::Builder::from_resource(
                "/app/drey/paper-plane/ui/session/content/message_menu.ui",
            )
            .object::<gtk::PopoverMenu>("menu")
            .unwrap();

            menu.set_halign(if self.direction() == gtk::TextDirection::Rtl {
                gtk::Align::End
            } else {
                gtk::Align::Start
            });

            let quick_reactions = ui::QuickReactions::default();
            assert!(menu.add_child(&quick_reactions, "reactions"));
            self.imp().quick_reactions.set(quick_reactions).unwrap();

            menu
        })
    }

    /// The row of reactions at the top of the message menu.
    pub(crate) fn quick_reactions(&self) -> &ui::QuickReactions {
        self.message_menu();
        self.imp().quick_reactions.get().unwrap()
    }

    pub(crate) fn handle_paste_action(&self) {
        self.imp().chat_action_bar.handle_paste_action();
    }

    pub(crate) fn chat(&self) -> Option<model::Chat> {
        self.imp().chat.upgrade()
    }

    /// Opens or closes the chat in TDLib. While a chat is opened in TDLib,
    /// no notifications are created for it.
    pub(crate) fn set_chat_open(&self, open: bool) {
        if let Some(chat) = self.chat() {
            if open {
                perform_chat_action(&chat, tdlib::functions::open_chat);
            } else {
                perform_chat_action(&chat, tdlib::functions::close_chat);
            }
        }
    }

    pub(crate) fn set_chat(&self, chat: Option<&model::Chat>) {
        let old_chat = self.chat();
        if chat == old_chat.as_ref() {
            return;
        }

        let imp = self.imp();

        // A selection belongs to one chat.
        imp.selection.set_chat(chat);

        if let Some(chat) = old_chat {
            chat.disconnect(imp.chat_handler.replace(None).unwrap());
            perform_chat_action(chat.as_ref(), tdlib::functions::close_chat);
        }

        if let Some(chat) = chat {
            self.set_is_secret_chat(matches!(chat.chat_type(), model::ChatType::Secret(_)));

            log::debug!(
                "Opening chat {} (unread_count = {}, anchor = {})",
                chat.id(),
                chat.unread_count(),
                if chat.unread_count() > 0 {
                    chat.last_read_inbox_message_id()
                } else {
                    0
                },
            );
            self.action_set_enabled(
                "chat-history.leave-chat",
                match chat.chat_type() {
                    model::ChatType::BasicGroup(data) => {
                        data.status().0 != tdlib::enums::ChatMemberStatus::Left
                    }
                    model::ChatType::Supergroup(data) => {
                        data.status().0 != tdlib::enums::ChatMemberStatus::Left
                    }
                    _ => false,
                },
            );

            let model = model::ChatHistoryModel::new(chat);

            let handler = chat.connect_new_message(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, msg| {
                    if msg.is_outgoing() {
                        obj.imp().background.animate();
                    }
                    // A message the user is sending from here is revealed,
                    // even if the view was scrolled up, as in the official
                    // clients (Yacha's 637fadfa in the modernization branch).
                    if msg.is_outgoing() && msg.sending_state().is_some() {
                        let at_newest = obj
                            .imp()
                            .model
                            .borrow()
                            .as_ref()
                            .is_none_or(|model| model.at_newest());
                        if at_newest {
                            obj.set_sticky(true);
                            obj.scroll_down();
                        } else {
                            // The newest messages are not loaded (the chat was
                            // opened at its unread messages or at an old one),
                            // so the sent message would wait out of sight:
                            // show the newest messages instead.
                            obj.go_to_newest();
                        }
                    }
                }
            ));
            imp.chat_handler.replace(Some(handler));

            self.set_history_model(chat, &model);

            perform_chat_action(chat, tdlib::functions::open_chat);
        }

        imp.chat.set(chat);
        self.notify("chat");
    }

    /// Creates the model shown by the list view, which wraps the chat history
    /// together with the sponsored messages, if needed.
    fn create_list_model(
        &self,
        chat: &model::Chat,
        model: &model::ChatHistoryModel,
    ) -> gio::ListModel {
        if matches!(chat.chat_type(), model::ChatType::Supergroup(supergroup) if supergroup.is_channel())
        {
            let list = gio::ListStore::new::<gio::ListModel>();

            // We need to create a list here so that we can append the sponsored message
            // to the chat history in the GtkListView using a GtkFlattenListModel
            let sponsored_message_list = gio::ListStore::new::<model::SponsoredMessage>();
            list.append(&sponsored_message_list);
            self.request_sponsored_message(chat, &sponsored_message_list);

            list.append(model);

            gtk::FlattenListModel::new(Some(list)).upcast()
        } else {
            model.clone().upcast()
        }
    }

    /// The number of sponsored messages shown before the chat history in the
    /// list view.
    fn sponsored_message_count(&self) -> u32 {
        let imp = self.imp();

        let Some(list_model) = imp.list_view.model() else {
            return 0;
        };

        let history_count = imp.model.borrow().as_ref().map_or(0, |m| m.n_items());

        list_model.n_items().saturating_sub(history_count)
    }

    /// Loads a window of messages around the anchor of the chat history and
    /// scrolls the list view to it.
    async fn load_around_anchor(&self, model: &model::ChatHistoryModel) {
        let imp = self.imp();

        let anchor = model.anchor();

        if let Err(e) = model.load_around_anchor().await {
            log::warn!("Couldn't load the messages around the anchor: {}", e);
            self.end_highlight_later(anchor);
            return;
        }

        let Some(mut position) = model.find_message_position(anchor) else {
            self.end_highlight_later(anchor);
            return;
        };

        // Show the unread messages below the anchor instead of the anchor itself
        if model.is_unread_anchor() {
            let unread_count = self
                .chat()
                .map(|chat| chat.unread_count())
                .unwrap_or_default();
            position = position.saturating_sub(unread_count as u32);
        }

        position += self.sponsored_message_count();

        // The list view shows the history in reverse order (newest at the bottom)
        let n_items = imp.list_view.model().map_or(0, |m| m.n_items());
        let position = n_items.saturating_sub(1).saturating_sub(position);

        imp.list_view
            .scroll_to(position, gtk::ListScrollFlags::NONE, None);

        // The rows are measured with estimated heights first. Once photos
        // and other media have their real height, the rows above the anchor
        // grow and push it out of view, so it is scrolled to again after
        // the layout settled.
        for delay in [50, 200] {
            glib::timeout_future(std::time::Duration::from_millis(delay)).await;
            let Some(current) = imp.model.borrow().as_ref().cloned() else {
                break;
            };
            if &current != model || imp.list_view.model().map_or(0, |m| m.n_items()) != n_items {
                // Another history was set, or messages were added meanwhile.
                break;
            }
            imp.list_view
                .scroll_to(position, gtk::ListScrollFlags::NONE, None);
        }

        self.end_highlight_later(anchor);
    }

    /// Installs the chat history model in the list view and fills it
    /// asynchronously.
    ///
    /// If the model is anchored at the newest message, the history is filled
    /// from the newest end until the viewport can scroll, e.g. when opening
    /// a chat without unread messages or jumping to its newest message.
    /// Otherwise, a window of messages is loaded around the anchor and the
    /// viewport is positioned at it, e.g. when opening a chat with unread
    /// messages or jumping to a specific message.
    fn set_history_model(&self, chat: &model::Chat, model: &model::ChatHistoryModel) {
        let imp = self.imp();

        // A highlight belongs to the history that was jumped to.
        if model.anchor() != imp.highlighted_message_id.get() {
            self.set_highlighted_message_id(0);
        }

        // Claim the loading flag synchronously, so that the scroll handler
        // can't start a concurrent load before the initial fill below had
        // a chance to run.
        imp.is_loading_messages.set(true);

        let list_view_model = self.create_list_model(chat, model);

        // The model keeps the newest message first, the list view shows the
        // newest message at the bottom.
        let list_view_model = model::ReversedListModel::new(&list_view_model);
        let selection = gtk::NoSelection::new(Some(list_view_model));
        imp.list_view.set_model(Some(&selection));

        imp.model.replace(Some(model.clone()));

        let widget = self.clone();
        let model_weak = model.downgrade();

        utils::spawn(clone!(
            #[strong(rename_to = obj)]
            widget,
            #[strong]
            model_weak,
            async move {
                let imp = obj.imp();

                log::debug!(
                    "Filling chat history (at_newest = {}, is_unread_anchor = {})",
                    model_weak.upgrade().map(|m| m.at_newest()).unwrap_or(false),
                    model_weak
                        .upgrade()
                        .map(|m| m.is_unread_anchor())
                        .unwrap_or(false),
                );

                let scrollbar = imp.scrolled_window.vscrollbar();
                scrollbar.set_visible(false);

                let adj = imp.list_view.vadjustment().unwrap();
                adj.set_value(0.0);

                if obj.chat().is_some() {
                    if let Some(model) = model_weak.upgrade() {
                        if model.at_newest() {
                            // Fill the viewport with messages, using
                            // progressively smaller chunks, so that the first
                            // paint happens after a single TDLib request
                            let mut limit = INITIAL_LOAD_LIMIT;

                            // Load until the messages fill the view. (This
                            // used to wait for the value to leave 0, which only
                            // happened with the reversed list view of the
                            // patched GTK; with a plain list view 0 is the top
                            // and stays so, and the whole history was loaded.)
                            loop {
                                // Let the list lay out what is loaded.
                                glib::timeout_future(std::time::Duration::from_millis(20)).await;
                                if adj.upper() > adj.page_size() * 1.5 {
                                    break;
                                }
                                let adj_value = adj.value();
                                let adj_upper = adj.upper();
                                let adj_page_size = adj.page_size();
                                match model.load_older_messages(limit).await {
                                    Ok(true) => {
                                        log::debug!(
                                            "Loaded older messages (limit = {limit}, remaining = true, adj.value = {adj_value}, adj.upper = {adj_upper}, adj.page_size = {adj_page_size})"
                                        );
                                        limit = (limit / 2).max(2);
                                    }
                                    Ok(false) => {
                                        log::debug!(
                                            "No more older messages (limit = {limit}, adj.value = {adj_value}, adj.upper = {adj_upper}, adj.page_size = {adj_page_size})"
                                        );
                                        break;
                                    }
                                    Err(model::ChatHistoryError::AlreadyLoading) => {
                                        // Another load is in flight. Try again
                                        // after giving it a chance to finish.
                                        log::debug!(
                                            "load_older_messages already in flight (limit = {limit}), retrying"
                                        );
                                        glib::timeout_future(std::time::Duration::from_millis(20))
                                            .await;
                                    }
                                    Err(model::ChatHistoryError::Tdlib(e)) => {
                                        log::warn!("Couldn't load initial history messages: {e:?}");
                                        break;
                                    }
                                }
                            }

                            log::debug!(
                                "Setting sticky (adj.value = {}, adj.upper = {}, adj.page_size = {}, n_items = {})",
                                adj.value(),
                                adj.upper(),
                                adj.page_size(),
                                model.n_items(),
                            );
                            obj.set_sticky(true);
                            obj.scroll_down();
                        } else {
                            log::debug!("Loading messages around the anchor");
                            obj.load_around_anchor(&model).await;

                            log::debug!("Anchor window loaded (n_items = {})", model.n_items(),);

                            obj.set_sticky(false);
                        }
                    }
                }

                log::debug!(
                    "Chat history fill done (n_items = {}), showing scrollbar",
                    model_weak.upgrade().map(|m| m.n_items()).unwrap_or(0),
                );
                scrollbar.set_visible(true);

                imp.is_loading_messages.set(false);

                if obj.chat().is_some() {
                    obj.view_messages();
                }
            }
        ));
    }

    /// Shows the message with the specified id in the chat history, loading
    /// the messages around it instead of the whole history in between.
    fn jump_to_message(&self, message_id: i64) {
        let Some(chat) = self.chat() else {
            return;
        };

        let model = model::ChatHistoryModel::new_for_message(&chat, message_id);

        self.set_highlighted_message_id(message_id);
        self.set_history_model(&chat, &model);
    }

    /// The message that was jumped to, e.g. from the profile or a reply,
    /// which is tinted for a moment, or 0.
    pub(crate) fn highlighted_message_id(&self) -> i64 {
        self.imp().highlighted_message_id.get()
    }

    fn set_highlighted_message_id(&self, message_id: i64) {
        let imp = self.imp();
        if imp.highlighted_message_id.get() == message_id {
            return;
        }
        imp.highlighted_message_id.set(message_id);
        imp.highlight_generation
            .set(imp.highlight_generation.get().wrapping_add(1));

        // The rows that are shown follow at once, the others when they are
        // shown.
        let mut child = imp.list_view.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.first_child().and_downcast::<ui::ChatHistoryRow>() {
                row.sync_highlight();
            }
            child = widget.next_sibling();
        }
    }

    /// Ends the highlight of `message_id` after a moment, once it was
    /// scrolled to.
    fn end_highlight_later(&self, message_id: i64) {
        let imp = self.imp();
        if imp.highlighted_message_id.get() != message_id {
            return;
        }
        let generation = imp.highlight_generation.get();

        glib::timeout_add_local_once(
            HIGHLIGHT_DURATION,
            clone!(
                #[weak(rename_to = obj)]
                self,
                move || {
                    if obj.imp().highlight_generation.get() == generation {
                        obj.set_highlighted_message_id(0);
                    }
                }
            ),
        );
    }

    pub(crate) fn sticky(&self) -> bool {
        self.imp().sticky.get()
    }

    fn set_sticky(&self, sticky: bool) {
        if self.sticky() == sticky {
            return;
        }

        self.imp().sticky.set(sticky);
        self.notify("sticky");
    }

    pub(crate) fn is_secret_chat(&self) -> bool {
        self.imp().is_secret_chat.get()
    }

    fn set_is_secret_chat(&self, is_secret_chat: bool) {
        if self.is_secret_chat() == is_secret_chat {
            return;
        }

        self.imp().is_secret_chat.set(is_secret_chat);
        self.notify("is-secret-chat");
    }

    fn scroll_down(&self) {
        let imp = self.imp();

        imp.is_auto_scrolling.set(true);

        imp.scrolled_window
            .emit_by_name::<bool>("scroll-child", &[&gtk::ScrollType::End, &false]);
    }

    /// Handles the scroll-down button press.
    ///
    /// If the chat has unread messages that are not in the loaded history
    /// window, jumps to the first unread message. If the newest messages are
    /// not loaded anymore, e.g. after jumping to an old message, re-opens
    /// the history at the newest message. Otherwise, plain scrolling to the
    /// bottom is used.
    fn handle_scroll_down(&self) {
        let Some(chat) = self.chat() else {
            self.scroll_down();
            return;
        };

        let Some(model) = self.imp().model.borrow().as_ref().cloned() else {
            self.scroll_down();
            return;
        };

        // The history is anchored at the last read message when there are
        // unread messages. A chat that was never read has no such anchor and
        // jumping to the newest message is used instead.
        if chat.unread_count() > 0 && chat.last_read_inbox_message_id() != 0 && !model.at_newest() {
            self.jump_to_first_unread();
        } else if !model.at_newest() {
            self.go_to_newest();
        } else {
            self.scroll_down();
        }
    }

    /// Re-anchors the history at the first unread message and positions the
    /// viewport at it. This is similar to opening the chat with unread
    /// messages, so that the unread block is shown instead of reloading
    /// newer messages one chunk at a time.
    fn jump_to_first_unread(&self) {
        let Some(chat) = self.chat() else {
            return;
        };

        let model = model::ChatHistoryModel::new(&chat);

        self.set_history_model(&chat, &model);
    }

    /// Re-opens the history at the newest message of the chat, discarding
    /// the currently loaded window. Used when the newest messages are not
    /// loaded anymore, e.g. after jumping to an old message.
    fn go_to_newest(&self) {
        let Some(chat) = self.chat() else {
            return;
        };

        // Not ChatHistoryModel::new, which opens chats with unread messages
        // at the first unread one.
        let model = model::ChatHistoryModel::new_at_newest(&chat);

        self.set_history_model(&chat, &model);
    }

    /// Schedules a flush of the viewed messages with a short delay, so that
    /// at most one `viewMessages` request is sent per delay interval.
    pub(crate) fn schedule_view_messages(&self) {
        let imp = self.imp();

        if imp.view_messages_scheduled.get() {
            return;
        }
        imp.view_messages_scheduled.set(true);

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                glib::timeout_future(std::time::Duration::from_millis(VIEW_MESSAGES_DELAY_MS))
                    .await;

                obj.imp().view_messages_scheduled.set(false);
                obj.view_messages();
            }
        ));
    }

    pub(crate) fn view_messages(&self) {
        let imp = self.imp();

        if imp.viewed_message_ids_changed.get() {
            imp.viewed_message_ids_changed.set(false);

            // The report is delayed, and the chat may have been closed in
            // the meantime (switching chats, quitting).
            let Some(chat) = self.chat() else {
                return;
            };
            let chat_id = chat.id();
            let client_id = chat.session_().client_().id();
            let viewed_message_ids =
                Vec::from_iter(imp.viewed_message_ids.borrow().iter().copied());

            log::debug!(
                "Reporting {count} viewed messages to TDLib (chat_id = {chat_id})",
                count = viewed_message_ids.len(),
            );

            utils::spawn(async move {
                // TDLib can reject the request (a deleted chat, a network or
                // server error); the messages are then reported again the next
                // time they are viewed.
                if let Err(e) = tdlib::functions::view_messages(
                    chat_id,
                    viewed_message_ids,
                    None,
                    true,
                    client_id,
                )
                .await
                {
                    log::warn!("Could not mark messages of chat {chat_id} as viewed: {e:?}");
                }
            });
        }
    }
}

/// The responses of the delete dialog besides Cancel, for deleting only for
/// the user and for everyone, as far as all selected messages allow it.
fn delete_responses(for_me: bool, for_all: bool) -> Vec<&'static str> {
    let mut responses = vec![];
    if for_me {
        responses.push("me");
    }
    if for_all {
        responses.push("everyone");
    }
    responses
}

/// Whether a response of the delete dialog revokes the messages, `None` for
/// the responses that delete nothing.
fn revoke_of_response(response: &str) -> Option<bool> {
    match response {
        "me" => Some(false),
        "everyone" => Some(true),
        _ => None,
    }
}

fn perform_chat_action<F, Fut>(chat: &model::Chat, op: F)
where
    F: Fn(i64, i32) -> Fut + 'static,
    Fut: Future<Output = Result<(), tdlib::types::Error>>,
{
    utils::spawn(clone!(
        #[weak]
        chat,
        async move {
            op(chat.id(), chat.session_().client_().id()).await.unwrap();
        }
    ));
}

/// The text selected in the text view of a message. Only the label of the
/// one selected row (or album caption) has a text view, so the row decides,
/// not the number of selected messages (an album selects several).
fn selected_text_in(widget: &gtk::Widget) -> Option<String> {
    if let Some(label) = widget.downcast_ref::<ui::MessageLabel>() {
        return label.text_view().and_then(|_| label.selected_text());
    }

    let mut child = widget.first_child();
    while let Some(child_) = child {
        if let Some(text) = selected_text_in(&child_) {
            return Some(text);
        }
        child = child_.next_sibling();
    }

    None
}

/// Checks of the secret chat title (#625) that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use crate::model::test_support::*;

    selection_gtk_check();
    message_tap_gtk_check();

    eprintln!("secret chat style in the chat history");

    isolated(|| {
        let session = session();
        add_user(&session, 42);
        let secret = add_secret_chat(&session, 900, 7, 42);
        let alice = add_private_chat(&session, 42, 42, "Alice");

        let history = ChatHistory::new();
        history.set_chat(Some(&secret));
        assert!(history.is_secret_chat());
        history.set_chat(Some(&alice));
        assert!(!history.is_secret_chat());

        // Sending a message while the newest messages are not loaded shows
        // the newest ones, instead of queueing the message out of sight.
        eprintln!("sending from a history opened at the unread messages");
        let session2 = session.clone();
        let chat = add_private_chat(&session2, 43, 42, "Unread");
        session2.handle_update(tdlib::enums::Update::ChatReadInbox(
            tdlib::types::UpdateChatReadInbox {
                chat_id: 43,
                last_read_inbox_message_id: 5,
                unread_count: 10,
            },
        ));
        history.set_chat(Some(&chat));
        let model = || history.imp().model.borrow().clone().unwrap();
        assert!(
            !model().at_newest(),
            "the chat opens at its unread messages"
        );
        let mut sending = message(43, 1 << 40, DAY, 0, text("sent from here"));
        sending.sending_state = Some(tdlib::enums::MessageSendingState::Pending(
            tdlib::types::MessageSendingStatePending { sending_id: 0 },
        ));
        new_message(&session2, sending);
        assert!(model().at_newest(), "sending shows the newest messages");

        // The report of viewed messages is delayed (78359245); a chat closed
        // in the meantime must not crash it. glib catches panics inside
        // spawned futures, so the delayed call is made directly here.
        eprintln!("delayed view report after closing the chat");
        history.set_chat(None);
        history.imp().viewed_message_ids.borrow_mut().insert(1);
        history.imp().viewed_message_ids_changed.set(true);
        history.view_messages();
    });
}

/// Whether the menu model has an item that activates the action.
#[cfg(test)]
fn menu_has_action(menu: &gio::MenuModel, action: &str) -> bool {
    (0..menu.n_items()).any(|i| {
        let item_action = menu
            .item_attribute_value(i, "action", Some(glib::VariantTy::STRING))
            .and_then(|v| v.get::<String>());
        item_action.as_deref() == Some(action)
            || [gio::MENU_LINK_SECTION, gio::MENU_LINK_SUBMENU]
                .iter()
                .filter_map(|link| menu.item_link(i, link))
                .any(|linked| menu_has_action(&linked, action))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "For everyone" is offered only if every message allows it.
    #[test]
    fn delete_dialog_offers_what_all_messages_allow() {
        assert_eq!(delete_responses(true, false), ["me"]);
        assert_eq!(delete_responses(true, true), ["me", "everyone"]);
        assert_eq!(delete_responses(false, true), ["everyone"]);
        assert!(delete_responses(false, false).is_empty());
    }

    #[test]
    fn delete_responses_revoke_only_for_everyone() {
        assert_eq!(revoke_of_response("me"), Some(false));
        assert_eq!(revoke_of_response("everyone"), Some(true));
        assert_eq!(revoke_of_response("cancel"), None);
        assert_eq!(revoke_of_response(""), None);
    }
}

/// Checks of the selection mode (header, action bar, albums, copy) that need
/// GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn selection_gtk_check() {
    use crate::model::test_support::*;

    selection_actions_gtk_check();

    eprintln!("selection mode: forward is enabled only while selecting");
    isolated(|| {
        let session = session();
        add_user(&session, 42);
        let chat = add_private_chat(&session, 42, 42, "Alice");
        let history = ChatHistory::new();
        history.set_chat(Some(&chat));
        new_message(&session, message(42, 1, DAY, 0, text("one")));
        // The sensitivity of an action button is only tracked in a window.
        let window = gtk::Window::new();
        window.set_child(Some(&history));

        let forward = &history.imp().forward_selected_button;
        assert!(!forward.is_sensitive());
        history
            .activate_action("chat-history.toggle-selection", Some(&1i64.to_variant()))
            .unwrap();
        assert!(forward.is_sensitive());
        history.selection().clear();
        assert!(!forward.is_sensitive());

        window.set_child(gtk::Widget::NONE);
    });

    isolated(|| {
        let session = session();
        add_user(&session, 42);
        let chat = add_private_chat(&session, 42, 42, "Alice");
        let history = ChatHistory::new();
        history.set_chat(Some(&chat));
        for id in 1..=3 {
            new_message(
                &session,
                message(42, id, DAY + id as i32 * 60, 0, text(&format!("text {id}"))),
            );
        }
        let imp = history.imp();
        let toggle = |id: i64| {
            history
                .activate_action("chat-history.toggle-selection", Some(&id.to_variant()))
                .unwrap()
        };
        let stack_page = |stack: &gtk::Stack| stack.visible_child_name().unwrap().to_string();

        eprintln!("selection mode: header, action bar and reply visibility");
        assert_eq!(stack_page(&imp.header_stack), "normal");
        assert_eq!(stack_page(&imp.bottom_stack), "composer");
        toggle(1);
        assert_eq!(stack_page(&imp.header_stack), "selection");
        assert_eq!(imp.selection_title.title(), "1 selected");
        assert_eq!(stack_page(&imp.bottom_stack), "selection");
        assert!(imp.reply_selected_button.is_visible());
        toggle(2);
        assert_eq!(imp.selection_title.title(), "2 selected");
        assert!(!imp.reply_selected_button.is_visible());
        history
            .activate_action("chat-history.cancel-selection", None)
            .unwrap();
        assert!(!history.selection().active());
        assert_eq!(stack_page(&imp.header_stack), "normal");
        assert_eq!(stack_page(&imp.bottom_stack), "composer");

        // Deselecting the last selected message leaves the mode.
        toggle(3);
        assert!(history.selection().active());
        toggle(3);
        assert!(!history.selection().active());
        assert_eq!(stack_page(&imp.bottom_stack), "composer");

        eprintln!("selection mode: copy");
        toggle(1);
        toggle(3);
        let copied = history.copy_selected();
        let lines: Vec<_> = copied.lines().collect();
        assert_eq!(lines.len(), 2, "{copied:?}");
        for (line, id) in lines.iter().zip([1, 3]) {
            let b = line.as_bytes();
            assert!(
                b[0] == b'[' && b[3] == b':' && b[6] == b']' && b[7] == b' ',
                "[HH:MM] prefix: {copied:?}"
            );
            assert!(line.ends_with(&format!(": text {id}")), "{copied:?}");
        }
        assert!(!history.selection().active(), "copying leaves the mode");
        toggle(2);
        assert_eq!(history.copy_selected(), "text 2");

        eprintln!("selection mode: album rows select all members");
        for id in 10..=11 {
            new_message(&session, message(42, id, DAY + 600, 77, photo("")));
        }
        toggle(10);
        assert_eq!(history.selection().ids(), [10, 11]);
        toggle(10);
        assert!(!history.selection().active());

        eprintln!("selection mode: the Select item of the message menu");
        assert!(menu_has_action(
            history.message_menu().menu_model().as_ref().unwrap(),
            "message-row.select"
        ));
        let row = ui::MessageRow::new(chat.message(1).unwrap().upcast_ref());
        row.set_parent(&history);
        row.activate_action("message-row.select", None).unwrap();
        assert_eq!(history.selection().ids(), [1]);
        row.activate_action("message-row.select", None).unwrap();
        assert!(!history.selection().active());
        row.unparent();

        // The real wiring of a row of the list view: `set_item` and the
        // `changed` connection made while the row is mapped.
        eprintln!("selection mode: mapped rows follow the selection and recycling");
        let window = gtk::Window::new();
        window.set_child(Some(&history));
        window.present();
        let context = glib::MainContext::default();
        for _ in 0..200 {
            if history.is_mapped() {
                break;
            }
            context.iteration(false);
        }
        assert!(history.is_mapped(), "the history is shown");

        let item = |id: i64| {
            model::ChatHistoryItem::for_message(chat.message(id).unwrap()).upcast::<glib::Object>()
        };
        let history_row = ui::ChatHistoryRow::new();
        history_row.set_parent(&history);
        history_row.set_item(Some(item(1)));
        assert!(history_row.is_mapped());
        let message_row = history_row
            .child()
            .and_downcast::<ui::MessageRow>()
            .unwrap();
        let checked = || message_row.has_css_class("selected-message");
        let check_shown = || message_row.first_child().is_some_and(|c| c.is_visible());

        assert!(!check_shown() && !checked());
        // Selecting reaches the mapped row through its `changed` connection.
        toggle(1);
        assert!(check_shown() && checked(), "selected row is checked");
        // The list view recycles the row for another message.
        history_row.set_item(Some(item(2)));
        assert!(check_shown() && !checked(), "recycled row is unchecked");
        history_row.set_item(Some(item(1)));
        assert!(checked(), "recycled back to the selected message");
        toggle(1);
        assert!(!check_shown() && !checked(), "leaving the mode clears it");

        eprintln!("selection mode: only the single selected message gets a text view");
        let second_row = ui::ChatHistoryRow::new();
        second_row.set_parent(&history);
        second_row.set_item(Some(item(2)));
        let message_row_2 = second_row.child().and_downcast::<ui::MessageRow>().unwrap();
        let has_view = |row: &ui::MessageRow| row.active_text_view().is_some();
        assert!(!has_view(&message_row) && !has_view(&message_row_2));
        toggle(1);
        assert!(has_view(&message_row) && !has_view(&message_row_2));

        // Copy takes the selected part of the text, if there is one
        let buffer = message_row.active_text_view().unwrap().buffer();
        buffer.select_range(&buffer.iter_at_offset(0), &buffer.iter_at_offset(4));
        assert_eq!(history.copy_selected(), "text");
        assert!(!has_view(&message_row), "copying leaves the mode");

        toggle(1);
        assert!(has_view(&message_row));
        assert_eq!(history.copy_selected(), "text 1", "no part selected");

        toggle(1);
        toggle(2);
        assert!(!has_view(&message_row) && !has_view(&message_row_2));
        toggle(1);
        assert!(!has_view(&message_row) && has_view(&message_row_2));
        history
            .activate_action("chat-history.cancel-selection", None)
            .unwrap();
        assert!(!has_view(&message_row) && !has_view(&message_row_2));

        second_row.unparent();
        history_row.unparent();
        window.set_child(None::<&gtk::Widget>);
        window.destroy();
    });
}

/// The first descendant of `widget` of type `T` that matches `wanted`.
#[cfg(test)]
fn find_descendant<T: IsA<gtk::Widget>>(
    widget: &gtk::Widget,
    wanted: &dyn Fn(&T) -> bool,
) -> Option<T> {
    if let Some(found) = widget.downcast_ref::<T>().filter(|found| wanted(found)) {
        return Some(found.clone());
    }
    let mut child = widget.first_child();
    while let Some(child_) = child {
        if let Some(found) = find_descendant(&child_, wanted) {
            return Some(found);
        }
        child = child_.next_sibling();
    }
    None
}

/// A tap on a message opens its menu with the row of reactions, see
/// `MessageBase`.
#[cfg(test)]
fn message_tap_gtk_check() {
    use gtk::graphene;

    use crate::model::test_support::*;

    eprintln!("message menu: a tap opens it, except on buttons and while selecting");
    isolated(|| {
        let session = session();
        add_user(&session, 42);
        let chat = add_private_chat(&session, 42, 42, "Alice");
        let history = ChatHistory::new();
        history.set_chat(Some(&chat));

        let reaction = |r#type, total_count| tdlib::types::MessageReaction {
            r#type,
            total_count,
            is_chosen: false,
            used_sender_id: None,
            recent_sender_ids: vec![],
        };
        let mut tapped = message(42, 1, DAY, 0, text("tap me"));
        tapped.interaction_info = Some(tdlib::types::MessageInteractionInfo {
            view_count: 0,
            forward_count: 0,
            reply_info: None,
            reactions: Some(tdlib::types::MessageReactions {
                reactions: vec![
                    reaction(model::reaction_type_from_key("👍"), 2),
                    reaction(tdlib::enums::ReactionType::Paid, 137),
                ],
                are_tags: false,
                paid_reactors: vec![],
                can_get_added_reactions: false,
            }),
        });
        new_message(&session, tapped);

        let window = gtk::Window::new();
        window.set_child(Some(&history));
        window.present();
        let context = glib::MainContext::default();
        for _ in 0..200 {
            if history.is_mapped() {
                break;
            }
            context.iteration(false);
        }
        assert!(history.is_mapped(), "the history is shown");

        let history_row = ui::ChatHistoryRow::new();
        history_row.set_parent(&history);
        history_row.set_item(Some(
            model::ChatHistoryItem::for_message(chat.message(1).unwrap()).upcast(),
        ));
        let (_, height, _, _) = history_row.measure(gtk::Orientation::Vertical, 360);
        history_row.size_allocate(&gtk::gdk::Rectangle::new(0, 0, 360, height), -1);

        let base = find_descendant::<ui::MessageBase>(history_row.upcast_ref(), &|_| true)
            .expect("the message widget");
        let tap = base
            .observe_controllers()
            .snapshot()
            .into_iter()
            .filter_map(|c| c.downcast::<gtk::GestureClick>().ok())
            .find(|click| click.is_touch_only())
            .expect("the tap gesture");
        let tap_at = |widget: &gtk::Widget, n_press: i32| {
            let center =
                graphene::Point::new(widget.width() as f32 / 2.0, widget.height() as f32 / 2.0);
            let point = widget.compute_point(&base, &center).unwrap();
            tap.emit_by_name::<()>(
                "released",
                &[&n_press, &(point.x() as f64), &(point.y() as f64)],
            );
        };
        let label = find_descendant::<gtk::Label>(base.upcast_ref(), &|label| {
            label.text().contains("tap me")
        })
        .expect("the text of the message");
        let chip = |paid: bool| {
            find_descendant::<gtk::Button>(base.upcast_ref(), &|button| {
                button.has_css_class("reaction-chip") && button.has_css_class("paid") == paid
            })
            .expect("a reaction chip")
        };
        let menu = history.message_menu();

        tap_at(label.upcast_ref(), 1);
        assert!(menu.is_visible(), "a tap on the text opens the menu");
        assert_eq!(
            history.quick_reactions().message().map(|m| m.id()),
            Some(1),
            "the reactions of the tapped message"
        );
        menu.popdown();

        tap_at(chip(false).upcast_ref(), 1);
        assert!(!menu.is_visible(), "a tap on a reaction only reacts");

        // The paid chip only shows the stars, a tap goes to the message.
        tap_at(chip(true).upcast_ref(), 1);
        assert!(
            menu.is_visible(),
            "a tap on the paid reaction opens the menu"
        );
        menu.popdown();

        tap_at(label.upcast_ref(), 2);
        assert!(
            !menu.is_visible(),
            "the second tap of a double tap does not"
        );

        history
            .activate_action("chat-history.toggle-selection", Some(&1i64.to_variant()))
            .unwrap();
        tap_at(label.upcast_ref(), 1);
        assert!(!menu.is_visible(), "a tap selects in the selection mode");
        history.selection().clear();

        history_row.unparent();
        window.set_child(None::<&gtk::Widget>);
        window.destroy();
    });
}

/// Escape, Reply, Forward and the delete response of the selection mode.
#[cfg(test)]
fn selection_actions_gtk_check() {
    use crate::model::test_support::*;

    let wait_until = |condition: &dyn Fn() -> bool| {
        let context = glib::MainContext::default();
        for _ in 0..200 {
            if condition() {
                return;
            }
            context.iteration(false);
        }
        panic!("timed out");
    };

    eprintln!("selection mode: Escape leaves it, and only while it is on");
    isolated(|| {
        let session = session();
        add_user(&session, 42);
        let chat = add_private_chat(&session, 42, 42, "Alice");
        let history = ChatHistory::new();
        history.set_chat(Some(&chat));
        new_message(&session, message(42, 1, DAY, 0, text("one")));

        let escape =
            gtk::KeyvalTrigger::new(gtk::gdk::Key::Escape, gtk::gdk::ModifierType::empty());
        let bound_to_escape = history
            .observe_controllers()
            .iter::<glib::Object>()
            .filter_map(|controller| controller.ok()?.downcast::<gtk::ShortcutController>().ok())
            .flat_map(|controller| {
                (0..controller.n_items())
                    .filter_map(|i| controller.item(i)?.downcast::<gtk::Shortcut>().ok())
                    .collect::<Vec<_>>()
            })
            .find(|shortcut| shortcut.trigger().is_some_and(|t| t.equal(&escape)))
            .expect("a shortcut for Escape");
        assert_eq!(
            bound_to_escape
                .action()
                .and_downcast::<gtk::NamedAction>()
                .unwrap()
                .action_name(),
            "chat-history.cancel-selection"
        );

        // A disabled action does not consume Escape, which falls through to
        // what is below (closing a dialog, the media viewer, ...). The
        // sensitivity of a button tracks the action, in a window.
        let window = gtk::Window::new();
        window.set_child(Some(&history));
        let cancel_enabled =
            || action_enabled(history.upcast_ref(), "chat-history.cancel-selection");
        assert!(!cancel_enabled(), "disabled without a selection");
        history
            .activate_action("chat-history.toggle-selection", Some(&1i64.to_variant()))
            .unwrap();
        assert!(cancel_enabled());
        history.imp().is_picking_chat.set(true);
        history.update_selection_ui();
        assert!(!cancel_enabled(), "disabled behind the chat picker");
        history.imp().is_picking_chat.set(false);
        history.update_selection_ui();
        assert!(cancel_enabled());
        history
            .activate_action("chat-history.cancel-selection", None)
            .unwrap();
        assert!(!history.selection().active());
        window.set_child(gtk::Widget::NONE);
    });

    eprintln!("selection mode: Reply needs a chat that can be written to");
    isolated(|| {
        let session = session();
        add_user(&session, 42);
        let private = add_private_chat(&session, 42, 42, "Alice");
        let left = add_basic_group_chat(&session, 50, 5, tdlib::enums::ChatMemberStatus::Left);
        let history = ChatHistory::new();
        history.set_chat(Some(&left));
        new_message(&session, message(50, 1, DAY, 0, text("in a group")));
        let toggle = |id: i64| {
            history
                .activate_action("chat-history.toggle-selection", Some(&id.to_variant()))
                .unwrap()
        };
        let window = gtk::Window::new();
        window.set_child(Some(&history));
        let reply = &history.imp().reply_selected_button;
        // The window is not shown, so only the own flag tells.
        let visible = || reply.property::<bool>("visible");
        toggle(1);
        assert!(visible());
        assert!(
            !reply.is_sensitive(),
            "can't reply in a group the user left"
        );

        history.set_chat(Some(&private));
        new_message(&session, message(42, 2, DAY, 0, text("in a chat")));
        toggle(2);
        assert!(visible());
        assert!(reply.is_sensitive());
        window.set_child(gtk::Widget::NONE);
    });

    eprintln!("selection mode: Forward refuses, guards and cancels");
    isolated(|| {
        let session = session();
        add_user(&session, 42);
        let chat = add_private_chat(&session, 42, 42, "Alice");
        let history = ChatHistory::new();
        history.set_chat(Some(&chat));
        for id in 1..=2 {
            new_message(&session, message(42, id, DAY + id as i32, 0, text("text")));
        }
        // The picker is a dialog of a window.
        let window = adw::Window::new();
        window.set_content(Some(&history));
        window.present();
        wait_until(&|| history.is_mapped());

        for id in 1..=2i64 {
            history
                .activate_action("chat-history.toggle-selection", Some(&id.to_variant()))
                .unwrap();
        }
        let cancel_enabled =
            || action_enabled(history.upcast_ref(), "chat-history.cancel-selection");

        // The properties of the test session never load (the client does not
        // answer), they are set by hand.
        let messages = history.selection().messages();
        let ids = history.selection().message_ids();
        let offer = || {
            // What `forward_selected` does before it asks TDLib
            history.imp().is_picking_chat.set(true);
            history.update_selection_ui();
            history.offer_forwarding(chat.clone(), messages.clone(), ids.clone());
        };
        messages[0].set_can_be_forwarded(true);
        messages[1].set_can_be_forwarded(false);
        offer();
        assert!(
            !history.imp().is_picking_chat.get(),
            "a message that can't be forwarded refuses the picker"
        );
        assert_eq!(window.dialogs().n_items(), 0);
        assert_eq!(history.selection().ids(), [1, 2], "the selection stays");
        assert!(cancel_enabled());

        messages[1].set_can_be_forwarded(true);
        offer();
        assert!(history.imp().is_picking_chat.get());
        assert_eq!(window.dialogs().n_items(), 1);
        assert!(!cancel_enabled(), "Escape belongs to the picker");

        let dialog = window.visible_dialog().unwrap();
        dialog.force_close();
        wait_until(&|| !history.imp().is_picking_chat.get());
        assert!(history.selection().active(), "closing the picker keeps it");
        assert!(cancel_enabled());

        // Activating Forward again while TDLib is still asked about the
        // properties (here: forever) must not queue a second picker, and
        // must not give Escape back.
        let pending = || {
            utils::spawn(clone!(
                #[weak]
                history,
                async move { history.forward_selected().await }
            ))
        };
        pending();
        // The spawned future runs on the context that `isolated` pushed.
        let context = glib::MainContext::ref_thread_default();
        for _ in 0..50 {
            context.iteration(false);
        }
        assert!(history.imp().is_picking_chat.get(), "set before the await");
        assert!(!cancel_enabled());
        pending();
        for _ in 0..50 {
            context.iteration(false);
        }
        assert!(!cancel_enabled());
        assert_eq!(window.dialogs().n_items(), 0);

        window.set_content(gtk::Widget::NONE);
        window.destroy();
    });

    eprintln!("selection mode: delete follows the choice for the same selection");
    isolated(|| {
        let session = session();
        add_user(&session, 42);
        let chat = add_private_chat(&session, 42, 42, "Alice");
        let history = ChatHistory::new();
        history.set_chat(Some(&chat));
        for id in 1..=2 {
            new_message(&session, message(42, id, DAY + id as i32, 0, text("text")));
        }
        let toggle = |id: i64| {
            history
                .activate_action("chat-history.toggle-selection", Some(&id.to_variant()))
                .unwrap()
        };
        toggle(1);
        let asked_about = history.selection().message_ids();
        toggle(2);

        history.delete_after_response("me", &asked_about);
        assert_eq!(
            history.selection().ids(),
            [1, 2],
            "the answer is about another selection"
        );
        history.delete_after_response("cancel", &[1, 2]);
        assert_eq!(history.selection().ids(), [1, 2], "cancelled");
        history.delete_after_response("everyone", &[1, 2]);
        assert!(!history.selection().active(), "deleted, mode left");
    });
}

/// Whether the button of the action is sensitive, which follows the state of
/// the action while the widget is in a window.
#[cfg(test)]
fn action_enabled(widget: &gtk::Widget, action: &str) -> bool {
    fn find(widget: &gtk::Widget, action: &str) -> Option<gtk::Widget> {
        if widget
            .dynamic_cast_ref::<gtk::Actionable>()
            .and_then(|a| a.action_name())
            .is_some_and(|name| name == action)
        {
            return Some(widget.clone());
        }
        let mut child = widget.first_child();
        while let Some(child_) = child {
            if let Some(found) = find(&child_, action) {
                return Some(found);
            }
            child = child_.next_sibling();
        }
        None
    }
    find(widget, action)
        .unwrap_or_else(|| panic!("a button for {action}"))
        .is_sensitive()
}
