use http::HeaderMap;
use serde_json::Map;
use serde_json::Value;

use crate::request_identity::SubscriptionTransport;

const TURN_METADATA_HEADER: &str = "x-codex-turn-metadata";
const WINDOW_HEADER: &str = "x-codex-window-id";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RequestIdentityEvidence {
    pub(crate) installation: Option<String>,
    pub(crate) conversation: Option<String>,
    pub(crate) responses_conversation: Option<String>,
    pub(crate) thread: Option<String>,
    pub(crate) parent_thread: Option<String>,
    pub(crate) forked_from_thread: Option<String>,
    pub(crate) explicit_thread_lineage: bool,
    pub(crate) turn: Option<String>,
    pub(crate) root_turn: Option<String>,
    pub(crate) parent_turn: Option<String>,
    pub(crate) items: Vec<ItemIdentityEvidence>,
    pub(crate) new_user_submission: bool,
    pub(crate) window_number: Option<u64>,
    pub(crate) request_kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ItemIdentityEvidence {
    pub(crate) id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) had_create_time: bool,
    pub(crate) is_user: bool,
}

impl RequestIdentityEvidence {
    pub(crate) fn extract(
        object: &Map<String, Value>,
        headers: &HeaderMap,
        transport: SubscriptionTransport,
        ignore_headers: bool,
    ) -> Self {
        let flat = object.get("client_metadata").and_then(Value::as_object);
        let body_turn = flat
            .and_then(|metadata| metadata.get(TURN_METADATA_HEADER))
            .and_then(Value::as_str)
            .and_then(parse_object);
        let header_turn = (!ignore_headers)
            .then(|| header_text(headers, TURN_METADATA_HEADER))
            .flatten()
            .as_deref()
            .and_then(parse_object);
        let header = |name| {
            (!ignore_headers)
                .then(|| header_text(headers, name))
                .flatten()
                .and_then(|value| nonempty(&value))
        };
        let flat_text = |name| object_text(flat, name);
        let body_turn_text = |name| object_text(body_turn.as_ref(), name);
        let header_turn_text = |name| object_text(header_turn.as_ref(), name);

        let thread = flat_text("thread_id")
            .or_else(|| body_turn_text("thread_id"))
            .or_else(|| header("thread-id"))
            .or_else(|| header_turn_text("thread_id"));
        let window = flat_text(WINDOW_HEADER)
            .or_else(|| body_turn_text("window_id"))
            .or_else(|| header(WINDOW_HEADER))
            .or_else(|| header_turn_text("window_id"));
        let conversation = flat_text("session_id")
            .or_else(|| body_turn_text("session_id"))
            .or_else(|| header("session-id"))
            .or_else(|| header_turn_text("session_id"))
            .or_else(|| flat_text("conversation_id"))
            .or_else(|| header("conversation_id"))
            .or_else(|| {
                object
                    .get("prompt_cache_key")
                    .and_then(Value::as_str)
                    .and_then(nonempty)
            })
            .or_else(|| thread.clone())
            .or_else(|| header("x-client-request-id"))
            .or_else(|| window.as_deref().and_then(window_thread));

        let parent_thread = body_turn_text("parent_thread_id")
            .or_else(|| body_turn_text("x-codex-parent-thread-id"))
            .or_else(|| header_turn_text("parent_thread_id"))
            .or_else(|| header("x-codex-parent-thread-id"))
            .or_else(|| flat_text("parent_thread_id"));
        let forked_from_thread = body_turn_text("forked_from_thread_id")
            .or_else(|| header_turn_text("forked_from_thread_id"))
            .or_else(|| flat_text("forked_from_thread_id"));
        let subagent = header_text(headers, "x-openai-subagent")
            .and_then(|value| nonempty(&value))
            .or_else(|| flat_text("x-openai-subagent"))
            .or_else(|| body_turn_text("subagent_kind"));

        let request_kind = body_turn_text("request_kind")
            .or_else(|| header_turn_text("request_kind"))
            .unwrap_or_else(|| {
                if transport == SubscriptionTransport::WebSocket
                    && object.get("generate").and_then(Value::as_bool) == Some(false)
                {
                    "prewarm".to_string()
                } else {
                    "turn".to_string()
                }
            });
        let turn = body_turn_text_allow_empty("turn_id", body_turn.as_ref())
            .or_else(|| header_turn_text_allow_empty("turn_id", header_turn.as_ref()))
            .or_else(|| flat_text_allow_empty("turn_id", flat));
        let root_turn = body_turn_text("root_turn_id")
            .or_else(|| header_turn_text("root_turn_id"))
            .or_else(|| flat_text("root_turn_id"));
        let parent_turn = body_turn_text("parent_turn_id")
            .or_else(|| header_turn_text("parent_turn_id"))
            .or_else(|| flat_text("parent_turn_id"));

        Self {
            installation: flat_text("x-codex-installation-id")
                .or_else(|| body_turn_text("installation_id"))
                .or_else(|| header("x-codex-installation-id"))
                .or_else(|| header_turn_text("installation_id")),
            conversation,
            responses_conversation: responses_conversation(object),
            thread,
            parent_thread,
            forked_from_thread,
            explicit_thread_lineage: subagent.is_some(),
            turn,
            root_turn,
            parent_turn,
            items: item_evidence(object),
            new_user_submission: new_user_submission(object),
            window_number: window.as_deref().and_then(window_number),
            request_kind,
        }
        .with_lineage()
    }

    fn with_lineage(mut self) -> Self {
        self.explicit_thread_lineage |=
            self.parent_thread.is_some() || self.forked_from_thread.is_some();
        self
    }

    pub(crate) fn is_prewarm(&self) -> bool {
        self.request_kind == "prewarm"
    }

    pub(crate) fn is_memory(&self) -> bool {
        self.request_kind == "memory"
    }
}

fn parse_object(raw: &str) -> Option<Map<String, Value>> {
    serde_json::from_str::<Value>(raw)
        .ok()?
        .as_object()
        .cloned()
}

fn object_text(object: Option<&Map<String, Value>>, name: &str) -> Option<String> {
    object?.get(name)?.as_str().and_then(nonempty)
}

fn flat_text_allow_empty(name: &str, object: Option<&Map<String, Value>>) -> Option<String> {
    object?.get(name)?.as_str().map(str::to_string)
}

fn body_turn_text_allow_empty(name: &str, object: Option<&Map<String, Value>>) -> Option<String> {
    flat_text_allow_empty(name, object)
}

fn header_turn_text_allow_empty(name: &str, object: Option<&Map<String, Value>>) -> Option<String> {
    flat_text_allow_empty(name, object)
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

fn window_thread(value: &str) -> Option<String> {
    let (thread, number) = value.rsplit_once(':')?;
    (!thread.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| thread.to_string())
}

fn window_number(value: &str) -> Option<u64> {
    let (_, number) = value.rsplit_once(':')?;
    number.parse().ok()
}

fn item_evidence(object: &Map<String, Value>) -> Vec<ItemIdentityEvidence> {
    object
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|item| {
            let metadata = item
                .get("internal_chat_message_metadata_passthrough")
                .and_then(Value::as_object);
            ItemIdentityEvidence {
                id: item.get("id").and_then(Value::as_str).and_then(nonempty),
                turn_id: metadata
                    .and_then(|metadata| metadata.get("turn_id"))
                    .and_then(Value::as_str)
                    .and_then(nonempty),
                had_create_time: metadata
                    .and_then(|metadata| metadata.get("create_time"))
                    .is_some_and(Value::is_number),
                is_user: item.get("role").and_then(Value::as_str) == Some("user"),
            }
        })
        .collect()
}

fn responses_conversation(object: &Map<String, Value>) -> Option<String> {
    match object.get("conversation")? {
        Value::String(value) => nonempty(value),
        Value::Object(conversation) => conversation
            .get("id")
            .and_then(Value::as_str)
            .and_then(nonempty),
        _ => None,
    }
}

fn new_user_submission(object: &Map<String, Value>) -> bool {
    match object.get("input") {
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(items)) => items
            .iter()
            .rev()
            .find_map(Value::as_object)
            .is_some_and(|item| item.get("role").and_then(Value::as_str) == Some("user")),
        _ => false,
    }
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn body_session_wins_and_root_conflicts_do_not_imply_lineage() {
        let object = serde_json::json!({
            "prompt_cache_key": "cache-conflict",
            "client_metadata": {
                "session_id": "body-session",
                "thread_id": "body-thread",
                "x-codex-turn-metadata": "{\"session_id\":\"nested-session\"}"
            }
        });
        let mut headers = HeaderMap::new();
        headers.insert("session-id", HeaderValue::from_static("header-session"));
        let evidence = RequestIdentityEvidence::extract(
            object.as_object().expect("object"),
            &headers,
            SubscriptionTransport::Http,
            false,
        );
        assert_eq!(evidence.conversation.as_deref(), Some("body-session"));
        assert_eq!(evidence.thread.as_deref(), Some("body-thread"));
        assert!(!evidence.explicit_thread_lineage);
    }

    #[test]
    fn parent_or_subagent_marks_explicit_child_lineage() {
        let object = serde_json::json!({
            "client_metadata": {
                "session_id": "root",
                "thread_id": "child",
                "x-codex-turn-metadata": "{\"parent_thread_id\":\"root\"}"
            }
        });
        let evidence = RequestIdentityEvidence::extract(
            object.as_object().expect("object"),
            &HeaderMap::new(),
            SubscriptionTransport::WebSocket,
            false,
        );
        assert!(evidence.explicit_thread_lineage);
        assert_eq!(evidence.parent_thread.as_deref(), Some("root"));
    }
}
