use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;

use glib::clone;
use glib::Properties;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::expressions;
use crate::model;
use crate::types::MessageSenderId;
use crate::utils;

#[derive(Clone, Debug, glib::Boxed)]
#[boxed_type(name = "MessageSender")]
pub(crate) enum MessageSender {
    User(model::User),
    Chat(model::Chat),
}

impl MessageSender {
    /// The sender of a message or chat action in `chat`.
    ///
    /// A sender chat that the session does not know (yet) falls back to `chat`
    /// itself instead of panicking: replies of TDLib can overtake the
    /// `updateNewChat` that introduces the sender chat.
    pub(crate) fn new(chat: &model::Chat, sender: &tdlib::enums::MessageSender) -> Self {
        use tdlib::enums::MessageSender::*;

        let session = chat.session_();
        match sender {
            User(data) => Self::User(session.user(data.user_id)),
            Chat(data) => Self::Chat(session.try_chat(data.chat_id).unwrap_or_else(|| {
                log::warn!(
                    "Unknown sender chat {} in chat {}, showing the chat itself",
                    data.chat_id,
                    chat.id()
                );
                chat.clone()
            })),
        }
    }

    pub(crate) fn as_user(&self) -> Option<&model::User> {
        match self {
            Self::User(user) => Some(user),
            _ => None,
        }
    }

    pub(crate) fn id(&self) -> MessageSenderId {
        match self {
            Self::User(user) => user.id(),
            Self::Chat(chat) => chat.id(),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::Message)]
    pub(crate) struct Message {
        #[property(get, set, construct_only)]
        pub(super) chat: glib::WeakRef<model::Chat>,
        #[property(get, set, construct_only)]
        pub(super) id: OnceCell<i64>,
        #[property(get, set, construct_only)]
        pub(super) sender: OnceCell<MessageSender>,
        #[property(get, set, construct_only)]
        pub(super) is_outgoing: OnceCell<bool>,
        #[property(get, set)]
        pub(super) can_be_edited: Cell<bool>,
        #[property(get, set)]
        pub(super) can_be_deleted_only_for_self: Cell<bool>,
        #[property(get, set)]
        pub(super) can_be_deleted_for_all_users: Cell<bool>,
        #[property(get, set)]
        pub(super) can_be_forwarded: Cell<bool>,
        #[property(get, set, construct_only)]
        pub(super) sending_state: OnceCell<Option<model::BoxedMessageSendingState>>,
        #[property(get, set, construct_only)]
        pub(super) date: OnceCell<i32>,
        #[property(get, set, construct_only)]
        pub(super) interaction_info: OnceCell<model::MessageInteractionInfo>,
        #[property(get, set, construct_only)]
        pub(super) forward_info: OnceCell<Option<model::MessageForwardInfo>>,
        #[property(get, set, construct_only)]
        pub(super) reply_to: OnceCell<Option<model::BoxedMessageReplyTo>>,
        #[property(get)]
        pub(super) content: RefCell<model::BoxedMessageContent>,
        #[property(get)]
        pub(super) is_edited: Cell<bool>,
        /// The id of the media album this message belongs to, or 0 if it
        /// doesn't belong to one.
        #[property(get, set, construct_only)]
        pub(super) media_album_id: OnceCell<i64>,
        /// The media album displayed by the row of this message.
        ///
        /// This is only set on the *representative* of a media album, which
        /// is the message displayed in the chat history on behalf of all the
        /// messages of the album. It is a weak reference because the album
        /// owns its messages, so holding a strong reference from a member to
        /// the album would create a cycle.
        #[property(get)]
        pub(super) album: glib::WeakRef<model::MediaAlbum>,
        pub(super) properties_fetched: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Message {
        const NAME: &'static str = "Message";
        type Type = super::Message;
    }

    impl ObjectImpl for Message {
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
    pub(crate) struct Message(ObjectSubclass<imp::Message>);
}

impl Message {
    pub(crate) fn new(chat: &model::Chat, td_message: tdlib::types::Message) -> Self {
        let obj: Self = glib::Object::builder()
            .property("chat", chat)
            .property("id", td_message.id)
            .property(
                "sender",
                model::MessageSender::new(chat, &td_message.sender_id),
            )
            .property("is-outgoing", td_message.is_outgoing)
            .property(
                "sending-state",
                td_message
                    .sending_state
                    .map(model::BoxedMessageSendingState),
            )
            .property("date", td_message.date)
            .property(
                "interaction-info",
                model::MessageInteractionInfo::from(td_message.interaction_info),
            )
            .property(
                "forward-info",
                td_message
                    .forward_info
                    .map(|forward_info| model::MessageForwardInfo::new(chat, forward_info)),
            )
            .property(
                "reply-to",
                td_message.reply_to.map(model::BoxedMessageReplyTo),
            )
            .property("media-album-id", td_message.media_album_id)
            .build();

        let imp = obj.imp();

        log::debug!(
            "Message::new: message id={:?} content={:?}",
            td_message.id,
            td_message.content,
        );

        imp.content
            .replace(model::BoxedMessageContent(td_message.content));
        imp.is_edited.set(td_message.edit_date > 0);

        obj
    }

    pub(crate) fn chat_(&self) -> model::Chat {
        self.chat().unwrap()
    }

    /// The media album this message represents in the chat history, if any.
    pub(crate) fn media_album(&self) -> Option<model::MediaAlbum> {
        self.imp().album.upgrade()
    }

    /// Sets this message as the representative of the given media album, or
    /// clears the representative role with `None`.
    pub(crate) fn set_media_album(&self, album: Option<&model::MediaAlbum>) {
        self.imp().album.set(album);
        self.notify("album");
    }

    /// Fetches the message properties (`can_be_edited`, `can_be_deleted_*`)
    /// from TDLib, if not already fetched.
    ///
    /// This is done lazily, because the properties are only needed when the
    /// context menu of a message is opened, and fetching them for every
    /// message in the history would be wasteful.
    pub(crate) fn fetch_properties(&self) {
        if self.imp().properties_fetched.get() {
            return;
        }
        self.imp().properties_fetched.set(true);

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                obj.load_properties().await;
            }
        ));
    }

    /// Requests the message properties from TDLib and stores them. Unlike
    /// `fetch_properties`, it always asks, so that a caller which needs the
    /// answer (e.g. deleting a selection) can wait for it.
    pub(crate) async fn load_properties(&self) {
        let chat = self.chat_();
        let Ok(client_id) = chat.client_id() else {
            return;
        };
        match tdlib::functions::get_message_properties(chat.id(), self.id(), client_id).await {
            Ok(tdlib::enums::MessageProperties::MessageProperties(properties)) => {
                self.set_can_be_edited(properties.can_be_edited);
                self.set_can_be_deleted_only_for_self(properties.can_be_deleted_only_for_self);
                self.set_can_be_deleted_for_all_users(properties.can_be_deleted_for_all_users);
                self.set_can_be_forwarded(properties.can_be_forwarded);
                self.imp().properties_fetched.set(true);
            }
            Err(e) => log::warn!("Error getting message properties: {e:?}"),
        }
    }

    /// Fetches the reactions that can be added to this message from TDLib.
    ///
    /// See [`merge_available_reactions`] for the order.
    pub(crate) async fn available_reactions(
        &self,
    ) -> Result<Vec<tdlib::types::AvailableReaction>, tdlib::types::Error> {
        let chat = self.chat_();
        let reactions = tdlib::functions::get_message_available_reactions(
            chat.id(),
            self.id(),
            8,
            chat.client_id()?,
        )
        .await?;

        let tdlib::enums::AvailableReactions::AvailableReactions(data) = reactions;
        Ok(merge_available_reactions(data))
    }

    pub(crate) fn handle_update(&self, update: tdlib::enums::Update) {
        use tdlib::enums::Update::*;

        match update {
            MessageContent(data) => {
                let new_content = model::BoxedMessageContent(data.new_content);
                self.set_content(new_content);
            }
            MessageEdited(data) => self.set_is_edited(data.edit_date > 0),
            MessageInteractionInfo(data) => self.interaction_info().update(data.interaction_info),
            _ => {}
        }
    }

    pub(crate) async fn delete(&self, revoke: bool) -> Result<(), tdlib::types::Error> {
        let chat = self.chat_();
        tdlib::functions::delete_messages(chat.id(), vec![self.id()], revoke, chat.client_id()?)
            .await
    }

    /// Adds the reaction to this message, or removes it if the current user
    /// already chose it. Paid reactions cost Telegram Stars and are left
    /// alone.
    pub(crate) fn toggle_reaction(&self, reaction_type: tdlib::enums::ReactionType) {
        if matches!(reaction_type, tdlib::enums::ReactionType::Paid) {
            return;
        }

        let is_chosen = self
            .interaction_info()
            .reactions()
            .0
            .iter()
            .any(|reaction| reaction.is_chosen && reaction.r#type == reaction_type);

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let chat = obj.chat_();
                let Ok(client_id) = chat.client_id() else {
                    return;
                };

                let result = if is_chosen {
                    tdlib::functions::remove_message_reaction(
                        chat.id(),
                        obj.id(),
                        reaction_type,
                        client_id,
                    )
                    .await
                } else {
                    tdlib::functions::add_message_reaction(
                        chat.id(),
                        obj.id(),
                        reaction_type,
                        false,
                        true,
                        client_id,
                    )
                    .await
                };

                if let Err(e) = result {
                    log::warn!("Error toggling a message reaction: {e:?}");
                }
            }
        ));
    }

    fn set_content(&self, content: model::BoxedMessageContent) {
        if self.content() == content {
            return;
        }
        log::debug!(
            "Message::set_content: message id={:?} old={:?} new={:?}",
            self.id(),
            self.content(),
            content,
        );
        self.imp().content.replace(content);
        self.notify_content();
    }

    fn set_is_edited(&self, is_edited: bool) {
        if self.is_edited() == is_edited {
            return;
        }
        self.imp().is_edited.set(is_edited);
        self.notify_is_edited();
    }

    pub(crate) fn sender_name_expression(&self) -> gtk::Expression {
        match self.sender() {
            MessageSender::User(user) => {
                let user_expression = gtk::ConstantExpression::new(user);
                expressions::user_display_name(&user_expression)
            }
            MessageSender::Chat(chat) => gtk::ConstantExpression::new(chat)
                .chain_property::<model::Chat>("title")
                .upcast(),
        }
    }

    pub(crate) fn sender_display_name_expression(&self) -> gtk::Expression {
        if self.chat_().is_own_chat() || self.chat_().is_replies_chat() {
            self.forward_info()
                .map(|forward_info| forward_info.origin())
                .map(|forward_origin| match forward_origin {
                    model::MessageForwardOrigin::User(user) => {
                        let user_expression = gtk::ObjectExpression::new(&user);
                        expressions::user_display_name(&user_expression)
                    }
                    model::MessageForwardOrigin::Chat { chat, .. }
                    | model::MessageForwardOrigin::Channel { chat, .. } => {
                        gtk::ConstantExpression::new(chat)
                            .chain_property::<model::Chat>("title")
                            .upcast()
                    }
                    model::MessageForwardOrigin::HiddenUser { sender_name } => {
                        gtk::ConstantExpression::new(sender_name).upcast()
                    }
                })
                .unwrap_or_else(|| self.sender_display_name_expression())
        } else {
            self.sender_name_expression()
        }
    }
}

/// The reactions offered for a message: the top ones first, then the recent
/// and the popular ones, without duplicates. Together they are every reaction
/// the chat allows, as in the full picker of Telegram. Paid reactions cost
/// Telegram Stars and are not offered.
pub(crate) fn merge_available_reactions(
    data: tdlib::types::AvailableReactions,
) -> Vec<tdlib::types::AvailableReaction> {
    let mut result: Vec<tdlib::types::AvailableReaction> = Vec::new();
    for reaction in data
        .top_reactions
        .into_iter()
        .chain(data.recent_reactions)
        .chain(data.popular_reactions)
    {
        if matches!(reaction.r#type, tdlib::enums::ReactionType::Paid) {
            continue;
        }
        if !result
            .iter()
            .any(|existing| existing.r#type == reaction.r#type)
        {
            result.push(reaction);
        }
    }
    result
}

/// A short text naming a reaction type: the emoji itself, `[ce<id>]` for a
/// custom emoji and `⭐` for the paid reaction. The inverse of
/// [`reaction_type_from_key`].
pub(crate) fn reaction_key(reaction_type: &tdlib::enums::ReactionType) -> String {
    match reaction_type {
        tdlib::enums::ReactionType::Emoji(data) => data.emoji.clone(),
        tdlib::enums::ReactionType::CustomEmoji(data) => format!("[ce{}]", data.custom_emoji_id),
        tdlib::enums::ReactionType::Paid => "⭐".to_owned(),
    }
}

/// The reaction type named by [`reaction_key`].
pub(crate) fn reaction_type_from_key(key: &str) -> tdlib::enums::ReactionType {
    if key == "⭐" {
        return tdlib::enums::ReactionType::Paid;
    }
    if let Some(id) = key
        .strip_prefix("[ce")
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|id| id.parse().ok())
    {
        return tdlib::enums::ReactionType::CustomEmoji(tdlib::types::ReactionTypeCustomEmoji {
            custom_emoji_id: id,
        });
    }
    tdlib::enums::ReactionType::Emoji(tdlib::types::ReactionTypeEmoji {
        emoji: key.to_owned(),
    })
}

#[cfg(test)]
mod reaction_tests {
    use super::*;
    use tdlib::enums::ReactionType;

    fn emoji(emoji: &str) -> ReactionType {
        ReactionType::Emoji(tdlib::types::ReactionTypeEmoji {
            emoji: emoji.to_owned(),
        })
    }

    fn custom(id: i64) -> ReactionType {
        ReactionType::CustomEmoji(tdlib::types::ReactionTypeCustomEmoji {
            custom_emoji_id: id,
        })
    }

    fn available(types: &[ReactionType]) -> Vec<tdlib::types::AvailableReaction> {
        types
            .iter()
            .map(|r#type| tdlib::types::AvailableReaction {
                r#type: r#type.clone(),
                needs_premium: false,
            })
            .collect()
    }

    #[test]
    fn available_reactions_include_the_popular_ones_once() {
        let data = tdlib::types::AvailableReactions {
            top_reactions: available(&[emoji("👍"), emoji("❤")]),
            recent_reactions: available(&[emoji("❤"), custom(137)]),
            popular_reactions: available(&[emoji("👍"), emoji("🔥"), ReactionType::Paid]),
            allow_custom_emoji: false,
            are_tags: false,
            unavailability_reason: None,
        };
        let types: Vec<ReactionType> = merge_available_reactions(data)
            .into_iter()
            .map(|reaction| reaction.r#type)
            .collect();
        assert_eq!(types, [emoji("👍"), emoji("❤"), custom(137), emoji("🔥")]);
    }

    #[test]
    fn reaction_keys_round_trip() {
        for r#type in [emoji("👍"), custom(137), ReactionType::Paid] {
            assert_eq!(reaction_type_from_key(&reaction_key(&r#type)), r#type);
        }
        assert_eq!(reaction_key(&custom(137)), "[ce137]");
        assert_eq!(reaction_type_from_key("[ce1x]"), emoji("[ce1x]"));
    }
}
