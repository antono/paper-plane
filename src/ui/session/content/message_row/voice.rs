use std::cell::Cell;
use std::cell::RefCell;
use std::sync::OnceLock;

use glib::clone;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::CompositeTemplate;
use ui::MessageBaseExt;

use super::FileStatus;
use super::MediaDownloadButton;
use super::MediaLoader;
use crate::model;
use crate::model::MediaType;
use crate::ui;
use crate::ui::session::playback_manager;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/voice.ui")]
    pub(crate) struct MessageVoiceNote {
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) handler_id: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) loader_handler_id: RefCell<Option<glib::SignalHandlerId>>,
        /// The handlers of the shared media stream and of the playback
        /// manager that keep the play button and the waveform progress in
        /// sync with the playback. Connected while the row is mapped.
        pub(super) stream_handler_ids: RefCell<Vec<glib::SignalHandlerId>>,
        pub(super) changed_handler_id: Cell<Option<playback_manager::ChangedHandlerId>>,
        /// Whether the playback should start once the download of the voice
        /// note finishes.
        pub(super) play_when_downloaded: Cell<bool>,
        /// The shared playback engine the row follows while it is mapped.
        pub(super) playback: RefCell<Option<playback_manager::PlaybackManager>>,
        /// The playback engine of a test, which has no session view.
        #[cfg(test)]
        pub(super) test_playback: RefCell<Option<playback_manager::PlaybackManager>>,
        pub(super) loader: MediaLoader,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) waveform: TemplateChild<ui::Waveform>,
        #[template_child]
        pub(super) duration_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) play_image: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) download_button: TemplateChild<MediaDownloadButton>,
        #[template_child]
        pub(super) click: TemplateChild<gtk::GestureClick>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageVoiceNote {
        const NAME: &'static str = "PaplMessageVoiceNote";
        type Type = super::MessageVoiceNote;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.set_css_name("messagevoice");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageVoiceNote {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![glib::ParamSpecObject::builder::<model::Message>("message")
                    .explicit_notify()
                    .build()]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "message" => self.obj().set_message(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "message" => self.message.upgrade().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            let handler_id = self.loader.connect_status_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.update_status_ui();
                }
            ));
            *self.loader_handler_id.borrow_mut() = Some(handler_id);

            self.click.connect_released(clone!(
                #[weak]
                obj,
                move |_, _, _, _| {
                    obj.handle_click();
                }
            ));

            obj.connect_map(|obj| obj.connect_playback());
            obj.connect_unmap(|obj| obj.disconnect_playback());
        }

        fn dispose(&self) {
            let obj = self.obj();

            if let Some(handler_id) = self.loader_handler_id.borrow_mut().take() {
                self.loader.disconnect(handler_id);
            }

            obj.disconnect_playback();
        }
    }

    impl WidgetImpl for MessageVoiceNote {}
    impl ui::MessageBaseImpl for MessageVoiceNote {}
}

glib::wrapper! {
    pub(crate) struct MessageVoiceNote(ObjectSubclass<imp::MessageVoiceNote>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MessageBaseExt for MessageVoiceNote {
    type Message = model::Message;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        let old_message = imp.message.upgrade();
        if old_message.as_ref() == Some(message) {
            return;
        }

        if let Some(old_message) = old_message {
            let handler_id = imp.handler_id.take().unwrap();
            old_message.disconnect(handler_id);
        }

        imp.message_bubble.update_from_message(message, true);
        imp.message_bubble.add_message_label_class("caption");
        imp.message_bubble.add_message_label_class("dim-label");

        // Update the message.
        let handler_id = message.connect_content_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |message| {
                obj.update_row(message);
            }
        ));
        imp.handler_id.replace(Some(handler_id));
        self.update_row(message);

        imp.message.set(Some(message));
        self.sync_playback_ui();
        self.notify("message");
    }
}

impl MessageVoiceNote {
    /// The shared playback engine of the session, if it is still alive.
    fn playback_manager(&self) -> Option<playback_manager::PlaybackManager> {
        #[cfg(test)]
        if let Some(manager) = self.imp().test_playback.borrow().clone() {
            return Some(manager);
        }
        self.ancestor(ui::Session::static_type())
            .and_downcast::<ui::Session>()
            .map(|session| session.playback_manager().clone())
    }

    /// The path of the voice note, if it has been downloaded.
    fn media_path(&self) -> Option<glib::GString> {
        if self.imp().loader.status() != FileStatus::Downloaded {
            return None;
        }

        self.imp().loader.path()
    }

    fn update_row(&self, message: &model::Message) {
        match message.content().0 {
            tdlib::enums::MessageContent::MessageVoiceNote(td_message) => {
                let imp = self.imp();

                let voice_note = td_message.voice_note;

                imp.waveform.set_waveform(&voice_note.waveform);
                imp.duration_label
                    .set_text(&format_duration(voice_note.duration));
                imp.message_bubble
                    .set_label(utils::parse_formatted_text(td_message.caption));

                imp.play_when_downloaded.set(false);
                imp.loader.bind(
                    &message.chat_().session_(),
                    MediaType::File,
                    &voice_note.voice,
                );
                imp.loader.maybe_start_auto_download();

                self.update_status_ui();
            }
            _ => unreachable!(),
        }
    }

    /// Updates the widgets according to the status of the loader.
    fn update_status_ui(&self) {
        let imp = self.imp();
        let status = imp.loader.status();

        match status {
            FileStatus::Downloaded => {
                imp.download_button.set_visible(false);
                self.sync_playback_ui();

                if imp.play_when_downloaded.take() {
                    self.toggle_playback();
                }
            }
            // Uploads are driven by the sender side, so there is nothing to
            // react to.
            FileStatus::Uploading(_) => imp.download_button.set_visible(false),
            _ => {
                imp.download_button.set_visible(true);
                imp.download_button.set_status(status);
            }
        }
    }

    fn handle_click(&self) {
        let imp = self.imp();

        match imp.loader.status() {
            // Clicking again cancels the download.
            FileStatus::Downloading(_) => imp.loader.cancel_download(),
            // Download the voice note and start playing it as soon as it is
            // done.
            FileStatus::CanBeDownloaded => {
                imp.play_when_downloaded.set(true);
                imp.loader.request_download();
            }
            // Uploads are driven by the sender side, so there is nothing to
            // react to.
            FileStatus::Uploading(_) => {}
            FileStatus::Downloaded => self.toggle_playback(),
        }
    }

    /// Plays or pauses the voice note on the shared media stream of the
    /// session.
    fn toggle_playback(&self) {
        let Some(manager) = self.playback_manager() else {
            return;
        };
        let Some(path) = self.media_path() else {
            return;
        };
        let Some(message) = self.imp().message.upgrade() else {
            return;
        };

        if manager.is_current(&path) {
            manager.toggle_pause();
            return;
        }

        manager.play_file(
            &path,
            || {},
            |media: &gtk::MediaFile| {
                // Set on the prepared stream, see MediaViewerPage
                media.set_muted(false);
                media.set_loop(false);
                media.play();
            },
            || {},
        );
        manager.set_audio_message(&message);
    }

    /// Connects the handlers that keep the play button and the waveform
    /// progress in sync with the shared playback.
    ///
    /// Every mapped voice note row follows the playback, so that a row shows
    /// the playback of its voice note also when it was recycled or created
    /// after the playback started.
    fn connect_playback(&self) {
        self.disconnect_playback();

        let Some(manager) = self.playback_manager() else {
            return;
        };
        let imp = self.imp();
        let media = manager.media_file();

        let ids = ["playing", "timestamp", "duration", "prepared", "ended"]
            .into_iter()
            .map(|property| {
                media.connect_notify_local(
                    Some(property),
                    clone!(
                        #[weak(rename_to = obj)]
                        self,
                        move |_: &gtk::MediaFile, _| obj.sync_playback_ui(),
                    ),
                )
            })
            .collect();
        *imp.stream_handler_ids.borrow_mut() = ids;

        imp.changed_handler_id
            .set(Some(manager.connect_changed(clone!(
                #[weak(rename_to = obj)]
                self,
                move || obj.sync_playback_ui()
            ))));
        *imp.playback.borrow_mut() = Some(manager);

        self.sync_playback_ui();
    }

    /// Disconnects the handlers of `connect_playback`.
    fn disconnect_playback(&self) {
        let imp = self.imp();

        let Some(manager) = imp.playback.take() else {
            return;
        };
        let media = manager.media_file();
        for id in imp.stream_handler_ids.take() {
            media.disconnect(id);
        }
        if let Some(id) = imp.changed_handler_id.take() {
            manager.disconnect_changed(id);
        }
    }

    /// Whether `manager` plays the voice note of the row.
    ///
    /// The voice note is recognized by its message, since the row of a
    /// voice note that is played may not know the downloaded file yet (a
    /// recycled one, for example).
    fn is_played_by(&self, manager: &playback_manager::PlaybackManager) -> bool {
        let Some(message) = self.imp().message.upgrade() else {
            return false;
        };
        match manager.audio_message() {
            Some(played) => {
                played == message
                    || (played.id() == message.id() && played.chat_().id() == message.chat_().id())
            }
            None => self
                .media_path()
                .is_some_and(|path| manager.is_current(&path)),
        }
    }

    /// Shows whether the voice note is played and how far.
    fn sync_playback_ui(&self) {
        let imp = self.imp();

        let current = imp
            .playback
            .borrow()
            .clone()
            .or_else(|| self.playback_manager())
            .filter(|manager| self.is_played_by(manager))
            .map(|manager| manager.media_file());

        let (playing, progress) = match current {
            Some(media) => {
                // The stream is paused until it is prepared, so a pending
                // playback is shown as a playing one.
                let pending = !media.is_prepared() && media.error().is_none();
                let duration = media.duration();
                let progress = if duration > 0 && !media.is_ended() {
                    media.timestamp() as f64 / duration as f64
                } else {
                    0.0
                };
                (media.is_playing() || pending, progress)
            }
            None => (false, 0.0),
        };

        imp.play_image.set_icon_name(Some(if playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        }));
        imp.waveform.set_progress(progress);
    }
}

fn format_duration(seconds: i32) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;

    format!("{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn formats_minutes_and_seconds() {
        assert_eq!(format_duration(0), "00:00");
        assert_eq!(format_duration(7), "00:07");
        assert_eq!(format_duration(137), "02:17");
        assert_eq!(format_duration(3600), "60:00");
    }
}

/// Checks of the voice note row that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use crate::model::test_support::*;

    eprintln!("voice notes play from their row");

    isolated(|| {
        let session = session();
        let chat = add_chat(&session, ME);
        let wav = playback_manager::tests::silent_wav("voice-note");
        // Larger than what is downloaded automatically
        let mut voice = file(600);
        voice.size = 1 << 30;
        voice.expected_size = 1 << 30;
        let content =
            tdlib::enums::MessageContent::MessageVoiceNote(tdlib::types::MessageVoiceNote {
                voice_note: tdlib::types::VoiceNote {
                    duration: 1,
                    waveform: String::new(),
                    mime_type: "audio/wav".to_owned(),
                    speech_recognition_result: None,
                    voice: voice.clone(),
                },
                caption: formatted_text(""),
                is_listened: false,
            });
        let message = model::Message::new(&chat, message(ME, 1, DAY, 0, content));

        let row: MessageVoiceNote = glib::Object::new();
        let manager = playback_manager::PlaybackManager::new();
        row.imp().test_playback.replace(Some(manager.clone()));
        row.set_message(&message);
        let imp = row.imp();

        // The row follows the playback while it is shown.
        let window = gtk::Window::new();
        window.set_child(Some(&row));
        window.present();
        let context = glib::MainContext::ref_thread_default();
        while !row.is_mapped() {
            context.iteration(true);
        }
        let icon = || imp.play_image.icon_name().unwrap_or_default();

        // Not downloaded: a click downloads the voice note (f966e155)...
        assert!(imp.download_button.is_visible());
        row.handle_click();
        assert!(imp.play_when_downloaded.get());
        assert!(matches!(imp.loader.status(), FileStatus::Downloading(_)));

        // ...and plays it as soon as the download is done.
        let mut downloaded = voice;
        downloaded.local.path = wav.to_str().unwrap().to_owned();
        downloaded.local.is_downloading_completed = true;
        downloaded.local.downloaded_size = downloaded.size;
        session.handle_update(tdlib::enums::Update::File(tdlib::types::UpdateFile {
            file: downloaded,
        }));
        while context.iteration(false) {}

        assert_eq!(imp.loader.status(), FileStatus::Downloaded);
        assert!(!imp.download_button.is_visible());
        assert!(!imp.play_when_downloaded.get());
        assert!(
            manager.is_current(wav.to_str().unwrap()),
            "the voice note is not played"
        );
        assert_eq!(manager.audio_message().as_ref(), Some(&message));
        assert_eq!(icon(), "media-playback-pause-symbolic");

        // Once prepared it plays; a second click pauses it.
        let media = manager.media_file();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !media.is_playing() && std::time::Instant::now() < deadline {
            context.iteration(false);
        }
        assert!(
            media.is_playing(),
            "the voice note did not start: {:?}",
            media.error()
        );
        assert_eq!(icon(), "media-playback-pause-symbolic");
        row.handle_click();
        assert!(!media.is_playing());
        assert_eq!(icon(), "media-playback-start-symbolic");
        if media.timestamp() > 0 {
            assert!(imp.waveform.progress() > 0.0, "the progress is not shown");
        }

        // Another row of the same voice note (a recycled one, for example)
        // shows the playback as well.
        let other: MessageVoiceNote = glib::Object::new();
        other.imp().test_playback.replace(Some(manager.clone()));
        other.set_message(&message);
        window.set_child(Some(&other));
        while !other.is_mapped() {
            context.iteration(true);
        }
        assert_eq!(
            other.imp().play_image.icon_name().unwrap_or_default(),
            "media-playback-start-symbolic"
        );
        manager.toggle_pause();
        // The short file may have ended already, which stops the playback.
        let expected = if media.is_playing() {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        };
        assert_eq!(
            other.imp().play_image.icon_name().unwrap_or_default(),
            expected
        );

        manager.stop();
        assert!(manager.audio_message().is_none());
        assert_eq!(
            other.imp().play_image.icon_name().unwrap_or_default(),
            "media-playback-start-symbolic"
        );
        window.destroy();
    });
}
