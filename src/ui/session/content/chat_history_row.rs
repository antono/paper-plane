use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::glib;

use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct ChatHistoryRow {
        /// An `ChatHistoryItem` or `SponsoredMessage`
        pub(super) item: RefCell<Option<glib::Object>>,
        /// The selection this row follows while it is mapped.
        pub(super) selection_handler:
            RefCell<Option<(ui::MessageSelection, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatHistoryRow {
        const NAME: &'static str = "PaplChatHistoryRow";
        type Type = super::ChatHistoryRow;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for ChatHistoryRow {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![glib::ParamSpecObject::builder::<glib::Object>("item")
                    .explicit_notify()
                    .build()]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "item" => obj.set_item(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "item" => obj.item().to_value(),
                _ => unimplemented!(),
            }
        }
    }

    impl WidgetImpl for ChatHistoryRow {
        fn map(&self) {
            self.parent_map();
            self.perform_history_action(ui::ChatHistory::add_to_viewed_message_ids);

            let obj = self.obj();
            if let Some(history) = obj.chat_history() {
                let selection = history.selection().clone();
                let handler = selection.connect_changed(clone!(
                    #[weak]
                    obj,
                    move |_| obj.sync_selection()
                ));
                self.selection_handler.replace(Some((selection, handler)));
            }
            obj.sync_selection();
        }

        fn unmap(&self) {
            if let Some((selection, handler)) = self.selection_handler.take() {
                selection.disconnect(handler);
            }

            self.parent_unmap();
            self.perform_history_action(ui::ChatHistory::remove_from_viewed_message_ids);
        }
    }
    impl BinImpl for ChatHistoryRow {}

    impl ChatHistoryRow {
        fn perform_history_action<F: Fn(&ui::ChatHistory, i64)>(&self, op: F) {
            let chat_history = utils::ancestor::<_, ui::ChatHistory>(&*self.obj());

            if let Some(item) = &*self.item.borrow() {
                for message_id in super::viewed_message_ids(item) {
                    op(&chat_history, message_id);
                }
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatHistoryRow(ObjectSubclass<imp::ChatHistoryRow>)
        @extends gtk::Widget, adw::Bin;
}

impl Default for ChatHistoryRow {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatHistoryRow {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    pub(crate) fn item(&self) -> Option<glib::Object> {
        self.imp().item.borrow().to_owned()
    }

    pub(crate) fn set_item(&self, item: Option<glib::Object>) {
        if self.item() == item {
            return;
        }

        if let Some(ref item) = item {
            if let Some(item) = item.downcast_ref::<model::ChatHistoryItem>() {
                match item.type_() {
                    model::ChatHistoryItemType::Message(message) => {
                        use tdlib::enums::MessageContent::*;

                        match message.content().0 {
                            MessageExpiredPhoto
                            | MessageExpiredVideo
                            | MessageCall(_)
                            | MessageBasicGroupChatCreate(_)
                            | MessageSupergroupChatCreate(_)
                            | MessageChatChangeTitle(_)
                            | MessageChatChangePhoto(_) // TODO: Show photo thumbnail
                            | MessageChatDeletePhoto
                            | MessageChatAddMembers(_)
                            | MessageChatJoinByLink
                            | MessageChatJoinByRequest
                            | MessageChatDeleteMember(_)
                            | MessagePinMessage(_)
                            | MessageScreenshotTaken
                            | MessageGameScore(_)
                            | MessageContactRegistered => {
                                self.get_or_create_event_row()
                                    .set_label(&strings::message_content(message));
                            }
                            _ => self.update_or_create_message_row(message.to_owned().upcast()),
                        }
                    }
                    model::ChatHistoryItemType::DayDivider(date) => {
                        let fmt = if date.year() == glib::DateTime::now_local().unwrap().year() {
                            // Translators: This is a date format in the day divider without the year
                            gettext("%B %e")
                        } else {
                            // Translators: This is a date format in the day divider with the year
                            gettext("%B %e, %Y")
                        };
                        let date = date.format(&fmt).unwrap().to_string();

                        let child = self.get_or_create_event_row();
                        child.set_label(&date);
                    }
                }
            } else if let Some(sponsored_message) = item.downcast_ref::<model::SponsoredMessage>() {
                let content = &sponsored_message.content().0;
                if !matches!(content, tdlib::enums::MessageContent::MessageText(_)) {
                    log::warn!("Unexpected sponsored message of type: {:?}", content);
                }

                self.update_or_create_message_row(sponsored_message.to_owned().upcast());
            } else {
                unreachable!("Unexpected item type: {:?}", item);
            }
        }

        self.imp().item.replace(item);
        self.sync_selection();
        self.notify("item");
    }

    fn chat_history(&self) -> Option<ui::ChatHistory> {
        self.ancestor(ui::ChatHistory::static_type())
            .and_downcast::<ui::ChatHistory>()
    }

    /// Shows the check mark of the message(s) this row displays from the
    /// selection of the chat history. The row is recycled by the list view, so
    /// the state is always computed from the item, never kept in the widget.
    fn sync_selection(&self) {
        if !self.is_mapped() {
            return;
        }
        if let Some(history) = self.chat_history() {
            self.sync_selection_with(history.selection());
        }
    }

    pub(crate) fn sync_selection_with(&self, selection: &ui::MessageSelection) {
        let Some(row) = self.child().and_downcast::<ui::MessageRow>() else {
            return;
        };

        let ids = self
            .item()
            .map(|item| viewed_message_ids(&item))
            .unwrap_or_default();
        // A sponsored message can't be selected.
        let selectable = self
            .item()
            .is_some_and(|item| item.is::<model::ChatHistoryItem>());

        let selected = selectable && selection.contains_all(&ids);
        row.set_selection_state(
            selection.active() && selectable,
            selected,
            // Only a single selected message has selectable text
            selected && selection.is_exactly(&ids),
        );
    }

    fn update_or_create_message_row(&self, message: glib::Object) {
        match self
            .child()
            .and_then(|w| w.downcast::<ui::MessageRow>().ok())
        {
            Some(child) => child.set_message(message),
            None => {
                let child = ui::MessageRow::new(&message);
                self.set_child(Some(&child));
            }
        }
    }

    fn get_or_create_event_row(&self) -> ui::EventRow {
        if let Some(Ok(child)) = self.child().map(|w| w.downcast::<ui::EventRow>()) {
            child
        } else {
            let child = ui::EventRow::new();
            self.set_child(Some(&child));
            child
        }
    }
}

/// The ids of the messages a row shows, which are viewed while it is mapped.
fn viewed_message_ids(item: &glib::Object) -> Vec<i64> {
    if let Some(item) = item.downcast_ref::<model::ChatHistoryItem>() {
        if let model::ChatHistoryItemType::Message(message) = item.type_() {
            // A media album is displayed by one row on behalf of all of its
            // messages, which must all be marked as viewed.
            return match message.media_album() {
                Some(album) => album.messages().iter().map(|m| m.id()).collect(),
                None => vec![message.id()],
            };
        }
    } else if let Some(message) = item.downcast_ref::<model::SponsoredMessage>() {
        return vec![message.message_id()];
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_support::*;

    /// A row of an album marks all of its members as viewed (eaa45b1e).
    #[test]
    fn album_rows_view_all_members() {
        isolated(|| {
            let session = session();
            let chat = add_chat(&session, ME);
            let history = model::ChatHistoryModel::new(&chat);
            new_message(&session, message(ME, 1, DAY, 0, text("alone")));
            for id in 2..=4 {
                new_message(&session, message(ME, id, DAY, 77, photo("")));
            }

            let mut viewed: Vec<Vec<i64>> = (0..history.n_items())
                .filter_map(|i| history.item(i))
                .map(|item| viewed_message_ids(&item))
                .filter(|ids| !ids.is_empty())
                .collect();
            viewed.iter_mut().for_each(|ids| ids.sort());
            viewed.sort();
            assert_eq!(viewed, [vec![1], vec![2, 3, 4]]);
        });
    }
}
