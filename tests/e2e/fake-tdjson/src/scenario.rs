//! The scenario a fake TDLib plays, and builders of the TDLib objects.

use serde::Deserialize;
use tdlib::enums;
use tdlib::types;

/// The default scenario: a logged-in user with two private chats.
pub const DEFAULT_SCENARIO: &str = include_str!("../../scenarios/two-chats.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    /// `ready` (already logged in), `phone` (log in with a phone number),
    /// `registration` (a new account to register with a name) or `email`
    /// (TDLib asks for an email address, which the app does not support).
    #[serde(default = "default_auth")]
    pub auth: String,
    /// The login code accepted when `auth` is `phone`.
    #[serde(default = "default_code")]
    pub code: String,
    pub me: Person,
    #[serde(default)]
    pub chats: Vec<ChatSpec>,
    /// Users found by searchPublicChats, whose chats are not in the chat
    /// list.
    #[serde(default)]
    pub public: Vec<Person>,
    /// Requests (by `@type`) that fail with the given error.
    #[serde(default)]
    pub fail: std::collections::HashMap<String, Failure>,
    /// Like libtdjson, whose static destructors race against its worker
    /// threads, the process hangs at a normal exit: only `_exit` ends it.
    #[serde(default)]
    pub hang_at_exit: bool,
    /// Ids of messages that can be deleted for the user only.
    #[serde(default)]
    pub undeletable_for_all: Vec<i64>,
    /// The date of the newest message; the others are older.
    #[serde(default = "default_now")]
    pub now: i32,
}

fn default_auth() -> String {
    "ready".into()
}

fn default_code() -> String {
    "12345".into()
}

fn default_now() -> i32 {
    1_795_089_600 // 2026-11-19 12:00 UTC
}

#[derive(Debug, Clone, Deserialize)]
pub struct Person {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatSpec {
    /// The other participant of the private chat; its id is the chat id.
    pub with: Person,
    #[serde(default)]
    pub unread: i32,
    #[serde(default)]
    pub messages: Vec<MessageSpec>,
    /// That many more messages ("Message <n>"), older than `messages`, one
    /// minute apart, with the ids before theirs.
    #[serde(default)]
    pub filler: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageSpec {
    pub id: i64,
    /// Whether the user of the session sent it.
    #[serde(default)]
    pub outgoing: bool,
    pub text: String,
    /// Seconds before `now`.
    #[serde(default)]
    pub ago: i32,
    /// A photo (with `text` as its caption), downloaded from the fake.
    #[serde(default)]
    pub photo: bool,
    /// A file with this name (with `text` as its caption).
    #[serde(default)]
    pub document: Option<String>,
    /// A voice note of this many seconds (with `text` as its caption).
    #[serde(default)]
    pub voice: Option<i32>,
    /// The message of the same chat that this one replies to.
    #[serde(default)]
    pub reply_to: Option<i64>,
    #[serde(default)]
    pub reactions: Vec<ReactionSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReactionSpec {
    /// The emoji, `[ce<id>]` for a custom emoji or `⭐` for the paid
    /// reaction, as the app names them (`model::reaction_key`).
    pub emoji: String,
    pub count: i32,
    /// Whether the user of the session chose it.
    #[serde(default)]
    pub chosen: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Failure {
    #[serde(default = "default_error_code")]
    pub code: i32,
    pub message: String,
}

fn default_error_code() -> i32 {
    500
}

pub fn user(person: &Person) -> types::User {
    let (first_name, last_name) = person
        .name
        .split_once(' ')
        .map(|(f, l)| (f.to_owned(), l.to_owned()))
        .unwrap_or_else(|| (person.name.clone(), String::new()));
    types::User {
        id: person.id,
        first_name,
        last_name,
        usernames: None,
        phone_number: format!("99966{}", person.id),
        status: enums::UserStatus::Empty,
        profile_photo: None,
        accent_color_id: (person.id % 7) as i32,
        background_custom_emoji_id: 0,
        upgraded_gift_colors: None,
        profile_accent_color_id: -1,
        profile_background_custom_emoji_id: 0,
        emoji_status: None,
        is_contact: true,
        is_mutual_contact: true,
        is_close_friend: false,
        verification_status: None,
        is_premium: false,
        is_support: false,
        restriction_info: None,
        active_story_state: None,
        restricts_new_chats: false,
        paid_message_star_count: 0,
        have_access: true,
        r#type: enums::UserType::Regular,
        language_code: String::new(),
        added_to_attachment_menu: false,
    }
}

fn permissions() -> types::ChatPermissions {
    types::ChatPermissions {
        can_send_basic_messages: true,
        can_send_audios: true,
        can_send_documents: true,
        can_send_photos: true,
        can_send_videos: true,
        can_send_video_notes: true,
        can_send_voice_notes: true,
        can_send_polls: true,
        can_send_other_messages: true,
        can_add_link_previews: true,
        can_react_to_messages: true,
        can_edit_tag: false,
        can_change_info: false,
        can_invite_users: false,
        can_pin_messages: true,
        can_create_topics: false,
    }
}

fn notification_settings() -> types::ChatNotificationSettings {
    types::ChatNotificationSettings {
        use_default_mute_for: true,
        mute_for: 0,
        use_default_sound: true,
        sound_id: 0,
        use_default_show_preview: true,
        show_preview: true,
        use_default_mute_stories: true,
        mute_stories: false,
        use_default_story_sound: true,
        story_sound_id: 0,
        use_default_show_story_poster: true,
        show_story_poster: false,
        use_default_disable_pinned_message_notifications: true,
        disable_pinned_message_notifications: false,
        use_default_disable_mention_notifications: true,
        disable_mention_notifications: false,
    }
}

pub fn main_position(order: i64) -> types::ChatPosition {
    types::ChatPosition {
        list: enums::ChatList::Main,
        order,
        is_pinned: false,
        source: None,
    }
}

/// A private chat, without positions and last message: TDLib sends those
/// in separate updates.
pub fn private_chat(spec: &ChatSpec, last_read_inbox_message_id: i64) -> types::Chat {
    types::Chat {
        id: spec.with.id,
        r#type: enums::ChatType::Private(types::ChatTypePrivate {
            user_id: spec.with.id,
        }),
        title: spec.with.name.clone(),
        photo: None,
        accent_color_id: (spec.with.id % 7) as i32,
        background_custom_emoji_id: 0,
        upgraded_gift_colors: None,
        profile_accent_color_id: -1,
        profile_background_custom_emoji_id: 0,
        permissions: permissions(),
        last_message: None,
        positions: vec![],
        chat_lists: vec![],
        message_sender_id: None,
        block_list: None,
        has_protected_content: false,
        is_translatable: false,
        is_marked_as_unread: false,
        view_as_topics: false,
        has_scheduled_messages: false,
        can_be_deleted_only_for_self: true,
        can_be_deleted_for_all_users: true,
        can_be_reported: true,
        default_disable_notification: false,
        unread_count: spec.unread,
        last_read_inbox_message_id,
        last_read_outbox_message_id: 0,
        unread_mention_count: 0,
        unread_reaction_count: 0,
        unread_poll_vote_count: 0,
        notification_settings: notification_settings(),
        available_reactions: enums::ChatAvailableReactions::All(
            types::ChatAvailableReactionsAll {
                max_reaction_count: 11,
            },
        ),
        message_auto_delete_time: 0,
        emoji_status: None,
        background: None,
        theme: None,
        action_bar: None,
        business_bot_manage_bar: None,
        video_chat: types::VideoChat {
            group_call_id: 0,
            has_participants: false,
            default_participant_id: None,
        },
        pending_join_requests: None,
        reply_markup_message_id: 0,
        draft_message: None,
        client_data: String::new(),
    }
}

pub fn text_message(
    chat_id: i64,
    id: i64,
    sender_id: i64,
    is_outgoing: bool,
    date: i32,
    text: &str,
    sending_state: Option<enums::MessageSendingState>,
) -> types::Message {
    types::Message {
        id,
        sender_id: enums::MessageSender::User(types::MessageSenderUser { user_id: sender_id }),
        chat_id,
        sending_state,
        scheduling_state: None,
        is_outgoing,
        is_pinned: false,
        is_from_offline: false,
        can_be_saved: true,
        has_timestamped_media: false,
        is_channel_post: false,
        is_paid_star_suggested_post: false,
        is_paid_ton_suggested_post: false,
        contains_unread_mention: false,
        contains_unread_poll_votes: false,
        date,
        edit_date: 0,
        forward_info: None,
        import_info: None,
        interaction_info: None,
        unread_reactions: vec![],
        fact_check: None,
        suggested_post_info: None,
        reply_to: None,
        topic_id: None,
        self_destruct_type: None,
        self_destruct_in: 0.0,
        auto_delete_in: 0.0,
        via_bot_user_id: 0,
        guest_bot_caller_id: None,
        sender_business_bot_user_id: 0,
        sender_boost_count: 0,
        sender_tag: String::new(),
        paid_message_star_count: 0,
        author_signature: String::new(),
        media_album_id: 0,
        effect_id: 0,
        restriction_info: None,
        summary_language_code: String::new(),
        content: enums::MessageContent::MessageText(types::MessageText {
            text: types::FormattedText {
                text: text.to_owned(),
                entities: vec![],
            },
            link_preview: None,
            link_preview_options: None,
        }),
        reply_markup: None,
    }
}

/// What the user may do with a message of a private chat.
pub fn message_properties(
    message: &types::Message,
    undeletable_for_all: &[i64],
) -> types::MessageProperties {
    let own = message.is_outgoing;
    types::MessageProperties {
        can_add_offer: false,
        can_add_tasks: false,
        can_be_approved: false,
        can_be_copied: true,
        can_be_copied_to_secret_chat: true,
        can_be_declined: false,
        can_be_deleted_only_for_self: true,
        can_be_deleted_for_all_users: !undeletable_for_all.contains(&message.id),
        can_be_edited: own,
        can_be_forwarded: true,
        can_be_paid: false,
        can_be_pinned: true,
        can_be_replied: true,
        can_be_replied_in_another_chat: true,
        can_be_saved: true,
        can_be_shared_in_story: false,
        can_delete_reactions: false,
        can_edit_media: false,
        can_edit_scheduling_state: false,
        can_edit_suggested_post_info: false,
        can_get_author: false,
        can_get_embedding_code: false,
        can_get_link: false,
        can_get_media_timestamp_links: false,
        can_get_message_thread: false,
        can_get_poll_vote_statistics: false,
        can_get_read_date: own,
        can_get_statistics: false,
        can_get_video_advertisements: false,
        can_get_viewers: false,
        can_mark_tasks_as_done: false,
        can_recognize_speech: false,
        can_report_chat: false,
        can_report_reactions: false,
        can_report_supergroup_spam: false,
        can_set_fact_check: false,
        has_protected_content_by_current_user: false,
        has_protected_content_by_other_user: false,
        need_show_statistics: false,
    }
}

/// The id of the file of the photo of a message.
pub fn photo_file_id(chat_id: i64, message_id: i64) -> i32 {
    (100_000 + (chat_id % 1000) * 1000 + message_id) as i32
}

pub fn file(id: i32, path: Option<&str>, size: i64) -> types::File {
    types::File {
        id,
        size,
        expected_size: size,
        local: types::LocalFile {
            path: path.unwrap_or_default().to_owned(),
            can_be_downloaded: true,
            can_be_deleted: path.is_some(),
            is_downloading_active: false,
            is_downloading_completed: path.is_some(),
            download_offset: 0,
            downloaded_prefix_size: if path.is_some() { size } else { 0 },
            downloaded_size: if path.is_some() { size } else { 0 },
        },
        remote: types::RemoteFile {
            id: format!("remote-{id}"),
            unique_id: format!("unique-{id}"),
            is_uploading_active: false,
            is_uploading_completed: true,
            uploaded_size: size,
        },
    }
}

pub fn photo_content(file_id: i32, size: i64, caption: &str) -> enums::MessageContent {
    enums::MessageContent::MessagePhoto(types::MessagePhoto {
        photo: types::Photo {
            has_stickers: false,
            minithumbnail: None,
            sizes: vec![types::PhotoSize {
                r#type: "x".to_owned(),
                photo: file(file_id, None, size),
                width: 64,
                height: 48,
                progressive_sizes: vec![],
            }],
        },
        video: None,
        caption: types::FormattedText {
            text: caption.to_owned(),
            entities: vec![],
        },
        show_caption_above_media: false,
        has_spoiler: false,
        is_secret: false,
    })
}

pub fn reply_to(chat_id: i64, message_id: i64) -> enums::MessageReplyTo {
    enums::MessageReplyTo::Message(types::MessageReplyToMessage {
        chat_id,
        message_id,
        quote: None,
        checklist_task_id: 0,
        poll_option_id: String::new(),
        origin: None,
        origin_send_date: 0,
        content: None,
    })
}

pub fn interaction_info(reactions: &[ReactionSpec]) -> Option<types::MessageInteractionInfo> {
    if reactions.is_empty() {
        return None;
    }
    Some(types::MessageInteractionInfo {
        view_count: 0,
        forward_count: 0,
        reply_info: None,
        reactions: Some(types::MessageReactions {
            reactions: reactions
                .iter()
                .map(|r| types::MessageReaction {
                    r#type: reaction_type(&r.emoji),
                    total_count: r.count,
                    is_chosen: r.chosen,
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

pub fn document_content(file_id: i32, file_name: &str, caption: &str) -> enums::MessageContent {
    enums::MessageContent::MessageDocument(types::MessageDocument {
        document: types::Document {
            file_name: file_name.to_owned(),
            mime_type: "application/octet-stream".to_owned(),
            minithumbnail: None,
            thumbnail: None,
            document: file(file_id, None, 1000),
        },
        caption: types::FormattedText {
            text: caption.to_owned(),
            entities: vec![],
        },
    })
}

/// A private chat with `person` outside of any chat list.
pub fn unlisted_chat(person: &Person) -> types::Chat {
    let spec = ChatSpec {
        with: person.clone(),
        unread: 0,
        messages: vec![],
        filler: 0,
    };
    let mut chat = private_chat(&spec, 0);
    chat.positions = vec![];
    chat
}

/// The reaction type of a key of a [`ReactionSpec`].
pub fn reaction_type(key: &str) -> enums::ReactionType {
    if key == "⭐" {
        return enums::ReactionType::Paid;
    }
    match key
        .strip_prefix("[ce")
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|id| id.parse().ok())
    {
        Some(custom_emoji_id) => {
            enums::ReactionType::CustomEmoji(types::ReactionTypeCustomEmoji { custom_emoji_id })
        }
        None => enums::ReactionType::Emoji(types::ReactionTypeEmoji {
            emoji: key.to_owned(),
        }),
    }
}

/// The key of a reaction type of a request, see [`reaction_type`].
pub fn reaction_key(request: &serde_json::Value) -> String {
    match request["@type"].as_str() {
        Some("reactionTypeCustomEmoji") => format!(
            "[ce{}]",
            request["custom_emoji_id"]
                .as_i64()
                .or_else(|| request["custom_emoji_id"].as_str()?.parse().ok())
                .unwrap_or_default()
        ),
        Some("reactionTypePaid") => "⭐".to_owned(),
        _ => request["emoji"].as_str().unwrap_or_default().to_owned(),
    }
}

/// The reactions offered for every message: a few top ones and the popular
/// ones, a custom emoji among them.
pub fn available_reactions() -> types::AvailableReactions {
    let available = |keys: &[&str]| {
        keys.iter()
            .map(|key| types::AvailableReaction {
                r#type: reaction_type(key),
                needs_premium: false,
            })
            .collect()
    };
    types::AvailableReactions {
        top_reactions: available(&["👍", "❤", "🔥"]),
        recent_reactions: available(&["[ce137]"]),
        popular_reactions: available(&["👍", "🎉", "🤔", "😢", "⭐"]),
        allow_custom_emoji: false,
        are_tags: false,
        unavailability_reason: None,
    }
}

pub fn voice_note_content(file_id: i32, duration: i32, caption: &str) -> enums::MessageContent {
    enums::MessageContent::MessageVoiceNote(types::MessageVoiceNote {
        voice_note: types::VoiceNote {
            duration,
            waveform: String::new(),
            mime_type: "audio/ogg".to_owned(),
            speech_recognition_result: None,
            voice: file(file_id, None, 1000),
        },
        caption: types::FormattedText {
            text: caption.to_owned(),
            entities: vec![],
        },
        is_listened: true,
    })
}

/// The URL entities of a text, as TDLib finds them: every word starting with
/// `http://` or `https://`, with UTF-16 offsets.
pub fn url_entities(text: &str) -> Vec<types::TextEntity> {
    let mut entities = vec![];
    let mut offset = 0;
    for word in text.split(' ') {
        let length = word.encode_utf16().count() as i32;
        if word.starts_with("http://") || word.starts_with("https://") {
            entities.push(types::TextEntity {
                offset,
                length,
                r#type: enums::TextEntityType::Url,
            });
        }
        offset += length + 1;
    }
    entities
}

/// Whether a message matches a `SearchMessagesFilter` of `searchChatMessages`.
pub fn matches_search_filter(message: &types::Message, filter: &str) -> bool {
    use enums::MessageContent as C;

    match filter {
        "searchMessagesFilterPhotoAndVideo" => {
            matches!(message.content, C::MessagePhoto(_) | C::MessageVideo(_))
        }
        "searchMessagesFilterPhoto" => matches!(message.content, C::MessagePhoto(_)),
        "searchMessagesFilterDocument" => matches!(message.content, C::MessageDocument(_)),
        "searchMessagesFilterVoiceNote" => matches!(message.content, C::MessageVoiceNote(_)),
        "searchMessagesFilterAudio" => matches!(message.content, C::MessageAudio(_)),
        "searchMessagesFilterUrl" => match &message.content {
            C::MessageText(data) => !data.text.entities.is_empty() || data.link_preview.is_some(),
            _ => false,
        },
        "" | "searchMessagesFilterEmpty" => true,
        _ => false,
    }
}
