//! Builders of model objects for unit tests.
//!
//! A [`model::ClientStateSession`] is created without a client manager and
//! without running the main loop, so no request ever reaches TDLib: the
//! objects are fed with the same `Update`s that TDLib would send.

use gtk::glib;
use gtk::glib::prelude::*;
use tdlib::enums;
use tdlib::types;

use crate::model;
use crate::types::ChatId;
use crate::types::MessageId;

/// Runs `f` with a main context of its own as the thread-default one.
///
/// The model objects spawn futures on the thread-default context (to talk to
/// TDLib, which never answers in tests). Without this, they would end up on
/// the global default context, which the GTK test iterates from another
/// thread.
pub(crate) fn isolated<R>(f: impl FnOnce() -> R) -> R {
    glib::MainContext::new()
        .with_thread_default(f)
        .expect("a new main context can be acquired")
}

/// The id of the user of the test session.
pub(crate) const ME: i64 = 137;

/// A day, in seconds since the epoch, at noon UTC (2026-11-19).
pub(crate) const DAY: i32 = 1_795_089_600;

pub(crate) fn user(id: i64) -> types::User {
    types::User {
        id,
        first_name: format!("User {id}"),
        last_name: String::new(),
        usernames: None,
        phone_number: String::new(),
        status: enums::UserStatus::Empty,
        profile_photo: None,
        accent_color_id: 0,
        background_custom_emoji_id: 0,
        upgraded_gift_colors: None,
        profile_accent_color_id: -1,
        profile_background_custom_emoji_id: 0,
        emoji_status: None,
        is_contact: false,
        is_mutual_contact: false,
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

/// A client without a client manager; its state starts as `ClientStateAuth`.
pub(crate) fn client() -> model::Client {
    glib::Object::builder()
        .property("remove-if-auth", false)
        .property("id", 0)
        .property(
            "database-info",
            model::BoxedDatabaseInfo(model::DatabaseInfo {
                directory_base_name: "test".to_owned(),
                use_test_dc: true,
            }),
        )
        .build()
}

/// A session of the user [`ME`] that is fed by hand.
pub(crate) fn session() -> model::ClientStateSession {
    let client = client();
    let session = model::ClientStateSession::new(&client, user(ME));
    // The session refers to its client weakly; in the application the client
    // manager owns it. Here the session keeps it alive.
    unsafe { session.set_data("test-client", client) };
    session
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
        can_pin_messages: false,
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

/// Adds the private chat with [`ME`] (the Saved Messages) to the session.
pub(crate) fn add_chat(session: &model::ClientStateSession, id: ChatId) -> model::Chat {
    add_private_chat(session, id, ME, "Saved Messages")
}

/// Adds a user to the session.
pub(crate) fn add_user(session: &model::ClientStateSession, id: i64) {
    session.handle_update(enums::Update::User(types::UpdateUser { user: user(id) }));
}

/// Adds the private chat with the user `user_id`, who must be known to the
/// session (see [`add_user`]).
pub(crate) fn add_private_chat(
    session: &model::ClientStateSession,
    id: ChatId,
    user_id: i64,
    title: &str,
) -> model::Chat {
    add_chat_of_type(
        session,
        id,
        enums::ChatType::Private(types::ChatTypePrivate { user_id }),
        title,
    )
}

/// Adds a secret chat with the user `user_id`, who must be known to the
/// session (see [`add_user`]).
pub(crate) fn add_secret_chat(
    session: &model::ClientStateSession,
    id: ChatId,
    secret_chat_id: i32,
    user_id: i64,
) -> model::Chat {
    session.handle_update(enums::Update::SecretChat(types::UpdateSecretChat {
        secret_chat: types::SecretChat {
            id: secret_chat_id,
            user_id,
            state: enums::SecretChatState::Ready,
            is_outbound: true,
            key_hash: String::new(),
            layer: 0,
        },
    }));
    add_chat_of_type(
        session,
        id,
        enums::ChatType::Secret(types::ChatTypeSecret {
            secret_chat_id,
            user_id,
        }),
        "Secret",
    )
}

/// The forward info of a message originally sent by the user `user_id`.
pub(crate) fn forwarded_from(user_id: i64) -> Option<types::MessageForwardInfo> {
    Some(types::MessageForwardInfo {
        origin: enums::MessageOrigin::User(types::MessageOriginUser {
            sender_user_id: user_id,
        }),
        date: DAY - 60,
        source: None,
        public_service_announcement_type: String::new(),
    })
}

fn add_chat_of_type(
    session: &model::ClientStateSession,
    id: ChatId,
    r#type: enums::ChatType,
    title: &str,
) -> model::Chat {
    let chat = types::Chat {
        id,
        r#type,
        title: title.to_owned(),
        photo: None,
        accent_color_id: 0,
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
        can_be_deleted_for_all_users: false,
        can_be_reported: false,
        default_disable_notification: false,
        unread_count: 0,
        last_read_inbox_message_id: 0,
        last_read_outbox_message_id: 0,
        unread_mention_count: 0,
        unread_reaction_count: 0,
        unread_poll_vote_count: 0,
        notification_settings: notification_settings(),
        available_reactions: enums::ChatAvailableReactions::All(types::ChatAvailableReactionsAll {
            max_reaction_count: 11,
        }),
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
    };

    session.handle_update(enums::Update::NewChat(types::UpdateNewChat { chat }));
    session.chat(id)
}

pub(crate) fn formatted_text(text: &str) -> types::FormattedText {
    types::FormattedText {
        text: text.to_owned(),
        entities: vec![],
    }
}

pub(crate) fn text(text: &str) -> enums::MessageContent {
    enums::MessageContent::MessageText(types::MessageText {
        text: formatted_text(text),
        link_preview: None,
        link_preview_options: None,
    })
}

fn file(id: i32) -> types::File {
    types::File {
        id,
        size: 1000,
        expected_size: 1000,
        local: types::LocalFile {
            path: String::new(),
            can_be_downloaded: true,
            can_be_deleted: false,
            is_downloading_active: false,
            is_downloading_completed: false,
            download_offset: 0,
            downloaded_prefix_size: 0,
            downloaded_size: 0,
        },
        remote: types::RemoteFile {
            id: String::new(),
            unique_id: String::new(),
            is_uploading_active: false,
            is_uploading_completed: true,
            uploaded_size: 1000,
        },
    }
}

pub(crate) fn photo(caption: &str) -> enums::MessageContent {
    enums::MessageContent::MessagePhoto(types::MessagePhoto {
        photo: types::Photo {
            has_stickers: false,
            minithumbnail: None,
            sizes: vec![types::PhotoSize {
                r#type: "x".to_owned(),
                photo: file(1),
                width: 800,
                height: 600,
                progressive_sizes: vec![],
            }],
        },
        video: None,
        caption: formatted_text(caption),
        show_caption_above_media: false,
        has_spoiler: false,
        is_secret: false,
    })
}

/// A message of [`ME`] in the chat `chat_id`, sent at `date`.
pub(crate) fn message(
    chat_id: ChatId,
    id: MessageId,
    date: i32,
    media_album_id: i64,
    content: enums::MessageContent,
) -> types::Message {
    types::Message {
        id,
        sender_id: enums::MessageSender::User(types::MessageSenderUser { user_id: ME }),
        chat_id,
        sending_state: None,
        scheduling_state: None,
        is_outgoing: true,
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
        media_album_id,
        effect_id: 0,
        restriction_info: None,
        summary_language_code: String::new(),
        content,
        reply_markup: None,
    }
}

/// Sends `message` to the session as an `updateNewMessage`.
pub(crate) fn new_message(session: &model::ClientStateSession, message: types::Message) {
    session.handle_update(enums::Update::NewMessage(types::UpdateNewMessage {
        message,
    }));
}

/// Sends a permanent deletion of the messages to the session.
pub(crate) fn delete_messages(
    session: &model::ClientStateSession,
    chat_id: ChatId,
    message_ids: &[MessageId],
) {
    session.handle_update(enums::Update::DeleteMessages(types::UpdateDeleteMessages {
        chat_id,
        message_ids: message_ids.to_vec(),
        is_permanent: true,
        from_cache: false,
    }));
}
