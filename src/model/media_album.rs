use std::cell::OnceCell;

use gio::prelude::*;
use glib::Properties;
use gtk::gio;
use gtk::glib;
use gtk::subclass::prelude::*;

use crate::model;
use crate::types::MessageId;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::MediaAlbum)]
    pub(crate) struct MediaAlbum {
        /// The TDLib media album id shared by all the messages of the album.
        #[property(get, set, construct_only)]
        pub(super) id: OnceCell<i64>,
        /// The messages of the album, sorted ascending by id.
        #[property(get)]
        pub(super) items: OnceCell<gio::ListStore>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MediaAlbum {
        const NAME: &'static str = "MediaAlbum";
        type Type = super::MediaAlbum;
    }

    impl ObjectImpl for MediaAlbum {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }
    }
}

glib::wrapper! {
    pub(crate) struct MediaAlbum(ObjectSubclass<imp::MediaAlbum>);
}

/// A group of media messages (photos, videos and animations) that were sent
/// together as an album.
///
/// The messages are kept in a [`gio::ListStore`] sorted ascending by id. The
/// album is displayed in the chat history by the row of its *representative*,
/// which is always its oldest message (see `ChatHistoryModel`).
impl MediaAlbum {
    pub(crate) fn new(id: i64) -> Self {
        let items = gio::ListStore::new::<model::Message>();
        let obj: Self = glib::Object::builder().property("id", id).build();
        obj.imp().items.set(items).unwrap();
        obj
    }

    /// The store holding the messages of this album.
    pub(crate) fn store(&self) -> gio::ListStore {
        self.imp().items.get().unwrap().clone()
    }

    /// The number of messages in this album.
    #[allow(clippy::len_without_is_empty)]
    pub(crate) fn len(&self) -> u32 {
        self.store().n_items()
    }

    /// Whether this album has no messages.
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds a message to this album, keeping the store sorted ascending by
    /// id.
    ///
    /// Does nothing if a message with the same id is already part of the
    /// album.
    pub(crate) fn add_message(&self, message: model::Message) {
        let store = self.store();
        let msg_id = message.id();
        let n = store.n_items();

        let mut low = 0;
        let mut high = n;
        while low < high {
            let mid = (low + high) / 2;
            let item = store
                .item(mid)
                .and_then(|obj| obj.downcast::<model::Message>().ok());

            // The store is typed to `model::Message` at construction time, so a
            // downcast failure is a programming error rather than a runtime
            // condition. Treat it as a "smaller id" to narrow the search range
            // toward the correct insertion point without panicking.
            let item_id = item.map(|item| item.id()).unwrap_or(msg_id);

            if item_id == msg_id {
                return;
            } else if item_id < msg_id {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        store.insert(low, &message);
    }

    /// Removes the message with the given id from this album and returns it,
    /// if it was part of the album.
    pub(crate) fn remove_message(&self, message_id: MessageId) -> Option<model::Message> {
        let store = self.store();
        let n = store.n_items();

        // The store is sorted ascending by message id, so a binary search
        // matches the invariant used by `add_message`.
        let mut low = 0;
        let mut high = n;
        while low < high {
            let mid = (low + high) / 2;
            let item = store
                .item(mid)
                .and_then(|obj| obj.downcast::<model::Message>().ok());

            let item = item?;
            let item_id = item.id();

            if item_id == message_id {
                store.remove(mid);
                return Some(item);
            } else if item_id < message_id {
                low = mid + 1;
            } else {
                high = mid;
            }
        }

        None
    }

    /// The messages of this album, ordered from the oldest to the newest.
    pub(crate) fn messages(&self) -> Vec<model::Message> {
        let store = self.store();
        let mut res = Vec::with_capacity(store.n_items() as usize);
        for i in 0..store.n_items() {
            if let Some(msg) = store
                .item(i)
                .and_then(|o| o.downcast::<model::Message>().ok())
            {
                res.push(msg);
            }
        }
        res
    }

    /// The oldest message of this album, which is its representative.
    pub(crate) fn first_message(&self) -> Option<model::Message> {
        self.store()
            .item(0)
            .and_then(|o| o.downcast::<model::Message>().ok())
    }

    /// The id of the oldest message of this album.
    pub(crate) fn first_message_id(&self) -> Option<MessageId> {
        self.first_message().map(|message| message.id())
    }

    /// The id of the newest message of this album.
    pub(crate) fn last_message_id(&self) -> Option<MessageId> {
        let len = self.len();
        if len > 0 {
            self.store()
                .item(len - 1)
                .and_then(|o| o.downcast::<model::Message>().ok())
                .map(|message| message.id())
        } else {
            None
        }
    }

    /// The caption to show for this album.
    ///
    /// Telegram's official clients display the caption of the last message of
    /// the album that has one.
    pub(crate) fn caption(&self) -> String {
        let store = self.store();
        let n = store.n_items();

        // Iterate in reverse so that the LAST non-empty caption is returned.
        for i in (0..n).rev() {
            if let Some(message) = store
                .item(i)
                .and_then(|o| o.downcast::<model::Message>().ok())
            {
                let content = message.content();
                let formatted_text = match &content.0 {
                    tdlib::enums::MessageContent::MessagePhoto(data) => Some(&data.caption),
                    tdlib::enums::MessageContent::MessageVideo(data) => Some(&data.caption),
                    _ => None,
                };

                if let Some(formatted_text) = formatted_text {
                    if !formatted_text.text.is_empty() {
                        return utils::parse_formatted_text(formatted_text.clone());
                    }
                }
            }
        }

        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_support::*;

    const CHAT: i64 = ME;

    fn messages(captions: &[(MessageId, &str)]) -> Vec<model::Message> {
        let session = session();
        let chat = add_chat(&session, CHAT);
        captions
            .iter()
            .map(|&(id, caption)| {
                model::Message::new(&chat, message(CHAT, id, DAY, 1, photo(caption)))
            })
            .collect()
    }

    fn ids(album: &MediaAlbum) -> Vec<MessageId> {
        album.messages().iter().map(|m| m.id()).collect()
    }

    #[test]
    fn keeps_members_sorted_and_unique() {
        isolated(|| {
            let msgs = messages(&[(30, ""), (10, ""), (20, "")]);
            let album = MediaAlbum::new(1);
            assert!(album.is_empty());

            for message in &msgs {
                album.add_message(message.clone());
            }
            album.add_message(msgs[0].clone());

            assert_eq!(album.len(), 3);
            assert_eq!(ids(&album), [10, 20, 30]);
            assert_eq!(album.first_message_id(), Some(10));
            assert_eq!(album.last_message_id(), Some(30));
        });
    }

    #[test]
    fn removes_members() {
        isolated(|| {
            let msgs = messages(&[(10, ""), (20, ""), (30, "")]);
            let album = MediaAlbum::new(1);
            for message in msgs {
                album.add_message(message);
            }

            assert_eq!(album.remove_message(20).map(|m| m.id()), Some(20));
            assert!(album.remove_message(20).is_none());
            assert_eq!(ids(&album), [10, 30]);

            album.remove_message(10);
            album.remove_message(30);
            assert!(album.is_empty());
            assert_eq!(album.first_message_id(), None);
            assert_eq!(album.last_message_id(), None);
        });
    }

    #[test]
    fn caption_is_the_newest_non_empty_one() {
        isolated(|| {
            let msgs = messages(&[(10, "first"), (20, "second"), (30, "")]);
            let album = MediaAlbum::new(1);
            for message in msgs {
                album.add_message(message);
            }
            assert_eq!(album.caption(), "second");

            album.remove_message(20);
            assert_eq!(album.caption(), "first");

            album.remove_message(10);
            assert_eq!(album.caption(), "");
        });
    }
}
