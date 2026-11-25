mod album_layout;
mod base;
mod bubble;
mod document;
mod file_status;
mod indicators;
mod label;
mod location;
mod media_album;
mod media_album_grid;
mod media_album_layout;
mod media_download_button;
mod media_loader;
mod media_photo_tile;
mod media_picture;
mod media_thumbnail;
mod media_video_tile;
mod photo;
mod quick_reactions;
mod reaction_icon;
mod reactions;
mod reactions_chooser;
mod reply;
mod sticker;
mod text;
mod venue;
mod video;
mod voice;

use std::cell::Cell;
use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::subclass::prelude::*;
use gtk::CompositeTemplate;

pub(crate) use self::base::MessageBase;
pub(crate) use self::base::MessageBaseExt;
pub(crate) use self::base::MessageBaseImpl;
pub(crate) use self::bubble::MessageBubble;
pub(crate) use self::document::MessageDocument;
pub(crate) use self::document::StatusIndicator as MessageDocumentStatusIndicator;
pub(crate) use self::file_status::FileStatus;
pub(crate) use self::indicators::MessageIndicators;
pub(crate) use self::label::MessageLabel;
pub(crate) use self::location::MessageLocation;
pub(crate) use self::media_album::MessageMediaAlbum;
pub(crate) use self::media_album_grid::MediaAlbumGrid;
pub(crate) use self::media_download_button::MediaDownloadButton;
pub(crate) use self::media_loader::MediaLoader;
pub(crate) use self::media_photo_tile::MediaPhotoTile;
pub(crate) use self::media_picture::MediaPicture;
pub(crate) use self::media_thumbnail::MediaThumbnail;
pub(crate) use self::media_video_tile::MediaVideoTile;
pub(crate) use self::photo::MessagePhoto;
pub(crate) use self::quick_reactions::QuickReactions;
pub(crate) use self::reactions::MessageReactions;
pub(crate) use self::reactions_chooser::ReactionsChooser;
pub(crate) use self::reply::MessageReply;
pub(crate) use self::sticker::MessageSticker;
pub(crate) use self::text::MessageText;
pub(crate) use self::venue::MessageVenue;
pub(crate) use self::video::MessageVideo;
pub(crate) use self::voice::MessageVoiceNote;
use crate::model;
use crate::ui;
use crate::utils;

const AVATAR_SIZE: i32 = 32;
const SPACING: i32 = 6;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/mod.ui")]
    pub(crate) struct Row {
        /// A `model::Message` or `SponsoredMessage`
        pub(super) message: RefCell<Option<glib::Object>>,
        pub(super) content: RefCell<Option<gtk::Widget>>,
        pub(super) avatar: RefCell<Option<ui::Avatar>>,
        /// The signal handler ids of the `can_be_*` properties of the current
        /// message, which are used to update the actions when the properties
        /// are fetched lazily.
        pub(super) properties_handler_ids: RefCell<Vec<glib::SignalHandlerId>>,
        /// The signal handler id of the `available-reactions` property of the
        /// current message's chat, which is used to update the actions when
        /// the chat's reaction settings change.
        pub(super) chat_properties_handler_id: RefCell<Option<glib::SignalHandlerId>>,
        /// The round check mark of the selection mode, the first child.
        pub(super) check: gtk::Image,
        pub(super) check_added: Cell<bool>,
        pub(super) selection_active: Cell<bool>,
        pub(super) selected: Cell<bool>,
        /// Whether the text of the message is selectable (it is the only
        /// selected message).
        pub(super) text_selectable: Cell<bool>,
        /// Whether the current press may toggle the selection on release.
        pub(super) selection_armed: Cell<bool>,
        /// How often the selection click claimed its sequence.
        #[cfg(test)]
        pub(super) claims: Cell<u32>,
        #[template_child]
        pub(super) selection_click: TemplateChild<gtk::GestureClick>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Row {
        const NAME: &'static str = "PaplMessageRow";
        type Type = super::Row;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_instance_callbacks();

            klass.install_action("message-row.reply", None, move |widget, _, _| {
                widget.reply()
            });
            klass.install_action("message-row.edit", None, move |widget, _, _| widget.edit());
            klass.install_action("message-row.revoke-delete", None, move |widget, _, _| {
                widget.show_delete_dialog(true)
            });
            klass.install_action("message-row.delete", None, move |widget, _, _| {
                widget.show_delete_dialog(false)
            });
            klass.install_action("message-row.react-menu", None, move |widget, _, _| {
                widget.show_reactions_chooser()
            });
            klass.install_action("message-row.copy-text", None, move |widget, _, _| {
                widget.copy_text()
            });
            klass.install_action("message-row.select", None, move |widget, _, _| {
                widget.select()
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Row {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![glib::ParamSpecObject::builder::<glib::Object>("message")
                    .explicit_notify()
                    .build()]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "message" => obj.set_message(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "message" => obj.message().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().ensure_check();
        }

        fn dispose(&self) {
            self.check.unparent();

            if let Some(avatar) = self.avatar.borrow().as_ref() {
                avatar.unparent();
            }

            if let Some(content) = self.content.borrow().as_ref() {
                content.unparent();
            }
        }
    }

    impl WidgetImpl for Row {}
}

glib::wrapper! {
    pub(crate) struct Row(ObjectSubclass<imp::Row>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

#[gtk::template_callbacks]
impl Row {
    pub(crate) fn new(message: &glib::Object) -> Self {
        let layout_manager = gtk::BoxLayout::builder().spacing(SPACING).build();
        glib::Object::builder()
            .property("layout-manager", layout_manager)
            .property("message", message)
            .build()
    }

    #[template_callback]
    fn on_released(&self, n_press: i32, _x: f64, _y: f64) {
        if n_press == 2 && self.can_reply_to_message() {
            self.reply();
        }
    }

    /// Whether a click selects the message instead of using what is below it:
    /// always in the selection mode, with Ctrl otherwise.
    fn selection_click_wanted_now(&self, ctrl: bool) -> bool {
        // Sponsored messages can't be selected.
        self.message().is::<model::Message>()
            && selection_click_wanted(self.imp().selection_active.get(), ctrl)
    }

    #[template_callback]
    fn on_selection_pressed(&self, n_press: i32, x: f64, y: f64) {
        let imp = self.imp();
        imp.selection_armed.set(false);

        // Taps, double taps and drags on the selectable text belong to its
        // text view (words, handles). A tap elsewhere in the row still toggles.
        if let Some(text_view) = self.active_text_view() {
            let picked = self.pick(x, y, gtk::PickFlags::DEFAULT);
            if picked.is_some_and(|picked| picked == text_view || picked.is_ancestor(&text_view)) {
                return;
            }
        }

        let ctrl = imp
            .selection_click
            .current_event_state()
            .contains(gdk::ModifierType::CONTROL_MASK);
        if !self.selection_click_wanted_now(ctrl) {
            return;
        }

        // A mouse has no scroll gesture to protect, so claiming at once also
        // cancels the links and media below. A touch is claimed on release
        // instead: a drag that scrolls the history is then claimed by the
        // scrolled window first and never toggles the row.
        let is_touch = imp
            .selection_click
            .current_event_device()
            .is_some_and(|device| device.source() == gdk::InputSource::Touchscreen);

        // The second press of a double click with a mouse must not undo the
        // first, but it is still claimed so that it does not reply to the
        // message. GTK counts quick taps as multi-presses wherever they land,
        // and every tap is a toggle for the user (undoing a mis-tap), so a
        // touch is always armed.
        imp.selection_armed
            .set(press_arms_toggle(is_touch, n_press));

        if !is_touch {
            self.claim_selection_click();
        }
    }

    #[template_callback]
    fn on_selection_released(&self, _n_press: i32, _x: f64, _y: f64) {
        let imp = self.imp();
        if !imp.selection_armed.replace(false) {
            return;
        }

        // Cancels the link and media gestures below before they handle the
        // release.
        self.claim_selection_click();
        self.select();
    }

    fn claim_selection_click(&self) {
        let imp = self.imp();
        imp.selection_click
            .set_state(gtk::EventSequenceState::Claimed);
        // An emitted signal has no event sequence to claim, so tests count.
        #[cfg(test)]
        imp.claims.set(imp.claims.get() + 1);
    }

    #[template_callback]
    fn on_selection_cancelled(&self) {
        self.imp().selection_armed.set(false);
    }

    fn select(&self) {
        if let Ok(message) = self.message().downcast::<model::Message>() {
            self.activate_action(
                "chat-history.toggle-selection",
                Some(&message.id().to_variant()),
            )
            .unwrap();
        }
    }

    /// Creates the check mark and puts it first, before the avatar and the
    /// content.
    fn ensure_check(&self) {
        let imp = self.imp();
        if imp.check_added.replace(true) {
            return;
        }
        let check = &imp.check;

        check.set_icon_name(Some("object-select-symbolic"));
        check.add_css_class("selection-check");
        check.set_valign(gtk::Align::Center);
        check.set_can_target(false);
        check.set_visible(false);
        check.insert_after(self, gtk::Widget::NONE);
    }

    /// The text view of the message while its text is selectable.
    pub(crate) fn active_text_view(&self) -> Option<gtk::TextView> {
        self.imp()
            .content
            .borrow()
            .as_ref()
            .and_then(find_message_label)
            .and_then(|label| label.text_view())
    }

    fn message_label(&self) -> Option<ui::MessageLabel> {
        self.imp()
            .content
            .borrow()
            .as_ref()
            .and_then(find_message_label)
    }

    fn apply_text_selectable(&self) {
        if let Some(label) = self.message_label() {
            label.set_text_selectable(self.imp().text_selectable.get());
        }
    }

    pub(crate) fn selection_active(&self) -> bool {
        self.imp().selection_active.get()
    }

    pub(crate) fn is_selected(&self) -> bool {
        self.imp().selected.get()
    }

    /// Shows or hides the check mark and highlights the row if selected. The
    /// text of the message is selectable if `text_selectable`.
    pub(crate) fn set_selection_state(&self, active: bool, selected: bool, text_selectable: bool) {
        let imp = self.imp();
        imp.selection_active.set(active);
        imp.selected.set(selected);
        imp.text_selectable
            .set(active && selected && text_selectable);
        self.apply_text_selectable();
        // A link in a label claims the touch before the row sees it, so while
        // selecting, the label is out of the way and a tap reaches the row.
        if let Some(label) = self.message_label() {
            label.set_selecting(active);
        }
        // The menu of an already selected message has no Select item, which
        // would deselect it.
        self.update_actions();

        imp.check.set_visible(active);
        for widget in [imp.check.upcast_ref::<gtk::Widget>(), self.upcast_ref()] {
            let class = if widget == self.upcast_ref::<gtk::Widget>() {
                "selected-message"
            } else {
                "selected"
            };
            if active && selected {
                widget.add_css_class(class);
            } else {
                widget.remove_css_class(class);
            }
        }
    }

    fn reply(&self) {
        if let Ok(message) = self.message().downcast::<model::Message>() {
            self.activate_action("chat-history.reply", Some(&message.id().to_variant()))
                .unwrap();
        }
    }

    fn edit(&self) {
        if let Ok(message) = self.message().downcast::<model::Message>() {
            self.activate_action("chat-history.edit", Some(&message.id().to_variant()))
                .unwrap();
        }
    }

    fn show_reactions_chooser(&self) {
        let Ok(message) = self.message().downcast::<model::Message>() else {
            return;
        };

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let reactions = match message.available_reactions().await {
                    Ok(reactions) => reactions,
                    Err(e) => {
                        log::debug!("Error getting available message reactions: {e:?}");
                        return;
                    }
                };

                if reactions.is_empty() {
                    return;
                }

                let chooser = ui::ReactionsChooser::new(&message);
                chooser.set_reactions(reactions);
                chooser.set_parent(&obj);
                chooser.connect_closed(|chooser| chooser.unparent());
                chooser.popup();
            }
        ));
    }

    fn show_delete_dialog(&self, revoke: bool) {
        let window: gtk::Window = self.root().and_then(|root| root.downcast().ok()).unwrap();

        let message = if revoke {
            gettext("Do you want to delete this message for <b>everyone</b>?")
        } else {
            gettext("Do you want to delete this message?")
        };

        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Confirm Message Deletion"))
            .body_use_markup(true)
            .body(message)
            .build();

        dialog.add_responses(&[("no", &gettext("_No")), ("yes", &gettext("_Yes"))]);
        dialog.set_default_response(Some("no"));
        dialog.set_response_appearance("yes", adw::ResponseAppearance::Destructive);

        dialog.choose(
            Some(&window),
            gio::Cancellable::NONE,
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |response| {
                    if response == "yes" {
                        if let Ok(message) = obj.message().downcast::<model::Message>() {
                            utils::spawn(async move {
                                if let Err(e) = message.delete(revoke).await {
                                    log::warn!(
                                        "Error deleting a message (revoke = {}): {:?}",
                                        revoke,
                                        e
                                    );
                                }
                            });
                        }
                    }
                }
            ),
        );
    }

    pub(crate) fn message(&self) -> glib::Object {
        self.imp().message.borrow().clone().unwrap()
    }

    pub(crate) fn set_message(&self, message: glib::Object) {
        let imp = self.imp();

        if imp.message.borrow().as_ref() == Some(&message) {
            // The message is unchanged, but the way it is displayed may have
            // changed, e.g. when it became the representative of a media
            // album with a second member.
            self.update_content(message);
            return;
        }

        if let Some(message) = message.downcast_ref::<model::Message>() {
            let show_avatar = if message.is_outgoing() {
                false
            } else if message.chat_().is_own_chat() || message.chat_().is_replies_chat() {
                message.forward_info().is_some()
            } else {
                match message.chat_().chat_type() {
                    model::ChatType::BasicGroup(_) => true,
                    model::ChatType::Supergroup(supergroup) => !supergroup.is_channel(),
                    _ => false,
                }
            };

            if show_avatar {
                let avatar = {
                    let mut avatar_borrow = imp.avatar.borrow_mut();
                    if let Some(avatar) = avatar_borrow.clone() {
                        avatar
                    } else {
                        let avatar = ui::Avatar::new();
                        avatar.set_size(AVATAR_SIZE);
                        avatar.set_valign(gtk::Align::End);

                        // Insert after the check mark, at the beginning
                        self.ensure_check();
                        avatar.insert_after(self, Some(&imp.check));

                        *avatar_borrow = Some(avatar.clone());
                        avatar
                    }
                };

                if message.chat_().is_own_chat() || message.chat_().is_replies_chat() {
                    match message.forward_info().unwrap().origin() {
                        model::MessageForwardOrigin::User(user) => {
                            avatar.set_custom_text(None);
                            avatar.set_item(Some(user.upcast()));
                        }
                        model::MessageForwardOrigin::Chat { chat, .. }
                        | model::MessageForwardOrigin::Channel { chat, .. } => {
                            avatar.set_custom_text(None);
                            avatar.set_item(Some(chat.upcast()));
                        }
                        model::MessageForwardOrigin::HiddenUser { sender_name } => {
                            avatar.set_item(None);
                            avatar.set_custom_text(Some(&sender_name));
                        }
                    }
                } else {
                    let avatar_item = match message.sender() {
                        model::MessageSender::User(user) => user.upcast(),
                        model::MessageSender::Chat(chat) => chat.upcast(),
                    };
                    avatar.set_custom_text(None);
                    avatar.set_item(Some(avatar_item));
                }
            } else {
                if let Some(avatar) = imp.avatar.borrow().as_ref() {
                    avatar.unparent();
                }
                imp.avatar.replace(None);
            }
        }

        self.update_content(message.clone());

        // Disconnect the properties handlers of the previous message
        let previous = imp.message.replace(Some(message));
        if let Some(previous) = previous {
            if let Some(handler_id) = imp.chat_properties_handler_id.take() {
                if let Some(previous_message) = previous.downcast_ref::<model::Message>() {
                    previous_message.chat_().disconnect(handler_id);
                }
            }
            for handler_id in imp.properties_handler_ids.borrow_mut().drain(..) {
                previous.disconnect(handler_id);
            }
        } else {
            debug_assert!(imp.properties_handler_ids.borrow().is_empty());
        }

        // The `can_be_*` properties of a message are fetched lazily, when its
        // context menu is opened. Update the actions whenever they change.
        if let Some(message) = self.message().downcast_ref::<model::Message>() {
            let obj_weak = self.downgrade();

            for pspec_name in [
                "can-be-edited",
                "can-be-deleted-only-for-self",
                "can-be-deleted-for-all-users",
            ] {
                let obj_weak = obj_weak.clone();

                let handler_id = message.connect_notify_local(Some(pspec_name), move |_, _| {
                    if let Some(obj) = obj_weak.upgrade() {
                        obj.update_actions();
                    }
                });
                imp.properties_handler_ids.borrow_mut().push(handler_id);
            }

            // Reactions availability is a chat-level setting. Update the
            // actions whenever the chat's `available-reactions` changes.
            let handler_id =
                message
                    .chat_()
                    .connect_notify_local(Some("available-reactions"), move |_, _| {
                        if let Some(obj) = obj_weak.upgrade() {
                            obj.update_actions();
                        }
                    });
            imp.chat_properties_handler_id.replace(Some(handler_id));
        }

        self.update_actions();

        self.notify("message");
    }

    fn can_reply_to_message(&self) -> bool {
        if let Some(message) = self.message().downcast_ref::<model::Message>() {
            can_send_messages_in_chat(&message.chat_())
        } else {
            false
        }
    }

    fn can_edit_message(&self) -> bool {
        if let Some(message) = self.message().downcast_ref::<model::Message>() {
            let is_text_message = matches!(
                message.content().0,
                tdlib::enums::MessageContent::MessageText(_)
            );

            // TODO: Support more message types in the future
            is_text_message
                && message.can_be_edited()
                && can_send_messages_in_chat(&message.chat_())
        } else {
            false
        }
    }

    /// Prepares the context menu of this message.
    ///
    /// The `can_be_*` properties of a message are only fetched when its
    /// context menu is opened, instead of for every message in the history.
    fn prepare_message_menu(&self) {
        if let Some(message) = self.message().downcast_ref::<model::Message>() {
            message.fetch_properties();
        }
    }

    fn copy_text(&self) {
        let label = self
            .imp()
            .content
            .borrow()
            .as_ref()
            .and_then(find_message_label);

        let Some(label) = label else {
            return;
        };

        let text = label.selected_text().unwrap_or_else(|| label.plain_text());

        if text.is_empty() {
            return;
        }

        self.clipboard().set_text(&text);
        utils::show_toast(self, gettext("Copied to clipboard"));
    }

    fn update_actions(&self) {
        self.action_set_enabled("message-row.reply", self.can_reply_to_message());
        self.action_set_enabled("message-row.edit", self.can_edit_message());
        self.action_set_enabled(
            "message-row.select",
            self.message().is::<model::Message>()
                && !(self.selection_active() && self.is_selected()),
        );

        if let Some(message) = self.message().downcast_ref::<model::Message>() {
            self.action_set_enabled("message-row.delete", message.can_be_deleted_only_for_self());
            self.action_set_enabled(
                "message-row.revoke-delete",
                message.can_be_deleted_for_all_users(),
            );
        } else {
            self.action_set_enabled("message-row.delete", false);
            self.action_set_enabled("message-row.revoke-delete", false);
        }

        if let Some(message) = self.message().downcast_ref::<model::Message>() {
            self.action_set_enabled("message-row.react-menu", message.chat_().can_react());
        } else {
            self.action_set_enabled("message-row.react-menu", false);
        }
    }

    fn update_content(&self, message: glib::Object) {
        use tdlib::enums::MessageContent::*;

        let is_outgoing = if let Some(message_) = message.downcast_ref::<model::Message>() {
            // Do not mark channel messages as outgoing
            let is_outgoing = match message_.chat_().chat_type() {
                model::ChatType::Supergroup(data) if data.is_channel() => false,
                _ => message_.is_outgoing(),
            };

            if message_.media_album().is_some_and(|album| album.len() >= 2) {
                // The message represents a media album, which is displayed as
                // a grid of tiles instead of as its individual content.
                self.update_specific_content::<_, ui::MessageMediaAlbum>(message_);
            } else {
                match message_.content().0 {
                    MessageAnimation(_) | MessageVideo(_) => {
                        self.update_specific_content::<_, ui::MessageVideo>(message_);
                    }
                    MessageAnimatedEmoji(data)
                        if data
                            .animated_emoji
                            .sticker
                            .clone()
                            .map(|s| {
                                matches!(
                                    s.format,
                                    tdlib::enums::StickerFormat::Webp
                                        | tdlib::enums::StickerFormat::Tgs
                                        | tdlib::enums::StickerFormat::Webm
                                )
                            })
                            .unwrap_or_default() =>
                    {
                        self.update_specific_content::<_, ui::MessageSticker>(message_);
                    }
                    MessageLocation(_) | MessageLiveLocation(_) => {
                        self.update_specific_content::<_, ui::MessageLocation>(message_);
                    }
                    MessagePhoto(_) => {
                        self.update_specific_content::<_, ui::MessagePhoto>(message_);
                    }
                    MessageSticker(data)
                        if matches!(
                            data.sticker.format,
                            tdlib::enums::StickerFormat::Webp
                                | tdlib::enums::StickerFormat::Tgs
                                | tdlib::enums::StickerFormat::Webm
                        ) =>
                    {
                        self.update_specific_content::<_, ui::MessageSticker>(message_);
                    }
                    MessageDocument(_) | MessageAudio(_) => {
                        self.update_specific_content::<_, ui::MessageDocument>(message_);
                    }
                    MessageVoiceNote(_) => {
                        self.update_specific_content::<_, ui::MessageVoiceNote>(message_);
                    }
                    MessageVenue(_) => {
                        self.update_specific_content::<_, ui::MessageVenue>(message_);
                    }
                    _ => {
                        self.update_specific_content::<_, ui::MessageText>(&message);
                    }
                }
            }

            is_outgoing
        } else {
            self.update_specific_content::<_, ui::MessageText>(&message);
            false
        };

        let content_ref = self.imp().content.borrow();
        let content = content_ref.as_ref().unwrap();

        if is_outgoing {
            content.set_halign(gtk::Align::End);
        } else {
            content.set_halign(gtk::Align::Start);
        }
    }

    fn update_specific_content<M, B>(&self, message: &M)
    where
        B: MessageBaseExt<Message = M>,
    {
        let mut content_ref = self.imp().content.borrow_mut();
        match content_ref.as_ref().and_then(|c| c.downcast_ref::<B>()) {
            Some(content) => {
                content.set_message(message);
            }
            None => {
                if let Some(old_content) = &*content_ref {
                    // Do not leave a text view behind in a discarded label
                    if let Some(label) = find_message_label(old_content) {
                        label.set_text_selectable(false);
                    }
                    old_content.unparent();
                }
                let text_selectable = self.imp().text_selectable.get();
                log::debug!(
                    "Row::update_specific_content: creating {}",
                    std::any::type_name::<B>(),
                );

                let content = B::new(message);
                content.set_hexpand(true);
                content.set_valign(gtk::Align::Start);

                // Insert at the end
                content.insert_before(self, gtk::Widget::NONE);

                *content_ref = Some(content.upcast());
                drop(content_ref);
                // The new content has a new label
                if let Some(label) = self.message_label() {
                    label.set_selecting(self.selection_active());
                }
                if text_selectable {
                    self.apply_text_selectable();
                }
            }
        }
    }
}

/// Whether a press in the selection mode toggles the message on release. A
/// touch toggles on every tap, a mouse not on the second press of a double
/// click.
fn press_arms_toggle(is_touch: bool, n_press: i32) -> bool {
    is_touch || n_press == 1
}

/// Whether a click on a row selects it, see `Row::on_selection_pressed`.
fn selection_click_wanted(active: bool, ctrl: bool) -> bool {
    active || ctrl
}

fn find_message_label(widget: &gtk::Widget) -> Option<ui::MessageLabel> {
    if let Some(label) = widget.downcast_ref::<ui::MessageLabel>() {
        return Some(label.clone());
    }

    let mut child = widget.first_child();
    while let Some(child_) = child {
        if let Some(label) = find_message_label(&child_) {
            return Some(label);
        }
        child = child_.next_sibling();
    }

    None
}

pub(super) fn can_send_messages_in_chat(chat: &model::Chat) -> bool {
    use tdlib::enums::ChatMemberStatus::*;
    let member_status = match chat.chat_type() {
        model::ChatType::Supergroup(supergroup) => Some(supergroup.status()),
        model::ChatType::BasicGroup(supergroup) => Some(supergroup.status()),
        _ => None,
    };
    member_status
        .map(|s| match s.0 {
            Creator(_) => true,
            Administrator(_) => true,
            Member(_) => chat.permissions().0.can_send_basic_messages,
            Restricted(data) => {
                chat.permissions().0.can_send_basic_messages
                    && data.permissions.can_send_basic_messages
            }
            Left => false,
            Banned(_) => false,
        })
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click selects a row in the selection mode, or with Ctrl.
    #[test]
    fn taps_toggle_every_time() {
        assert!(press_arms_toggle(false, 1));
        assert!(!press_arms_toggle(false, 2));
        assert!(press_arms_toggle(true, 1));
        assert!(press_arms_toggle(true, 2));
        assert!(press_arms_toggle(true, 3));
    }

    #[test]
    fn selection_click_wanted() {
        assert!(!super::selection_click_wanted(false, false));
        assert!(super::selection_click_wanted(false, true));
        assert!(super::selection_click_wanted(true, false));
        assert!(super::selection_click_wanted(true, true));
    }
}

/// The checks of the message rows that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_checks() {
    rows_gtk_check();
    selection_row_gtk_check();
    bubble::gtk_check();
    voice::gtk_check();
    media_video_tile::gtk_check();
    label::gtk_check();
    reactions_chooser::gtk_check();
    reaction_icon::gtk_check();
    quick_reactions::gtk_check();
    base::gtk_check();
    video::gtk_check();
    document::gtk_check();

    eprintln!("message label lookup for Copy Text");

    // The text of a message is found wherever its label sits in the
    // content widget, e.g. below the media of a caption.
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&gtk::Picture::new());
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    content.append(&inner);
    assert!(find_message_label(content.upcast_ref()).is_none());

    let label: ui::MessageLabel = glib::Object::new();
    inner.append(&label);
    assert_eq!(
        find_message_label(content.upcast_ref()),
        Some(label.clone())
    );
    assert_eq!(find_message_label(label.upcast_ref()), Some(label));
}

/// Which widget a message row shows, the Replies chat avatars and Copy Text.
#[cfg(test)]
fn rows_gtk_check() {
    use adw::prelude::*;

    use crate::model::test_support::*;

    eprintln!("message rows: live locations, albums, Replies avatars, Copy Text");

    const REPLIES: i64 = 1271266957;

    isolated(|| {
        let session = session();
        let chat = add_chat(&session, ME);
        let history = model::ChatHistoryModel::new(&chat);
        let content = |row: &Row| row.imp().content.borrow().clone().unwrap();

        // A live location is shown like a location (#6a322b1f).
        let live = model::Message::new(&chat, message(ME, 1, DAY, 0, live_location(900)));
        let row = Row::new(live.upcast_ref());
        assert!(content(&row).is::<ui::MessageLocation>());

        // Albums of two or more are shown as an album grid (eaa45b1e), and
        // the row follows when its representative changes.
        new_message(&session, message(ME, 10, DAY, 77, photo("")));
        let first = chat.message(10).unwrap();
        let row = Row::new(first.upcast_ref());
        assert!(content(&row).is::<ui::MessagePhoto>());
        new_message(&session, message(ME, 11, DAY, 77, photo("")));
        let row = Row::new(first.upcast_ref());
        assert!(content(&row).is::<ui::MessageMediaAlbum>());

        // In the Replies chat, a reply shows the avatar of its original
        // author (#467).
        add_user(&session, REPLIES);
        add_user(&session, 42);
        let replies = add_private_chat(&session, REPLIES, REPLIES, "Replies");
        let mut td_reply = message(REPLIES, 1, DAY, 0, text("a reply"));
        td_reply.sender_id =
            tdlib::enums::MessageSender::User(tdlib::types::MessageSenderUser { user_id: REPLIES });
        td_reply.is_outgoing = false;
        td_reply.forward_info = forwarded_from(42);
        let reply = model::Message::new(&replies, td_reply);
        let row = Row::new(reply.upcast_ref());
        let avatar = row.imp().avatar.borrow().clone().expect("an avatar");
        let user = avatar.item().and_downcast::<model::User>().unwrap();
        assert_eq!(user.id(), 42);

        // Copy Text puts the plain text of the message on the clipboard and
        // says so (743880f7).
        let overlay = adw::ToastOverlay::new();
        let text_message = model::Message::new(&chat, message(ME, 2, DAY, 0, text("copy *me*")));
        let row = Row::new(text_message.upcast_ref());
        overlay.set_child(Some(&row));
        row.activate_action("message-row.copy-text", None).unwrap();
        let copied = glib::MainContext::default()
            .block_on(row.clipboard().read_text_future())
            .unwrap()
            .map(|t| t.to_string());
        assert_eq!(copied.as_deref(), Some("copy *me*"));

        // With a selection in the text view of the single selected message,
        // Copy Text copies the selection (and not the whole text).
        row.set_selection_state(true, true, true);
        let buffer = row.active_text_view().expect("a text view").buffer();
        buffer.select_range(&buffer.iter_at_offset(6), &buffer.iter_at_offset(8));
        row.activate_action("message-row.copy-text", None).unwrap();
        let copied = glib::MainContext::default()
            .block_on(row.clipboard().read_text_future())
            .unwrap()
            .map(|t| t.to_string());
        assert_eq!(copied.as_deref(), Some("me"));
        // A content type switch of the selected message keeps exactly one
        // text view, on the label of the new content.
        let old_label = find_message_label(&content(&row)).expect("a label");
        new_message(&session, message(ME, 3, DAY, 0, photo("a caption")));
        let photo_message = chat.message(3).unwrap();
        row.set_message(photo_message.upcast());
        assert!(
            old_label.text_view().is_none(),
            "the old label lost its text view"
        );
        let views = |row: &Row| usize::from(row.active_text_view().is_some());
        assert_eq!(views(&row), 1, "the caption of the photo has the view");
        row.set_message(text_message.clone().upcast());
        assert_eq!(views(&row), 1, "back to text: one text view");
        row.set_selection_state(false, false, false);
        assert!(row.active_text_view().is_none());

        drop(history);
    });
}

/// The check mark of a row in the selection mode and the Select menu item.
#[cfg(test)]
fn selection_row_gtk_check() {
    use crate::model::test_support::*;

    eprintln!("selection mode: row check mark follows recycling");

    isolated(|| {
        let session = session();
        let chat = add_chat(&session, ME);
        // The history keeps the messages alive, the chat only holds them weakly.
        let history = model::ChatHistoryModel::new(&chat);
        new_message(&session, message(ME, 1, DAY, 0, text("one")));
        new_message(&session, message(ME, 2, DAY, 0, text("two")));
        let (one, two) = (chat.message(1).unwrap(), chat.message(2).unwrap());

        let row = Row::new(one.upcast_ref());
        let check = row.imp().check.clone();
        assert!(
            !check.is_visible(),
            "no check mark outside the selection mode"
        );
        assert!(row.first_child() == Some(check.clone().upcast()));

        row.set_selection_state(true, true, false);
        assert!(check.is_visible());
        assert!(check.has_css_class("selected"));
        assert!(row.has_css_class("selected-message"));

        // The list view reuses the row for another message, which is not
        // selected: the state is computed from the new item.
        let selection = ui::MessageSelection::default();
        selection.set_chat(Some(&chat));
        selection.toggle(&[1]);
        let history_row = ui::ChatHistoryRow::new();
        history_row.set_item(Some(
            model::ChatHistoryItem::for_message(two.clone()).upcast(),
        ));
        let row = history_row.child().and_downcast::<Row>().unwrap();
        row.set_selection_state(true, true, false);
        history_row.sync_selection_with(&selection);
        assert!(row.imp().check.is_visible());
        assert!(!row.imp().check.has_css_class("selected"));
        assert!(!row.has_css_class("selected-message"));

        history_row.set_item(Some(
            model::ChatHistoryItem::for_message(one.clone()).upcast(),
        ));
        history_row.sync_selection_with(&selection);
        assert!(row.imp().check.has_css_class("selected"));
        drop(history);
    });

    eprintln!("selection mode: taps, long press and the Select item of a row");
    isolated(|| {
        let session = session();
        add_user(&session, 42);
        let chat = add_private_chat(&session, 42, 42, "Alice");
        let history = ui::ChatHistory::new();
        history.set_chat(Some(&chat));
        for id in 1..=2 {
            new_message(&session, message(42, id, DAY + id as i32, 0, text("text")));
        }
        let row = Row::new(chat.message(1).unwrap().upcast_ref());
        row.set_parent(&history);
        let click = row.imp().selection_click.clone();
        let tap = |n_press: i32| {
            click.emit_by_name::<()>("pressed", &[&n_press, &5f64, &5f64]);
            click.emit_by_name::<()>("released", &[&n_press, &5f64, &5f64]);
        };
        let claims = || row.imp().claims.get();
        let toggle = |id: i64| {
            history
                .activate_action("chat-history.toggle-selection", Some(&id.to_variant()))
                .unwrap()
        };

        // A plain click is for the links and the media below, not for the row.
        tap(1);
        assert!(!history.selection().active());
        assert_eq!(claims(), 0, "nothing is claimed outside the mode");

        // In the mode a tap toggles the message and claims the sequence, so
        // that the link or media below does not activate.
        toggle(2);
        row.set_selection_state(true, false, false);
        tap(1);
        assert_eq!(history.selection().ids(), [1, 2]);
        assert_eq!(claims(), 2, "claimed on press and on release");
        tap(1);
        assert_eq!(history.selection().ids(), [2]);

        // The second press of a double click with a mouse is claimed only.
        let before = claims();
        tap(2);
        assert_eq!(
            history.selection().ids(),
            [2],
            "no toggle by a double click"
        );
        assert_eq!(claims(), before + 1, "but claimed on press");

        // Links must not take the tap of a message that is being selected.
        let label = row.message_label().unwrap();
        assert_eq!(label.links_work(), (false, false));
        row.set_selection_state(false, false, false);
        assert_eq!(label.links_work(), (true, true));
        row.set_selection_state(true, false, false);

        // The Select item of an already selected message would deselect it.
        assert!(row.activate_action("message-row.select", None).is_ok());
        assert_eq!(history.selection().ids(), [1, 2], "Select selects");
        row.set_selection_state(true, true, false);
        row.activate_action("message-row.select", None).unwrap();
        assert_eq!(
            history.selection().ids(),
            [1, 2],
            "no Select item for a selected message"
        );
        history.selection().clear();
        row.set_selection_state(false, false, false);

        // A long press starts the mode with the message.
        let long_press = row
            .imp()
            .content
            .borrow()
            .as_ref()
            .unwrap()
            .observe_controllers()
            .iter::<glib::Object>()
            .find_map(|c| c.ok()?.downcast::<gtk::GestureLongPress>().ok())
            .expect("a long press gesture");
        long_press.emit_by_name::<()>("pressed", &[&5f64, &5f64]);
        assert_eq!(history.selection().ids(), [1]);

        row.unparent();
    });
}
