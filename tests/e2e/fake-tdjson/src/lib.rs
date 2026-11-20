//! A scripted stand-in for TDLib's JSON interface (`libtdjson`).
//!
//! Paper Plane talks to TDLib through three C functions: `td_create_client_id`,
//! `td_send` and `td_receive`. This library implements them on top of a
//! scenario (users, chats, messages, injected failures) instead of the
//! Telegram network, so the application can be driven end to end, offline
//! and deterministically.
//!
//! Environment:
//! - `FAKE_TDJSON_SCENARIO`: path of a scenario JSON file (see
//!   `scenario::Scenario`); the built-in two-chat scenario otherwise.
//! - `FAKE_TDJSON_LOG`: a file that gets one JSON line per request the
//!   application sends, for the tests to assert on.
//!
//! Requests the fake does not know are answered with an error, like TDLib
//! does for invalid requests, and logged with `"unsupported": true`.

pub mod scenario;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::ffi::CStr;
use std::ffi::CString;
use std::io::Write;
use std::os::raw::c_char;
use std::os::raw::c_double;
use std::os::raw::c_int;
use std::sync::Condvar;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use tdlib::enums;
use tdlib::types;

use scenario::Scenario;

/// The first id of messages sent through the fake.
const FIRST_SENT_MESSAGE_ID: i64 = 1 << 30;
/// Ids of messages that are still being sent.
const FIRST_PENDING_MESSAGE_ID: i64 = 1 << 40;

#[derive(Default)]
struct ClientState {
    started: bool,
    main_list_loaded: bool,
}

pub struct Engine {
    scenario: Scenario,
    clients: HashMap<i32, ClientState>,
    next_client_id: i32,
    outbox: VecDeque<String>,
    /// The messages of every chat, by id.
    messages: HashMap<i64, BTreeMap<i64, types::Message>>,
    next_sent_id: i64,
    next_pending_id: i64,
    log: Option<std::fs::File>,
}

impl Engine {
    pub fn new(scenario: Scenario, log: Option<std::fs::File>) -> Self {
        let mut messages = HashMap::new();
        for chat in &scenario.chats {
            let chat_messages: BTreeMap<i64, types::Message> = chat
                .messages
                .iter()
                .map(|m| {
                    let sender = if m.outgoing { scenario.me.id } else { chat.with.id };
                    let message = scenario::text_message(
                        chat.with.id,
                        m.id,
                        sender,
                        m.outgoing,
                        scenario.now - m.ago,
                        &m.text,
                        None,
                    );
                    (m.id, message)
                })
                .collect();
            messages.insert(chat.with.id, chat_messages);
        }

        Self {
            scenario,
            clients: HashMap::new(),
            next_client_id: 1,
            outbox: VecDeque::new(),
            messages,
            next_sent_id: FIRST_SENT_MESSAGE_ID,
            next_pending_id: FIRST_PENDING_MESSAGE_ID,
            log,
        }
    }

    fn from_env() -> Self {
        let scenario = match std::env::var("FAKE_TDJSON_SCENARIO") {
            Ok(path) => std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("fake-tdjson: cannot read {path}: {e}")),
            Err(_) => scenario::DEFAULT_SCENARIO.to_owned(),
        };
        let scenario: Scenario =
            serde_json::from_str(&scenario).expect("fake-tdjson: invalid scenario");
        let log = std::env::var("FAKE_TDJSON_LOG").ok().map(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap_or_else(|e| panic!("fake-tdjson: cannot open {path}: {e}"))
        });
        Self::new(scenario, log)
    }

    pub fn create_client(&mut self) -> i32 {
        let id = self.next_client_id;
        self.next_client_id += 1;
        self.clients.insert(id, ClientState::default());
        id
    }

    /// Takes the next response or update, if there is one.
    pub fn pop(&mut self) -> Option<String> {
        self.outbox.pop_front()
    }

    fn emit(&mut self, client_id: i32, object: impl Serialize, extra: Option<&Value>) {
        let mut value = serde_json::to_value(object).expect("serializable TDLib object");
        value["@client_id"] = json!(client_id);
        if let Some(extra) = extra {
            value["@extra"] = extra.clone();
        }
        self.outbox.push_back(value.to_string());
    }

    fn update(&mut self, client_id: i32, update: enums::Update) {
        self.emit(client_id, update, None);
    }

    fn auth_state(&mut self, client_id: i32, state: enums::AuthorizationState) {
        self.update(
            client_id,
            enums::Update::AuthorizationState(types::UpdateAuthorizationState {
                authorization_state: state,
            }),
        );
    }

    fn log_request(&mut self, client_id: i32, request: &Value, supported: bool) {
        if let Some(log) = &mut self.log {
            let mut line = json!({ "client_id": client_id, "request": request });
            if !supported {
                line["unsupported"] = json!(true);
            }
            let _ = writeln!(log, "{line}");
            let _ = log.flush();
        }
    }

    /// Handles a request of the application.
    pub fn send(&mut self, client_id: i32, request: &str) {
        let request: Value = match serde_json::from_str(request) {
            Ok(request) => request,
            Err(_) => return,
        };
        let type_ = request["@type"].as_str().unwrap_or_default().to_owned();
        let extra = request.get("@extra").cloned();

        // TDLib announces the first state when a client gets its first request.
        let client = self.clients.entry(client_id).or_default();
        if !client.started {
            client.started = true;
            self.auth_state(client_id, enums::AuthorizationState::WaitTdlibParameters);
        }

        if let Some(failure) = self.scenario.fail.get(&type_).cloned() {
            self.log_request(client_id, &request, true);
            self.error(client_id, extra.as_ref(), failure.code, &failure.message);
            return;
        }

        let supported = self.handle(client_id, &type_, &request, extra.as_ref());
        self.log_request(client_id, &request, supported);
        if !supported {
            self.error(
                client_id,
                extra.as_ref(),
                400,
                &format!("fake-tdjson does not implement {type_}"),
            );
        }
    }

    fn ok(&mut self, client_id: i32, extra: Option<&Value>) {
        self.emit(client_id, json!({ "@type": "ok" }), extra);
    }

    fn error(&mut self, client_id: i32, extra: Option<&Value>, code: i32, message: &str) {
        self.emit(
            client_id,
            enums::Error::Error(types::Error {
                code,
                message: message.to_owned(),
            }),
            extra,
        );
    }

    fn ready(&mut self, client_id: i32) {
        let me = scenario::user(&self.scenario.me);
        self.update(client_id, enums::Update::User(types::UpdateUser { user: me }));
        self.auth_state(client_id, enums::AuthorizationState::Ready);
    }

    /// Returns whether the request is supported.
    fn handle(&mut self, client_id: i32, type_: &str, request: &Value, extra: Option<&Value>) -> bool {
        match type_ {
            // Settings and notifications about what the user looks at
            "setLogVerbosityLevel" | "setOption" | "openChat" | "closeChat" | "viewMessages"
            | "sendChatAction" | "setChatDraftMessage" | "toggleChatIsMarkedAsUnread"
            | "toggleChatIsPinned" | "setChatNotificationSettings" | "addRecentlyFoundChat"
            | "optimizeStorage" => self.ok(client_id, extra),

            "setTdlibParameters" => {
                self.ok(client_id, extra);
                match self.scenario.auth.as_str() {
                    "ready" => self.ready(client_id),
                    // A login step Paper Plane does not implement
                    "email" => self.auth_state(
                        client_id,
                        enums::AuthorizationState::WaitEmailAddress(
                            types::AuthorizationStateWaitEmailAddress {
                                allow_apple_id: false,
                                allow_google_id: false,
                            },
                        ),
                    ),
                    _ => self.auth_state(client_id, enums::AuthorizationState::WaitPhoneNumber),
                }
            }
            "setAuthenticationPhoneNumber" => {
                let phone_number = request["phone_number"].as_str().unwrap_or_default().to_owned();
                self.ok(client_id, extra);
                self.auth_state(
                    client_id,
                    enums::AuthorizationState::WaitCode(types::AuthorizationStateWaitCode {
                        code_info: types::AuthenticationCodeInfo {
                            phone_number,
                            r#type: enums::AuthenticationCodeType::Sms(
                                types::AuthenticationCodeTypeSms {
                                    length: self.scenario.code.len() as i32,
                                },
                            ),
                            next_type: None,
                            timeout: 0,
                        },
                    }),
                );
            }
            "checkAuthenticationCode" => {
                if request["code"].as_str() == Some(self.scenario.code.as_str()) {
                    self.ok(client_id, extra);
                    self.ready(client_id);
                } else {
                    self.error(client_id, extra, 400, "PHONE_CODE_INVALID");
                }
            }
            "getMe" => {
                let me = scenario::user(&self.scenario.me);
                self.emit(client_id, enums::User::User(me), extra);
            }
            "getUser" => {
                let id = request["user_id"].as_i64().unwrap_or_default();
                let person = std::iter::once(&self.scenario.me)
                    .chain(self.scenario.chats.iter().map(|c| &c.with))
                    .find(|p| p.id == id)
                    .cloned();
                match person {
                    Some(person) => {
                        self.emit(client_id, enums::User::User(scenario::user(&person)), extra)
                    }
                    None => self.error(client_id, extra, 404, "Not Found"),
                }
            }
            "loadChats" => {
                let is_main = request["chat_list"]["@type"] == "chatListMain";
                let client = self.clients.entry(client_id).or_default();
                if !is_main || client.main_list_loaded {
                    self.error(client_id, extra, 404, "Not Found");
                } else {
                    client.main_list_loaded = true;
                    self.send_chats(client_id);
                    self.ok(client_id, extra);
                }
            }
            "getChatHistory" => {
                let chat_id = request["chat_id"].as_i64().unwrap_or_default();
                let from = request["from_message_id"].as_i64().unwrap_or_default();
                let offset = request["offset"].as_i64().unwrap_or_default();
                let limit = request["limit"].as_i64().unwrap_or(50).max(0) as usize;
                let messages = self.history(chat_id, from, offset, limit);
                self.emit(
                    client_id,
                    enums::Messages::Messages(types::Messages {
                        total_count: messages.len() as i32,
                        messages: messages.into_iter().map(Some).collect(),
                    }),
                    extra,
                );
            }
            "getMessage" => {
                let chat_id = request["chat_id"].as_i64().unwrap_or_default();
                let id = request["message_id"].as_i64().unwrap_or_default();
                match self.messages.get(&chat_id).and_then(|m| m.get(&id)).cloned() {
                    Some(message) => self.emit(client_id, enums::Message::Message(message), extra),
                    None => self.error(client_id, extra, 404, "Not Found"),
                }
            }
            "getMessageProperties" => {
                let chat_id = request["chat_id"].as_i64().unwrap_or_default();
                let id = request["message_id"].as_i64().unwrap_or_default();
                match self.messages.get(&chat_id).and_then(|m| m.get(&id)).cloned() {
                    Some(message) => self.emit(
                        client_id,
                        enums::MessageProperties::MessageProperties(scenario::message_properties(
                            &message,
                        )),
                        extra,
                    ),
                    None => self.error(client_id, extra, 404, "Not Found"),
                }
            }
            // Text without markup stays as it is.
            "parseMarkdown" => {
                let text: types::FormattedText = serde_json::from_value(request["text"].clone())
                    .unwrap_or(types::FormattedText {
                        text: String::new(),
                        entities: vec![],
                    });
                self.emit(client_id, enums::FormattedText::FormattedText(text), extra);
            }
            "getCountries" => self.emit(
                client_id,
                enums::Countries::Countries(types::Countries { countries: vec![] }),
                extra,
            ),
            "sendMessage" => self.send_message(client_id, request, extra),
            "close" => {
                self.ok(client_id, extra);
                self.auth_state(client_id, enums::AuthorizationState::Closing);
                self.auth_state(client_id, enums::AuthorizationState::Closed);
            }
            _ => return false,
        }
        true
    }

    fn send_chats(&mut self, client_id: i32) {
        let chats = self.scenario.chats.clone();
        let count = chats.len() as i64;
        for (i, spec) in chats.iter().enumerate() {
            let user = scenario::user(&spec.with);
            self.update(client_id, enums::Update::User(types::UpdateUser { user }));

            let last = self.messages[&spec.with.id].values().next_back().cloned();
            let last_read = last
                .as_ref()
                .map(|m| {
                    let ids: Vec<i64> = self.messages[&spec.with.id].keys().copied().collect();
                    let unread = (spec.unread.max(0) as usize).min(ids.len());
                    if unread == 0 {
                        m.id
                    } else {
                        ids.get(ids.len() - unread - 1).copied().unwrap_or(0)
                    }
                })
                .unwrap_or(0);
            let chat = scenario::private_chat(spec, last_read);
            self.update(client_id, enums::Update::NewChat(types::UpdateNewChat { chat }));

            // The first chat of the scenario is on top.
            let position = scenario::main_position(count - i as i64);
            self.update(
                client_id,
                enums::Update::ChatLastMessage(types::UpdateChatLastMessage {
                    chat_id: spec.with.id,
                    last_message: last,
                    positions: vec![position],
                }),
            );
        }
    }

    /// The messages of a chat like `getChatHistory`: from `from` (the newest
    /// when 0) toward older ones, shifted by `offset` (negative for newer
    /// ones), newest first.
    fn history(&self, chat_id: i64, from: i64, offset: i64, limit: usize) -> Vec<types::Message> {
        let Some(messages) = self.messages.get(&chat_id) else {
            return vec![];
        };
        let newest_first: Vec<&types::Message> = messages.values().rev().collect();
        let start = if from == 0 {
            0
        } else {
            newest_first.iter().position(|m| m.id <= from).unwrap_or(newest_first.len())
        } as i64;
        let start = (start + offset).clamp(0, newest_first.len() as i64) as usize;
        newest_first[start..].iter().take(limit).map(|m| (*m).clone()).collect()
    }

    fn send_message(&mut self, client_id: i32, request: &Value, extra: Option<&Value>) {
        let chat_id = request["chat_id"].as_i64().unwrap_or_default();
        let content = &request["input_message_content"];
        if content["@type"] != "inputMessageText" || !self.messages.contains_key(&chat_id) {
            self.error(client_id, extra, 400, "fake-tdjson only sends text to known chats");
            return;
        }
        let text = content["text"]["text"].as_str().unwrap_or_default().to_owned();
        let me = self.scenario.me.id;
        let date = self.scenario.now + 1;

        let pending_id = self.next_pending_id;
        self.next_pending_id += 1;
        let pending = scenario::text_message(
            chat_id,
            pending_id,
            me,
            true,
            date,
            &text,
            Some(enums::MessageSendingState::Pending(
                types::MessageSendingStatePending { sending_id: 0 },
            )),
        );
        self.emit(client_id, enums::Message::Message(pending.clone()), extra);
        self.update(
            client_id,
            enums::Update::NewMessage(types::UpdateNewMessage { message: pending }),
        );

        let id = self.next_sent_id;
        self.next_sent_id += 1;
        let sent = scenario::text_message(chat_id, id, me, true, date, &text, None);
        self.messages.get_mut(&chat_id).unwrap().insert(id, sent.clone());
        self.update(
            client_id,
            enums::Update::MessageSendSucceeded(types::UpdateMessageSendSucceeded {
                message: sent,
                old_message_id: pending_id,
            }),
        );
    }
}

// ---------------------------------------------------------------------------
// The C interface of libtdjson

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
static WAKE: Condvar = Condvar::new();

thread_local! {
    /// The last string returned by `td_receive`, valid until the next call
    /// on the same thread, as with TDLib.
    static RECEIVED: std::cell::RefCell<CString> = std::cell::RefCell::new(CString::default());
}

fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    let mut engine = ENGINE.lock().unwrap();
    let engine = engine.get_or_insert_with(Engine::from_env);
    f(engine)
}

#[no_mangle]
pub extern "C" fn td_create_client_id() -> c_int {
    with_engine(|e| e.create_client())
}

/// # Safety
/// `request` must be a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn td_send(client_id: c_int, request: *const c_char) {
    let request = unsafe { CStr::from_ptr(request) }.to_string_lossy().into_owned();
    with_engine(|e| e.send(client_id, &request));
    WAKE.notify_all();
}

#[no_mangle]
pub extern "C" fn td_receive(timeout: c_double) -> *const c_char {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout.max(0.0));
    let mut guard = ENGINE.lock().unwrap();
    loop {
        let engine = guard.get_or_insert_with(Engine::from_env);
        if let Some(next) = engine.pop() {
            drop(guard);
            return RECEIVED.with(|received| {
                *received.borrow_mut() = CString::new(next).unwrap_or_default();
                received.borrow().as_ptr()
            });
        }
        let now = Instant::now();
        if now >= deadline {
            return std::ptr::null();
        }
        guard = WAKE.wait_timeout(guard, deadline - now).unwrap().0;
    }
}

/// Synchronous requests are not used by Paper Plane.
#[no_mangle]
pub extern "C" fn td_execute(_request: *const c_char) -> *const c_char {
    std::ptr::null()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::new(serde_json::from_str(scenario::DEFAULT_SCENARIO).unwrap(), None)
    }

    fn drain(engine: &mut Engine) -> Vec<Value> {
        std::iter::from_fn(|| engine.pop())
            .map(|s| serde_json::from_str(&s).unwrap())
            .collect()
    }

    fn types(values: &[Value]) -> Vec<String> {
        values
            .iter()
            .map(|v| {
                let t = v["@type"].as_str().unwrap().to_owned();
                match v.get("authorization_state") {
                    Some(state) => format!("{t}:{}", state["@type"].as_str().unwrap()),
                    None => t,
                }
            })
            .collect()
    }

    #[test]
    fn starts_logged_in() {
        let mut e = engine();
        let c = e.create_client();
        e.send(c, r#"{"@type":"setLogVerbosityLevel","new_verbosity_level":0,"@extra":1}"#);
        e.send(c, r#"{"@type":"setTdlibParameters","@extra":2}"#);
        let out = drain(&mut e);
        assert_eq!(
            types(&out),
            [
                "updateAuthorizationState:authorizationStateWaitTdlibParameters",
                "ok",
                "ok",
                "updateUser",
                "updateAuthorizationState:authorizationStateReady",
            ]
        );
        assert!(out.iter().all(|v| v["@client_id"] == c));
        assert_eq!(out[1]["@extra"], 1);
    }

    #[test]
    fn every_object_parses_as_tdlib_types() {
        let mut e = engine();
        let c = e.create_client();
        for request in [
            r#"{"@type":"setTdlibParameters","@extra":1}"#,
            r#"{"@type":"getMe","@extra":2}"#,
            r#"{"@type":"loadChats","chat_list":{"@type":"chatListMain"},"limit":20,"@extra":3}"#,
            r#"{"@type":"getChatHistory","chat_id":1001,"from_message_id":0,"offset":0,"limit":10,"only_local":false,"@extra":4}"#,
            r#"{"@type":"sendMessage","chat_id":1001,"input_message_content":{"@type":"inputMessageText","text":{"@type":"formattedText","text":"hi","entities":[]},"clear_draft":true},"@extra":5}"#,
        ] {
            e.send(c, request);
        }
        for value in drain(&mut e) {
            if value.get("@extra").is_some() {
                continue;
            }
            let update: Result<enums::Update, _> = serde_json::from_value(value.clone());
            assert!(update.is_ok(), "{value}: {update:?}");
        }
    }

    #[test]
    fn history_pages() {
        let e = engine();
        let ids = |from, offset, limit| -> Vec<i64> {
            e.history(1001, from, offset, limit).iter().map(|m| m.id).collect()
        };
        assert_eq!(ids(0, 0, 2), [3, 2]);
        assert_eq!(ids(2, 0, 10), [2, 1]);
        assert_eq!(ids(1, -2, 10), [3, 2, 1]);
        assert!(ids(0, 0, 10).len() == 3);
    }

    #[test]
    fn injected_failures() {
        let mut scenario: Scenario = serde_json::from_str(scenario::DEFAULT_SCENARIO).unwrap();
        scenario.fail.insert(
            "viewMessages".into(),
            scenario::Failure {
                code: 500,
                message: "injected".into(),
            },
        );
        let mut e = Engine::new(scenario, None);
        let c = e.create_client();
        e.send(c, r#"{"@type":"setOption","@extra":1}"#);
        e.send(c, r#"{"@type":"viewMessages","@extra":2}"#);
        let out = drain(&mut e);
        assert_eq!(out.last().unwrap()["@type"], "error");
        assert_eq!(out.last().unwrap()["message"], "injected");
    }

    #[test]
    fn unknown_requests_fail() {
        let mut e = engine();
        let c = e.create_client();
        e.send(c, r#"{"@type":"getStarRevenueStatistics","@extra":7}"#);
        let out = drain(&mut e);
        assert_eq!(out.last().unwrap()["@type"], "error");
        assert_eq!(out.last().unwrap()["@extra"], 7);
    }
}
