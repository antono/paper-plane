use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::OnceLock;

use glib::subclass::Signal;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::strings;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct MessageSelection {
        pub(super) ids: RefCell<BTreeSet<i64>>,
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) deleted_handler: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageSelection {
        const NAME: &'static str = "PaplMessageSelection";
        type Type = super::MessageSelection;
    }

    impl ObjectImpl for MessageSelection {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecBoolean::builder("active")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt::builder("n-selected")
                        .read_only()
                        .build(),
                ]
            })
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "active" => obj.active().to_value(),
                "n-selected" => obj.n_selected().to_value(),
                _ => unimplemented!(),
            }
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("changed").build()])
        }

        fn dispose(&self) {
            if let (Some(chat), Some(handler)) = (self.chat.upgrade(), self.deleted_handler.take())
            {
                chat.disconnect(handler);
            }
        }
    }
}

glib::wrapper! {
    /// The messages selected in a chat history, as a set of ids. The set (and
    /// not the list view's selection model) is the source of truth, so that the
    /// rows, which the list view recycles, recompute their check marks from it.
    pub(crate) struct MessageSelection(ObjectSubclass<imp::MessageSelection>);
}

impl Default for MessageSelection {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl MessageSelection {
    pub(crate) fn active(&self) -> bool {
        !self.imp().ids.borrow().is_empty()
    }

    pub(crate) fn n_selected(&self) -> u32 {
        self.imp().ids.borrow().len() as u32
    }

    pub(crate) fn connect_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("changed", false, move |values| {
            f(&values[0].get().unwrap());
            None
        })
    }

    /// Switches to another chat, which empties the selection.
    pub(crate) fn set_chat(&self, chat: Option<&model::Chat>) {
        let imp = self.imp();

        if let (Some(old), Some(handler)) = (imp.chat.upgrade(), imp.deleted_handler.take()) {
            old.disconnect(handler);
        }
        self.clear();

        imp.chat.set(chat);

        if let Some(chat) = chat {
            let handler = chat.connect_deleted_message(glib::clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, message| obj.remove(message.id())
            ));
            imp.deleted_handler.replace(Some(handler));
        }
    }

    /// Selects the messages if any of them is not selected, otherwise
    /// deselects them all. An album row passes the ids of all its members.
    pub(crate) fn toggle(&self, ids: &[i64]) {
        let all = self.contains_all(ids);
        self.update(|set| {
            for id in ids {
                if all {
                    set.remove(id);
                } else {
                    set.insert(*id);
                }
            }
        });
    }

    pub(crate) fn clear(&self) {
        self.update(|set| set.clear());
    }

    fn remove(&self, id: i64) {
        self.update(|set| {
            set.remove(&id);
        });
    }

    /// Applies a change and notifies about it only if the set differs after it.
    fn update(&self, f: impl FnOnce(&mut BTreeSet<i64>)) {
        let was_active = self.active();
        let old_n = self.n_selected();

        let changed = {
            let mut set = self.imp().ids.borrow_mut();
            let before = set.clone();
            f(&mut set);
            *set != before
        };
        if !changed {
            return;
        }

        let _guard = self.freeze_notify();
        if was_active != self.active() {
            self.notify("active");
        }
        if old_n != self.n_selected() {
            self.notify("n-selected");
        }
        drop(_guard);
        self.emit_by_name::<()>("changed", &[]);
    }

    pub(crate) fn contains_all(&self, ids: &[i64]) -> bool {
        let set = self.imp().ids.borrow();
        !ids.is_empty() && ids.iter().all(|id| set.contains(id))
    }

    /// The selected ids, the oldest message first (ids grow with time).
    pub(crate) fn ids(&self) -> Vec<i64> {
        self.imp().ids.borrow().iter().copied().collect()
    }

    /// The ids of the selected messages that are still known, the oldest
    /// first. A message whose object is gone is not reported as deleted, so
    /// its id can stay in the set; everything that acts on the selection uses
    /// these ids, to act on what the user sees and checked.
    pub(crate) fn message_ids(&self) -> Vec<i64> {
        self.messages().iter().map(|m| m.id()).collect()
    }

    pub(crate) fn is_exactly(&self, ids: &[i64]) -> bool {
        let set = self.imp().ids.borrow();
        set.len() == ids.iter().collect::<BTreeSet<_>>().len()
            && ids.iter().all(|id| set.contains(id))
    }

    /// The selected messages, the oldest first. Unknown ids are skipped.
    pub(crate) fn messages(&self) -> Vec<model::Message> {
        let Some(chat) = self.imp().chat.upgrade() else {
            return vec![];
        };
        self.ids()
            .into_iter()
            .filter_map(|id| chat.message(id))
            .collect()
    }
}

/// Formats messages the way Telegram copies a selection: a line
/// `[HH:MM] Sender: text` for each of them.
pub(crate) fn format_copied(entries: &[(String, String, String)]) -> String {
    entries
        .iter()
        .map(|(time, sender, text)| format!("[{time}] {sender}: {text}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text a Copy of the messages puts on the clipboard: the bare text of a
/// single message (like Copy Text), otherwise one line per message.
pub(crate) fn copy_text_of(messages: &[model::Message]) -> String {
    if let [message] = messages {
        return strings::message_content(message);
    }

    let entries: Vec<_> = messages
        .iter()
        .map(|message| {
            let time = glib::DateTime::from_unix_local(message.date() as i64)
                .and_then(|date| date.format("%H:%M"))
                .map(|time| time.to_string())
                .unwrap_or_default();
            (
                time,
                strings::message_sender(&message.sender(), true),
                strings::message_content(message),
            )
        })
        .collect();
    format_copied(&entries)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;
    use crate::model::test_support::*;

    #[test]
    fn formats_copied_lines() {
        let entries = [
            ("09:05".to_owned(), "Alice".to_owned(), "hi".to_owned()),
            (
                "09:06".to_owned(),
                "Bob".to_owned(),
                "two\nlines".to_owned(),
            ),
        ];
        assert_eq!(
            format_copied(&entries),
            "[09:05] Alice: hi\n[09:06] Bob: two\nlines"
        );
        assert_eq!(format_copied(&[]), "");
    }

    #[test]
    fn toggle_adds_and_removes_groups() {
        isolated(|| {
            let selection = MessageSelection::default();
            let changes = Rc::new(Cell::new(0));
            selection.connect_changed({
                let changes = changes.clone();
                move |_| changes.set(changes.get() + 1)
            });
            let notified = Rc::new(Cell::new(0));
            selection.connect_notify_local(Some("n-selected"), {
                let notified = notified.clone();
                move |_, _| notified.set(notified.get() + 1)
            });

            selection.toggle(&[1, 2]);
            assert!(selection.active());
            assert_eq!(selection.n_selected(), 2);
            selection.toggle(&[1, 2]);
            assert!(!selection.active());
            assert_eq!(selection.n_selected(), 0);

            selection.toggle(&[1]);
            selection.toggle(&[1, 2]);
            assert_eq!(selection.ids(), [1, 2]);
            assert!(selection.is_exactly(&[2, 1]));
            assert!(!selection.is_exactly(&[1]));

            assert_eq!(changes.get(), 4);
            assert_eq!(notified.get(), 4);

            // Nothing changes, nothing is emitted.
            selection.toggle(&[]);
            selection.clear();
            selection.clear();
            assert_eq!(changes.get(), 5);
        });
    }
}

/// Checks of the selection that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use crate::model::test_support::*;

    eprintln!("selection mode: deleted messages leave the selection");

    isolated(|| {
        let session = session();
        let chat = add_chat(&session, ME);
        // The history keeps the messages alive, the chat only holds them weakly.
        let history = model::ChatHistoryModel::new(&chat);
        new_message(&session, message(ME, 1, DAY, 0, text("one")));
        new_message(&session, message(ME, 2, DAY, 0, text("two")));

        let selection = MessageSelection::default();
        selection.set_chat(Some(&chat));
        selection.toggle(&[1]);
        selection.toggle(&[2]);
        assert_eq!(selection.messages().len(), 2);

        delete_messages(&session, ME, &[1]);
        assert_eq!(selection.ids(), [2]);
        delete_messages(&session, ME, &[2]);
        assert!(!selection.active());

        // An id whose message object is gone stays in the set, but nothing
        // acts on it.
        new_message(&session, message(ME, 4, DAY, 0, text("four")));
        selection.toggle(&[99]);
        selection.toggle(&[4]);
        assert_eq!(selection.ids(), [4, 99]);
        assert_eq!(selection.message_ids(), [4]);
        selection.clear();

        // Switching the chat empties the selection.
        new_message(&session, message(ME, 3, DAY, 0, text("three")));
        selection.toggle(&[3]);
        selection.set_chat(None);
        assert!(!selection.active());
        drop(history);
    });
}
