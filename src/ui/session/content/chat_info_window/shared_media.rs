//! The pieces of the shared media tabs of the chat info dialog: the kinds of
//! shared messages, the square tiles of the media grid and the rows of the
//! files, links, voice and music lists.

use std::cell::Cell;
use std::cell::OnceCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::gio;
use gtk::glib;

use crate::model;
use crate::model::MediaType;
use crate::types::MessageId;
use crate::utils;

/// A tab of the shared media, in the order of the tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SharedMediaKind {
    /// Photos and videos.
    Media,
    Files,
    Links,
    Voice,
    Music,
}

impl SharedMediaKind {
    pub(super) const ALL: [Self; 5] = [
        Self::Media,
        Self::Files,
        Self::Links,
        Self::Voice,
        Self::Music,
    ];

    pub(super) fn filter(self) -> tdlib::enums::SearchMessagesFilter {
        use tdlib::enums::SearchMessagesFilter;

        match self {
            Self::Media => SearchMessagesFilter::PhotoAndVideo,
            Self::Files => SearchMessagesFilter::Document,
            Self::Links => SearchMessagesFilter::Url,
            Self::Voice => SearchMessagesFilter::VoiceNote,
            Self::Music => SearchMessagesFilter::Audio,
        }
    }

    /// The size of the first page, which only has to tell whether the tab
    /// has content and fill the first screen.
    pub(super) fn first_page_size(self) -> i32 {
        match self {
            Self::Media => 30,
            _ => 20,
        }
    }

    pub(super) fn page_size(self) -> i32 {
        match self {
            Self::Media => 60,
            _ => 40,
        }
    }
}

/// The edge of a tile when it is not allocated yet.
const TILE_NATURAL_SIZE: i32 = 110;
/// The smallest edge, in logical pixels, of the photo size used for a tile.
const TILE_PHOTO_MIN_SIZE: i32 = 160;

mod tile_imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct SharedMediaTile {
        pub(super) message_id: Cell<MessageId>,
        pub(super) message: OnceCell<model::Message>,
        pub(super) picture: OnceCell<gtk::Picture>,
        pub(super) overlay: OnceCell<gtk::Overlay>,
        pub(super) thumbnail_requested: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SharedMediaTile {
        const NAME: &'static str = "PaplSharedMediaTile";
        type Type = super::SharedMediaTile;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("shared-media-tile");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for SharedMediaTile {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();
            obj.set_overflow(gtk::Overflow::Hidden);

            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .can_shrink(true)
                .build();
            let overlay = gtk::Overlay::builder().child(&picture).build();
            overlay.set_parent(&*obj);

            self.picture.set(picture).unwrap();
            self.overlay.set(overlay).unwrap();
        }

        fn dispose(&self) {
            if let Some(overlay) = self.overlay.get() {
                overlay.unparent();
            }
        }
    }

    impl WidgetImpl for SharedMediaTile {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            // The overlay is measured as GTK requires before allocating it,
            // but the tile is a square, as wide as the grid makes it.
            if let Some(overlay) = self.overlay.get() {
                overlay.measure(orientation, -1);
            }

            match orientation {
                // At least 96 px wide, so that phones show three per line.
                gtk::Orientation::Horizontal => (96, TILE_NATURAL_SIZE, -1, -1),
                _ => {
                    let size = if for_size > 0 {
                        for_size
                    } else {
                        TILE_NATURAL_SIZE
                    };
                    // No minimum, as the grid may round the height down.
                    (0, size, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(overlay) = self.overlay.get() {
                overlay.allocate(width, height, baseline, None);
            }
        }

        fn map(&self) {
            self.parent_map();
            // Only the tiles that are scrolled to download their thumbnail.
            self.obj().download_thumbnail();
        }
    }
}

glib::wrapper! {
    /// A square preview of a shared photo or video.
    pub(crate) struct SharedMediaTile(ObjectSubclass<tile_imp::SharedMediaTile>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SharedMediaTile {
    /// Returns a tile for a photo or video message, or `None` for any other
    /// message.
    pub(super) fn new(message: &model::Message) -> Option<Self> {
        let (minithumbnail, label) = match &message.content().0 {
            tdlib::enums::MessageContent::MessagePhoto(data) => {
                (data.photo.minithumbnail.clone(), gettext("Photo"))
            }
            tdlib::enums::MessageContent::MessageVideo(data) => {
                (data.video.minithumbnail.clone(), gettext("Video"))
            }
            _ => return None,
        };

        let obj: Self = glib::Object::new();
        let imp = obj.imp();
        imp.message_id.set(message.id());
        imp.message.set(message.clone()).unwrap();

        obj.update_property(&[gtk::accessible::Property::Label(&label)]);

        if let Some(texture) = minithumbnail
            .as_ref()
            .and_then(utils::texture_from_minithumbnail)
        {
            imp.picture.get().unwrap().set_paintable(Some(&texture));
        }

        if let tdlib::enums::MessageContent::MessageVideo(data) = &message.content().0 {
            let duration = gtk::Label::builder()
                .label(utils::human_friendly_duration(data.video.duration))
                .halign(gtk::Align::End)
                .valign(gtk::Align::End)
                .css_classes(["osd", "duration"])
                .build();
            imp.overlay.get().unwrap().add_overlay(&duration);
        }

        Some(obj)
    }

    pub(super) fn message_id(&self) -> MessageId {
        self.imp().message_id.get()
    }

    /// The file of the thumbnail shown by the tile, a small photo size for a
    /// photo and the thumbnail of a video.
    fn thumbnail_file(&self) -> Option<tdlib::types::File> {
        let message = self.imp().message.get()?;
        let min_size = TILE_PHOTO_MIN_SIZE * self.scale_factor();

        match &message.content().0 {
            tdlib::enums::MessageContent::MessagePhoto(data) => data
                .photo
                .sizes
                .iter()
                .find(|size| size.width.min(size.height) >= min_size)
                .or_else(|| data.photo.sizes.last())
                .map(|size| size.photo.clone()),
            tdlib::enums::MessageContent::MessageVideo(data) => data
                .video
                .thumbnail
                .as_ref()
                .map(|thumbnail| thumbnail.file.clone()),
            _ => None,
        }
    }

    fn download_thumbnail(&self) {
        let imp = self.imp();
        if imp.thumbnail_requested.replace(true) {
            return;
        }

        let Some(file) = self.thumbnail_file() else {
            return;
        };
        let Some(message) = imp.message.get() else {
            return;
        };
        let session = message.chat_().session_();

        if file.local.is_downloading_completed {
            self.show_image(file.local.path);
            return;
        }

        let size = file.size.max(file.expected_size) as u64;
        if !session
            .media_manager()
            .should_auto_download(MediaType::Photo, size)
        {
            return;
        }

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                match session.download_file(file.id).await {
                    Ok(file) => obj.show_image(file.local.path),
                    Err(e) => log::warn!("Failed to download a shared media thumbnail: {e:?}"),
                }
            }
        ));
    }

    fn show_image(&self, path: String) {
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let result = gio::spawn_blocking(move || utils::decode_image_from_path(&path))
                    .await
                    .unwrap();

                match result {
                    Ok(texture) => obj
                        .imp()
                        .picture
                        .get()
                        .unwrap()
                        .set_paintable(Some(&texture)),
                    // The minithumbnail stays.
                    Err(e) => log::warn!("Failed to decode a shared media thumbnail: {e:?}"),
                }
            }
        ));
    }
}

mod row_imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct SharedMessageRow {
        pub(super) message_id: Cell<MessageId>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SharedMessageRow {
        const NAME: &'static str = "PaplSharedMessageRow";
        type Type = super::SharedMessageRow;
        type ParentType = adw::ActionRow;
    }

    impl ObjectImpl for SharedMessageRow {}
    impl WidgetImpl for SharedMessageRow {}
    impl ListBoxRowImpl for SharedMessageRow {}
    impl PreferencesRowImpl for SharedMessageRow {}
    impl ActionRowImpl for SharedMessageRow {}
}

glib::wrapper! {
    /// A row of a shared file, link, voice or music message, which shows the
    /// message in the chat when activated.
    pub(crate) struct SharedMessageRow(ObjectSubclass<row_imp::SharedMessageRow>)
        @extends gtk::Widget, gtk::ListBoxRow, adw::PreferencesRow, adw::ActionRow,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl SharedMessageRow {
    fn new(message: &model::Message, icon_name: &str, title: &str, subtitle: &str) -> Self {
        let obj: Self = glib::Object::builder()
            .property("use-markup", false)
            .property("activatable", true)
            .property("title", title)
            .property("subtitle", subtitle)
            .property("title-lines", 1)
            .property("subtitle-lines", 2)
            .build();
        obj.imp().message_id.set(message.id());

        let icon = gtk::Image::builder()
            .icon_name(icon_name)
            .css_classes(["shared-media-icon"])
            .build();
        obj.add_prefix(&icon);

        obj
    }
}

/// The id of the message of a row of a shared media list, or 0.
pub(super) fn row_message_id(row: &gtk::ListBoxRow) -> MessageId {
    row.downcast_ref::<SharedMessageRow>()
        .map(|row| row.imp().message_id.get())
        .unwrap_or_default()
}

fn format_date(message: &model::Message) -> String {
    glib::DateTime::from_unix_local(message.date() as i64)
        .and_then(|date| date.format("%x"))
        .map(Into::into)
        .unwrap_or_default()
}

fn join_details(details: &[String]) -> String {
    details
        .iter()
        .filter(|detail| !detail.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
}

pub(super) fn file_row(message: &model::Message) -> Option<gtk::Widget> {
    let tdlib::enums::MessageContent::MessageDocument(data) = &message.content().0 else {
        return None;
    };

    let document = &data.document;
    let title = if document.file_name.is_empty() {
        gettext("File")
    } else {
        document.file_name.clone()
    };
    let size = document.document.size.max(document.document.expected_size);
    let subtitle = join_details(&[
        if size > 0 {
            glib::format_size(size as u64).into()
        } else {
            String::new()
        },
        format_date(message),
    ]);

    Some(SharedMessageRow::new(message, "text-x-generic-symbolic", &title, &subtitle).upcast())
}

pub(super) fn link_row(message: &model::Message) -> gtk::Widget {
    let (title, url) = message_link(&message.content().0);
    let subtitle = join_details(&[url.unwrap_or_default(), format_date(message)]);

    SharedMessageRow::new(message, "web-browser-symbolic", &title, &subtitle).upcast()
}

pub(super) fn voice_row(message: &model::Message) -> Option<gtk::Widget> {
    let tdlib::enums::MessageContent::MessageVoiceNote(data) = &message.content().0 else {
        return None;
    };

    let subtitle = join_details(&[
        utils::human_friendly_duration(data.voice_note.duration),
        format_date(message),
    ]);

    let row = SharedMessageRow::new(message, "audio-input-microphone-symbolic", "", &subtitle);
    // The sender, who can be renamed while the dialog is open.
    message
        .sender_name_expression()
        .bind(&row, "title", glib::Object::NONE);

    Some(row.upcast())
}

pub(super) fn music_row(message: &model::Message) -> Option<gtk::Widget> {
    let tdlib::enums::MessageContent::MessageAudio(data) = &message.content().0 else {
        return None;
    };

    let audio = &data.audio;
    let title = if !audio.title.is_empty() {
        audio.title.clone()
    } else if !audio.file_name.is_empty() {
        audio.file_name.clone()
    } else {
        gettext("Audio")
    };
    let subtitle = join_details(&[
        audio.performer.clone(),
        utils::human_friendly_duration(audio.duration),
        format_date(message),
    ]);

    Some(SharedMessageRow::new(message, "audio-x-generic-symbolic", &title, &subtitle).upcast())
}

/// The title and the URL of the first link of a message: the link preview
/// when there is one, otherwise the first URL or text link of the text.
fn message_link(content: &tdlib::enums::MessageContent) -> (String, Option<String>) {
    use tdlib::enums::MessageContent;

    let text = match content {
        MessageContent::MessageText(data) => {
            if let Some(preview) = &data.link_preview {
                let title = [&preview.title, &preview.site_name, &preview.display_url]
                    .into_iter()
                    .find(|text| !text.is_empty())
                    .cloned()
                    .unwrap_or_else(|| preview.url.clone());
                return (title, Some(preview.url.clone()));
            }
            Some(&data.text)
        }
        MessageContent::MessagePhoto(data) => Some(&data.caption),
        MessageContent::MessageVideo(data) => Some(&data.caption),
        MessageContent::MessageDocument(data) => Some(&data.caption),
        MessageContent::MessageAudio(data) => Some(&data.caption),
        MessageContent::MessageAnimation(data) => Some(&data.caption),
        MessageContent::MessageVoiceNote(data) => Some(&data.caption),
        _ => None,
    };

    let Some(text) = text else {
        return (gettext("Link"), None);
    };

    let url = first_url(text);
    let title = text
        .text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
        .or_else(|| url.clone())
        .unwrap_or_else(|| gettext("Link"));

    (title, url)
}

/// The first URL or text link of a formatted text.
pub(super) fn first_url(text: &tdlib::types::FormattedText) -> Option<String> {
    text.entities
        .iter()
        .find_map(|entity| match &entity.r#type {
            tdlib::enums::TextEntityType::TextUrl(data) => Some(data.url.clone()),
            tdlib::enums::TextEntityType::Url => {
                utf16_slice(&text.text, entity.offset, entity.length)
            }
            _ => None,
        })
}

/// The part of `text` at the given offset and length, which TDLib counts in
/// UTF-16 code units.
fn utf16_slice(text: &str, offset: i32, length: i32) -> Option<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let start = usize::try_from(offset).ok()?;
    let end = start.checked_add(usize::try_from(length).ok()?)?;
    units
        .get(start..end)
        .and_then(|slice| String::from_utf16(slice).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(
        offset: i32,
        length: i32,
        r#type: tdlib::enums::TextEntityType,
    ) -> tdlib::types::TextEntity {
        tdlib::types::TextEntity {
            offset,
            length,
            r#type,
        }
    }

    #[test]
    fn first_url_counts_utf16_code_units() {
        // The emoji takes two UTF-16 code units.
        let text = tdlib::types::FormattedText {
            text: "😀 see https://example.org/137 now".to_owned(),
            entities: vec![entity(7, 23, tdlib::enums::TextEntityType::Url)],
        };

        assert_eq!(first_url(&text).as_deref(), Some("https://example.org/137"));
    }

    #[test]
    fn first_url_prefers_the_first_link_of_any_kind() {
        let text = tdlib::types::FormattedText {
            text: "docs and https://example.org".to_owned(),
            entities: vec![
                entity(
                    0,
                    4,
                    tdlib::enums::TextEntityType::TextUrl(tdlib::types::TextEntityTypeTextUrl {
                        url: "https://docs.example.org".to_owned(),
                    }),
                ),
                entity(9, 19, tdlib::enums::TextEntityType::Url),
            ],
        };

        assert_eq!(
            first_url(&text).as_deref(),
            Some("https://docs.example.org")
        );
    }

    #[test]
    fn utf16_slice_out_of_range_is_none() {
        assert_eq!(utf16_slice("short", 3, 137), None);
        assert_eq!(utf16_slice("short", -1, 2), None);
    }
}
