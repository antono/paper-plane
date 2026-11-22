use std::rc::Rc;

use adw::subclass::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::CompositeTemplate;

use crate::config;
use crate::model;
use crate::strings;
use crate::types::ChatId;
use crate::types::ClientId;
use crate::ui;
use crate::utils;
use crate::Application;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/window.ui")]
    pub(crate) struct Window {
        pub(super) settings: utils::PaperPlaneSettings,
        #[template_child]
        pub(super) client_manager_view: TemplateChild<ui::ClientManagerView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "PaplWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = &*self.obj();

            obj.client_manager_view()
                .client_manager()
                .connect_update_notification_group(clone!(
                    #[weak]
                    obj,
                    move |_, notification_group, session| {
                        obj.handle_notifications(notification_group, session);
                    }
                ));

            // Devel profile
            if config::PROFILE == "Devel" {
                obj.add_css_class("devel");
            }

            // Load latest window state
            obj.load_window_size();
        }
    }

    impl WidgetImpl for Window {}

    impl WindowImpl for Window {
        // Save window state on delete event
        fn close_request(&self) -> glib::Propagation {
            if let Err(err) = self.obj().save_window_size() {
                log::warn!("Failed to save window state, {}", err);
            }

            // If 'Run in Background' is enabled, keep the app alive in the
            // background by hiding the window instead of closing it, so that
            // notifications can still be received.
            if self.settings.boolean("run-in-background") {
                let app = self
                    .obj()
                    .application()
                    .unwrap()
                    .downcast::<Application>()
                    .unwrap();
                app.enter_background();

                self.obj().set_visible(false);

                // Close the viewed chat in TDLib so that notifications are
                // created for it while no chat is being viewed.
                self.obj().client_manager_view().set_chats_open(false);
                log::debug!("Window hidden, running in the background");

                glib::Propagation::Stop
            } else {
                // Let TDLib save its state and stop before the process exits
                self.obj()
                    .client_manager_view()
                    .client_manager()
                    .close_all();

                // Pass close request on to the parent
                self.parent_close_request()
            }
        }
    }

    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}

    #[gtk::template_callbacks]
    impl Window {
        #[template_callback]
        fn on_key_pressed(
            &self,
            key: gdk::Key,
            _: u32,
            modifier: gdk::ModifierType,
            _: &gtk::EventControllerKey,
        ) -> glib::Propagation {
            if key == gdk::Key::w && modifier == gdk::ModifierType::CONTROL_MASK {
                self.obj().close();
            }

            glib::Propagation::Proceed
        }

        #[template_callback]
        fn on_notify_is_active(&self) {
            self.obj().client_manager_view().set_active_client_online();
        }
    }
}

glib::wrapper! {
    pub(crate) struct Window(ObjectSubclass<imp::Window>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow;
}

impl Window {
    pub(crate) fn new(app: &Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    pub(crate) fn select_chat(&self, client_id: ClientId, chat_id: ChatId) {
        self.client_manager_view().select_chat(client_id, chat_id);
    }

    pub(crate) fn client_manager_view(&self) -> &ui::ClientManagerView {
        &self.imp().client_manager_view
    }

    fn handle_notifications(
        &self,
        notification_group: model::BoxedUpdateNotificationGroup,
        session: &model::ClientStateSession,
    ) {
        let app = self.application().unwrap();
        let send_app = app.downgrade();
        let send: SendNotification = Rc::new(move |id, notification| {
            if let Some(app) = send_app.upgrade() {
                app.send_notification(Some(id), notification);
            }
        });
        handle_notification_group(notification_group.0, session, send, &|id| {
            app.withdraw_notification(id)
        });
    }

    pub(crate) fn save_window_size(&self) -> Result<(), glib::BoolError> {
        let imp = self.imp();

        let (width, height) = self.default_size();
        imp.settings.set_int("window-width", width)?;
        imp.settings.set_int("window-height", height)?;

        imp.settings
            .set_boolean("is-maximized", self.is_maximized())?;

        Ok(())
    }

    fn load_window_size(&self) {
        let imp = self.imp();

        let width = imp.settings.int("window-width");
        let height = imp.settings.int("window-height");
        self.set_default_size(width, height);

        let is_maximized = imp.settings.boolean("is-maximized");
        if is_maximized {
            self.maximize();
        }
    }
}

/// Sends a notification under the given id.
type SendNotification = Rc<dyn Fn(&str, &gio::Notification)>;

/// The title and the body of a notification, `None` for the types that are
/// not shown.
fn notification_text(
    chat: &model::Chat,
    notification_type: tdlib::enums::NotificationType,
) -> Option<(String, String)> {
    match notification_type {
        tdlib::enums::NotificationType::NewMessage(data) => {
            let message = model::Message::new(chat, data.message);
            let mut body = strings::message_content(&message);

            // Add the sender's name to the body if the chat is a group
            if matches!(chat.chat_type(), model::ChatType::BasicGroup(_))
                || matches!(chat.chat_type(), model::ChatType::Supergroup(s) if !s.is_channel())
            {
                let sender_name = strings::message_sender(&message.sender(), true);
                body.insert_str(0, &(sender_name + ": "));
            }

            Some((chat.title(), body))
        }
        tdlib::enums::NotificationType::NewCall(_) => {
            Some((chat.title(), gettext("Incoming call")))
        }
        other => {
            log::debug!("Notification: ignoring type {other:?}");
            None
        }
    }
}

/// Sends the added notifications of a group at once, also when the avatar of
/// the chat is not downloaded yet (it is added when it is), and withdraws the
/// removed ones.
fn handle_notification_group(
    notification_group: tdlib::types::UpdateNotificationGroup,
    session: &model::ClientStateSession,
    send: SendNotification,
    withdraw: &dyn Fn(&str),
) {
    log::debug!(
        "Handling notification group update: {} added, {} removed",
        notification_group.added_notifications.len(),
        notification_group.removed_notification_ids.len()
    );

    let chat = session.chat(notification_group.chat_id);

    for notification in notification_group.added_notifications {
        let notification_id = notification.id.to_string();

        let Some((title, body)) = notification_text(&chat, notification.r#type) else {
            continue;
        };
        let notification = gio::Notification::new(&title);
        notification.set_body(Some(&body));
        notification.set_default_action_and_target_value(
            "app.select-chat",
            Some(&(session.client_().id(), chat.id()).to_variant()),
        );

        let Some(avatar) = chat.avatar() else {
            log::debug!("Notification {notification_id}: sending without icon");
            send(&notification_id, &notification);
            continue;
        };

        let avatar_file = &avatar.0;
        if avatar_file.local.is_downloading_completed {
            match gdk::Texture::from_filename(&avatar_file.local.path) {
                Ok(texture) => notification.set_icon(&texture),
                Err(e) => log::warn!(
                    "Notification {notification_id}: failed to load avatar texture: {e:?}"
                ),
            }
            log::debug!("Notification {notification_id}: sending with loaded avatar");
            send(&notification_id, &notification);
            continue;
        }

        log::debug!(
            "Notification {notification_id}: sending without icon, downloading {}",
            avatar_file.id
        );
        send(&notification_id, &notification);

        let file_id = avatar_file.id;
        let send = send.clone();
        utils::spawn(clone!(
            #[weak]
            session,
            async move {
                match session.download_file(file_id).await {
                    Ok(file) => match gdk::Texture::from_filename(file.local.path) {
                        Ok(texture) => {
                            notification.set_icon(&texture);
                            log::debug!(
                                "Notification {notification_id}: re-sending with downloaded avatar"
                            );
                            send(&notification_id, &notification);
                        }
                        Err(e) => log::warn!("Failed to load a downloaded avatar: {e:?}"),
                    },
                    Err(e) => log::warn!("Failed to download an avatar: {e:?}"),
                }
            }
        ));
    }

    for notification_id in notification_group.removed_notification_ids {
        log::debug!("Notification {notification_id}: withdrawing");
        withdraw(&notification_id.to_string());
    }
}

/// Checks of the notifications (5418fc72) that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use std::cell::RefCell;

    use crate::model::test_support::*;

    eprintln!("notifications are sent at once");

    isolated(|| {
        let session = session();
        add_user(&session, 42);
        let alice = add_private_chat(&session, 42, 42, "Alice");
        add_user(&session, 43);
        let bob = add_private_chat(&session, 43, 43, "Bob");
        // Bob's photo is not downloaded yet
        session.handle_update(tdlib::enums::Update::ChatPhoto(
            tdlib::types::UpdateChatPhoto {
                chat_id: bob.id(),
                photo: Some(tdlib::types::ChatPhotoInfo {
                    small: file(500),
                    big: file(501),
                    minithumbnail: None,
                    has_animation: false,
                    is_personal: false,
                }),
            },
        ));
        assert!(alice.avatar().is_none());
        assert!(bob.avatar().is_some());

        let new_message = |chat: &model::Chat, id: i32, body: &str| tdlib::types::Notification {
            id,
            date: DAY,
            is_silent: false,
            r#type: tdlib::enums::NotificationType::NewMessage(
                tdlib::types::NotificationTypeNewMessage {
                    message: message(chat.id(), id as i64, DAY, 0, text(body)),
                    show_preview: true,
                },
            ),
        };
        let group = |chat: &model::Chat, added, removed| tdlib::types::UpdateNotificationGroup {
            notification_group_id: 1,
            r#type: tdlib::enums::NotificationGroupType::Messages,
            chat_id: chat.id(),
            notification_settings_chat_id: chat.id(),
            notification_sound_id: 0,
            total_count: 1,
            added_notifications: added,
            removed_notification_ids: removed,
        };

        assert_eq!(
            notification_text(&alice, new_message(&alice, 1, "hi").r#type),
            Some(("Alice".to_owned(), "hi".to_owned()))
        );
        assert_eq!(
            notification_text(
                &alice,
                tdlib::enums::NotificationType::NewCall(tdlib::types::NotificationTypeNewCall {
                    call_id: 7
                })
            ),
            Some(("Alice".to_owned(), gettext("Incoming call")))
        );

        let sent = Rc::new(RefCell::new(Vec::<String>::new()));
        let withdrawn = RefCell::new(Vec::<String>::new());
        let send: SendNotification = {
            let sent = sent.clone();
            Rc::new(move |id, _| sent.borrow_mut().push(id.to_owned()))
        };

        // A chat without a photo, and a chat whose photo is still to be
        // downloaded: both are sent right away, not after the download.
        handle_notification_group(
            group(&alice, vec![new_message(&alice, 1, "hi")], vec![]),
            &session,
            send.clone(),
            &|id| withdrawn.borrow_mut().push(id.to_owned()),
        );
        handle_notification_group(
            group(&bob, vec![new_message(&bob, 2, "yo")], vec![1]),
            &session,
            send,
            &|id| withdrawn.borrow_mut().push(id.to_owned()),
        );
        assert_eq!(*sent.borrow(), ["1", "2"]);
        assert_eq!(*withdrawn.borrow(), ["1"]);
    });
}
