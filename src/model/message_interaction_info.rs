use std::cell::Cell;
use std::cell::RefCell;

use glib::Properties;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

#[derive(Clone, Debug, Default, PartialEq, glib::Boxed)]
#[boxed_type(name = "BoxedMessageReactions")]
pub(crate) struct BoxedMessageReactions(pub(crate) Vec<tdlib::types::MessageReaction>);

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::MessageInteractionInfo)]
    pub(crate) struct MessageInteractionInfo {
        #[property(get)]
        pub(super) reply_count: Cell<u32>,
        #[property(get)]
        pub(super) reactions: RefCell<BoxedMessageReactions>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageInteractionInfo {
        const NAME: &'static str = "MessageInteractionInfo";
        type Type = super::MessageInteractionInfo;
    }

    impl ObjectImpl for MessageInteractionInfo {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }
        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }
    }
}

glib::wrapper! {
    pub(crate) struct MessageInteractionInfo(ObjectSubclass<imp::MessageInteractionInfo>);
}

impl From<Option<tdlib::types::MessageInteractionInfo>> for MessageInteractionInfo {
    fn from(interaction_info: Option<tdlib::types::MessageInteractionInfo>) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp()
            .reply_count
            .set(extract_reply_count(&interaction_info));
        obj.set_reactions(extract_reactions(&interaction_info));
        obj
    }
}

impl MessageInteractionInfo {
    pub(crate) fn update(&self, interaction_info: Option<tdlib::types::MessageInteractionInfo>) {
        self.set_reply_count(extract_reply_count(&interaction_info));
        self.set_reactions(extract_reactions(&interaction_info));
    }

    fn set_reply_count(&self, reply_count: u32) {
        if self.reply_count() == reply_count {
            return;
        }
        self.imp().reply_count.set(reply_count);
        self.notify_reply_count()
    }

    fn set_reactions(&self, reactions: BoxedMessageReactions) {
        if *self.imp().reactions.borrow() == reactions {
            return;
        }
        self.imp().reactions.replace(reactions);
        self.notify_reactions();
    }
}

fn extract_reply_count(interaction_info: &Option<tdlib::types::MessageInteractionInfo>) -> u32 {
    interaction_info
        .as_ref()
        .and_then(|interaction_info| interaction_info.reply_info.as_ref())
        .map(|reply_info| reply_info.reply_count)
        .unwrap_or(0) as u32
}

fn extract_reactions(
    interaction_info: &Option<tdlib::types::MessageInteractionInfo>,
) -> BoxedMessageReactions {
    BoxedMessageReactions(
        interaction_info
            .as_ref()
            .and_then(|interaction_info| interaction_info.reactions.as_ref())
            .map(|reactions| reactions.reactions.clone())
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use tdlib::enums;
    use tdlib::types;

    use crate::model;
    use crate::model::test_support::*;

    const CHAT: i64 = ME;

    fn interaction_info(reactions: &[(&str, i32, bool)]) -> Option<types::MessageInteractionInfo> {
        Some(types::MessageInteractionInfo {
            view_count: 0,
            forward_count: 0,
            reply_info: None,
            reactions: Some(types::MessageReactions {
                reactions: reactions
                    .iter()
                    .map(|&(emoji, total_count, is_chosen)| types::MessageReaction {
                        r#type: enums::ReactionType::Emoji(types::ReactionTypeEmoji {
                            emoji: emoji.to_owned(),
                        }),
                        total_count,
                        is_chosen,
                        used_sender_id: None,
                        recent_sender_ids: vec![],
                    })
                    .collect(),
                are_tags: false,
                paid_reactors: vec![],
                can_get_added_reactions: false,
            }),
        })
    }

    fn summary(message: &model::Message) -> Vec<(String, i32, bool)> {
        message
            .interaction_info()
            .reactions()
            .0
            .iter()
            .map(|r| match &r.r#type {
                enums::ReactionType::Emoji(e) => (e.emoji.clone(), r.total_count, r.is_chosen),
                _ => (String::new(), r.total_count, r.is_chosen),
            })
            .collect()
    }

    #[test]
    fn reactions_of_new_messages_and_updates() {
        isolated(|| {
            let session = session();
            let chat = add_chat(&session, CHAT);
            let _history = model::ChatHistoryModel::new(&chat);

            let mut td_message = message(CHAT, 1, DAY, 0, text("react to me"));
            td_message.interaction_info = interaction_info(&[("👍", 2, true)]);
            new_message(&session, td_message);
            let message = chat.message(1).unwrap();
            assert_eq!(summary(&message), [("👍".to_owned(), 2, true)]);

            let notified = std::rc::Rc::new(std::cell::Cell::new(0));
            message
                .interaction_info()
                .connect_reactions_notify(gtk::glib::clone!(
                    #[strong]
                    notified,
                    move |_| notified.set(notified.get() + 1)
                ));

            // updateMessageInteractionInfo reaches the message through the
            // session and the chat.
            session.handle_update(enums::Update::MessageInteractionInfo(
                types::UpdateMessageInteractionInfo {
                    chat_id: CHAT,
                    message_id: 1,
                    interaction_info: interaction_info(&[("👍", 1, false), ("🔥", 3, false)]),
                },
            ));
            assert_eq!(
                summary(&message),
                [("👍".to_owned(), 1, false), ("🔥".to_owned(), 3, false)]
            );
            assert_eq!(notified.get(), 1);

            // The same reactions again do not notify.
            session.handle_update(enums::Update::MessageInteractionInfo(
                types::UpdateMessageInteractionInfo {
                    chat_id: CHAT,
                    message_id: 1,
                    interaction_info: interaction_info(&[("👍", 1, false), ("🔥", 3, false)]),
                },
            ));
            assert_eq!(notified.get(), 1);

            session.handle_update(enums::Update::MessageInteractionInfo(
                types::UpdateMessageInteractionInfo {
                    chat_id: CHAT,
                    message_id: 1,
                    interaction_info: None,
                },
            ));
            assert!(summary(&message).is_empty());
        });
    }
}
