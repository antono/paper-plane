//! The picture of a reaction: its emoji, the sticker of a custom emoji, or a
//! star for the paid reaction.

use std::cell::RefCell;
use std::collections::HashMap;

use glib::clone;
use gtk::glib;
use gtk::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

thread_local! {
    /// The stickers of the custom emoji already fetched, by custom emoji id.
    static CUSTOM_EMOJI: RefCell<HashMap<i64, tdlib::types::Sticker>> =
        RefCell::new(HashMap::new());
}

/// Creates the picture of a reaction of `size` pixels. The sticker of a
/// custom emoji is fetched from TDLib in the background.
pub(crate) fn new(
    reaction_type: &tdlib::enums::ReactionType,
    session: Option<model::ClientStateSession>,
    size: i32,
) -> gtk::Widget {
    match reaction_type {
        tdlib::enums::ReactionType::Emoji(data) => emoji_label(&utils::emoji_string(&data.emoji)),
        tdlib::enums::ReactionType::Paid => {
            let label = emoji_label("⭐");
            label.add_css_class("paid");
            label
        }
        tdlib::enums::ReactionType::CustomEmoji(data) => {
            custom_emoji(data.custom_emoji_id, session, size)
        }
    }
}

fn emoji_label(text: &str) -> gtk::Widget {
    gtk::Label::builder()
        .label(text)
        .css_classes(["emoji"])
        .build()
        .upcast()
}

fn custom_emoji(
    id: i64,
    session: Option<model::ClientStateSession>,
    size: i32,
) -> gtk::Widget {
    // A box of a fixed size, so that the chip does not grow when the sticker
    // arrives.
    let frame = gtk::Box::builder()
        .width_request(size)
        .height_request(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["custom-emoji"])
        .build();
    // Assistive technologies get the id at least.
    frame.update_property(&[gtk::accessible::Property::Label(&format!(
        "custom emoji {id}"
    ))]);

    let Some(session) = session else {
        return frame.upcast();
    };

    let sticker = glib::Object::new::<ui::Sticker>();
    sticker.set_longer_side_size(size);
    sticker.set_hexpand(true);
    sticker.set_halign(gtk::Align::Center);
    sticker.set_valign(gtk::Align::Center);
    frame.append(&sticker);

    if let Some(cached) = CUSTOM_EMOJI.with(|cache| cache.borrow().get(&id).cloned()) {
        show_sticker(&sticker, cached, session);
        return frame.upcast();
    }

    utils::spawn(clone!(
        #[weak]
        sticker,
        async move {
            let Some(client_id) = session.client().map(|client| client.id()) else {
                return;
            };
            match tdlib::functions::get_custom_emoji_stickers(vec![id], client_id).await {
                Ok(tdlib::enums::Stickers::Stickers(data)) => {
                    let Some(found) = data.stickers.into_iter().next() else {
                        log::debug!("Custom emoji {id} not found");
                        return;
                    };
                    CUSTOM_EMOJI.with(|cache| cache.borrow_mut().insert(id, found.clone()));
                    show_sticker(&sticker, found, session);
                }
                Err(e) => log::debug!("Error getting custom emoji {id}: {e:?}"),
            }
        }
    ));

    frame.upcast()
}

fn show_sticker(
    sticker: &ui::Sticker,
    data: tdlib::types::Sticker,
    session: model::ClientStateSession,
) {
    sticker.set_tooltip_text(Some(&data.emoji));
    // Custom emoji play once, a list of them looping costs a phone too much.
    sticker.update_sticker(data, false, session);
}

/// Checks of the reaction pictures that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use crate::model::test_support::*;

    eprintln!("reaction pictures");

    isolated(|| {
        let label_text = |widget: &gtk::Widget| {
            widget
                .downcast_ref::<gtk::Label>()
                .map(|label| label.label().to_string())
        };

        let thumbs = new(&model::reaction_type_from_key("👍"), None, 20);
        assert_eq!(label_text(&thumbs), Some(utils::emoji_string("👍")));

        let paid = new(&tdlib::enums::ReactionType::Paid, None, 20);
        assert_eq!(label_text(&paid).as_deref(), Some("⭐"));
        assert!(paid.has_css_class("paid"));

        // Without a session the custom emoji keeps its place, empty.
        let custom = new(&model::reaction_type_from_key("[ce137]"), None, 20);
        assert!(custom.has_css_class("custom-emoji"));
        assert_eq!(custom.width_request(), 20);
        assert!(custom.first_child().is_none());

        // A known custom emoji is shown at once, from the cache.
        let session = session();
        let mut sticker_file = file(137);
        sticker_file.local.is_downloading_completed = false;
        CUSTOM_EMOJI.with(|cache| {
            cache.borrow_mut().insert(
                137,
                tdlib::types::Sticker {
                    id: 137,
                    set_id: 0,
                    width: 100,
                    height: 100,
                    emoji: "🙂".to_owned(),
                    format: tdlib::enums::StickerFormat::Webp,
                    full_type: tdlib::enums::StickerFullType::CustomEmoji(
                        tdlib::types::StickerFullTypeCustomEmoji {
                            custom_emoji_id: 137,
                            needs_repainting: false,
                        },
                    ),
                    thumbnail: None,
                    sticker: sticker_file,
                },
            )
        });
        let custom = new(&model::reaction_type_from_key("[ce137]"), Some(session), 32);
        let sticker = custom
            .first_child()
            .and_downcast::<ui::Sticker>()
            .expect("the sticker of a cached custom emoji");
        assert_eq!(sticker.longer_side_size(), 32);
        assert_eq!(sticker.tooltip_text().as_deref(), Some("🙂"));
        CUSTOM_EMOJI.with(|cache| cache.borrow_mut().clear());
    });
}
