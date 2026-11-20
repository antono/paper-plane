use std::future::Future;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::OnceLock;

use gettextrs::gettext;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use image::ImageReader;
use regex::Regex;
use thiserror::Error;

use crate::config;
use crate::APPLICATION_OPTS;
use crate::TEMP_DIR;

fn protocol_re() -> &'static Regex {
    static PROTOCOL_RE: OnceLock<Regex> = OnceLock::new();
    PROTOCOL_RE.get_or_init(|| Regex::new(r"^\w+://").unwrap())
}

#[derive(Debug)]
pub(crate) struct PaperPlaneSettings(gio::Settings);

impl Default for PaperPlaneSettings {
    fn default() -> Self {
        Self(gio::Settings::new(config::APP_ID))
    }
}

impl Deref for PaperPlaneSettings {
    type Target = gio::Settings;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

/// Replace variables in the given string with the given dictionary.
///
/// The expected format to replace is `{name}`, where `name` is the first string
/// in the dictionary entry tuple.
// Function taken from Fractal: https://gitlab.gnome.org/GNOME/fractal/-/blob/main/src/utils.rs
pub(crate) fn freplace(s: String, args: &[(&str, &str)]) -> String {
    let mut s = s;
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// Forces emoji presentation for the given emoji by appending the variation
/// selector-16 (U+FE0F) if the emoji doesn't already have an explicit
/// presentation.
///
/// Some emoji like ❤ (U+2764) default to text presentation and would otherwise
/// be rendered as monochrome glyphs by Pango.
pub(crate) fn emoji_string(emoji: &str) -> String {
    if emoji.contains('\u{FE0E}') || emoji.contains('\u{FE0F}') {
        emoji.to_string()
    } else {
        format!("{emoji}\u{FE0F}")
    }
}

pub(crate) fn linkify(text: &str) -> String {
    if !protocol_re().is_match(text) {
        format!("http://{text}")
    } else {
        text.to_string()
    }
}

pub(crate) fn convert_to_markup(text: String, entity: &tdlib::enums::TextEntityType) -> String {
    use tdlib::enums::TextEntityType::*;

    match entity {
        Url => format!("<a href='{}'>{}</a>", linkify(&text), text),
        EmailAddress => format!("<a href='mailto:{text}'>{text}</a>"),
        PhoneNumber => format!("<a href='tel:{text}'>{text}</a>"),
        Bold => format!("<b>{text}</b>"),
        Italic => format!("<i>{text}</i>"),
        Underline => format!("<u>{text}</u>"),
        Strikethrough => format!("<s>{text}</s>"),
        Code | Pre | PreCode(_) => format!("<tt>{text}</tt>"),
        TextUrl(data) => format!("<a href='{}'>{}</a>", escape(&data.url), text),
        _ => text,
    }
}

pub(crate) fn parse_formatted_text(formatted_text: tdlib::types::FormattedText) -> String {
    let mut entities = formatted_text.entities.iter();
    let mut entity = entities.next();
    let mut output = String::new();
    let mut buffer = String::new();
    let mut is_inside_entity = false;

    // This is the offset in utf16 code units of the text to parse. We need this variable
    // because tdlib stores the offset and length parameters as utf16 code units instead
    // of regular code points.
    let mut code_units_offset = 0;

    for c in formatted_text.text.chars() {
        if !is_inside_entity
            && entity.is_some()
            && code_units_offset >= entity.unwrap().offset as usize
        {
            is_inside_entity = true;

            if !buffer.is_empty() {
                output.push_str(&escape(&buffer));
                buffer = String::new();
            }
        }

        buffer.push(c);
        code_units_offset += c.len_utf16();

        if let Some(entity_) = entity {
            if code_units_offset >= (entity_.offset + entity_.length) as usize {
                buffer = escape(&buffer);

                entity = loop {
                    let entity = entities.next();

                    // Handle eventual nested entities
                    match entity {
                        Some(entity) => {
                            if entity.offset == entity_.offset {
                                buffer = convert_to_markup(buffer, &entity.r#type);
                            } else {
                                break Some(entity);
                            }
                        }
                        None => break None,
                    }
                };

                output.push_str(&convert_to_markup(buffer, &entity_.r#type));
                buffer = String::new();
                is_inside_entity = false;
            }
        }
    }

    // Add the eventual leftovers from the buffer to the output
    if !buffer.is_empty() {
        output.push_str(&escape(&buffer));
    }

    output
}

pub(crate) fn human_friendly_duration(mut seconds: i32) -> String {
    let hours = seconds / (60 * 60);
    if hours > 0 {
        seconds %= 60 * 60;
        let minutes = seconds / 60;
        if minutes > 0 {
            seconds %= 60;
            gettext!("{} h {} min {} s", hours, minutes, seconds)
        } else {
            gettext!("{} h {} s", hours, seconds)
        }
    } else {
        let minutes = seconds / 60;
        if minutes > 0 {
            seconds %= 60;
            if seconds > 0 {
                gettext!("{} min {} s", minutes, seconds)
            } else {
                gettext!("{} min", minutes)
            }
        } else {
            gettext!("{} s", seconds)
        }
    }
}

/// Returns the Paper Plane data directory (e.g. /home/bob/.local/share/paper-plane).
pub(crate) fn data_dir() -> &'static PathBuf {
    &APPLICATION_OPTS.get().unwrap().data_dir
}

/// Returns the Paper Plane temp directory (e.g. /tmp/paper-plane2-0).
pub(crate) fn temp_dir() -> Option<&'static PathBuf> {
    TEMP_DIR.get()
}

/// Spawn a future on the thread-default `MainContext`
pub(crate) fn spawn<F: Future<Output = ()> + 'static>(fut: F) {
    // The thread-default context is the global default one in the
    // application; unit tests push their own one (see
    // `model::test_support::isolated`).
    let ctx = glib::MainContext::ref_thread_default();
    ctx.spawn_local(fut);
}

/// Run a future on the default `MainContext` and block until finished
pub(crate) fn block_on<F: Future>(fut: F) -> F::Output {
    let ctx = glib::MainContext::default();
    ctx.block_on(fut)
}

#[derive(Error, Debug)]
pub(crate) enum DecodeError {
    #[error("I/O error: {0:?}")]
    IoError(std::io::Error),
    #[error("Image decoding error: {0:?}")]
    ImageError(image::error::ImageError),
    #[error("Decoding for this image format is not currently implemented")]
    Unimplemented,
}

pub(crate) fn decode_image_from_path(path: &str) -> Result<gdk::MemoryTexture, DecodeError> {
    use image::DynamicImage::*;

    let dynamic_image = ImageReader::open(path)
        .map_err(DecodeError::IoError)?
        .decode()
        .map_err(DecodeError::ImageError)?;
    let (memory_format, layout, data) = match dynamic_image {
        ImageLuma8(_) => {
            let buffer = dynamic_image.to_rgb8();
            (
                gdk::MemoryFormat::R8g8b8,
                buffer.sample_layout(),
                buffer.into_raw(),
            )
        }
        ImageLumaA8(_) => {
            let buffer = dynamic_image.to_rgba8();
            (
                gdk::MemoryFormat::R8g8b8a8,
                buffer.sample_layout(),
                buffer.into_raw(),
            )
        }
        ImageRgb8(buffer) => (
            gdk::MemoryFormat::R8g8b8,
            buffer.sample_layout(),
            buffer.into_raw(),
        ),
        ImageRgba8(buffer) => (
            gdk::MemoryFormat::R8g8b8a8,
            buffer.sample_layout(),
            buffer.into_raw(),
        ),
        _ => return Err(DecodeError::Unimplemented),
    };

    let bytes = glib::Bytes::from_owned(data);
    let texture = gdk::MemoryTexture::new(
        layout.width as i32,
        layout.height as i32,
        memory_format,
        &bytes,
        layout.height_stride,
    );

    Ok(texture)
}

/// Returns a texture for the given minithumbnail, or `None` if it could not
/// be decoded.
pub(crate) fn texture_from_minithumbnail(
    minithumbnail: &tdlib::types::Minithumbnail,
) -> Option<gdk::Texture> {
    gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(
        &minithumbnail.data,
    )))
    .ok()
}

/// Returns the photo size that should be loaded for the given scale factor.
///
/// See https://core.telegram.org/api/files#image-thumbnail-types for more
/// information about photo sizes.
pub(crate) fn photo_size_for_scale_factor(
    sizes: &[tdlib::types::PhotoSize],
    scale_factor: i32,
) -> Option<tdlib::types::PhotoSize> {
    let last = || sizes.last().cloned();

    if scale_factor > 2 {
        return last();
    }

    let type_ = if scale_factor > 1 { "y" } else { "x" };

    sizes
        .iter()
        .find(|s| s.r#type == type_)
        .cloned()
        .or_else(last)
}

pub(crate) fn show_toast<W: IsA<gtk::Widget>>(widget: &W, title: impl Into<glib::GString>) {
    widget
        .ancestor(adw::ToastOverlay::static_type())
        .unwrap()
        .downcast::<adw::ToastOverlay>()
        .unwrap()
        .add_toast(
            adw::Toast::builder()
                .title(title)
                .timeout(3)
                .priority(adw::ToastPriority::High)
                .build(),
        );
}

pub(crate) fn ancestor<W: IsA<gtk::Widget>, T: IsA<gtk::Widget>>(widget: &W) -> T {
    widget
        .ancestor(T::static_type())
        .and_downcast::<T>()
        .unwrap()
}

pub(crate) fn unparent_children<W: IsA<gtk::Widget>>(widget: &W) {
    let mut child = widget.first_child();
    while let Some(child_) = child {
        child = child_.next_sibling();
        child_.unparent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(type_: &str, width: i32) -> tdlib::types::PhotoSize {
        tdlib::types::PhotoSize {
            r#type: type_.to_owned(),
            photo: tdlib::types::File {
                id: width,
                size: 0,
                expected_size: 0,
                local: tdlib::types::LocalFile {
                    path: String::new(),
                    can_be_downloaded: true,
                    can_be_deleted: false,
                    is_downloading_active: false,
                    is_downloading_completed: false,
                    download_offset: 0,
                    downloaded_prefix_size: 0,
                    downloaded_size: 0,
                },
                remote: tdlib::types::RemoteFile {
                    id: String::new(),
                    unique_id: String::new(),
                    is_uploading_active: false,
                    is_uploading_completed: true,
                    uploaded_size: 0,
                },
            },
            width,
            height: width,
            progressive_sizes: vec![],
        }
    }

    fn picked(sizes: &[tdlib::types::PhotoSize], scale: i32) -> Option<String> {
        photo_size_for_scale_factor(sizes, scale).map(|s| s.r#type)
    }

    #[test]
    fn emoji_are_shown_in_colour() {
        // U+FE0F asks for the emoji presentation, so emoji are drawn in
        // colour; an explicit presentation is kept.
        assert_eq!(emoji_string("👍"), "👍\u{FE0F}");
        assert_eq!(emoji_string("❤\u{FE0F}"), "❤\u{FE0F}");
        assert_eq!(emoji_string("❤\u{FE0E}"), "❤\u{FE0E}");
    }

    #[test]
    fn photo_sizes_for_scale_factors() {
        let sizes = [size("m", 320), size("x", 800), size("y", 1280), size("w", 2560)];
        assert_eq!(picked(&sizes, 1).as_deref(), Some("x"));
        assert_eq!(picked(&sizes, 2).as_deref(), Some("y"));
        assert_eq!(picked(&sizes, 3).as_deref(), Some("w"));

        // Without the wanted type, the largest size is used.
        let small = [size("s", 90), size("m", 320)];
        assert_eq!(picked(&small, 1).as_deref(), Some("m"));
        assert_eq!(picked(&[], 1), None);
    }
}

/// Checks of the media helpers that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    eprintln!("minithumbnail textures");

    let pixels = glib::Bytes::from_owned(vec![255u8, 0, 0, 255]);
    let png = gdk::MemoryTexture::new(1, 1, gdk::MemoryFormat::R8g8b8a8, &pixels, 4)
        .save_to_png_bytes();
    let minithumbnail = |data: String| tdlib::types::Minithumbnail {
        width: 1,
        height: 1,
        data,
    };

    let texture = texture_from_minithumbnail(&minithumbnail(glib::base64_encode(&png).into()))
        .expect("a valid image");
    assert_eq!((texture.width(), texture.height()), (1, 1));
    assert!(texture_from_minithumbnail(&minithumbnail("not an image".into())).is_none());
}
