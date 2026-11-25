//! The bar that shows the voice note that is played, with its progress and
//! buttons to pause and to close it, like the player of the Telegram apps.
//!
//! The bar follows the shared playback of the session, see
//! `PlaybackManager`, so every instance of it (above the chat history and,
//! in the narrow layout, above the chat list) shows the same playback, also
//! when another chat is opened.

use std::cell::Cell;
use std::cell::RefCell;
use std::sync::OnceLock;

use gettextrs::gettext;
use glib::clone;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::playback_manager;
use crate::model;
use crate::strings;
use crate::ui;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct AudioPlayerBar {
        /// Whether the bar may be shown at all. The bar above the chat list
        /// is only enabled in the narrow layout, where the chat history is
        /// not visible next to it.
        pub(super) enabled: Cell<bool>,
        /// The shared playback engine the bar follows while it is mapped.
        pub(super) playback: RefCell<Option<playback_manager::PlaybackManager>>,
        pub(super) stream_handler_ids: RefCell<Vec<glib::SignalHandlerId>>,
        pub(super) changed_handler_id: Cell<Option<playback_manager::ChangedHandlerId>>,
        pub(super) revealer: gtk::Revealer,
        pub(super) play_button: gtk::Button,
        pub(super) title_label: gtk::Label,
        pub(super) subtitle_label: gtk::Label,
        pub(super) progress_bar: gtk::ProgressBar,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AudioPlayerBar {
        const NAME: &'static str = "PaplAudioPlayerBar";
        type Type = super::AudioPlayerBar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("audioplayerbar");
            klass.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for AudioPlayerBar {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![glib::ParamSpecBoolean::builder("enabled")
                    .default_value(true)
                    .explicit_notify()
                    .build()]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "enabled" => self.obj().set_enabled(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "enabled" => self.enabled.get().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = &*self.obj();
            self.enabled.set(true);

            self.play_button
                .set_icon_name("media-playback-start-symbolic");
            self.play_button.set_valign(gtk::Align::Center);
            self.play_button.add_css_class("flat");
            self.play_button.add_css_class("circular");
            self.play_button.set_tooltip_text(Some(&gettext("Play")));
            self.play_button.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| {
                    let manager = obj.imp().playback.borrow().clone();
                    if let Some(manager) = manager {
                        manager.toggle_pause();
                    }
                }
            ));

            self.title_label.set_xalign(0.0);
            self.title_label
                .set_ellipsize(gtk::pango::EllipsizeMode::End);
            self.title_label.add_css_class("heading");

            self.subtitle_label.set_xalign(0.0);
            self.subtitle_label
                .set_ellipsize(gtk::pango::EllipsizeMode::End);
            self.subtitle_label.add_css_class("caption");
            self.subtitle_label.add_css_class("dim-label");
            self.subtitle_label.add_css_class("numeric");

            let labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
            labels.set_hexpand(true);
            labels.set_valign(gtk::Align::Center);
            labels.append(&self.title_label);
            labels.append(&self.subtitle_label);
            labels.set_cursor_from_name(Some("pointer"));
            labels.set_tooltip_text(Some(&gettext("Show the Message")));
            let click = gtk::GestureClick::builder().button(1).build();
            click.connect_released(clone!(
                #[weak]
                obj,
                move |_, _, _, _| obj.open_chat()
            ));
            labels.add_controller(click);

            let close_button = gtk::Button::from_icon_name("window-close-symbolic");
            close_button.set_valign(gtk::Align::Center);
            close_button.add_css_class("flat");
            close_button.add_css_class("circular");
            close_button.set_tooltip_text(Some(&gettext("Close Player")));
            close_button.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| {
                    let manager = obj.imp().playback.borrow().clone();
                    if let Some(manager) = manager {
                        manager.stop();
                    }
                }
            ));

            let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            controls.add_css_class("controls");
            controls.append(&self.play_button);
            controls.append(&labels);
            controls.append(&close_button);

            self.progress_bar.set_hexpand(true);

            let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
            content.append(&controls);
            content.append(&self.progress_bar);

            self.revealer.set_child(Some(&content));
            self.revealer
                .set_transition_type(gtk::RevealerTransitionType::SlideDown);
            self.revealer.set_parent(obj);

            // Hidden until something plays, so that it takes no room.
            obj.set_visible(false);
            self.revealer.connect_child_revealed_notify(clone!(
                #[weak]
                obj,
                move |revealer| {
                    if !revealer.reveals_child() {
                        obj.set_visible(false);
                    }
                }
            ));

            obj.connect_root_notify(|obj| obj.connect_playback());
        }

        fn dispose(&self) {
            self.obj().disconnect_playback();
            self.revealer.unparent();
        }
    }

    impl WidgetImpl for AudioPlayerBar {}
}

glib::wrapper! {
    pub(crate) struct AudioPlayerBar(ObjectSubclass<imp::AudioPlayerBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for AudioPlayerBar {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl AudioPlayerBar {
    pub(crate) fn set_enabled(&self, enabled: bool) {
        let imp = self.imp();
        if imp.enabled.get() == enabled {
            return;
        }
        imp.enabled.set(enabled);
        self.sync();
        self.notify("enabled");
    }

    /// Follows the playback engine of the session the bar is in.
    ///
    /// The bar is connected as long as it is in a window (not only while it
    /// is mapped), since it is hidden, and so unmapped, while nothing plays.
    fn connect_playback(&self) {
        let manager = self
            .ancestor(ui::Session::static_type())
            .and_downcast::<ui::Session>()
            .map(|session| session.playback_manager().clone());
        self.connect_to(manager);
    }

    fn connect_to(&self, manager: Option<playback_manager::PlaybackManager>) {
        self.disconnect_playback();

        let Some(manager) = manager else {
            self.sync();
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
                        move |_: &gtk::MediaFile, _| obj.sync(),
                    ),
                )
            })
            .collect();
        *imp.stream_handler_ids.borrow_mut() = ids;

        imp.changed_handler_id
            .set(Some(manager.connect_changed(clone!(
                #[weak(rename_to = obj)]
                self,
                move || obj.sync()
            ))));
        *imp.playback.borrow_mut() = Some(manager);

        self.sync();
    }

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

    /// The voice note the bar shows, if any.
    fn message(&self) -> Option<model::Message> {
        self.imp()
            .playback
            .borrow()
            .as_ref()
            .and_then(|manager| manager.audio_message())
    }

    /// Updates the bar from the state of the playback.
    fn sync(&self) {
        let imp = self.imp();

        let message = self.message().filter(|_| imp.enabled.get());
        let Some(message) = message else {
            imp.revealer.set_reveal_child(false);
            if !imp.revealer.is_child_revealed() {
                self.set_visible(false);
            }
            return;
        };
        let Some(manager) = imp.playback.borrow().clone() else {
            return;
        };
        let media = manager.media_file();

        self.set_visible(true);
        imp.revealer.set_reveal_child(true);

        // The stream is paused until it is prepared, so a pending playback
        // is shown as a playing one.
        let pending = !media.is_prepared() && media.error().is_none();
        let playing = media.is_playing() || pending;
        imp.play_button.set_icon_name(if playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        imp.play_button.set_tooltip_text(Some(&if playing {
            gettext("Pause")
        } else {
            gettext("Play")
        }));

        imp.title_label
            .set_label(&strings::message_sender(&message.sender(), true));

        let duration = media.duration();
        let timestamp = media.timestamp();
        let duration = if duration > 0 {
            duration
        } else {
            voice_note_duration(&message).unwrap_or_default()
        };
        let chat = message.chat_();
        let mut subtitle = format!("{} / {}", format_time(timestamp), format_time(duration));
        let sender = imp.title_label.label();
        let chat_title = chat.title();
        if chat_title != sender.as_str() {
            subtitle = format!("{subtitle} · {chat_title}");
        }
        imp.subtitle_label.set_label(&subtitle);

        imp.progress_bar.set_fraction(if duration > 0 {
            (timestamp as f64 / duration as f64).clamp(0.0, 1.0)
        } else {
            0.0
        });
    }

    /// Opens the chat of the voice note.
    fn open_chat(&self) {
        let Some(message) = self.message() else {
            return;
        };
        let Some(session) = self
            .ancestor(ui::Session::static_type())
            .and_downcast::<ui::Session>()
        else {
            return;
        };

        session.select_chat(message.chat_().id());
        session.set_chat_open(true);
    }
}

/// The duration of the voice note `message` in microseconds, as the stream
/// reports it.
fn voice_note_duration(message: &model::Message) -> Option<i64> {
    match message.content().0 {
        tdlib::enums::MessageContent::MessageVoiceNote(data) => {
            Some(i64::from(data.voice_note.duration) * 1_000_000)
        }
        _ => None,
    }
}

/// Formats a time in microseconds as `m:ss`.
fn format_time(useconds: i64) -> String {
    let seconds = (useconds.max(0) / 1_000_000) as i32;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::format_time;

    #[test]
    fn formats_minutes_and_seconds() {
        assert_eq!(format_time(0), "0:00");
        assert_eq!(format_time(7_500_000), "0:07");
        assert_eq!(format_time(137_000_000), "2:17");
    }
}

/// The bar shows the voice note that is played and controls it. Needs GTK,
/// see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use crate::model::test_support::*;

    eprintln!("the player bar shows the played voice note");

    isolated(|| {
        let session = session();
        let chat = add_chat(&session, ME);
        let wav = playback_manager::tests::silent_wav("player-bar");
        let mut voice = file(601);
        voice.local.path = wav.to_str().unwrap().to_owned();
        voice.local.is_downloading_completed = true;
        let content =
            tdlib::enums::MessageContent::MessageVoiceNote(tdlib::types::MessageVoiceNote {
                voice_note: tdlib::types::VoiceNote {
                    duration: 137,
                    waveform: String::new(),
                    mime_type: "audio/wav".to_owned(),
                    speech_recognition_result: None,
                    voice,
                },
                caption: formatted_text(""),
                is_listened: false,
            });
        let message = model::Message::new(&chat, message(ME, 1, DAY, 0, content));

        let manager = playback_manager::PlaybackManager::new();
        let bar = AudioPlayerBar::default();
        bar.connect_to(Some(manager.clone()));
        let imp = bar.imp();
        assert!(!bar.is_visible(), "the bar is shown while nothing plays");

        manager.play_file(wav.to_str().unwrap(), || {}, |_| {}, || {});
        assert!(!bar.is_visible(), "the bar is shown for a non voice note");

        manager.set_audio_message(&message);
        assert!(bar.is_visible());
        assert!(imp.revealer.reveals_child());
        assert_eq!(
            imp.play_button.icon_name().as_deref(),
            Some("media-playback-pause-symbolic")
        );
        assert!(imp.subtitle_label.label().contains("2:17"));

        // Only the enabled bar is shown.
        bar.set_enabled(false);
        assert!(!imp.revealer.reveals_child());
        bar.set_enabled(true);
        assert!(imp.revealer.reveals_child());

        // Closing a stream that is still being prepared upsets GTK.
        let media = manager.media_file();
        let context = glib::MainContext::ref_thread_default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !media.is_prepared()
            && media.error().is_none()
            && std::time::Instant::now() < deadline
        {
            context.iteration(false);
        }

        manager.stop();
        assert!(!imp.revealer.reveals_child());
        std::fs::remove_file(wav).ok();
    });
}
