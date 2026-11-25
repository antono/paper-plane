mod status_indicator;

use std::cell::Cell;
use std::cell::RefCell;
use std::sync::OnceLock;

use gettextrs::gettext;
use glib::clone;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::CompositeTemplate;

pub(crate) use self::status_indicator::StatusIndicator;
pub(crate) use super::file_status::FileStatus;
use crate::model;
use crate::model::MediaType;
use crate::ui;
use crate::ui::MessageBaseExt;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/document/mod.ui")]
    pub(crate) struct MessageDocument {
        pub(super) bindings: RefCell<Vec<gtk::ExpressionWatch>>,
        pub(super) handler_id: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) click_handler_id: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) loader_handler_id: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) message: glib::WeakRef<model::Message>,
        /// The file id of the thumbnail that should be downloaded once the
        /// widget is shown, or 0 if there is none.
        pub(super) pending_thumbnail_file_id: Cell<i32>,
        /// The path of the thumbnail that is shown or being decoded, so that
        /// a thumbnail decoded for a previous message is not shown.
        pub(super) thumbnail_path: RefCell<Option<String>>,
        pub(super) loader: super::super::MediaLoader,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) file_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) click: TemplateChild<gtk::GestureClick>,
        #[template_child]
        pub(super) file_thumbnail_picture: TemplateChild<gtk::Picture>,
        #[template_child]
        pub(super) status_indicator: TemplateChild<ui::MessageDocumentStatusIndicator>,
        #[template_child]
        pub(super) file_name_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) file_size_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageDocument {
        const NAME: &'static str = "PaplMessageDocument";
        type Type = super::MessageDocument;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageDocument {
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
            match pspec.name() {
                "message" => self.message.upgrade().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            let handler_id = self.loader.connect_status_notify(clone!(
                #[weak(rename_to = obj)]
                self.obj(),
                move |_| {
                    obj.update_status_ui();
                }
            ));
            *self.loader_handler_id.borrow_mut() = Some(handler_id);
        }

        fn dispose(&self) {
            if let Some(handler_id) = self.loader_handler_id.borrow_mut().take() {
                self.loader.disconnect(handler_id);
            }
        }
    }

    impl WidgetImpl for MessageDocument {
        fn map(&self) {
            self.parent_map();

            self.obj().maybe_download_thumbnail();
        }
    }
    impl ui::MessageBaseImpl for MessageDocument {}
}

glib::wrapper! {
    pub(crate) struct MessageDocument(ObjectSubclass<imp::MessageDocument>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessageDocument {
    type Message = model::Message;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        if imp.message.upgrade().as_ref() == Some(message) {
            return;
        }

        let mut bindings = imp.bindings.borrow_mut();

        while let Some(binding) = bindings.pop() {
            binding.unwatch();
        }

        imp.message_bubble.update_from_message(message, false);

        let handler_id = message.connect_content_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |message| {
                obj.update_document(message);
            }
        ));
        imp.handler_id.replace(Some(handler_id));

        imp.message.set(Some(message));
        self.update_document(message);

        self.notify("message");
    }
}

impl MessageDocument {
    /// Downloads the pending thumbnail, if the widget is shown.
    fn maybe_download_thumbnail(&self) {
        let imp = self.imp();

        if !self.is_mapped() || imp.pending_thumbnail_file_id.get() == 0 {
            return;
        }

        let file_id = imp.pending_thumbnail_file_id.take();

        let Some(message) = imp.message.upgrade() else {
            return;
        };
        let session = message.chat_().session_();

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            session,
            async move {
                if let Ok(file) = session.download_file(file_id).await {
                    obj.show_thumbnail_file(file.local.path);
                }
            }
        ));
    }

    fn update_document(&self, message: &model::Message) {
        if let Some(content) = FileContent::of(message.content().0) {
            let imp = self.imp();

            let message_text = utils::parse_formatted_text(content.caption);
            imp.message_bubble.set_label(message_text);

            imp.file_name_label.set_label(&content.name);

            self.try_load_thumbnail(content.thumbnail, content.minithumbnail);

            imp.loader
                .bind(&message.chat_().session_(), MediaType::File, &content.file);

            // In contrast to photos and videos, documents are downloaded as
            // soon as they are shown in the history, even if the widget is
            // not mapped yet.
            imp.loader.maybe_start_auto_download();

            self.update_status_ui();
        }
    }

    /// Replaces the click gesture handler of the file box.
    fn replace_click_handler<F: Fn(&gtk::GestureClick, i32, f64, f64) + 'static>(&self, f: F) {
        let imp = self.imp();
        let click = &*imp.click;

        let handler_id = click.connect_released(f);

        if let Some(handler_id) = imp.click_handler_id.borrow_mut().replace(handler_id) {
            click.disconnect(handler_id);
        }
    }

    fn disconnect_click_handler(&self) {
        let imp = self.imp();

        if let Some(handler_id) = imp.click_handler_id.borrow_mut().take() {
            imp.click.disconnect(handler_id);
        }
    }

    /// Updates the widgets according to the status of the loader.
    fn update_status_ui(&self) {
        let imp = self.imp();
        let status = imp.loader.status();

        imp.status_indicator.set_status(status);
        self.update_size_label(status, imp.loader.size());

        match status {
            FileStatus::Downloading(_) => {
                if imp.loader.is_auto() {
                    // Ignore clicks while the file is downloading
                    // automatically.
                    self.disconnect_click_handler();
                } else {
                    // Clicking again cancels the download.
                    self.replace_click_handler(clone!(
                        #[weak(rename_to = obj)]
                        self,
                        move |_, _, _, _| {
                            obj.imp().loader.cancel_download();
                        }
                    ));
                }
            }
            // Uploads are driven by the sender side, so there is nothing to
            // react to.
            FileStatus::Uploading(_) => self.disconnect_click_handler(),
            FileStatus::CanBeDownloaded => {
                self.replace_click_handler(clone!(
                    #[weak(rename_to = obj)]
                    self,
                    move |_, _, _, _| {
                        obj.imp().loader.request_download();
                    }
                ));
            }
            FileStatus::Downloaded => {
                // Open file
                if imp.file_thumbnail_picture.file().is_some() {
                    imp.status_indicator.set_visible(false);
                }

                let Some(path) = imp.loader.path() else {
                    self.disconnect_click_handler();
                    return;
                };

                let gio_file = gio::File::for_path(path.as_str());
                self.replace_click_handler(move |_, _, _, _| {
                    if let Err(err) = gio::AppInfo::launch_default_for_uri(
                        &gio_file.uri(),
                        gio::AppLaunchContext::NONE,
                    ) {
                        log::error!("Error: {}", err);
                    }
                });
            }
        }
    }

    fn update_size_label(&self, status: FileStatus, size: u64) {
        let size_label = &self.imp().file_size_label;

        match status {
            FileStatus::Downloading(progress) | FileStatus::Uploading(progress) => {
                let downloaded = glib::format_size((size as f64 * progress) as u64);
                let full_size = glib::format_size(size);

                size_label.set_label(&format!("{downloaded} / {full_size}"));
            }
            FileStatus::CanBeDownloaded | FileStatus::Downloaded => {
                size_label.set_label(&glib::format_size(size));
            }
        }
    }

    /// Shows the thumbnail at `path`, decoded off the main thread unless it
    /// has been shown before.
    fn show_thumbnail_file(&self, path: String) {
        let imp = self.imp();

        if let Some(texture) = ui::texture_loader::cached(&path) {
            imp.thumbnail_path.replace(Some(path));
            imp.file_thumbnail_picture.set_paintable(Some(&texture));
            return;
        }

        imp.thumbnail_path.replace(Some(path.clone()));
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                match ui::texture_loader::load(&path).await {
                    Ok(texture) => {
                        let imp = obj.imp();
                        if imp.thumbnail_path.borrow().as_deref() == Some(path.as_str()) {
                            imp.file_thumbnail_picture.set_paintable(Some(&texture));
                        }
                    }
                    Err(e) => log::warn!("Failed to load a document thumbnail: {e}"),
                }
            }
        ));
    }

    fn try_load_thumbnail(
        &self,
        thumbnail: Option<tdlib::types::Thumbnail>,
        minithumbnail: Option<tdlib::types::Minithumbnail>,
    ) {
        let imp = self.imp();
        if let Some(thumbnail) = thumbnail {
            imp.status_indicator.set_masked(false);
            imp.file_thumbnail_picture.set_visible(true);
            imp.file_box.add_css_class("with-thumbnail");
            if thumbnail.file.local.is_downloading_completed {
                self.show_thumbnail_file(thumbnail.file.local.path);
            } else {
                imp.thumbnail_path.replace(None);
                let minithumbnail = minithumbnail
                    .as_ref()
                    .and_then(ui::texture_loader::minithumbnail);
                imp.file_thumbnail_picture
                    .set_paintable(minithumbnail.as_ref());

                // The thumbnail is downloaded when the widget is shown, so
                // that thumbnails that are only scrolled past are not
                // downloaded.
                imp.pending_thumbnail_file_id.set(thumbnail.file.id);
                self.maybe_download_thumbnail();
            }
        } else {
            imp.thumbnail_path.replace(None);
            imp.status_indicator.set_masked(true);
            imp.file_box.remove_css_class("with-thumbnail");
            imp.file_thumbnail_picture.set_visible(false);
            imp.file_thumbnail_picture
                .set_paintable(gdk::Paintable::NONE);
        }
    }
}

/// What the widget shows of a file message: a document, or an audio file
/// (music), which is shown as a file with its album cover as the thumbnail.
struct FileContent {
    caption: tdlib::types::FormattedText,
    name: String,
    file: tdlib::types::File,
    thumbnail: Option<tdlib::types::Thumbnail>,
    minithumbnail: Option<tdlib::types::Minithumbnail>,
}

impl FileContent {
    fn of(content: tdlib::enums::MessageContent) -> Option<Self> {
        match content {
            tdlib::enums::MessageContent::MessageDocument(data) => Some(Self {
                caption: data.caption,
                name: data.document.file_name,
                file: data.document.document,
                thumbnail: data.document.thumbnail,
                minithumbnail: data.document.minithumbnail,
            }),
            tdlib::enums::MessageContent::MessageAudio(data) => Some(Self {
                caption: data.caption,
                name: audio_name(&data.audio),
                file: data.audio.audio,
                thumbnail: data.audio.album_cover_thumbnail,
                minithumbnail: data.audio.album_cover_minithumbnail,
            }),
            _ => None,
        }
    }
}

/// The name of an audio file: its performer and title, or its file name if
/// it has no title.
fn audio_name(audio: &tdlib::types::Audio) -> String {
    match (audio.performer.is_empty(), audio.title.is_empty()) {
        (_, true) if !audio.file_name.is_empty() => audio.file_name.clone(),
        (_, true) => gettext("Audio"),
        (true, false) => audio.title.clone(),
        (false, false) => format!("{} – {}", audio.performer, audio.title),
    }
}

/// Music is shown as a file (it was "unsupported"), see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use crate::model::test_support::*;

    eprintln!("audio messages are shown as files");

    let audio = |title: &str, performer: &str, file_name: &str| {
        tdlib::enums::MessageContent::MessageAudio(tdlib::types::MessageAudio {
            audio: tdlib::types::Audio {
                duration: 137,
                title: title.to_owned(),
                performer: performer.to_owned(),
                file_name: file_name.to_owned(),
                mime_type: "audio/mpeg".to_owned(),
                album_cover_minithumbnail: None,
                album_cover_thumbnail: Some(tdlib::types::Thumbnail {
                    format: tdlib::enums::ThumbnailFormat::Jpeg,
                    width: 90,
                    height: 90,
                    file: file(2),
                }),
                external_album_covers: vec![],
                audio: file(1),
            },
            caption: formatted_text("a caption"),
        })
    };
    let document = tdlib::enums::MessageContent::MessageDocument(tdlib::types::MessageDocument {
        document: tdlib::types::Document {
            file_name: "notes.txt".to_owned(),
            mime_type: "text/plain".to_owned(),
            minithumbnail: None,
            thumbnail: None,
            document: file(3),
        },
        caption: formatted_text(""),
    });

    isolated(|| {
        let session = session();
        let chat = add_chat(&session, ME);
        let message = |id, content| model::Message::new(&chat, message(ME, id, DAY, 0, content));

        let row: MessageDocument = glib::Object::new();
        row.set_message(&message(1, audio("Song", "Band", "song.mp3")));
        let imp = row.imp();
        assert_eq!(imp.file_name_label.label(), "Band – Song");
        assert_eq!(imp.message_bubble.label_text(), "a caption");
        // The album cover is the thumbnail.
        assert!(imp.file_box.has_css_class("with-thumbnail"));

        row.set_message(&message(2, audio("", "Band", "song.mp3")));
        assert_eq!(imp.file_name_label.label(), "song.mp3");
        row.set_message(&message(3, audio("Song", "", "")));
        assert_eq!(imp.file_name_label.label(), "Song");

        // A file without a thumbnail does not keep the style of the cover.
        row.set_message(&message(4, document));
        assert_eq!(imp.file_name_label.label(), "notes.txt");
        assert!(!imp.file_box.has_css_class("with-thumbnail"));
    });
}
