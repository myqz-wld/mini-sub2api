use http::HeaderMap;
use serde_json::Map;
use serde_json::Value;

use crate::lifecycle_carriers::CarrierContainer;
use crate::lifecycle_carriers::CarrierRule;
use crate::lifecycle_carriers::CarrierShape;
use crate::lifecycle_carriers::RelationshipCarrier;
use crate::lifecycle_carriers::TURN_METADATA_HEADER;
use crate::lifecycle_carriers::evidence_rules;
use crate::request_identity::CodexTransport;

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
        transport: CodexTransport,
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
        let sources = EvidenceSources {
            object,
            headers,
            client_metadata: flat,
            body_turn_metadata: body_turn.as_ref(),
            header_turn_metadata: header_turn.as_ref(),
            headers_already_projected: ignore_headers,
        };

        let thread = evidence_text(RelationshipCarrier::Thread, &sources);
        let window = evidence_text(RelationshipCarrier::Window, &sources);
        let conversation = evidence_text(RelationshipCarrier::Session, &sources)
            .or_else(|| thread.clone())
            .or_else(|| evidence_text(RelationshipCarrier::ClientRequest, &sources))
            .or_else(|| window.as_deref().and_then(window_thread));
        let parent_thread = evidence_text(RelationshipCarrier::ParentThread, &sources);
        let forked_from_thread = evidence_text(RelationshipCarrier::ForkedFromThread, &sources);
        let subagent = evidence_text(RelationshipCarrier::Subagent, &sources);
        let request_kind = evidence_text(RelationshipCarrier::RequestKind, &sources)
            .unwrap_or_else(|| {
                if transport == CodexTransport::WebSocket
                    && object.get("generate").and_then(Value::as_bool) == Some(false)
                {
                    "prewarm".to_string()
                } else {
                    "turn".to_string()
                }
            });
        let turn = evidence_text(RelationshipCarrier::Turn, &sources);
        let root_turn = evidence_text(RelationshipCarrier::RootTurn, &sources);
        let parent_turn = evidence_text(RelationshipCarrier::ParentTurn, &sources);

        Self {
            installation: evidence_text(RelationshipCarrier::Installation, &sources),
            conversation,
            responses_conversation: evidence_text(
                RelationshipCarrier::ResponsesConversation,
                &sources,
            ),
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

struct EvidenceSources<'a> {
    object: &'a Map<String, Value>,
    headers: &'a HeaderMap,
    client_metadata: Option<&'a Map<String, Value>>,
    body_turn_metadata: Option<&'a Map<String, Value>>,
    header_turn_metadata: Option<&'a Map<String, Value>>,
    headers_already_projected: bool,
}

fn evidence_text(
    relationship: RelationshipCarrier,
    sources: &EvidenceSources<'_>,
) -> Option<String> {
    evidence_rules(relationship)
        .filter_map(|rule| carrier_text(rule, sources).map(|value| (rule.priority, value)))
        .min_by_key(|(priority, _)| *priority)
        .map(|(_, value)| value)
}

fn carrier_text(rule: &CarrierRule, sources: &EvidenceSources<'_>) -> Option<String> {
    if sources.headers_already_projected && rule.skip_after_header_projection() {
        return None;
    }
    let value = match rule.container {
        CarrierContainer::TopLevel => sources.object.get(rule.name)?,
        CarrierContainer::ClientMetadata => sources.client_metadata?.get(rule.name)?,
        CarrierContainer::TurnMetadata => sources.body_turn_metadata?.get(rule.name)?,
        CarrierContainer::HeaderTurnMetadata => sources.header_turn_metadata?.get(rule.name)?,
        CarrierContainer::Header => {
            return header_text(sources.headers, rule.name)
                .and_then(|value| normalize_text(value, rule.allow_empty()));
        }
        _ => return None,
    };
    if rule.shape == CarrierShape::Conversation {
        return conversation_text(value);
    }
    value
        .as_str()
        .map(str::to_string)
        .and_then(|value| normalize_text(value, rule.allow_empty()))
}

fn conversation_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => nonempty(value),
        Value::Object(conversation) => conversation
            .get("id")
            .and_then(Value::as_str)
            .and_then(nonempty),
        _ => None,
    }
}

fn normalize_text(value: String, allow_empty: bool) -> Option<String> {
    (allow_empty || !value.trim().is_empty()).then_some(value)
}

fn parse_object(raw: &str) -> Option<Map<String, Value>> {
    serde_json::from_str::<Value>(raw)
        .ok()?
        .as_object()
        .cloned()
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
                turn_id: item_turn_id(metadata),
                had_create_time: metadata
                    .and_then(|metadata| metadata.get("create_time"))
                    .is_some_and(Value::is_number),
                is_user: item.get("role").and_then(Value::as_str) == Some("user"),
            }
        })
        .collect()
}

fn item_turn_id(metadata: Option<&Map<String, Value>>) -> Option<String> {
    let metadata = metadata?;
    evidence_rules(RelationshipCarrier::ItemTurn)
        .filter(|rule| rule.container == CarrierContainer::ItemPassthroughMetadata)
        .find_map(|rule| {
            metadata
                .get(rule.name)
                .and_then(Value::as_str)
                .and_then(nonempty)
        })
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
            CodexTransport::Http,
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
            CodexTransport::WebSocket,
            false,
        );
        assert!(evidence.explicit_thread_lineage);
        assert_eq!(evidence.parent_thread.as_deref(), Some("root"));
    }
}
