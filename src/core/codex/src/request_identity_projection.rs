use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use serde_json::Map;
use serde_json::Value;

use crate::ascii_json::to_ascii_json_string;
use crate::lifecycle_carriers::CarrierAction;
use crate::lifecycle_carriers::CarrierContainer;
use crate::lifecycle_carriers::CarrierShape;
use crate::lifecycle_carriers::RelationshipCarrier;
use crate::lifecycle_carriers::TURN_METADATA_HEADER;
use crate::lifecycle_carriers::projection_rules;
use crate::lifecycle_carriers::turn_metadata_rules;
use crate::request_identity::turn_metadata::bounded_turn_metadata;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedRequestIdentity {
    pub(crate) installation_id: String,
    pub(crate) session_id: String,
    pub(crate) thread_id: String,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) forked_from_thread_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) root_turn_id: Option<String>,
    pub(crate) parent_turn_id: Option<String>,
    pub(crate) window_number: u64,
    pub(crate) request_kind: String,
    pub(crate) turn_started_at_unix_ms: Option<i64>,
}

impl ResolvedRequestIdentity {
    pub(crate) fn window_id(&self) -> String {
        format!("{}:{}", self.thread_id, self.window_number)
    }

    fn prewarm(&self) -> bool {
        self.request_kind == "prewarm"
    }

    fn memory(&self) -> bool {
        self.request_kind == "memory"
    }
}

pub(crate) fn apply(
    headers: &mut HeaderMap,
    object: &mut Map<String, Value>,
    identity: &ResolvedRequestIdentity,
) -> Result<(), ()> {
    project_object(object, CarrierContainer::TopLevel, identity);
    project_headers(headers, identity)?;

    let metadata = object
        .entry("client_metadata".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !metadata.is_object() {
        *metadata = Value::Object(Map::new());
    }
    let metadata = metadata.as_object_mut().ok_or(())?;
    let existing_turn = metadata
        .get(TURN_METADATA_HEADER)
        .and_then(Value::as_str)
        .and_then(parse_object)
        .or_else(|| {
            headers
                .get(TURN_METADATA_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_object)
        })
        .unwrap_or_default();
    let turn = canonical_turn_metadata(existing_turn, identity);
    let serialized = to_ascii_json_string(&Value::Object(turn)).map_err(|_| ())?;

    project_object(metadata, CarrierContainer::ClientMetadata, identity);
    metadata.insert(
        TURN_METADATA_HEADER.to_string(),
        Value::String(serialized.clone()),
    );

    let header_turn = bounded_turn_metadata(&serialized).unwrap_or(serialized);
    insert_header(headers, TURN_METADATA_HEADER, &header_turn)?;
    Ok(())
}

fn canonical_turn_metadata(
    mut turn: Map<String, Value>,
    identity: &ResolvedRequestIdentity,
) -> Map<String, Value> {
    if !identity.memory() {
        crate::sandbox_projection::normalize(&mut turn);
    }
    for rule in
        turn_metadata_rules().filter(|rule| rule.action == CarrierAction::RelationshipProjection)
    {
        match projection_decision(identity, rule.relationship) {
            ProjectionDecision::Set(value) => {
                turn.insert(rule.name.to_string(), value);
            }
            ProjectionDecision::Remove => {
                turn.remove(rule.name);
            }
            ProjectionDecision::Preserve => {}
        }
    }
    turn
}

enum ProjectionDecision {
    Set(Value),
    Remove,
    Preserve,
}

fn project_object(
    object: &mut Map<String, Value>,
    container: CarrierContainer,
    identity: &ResolvedRequestIdentity,
) {
    for rule in projection_rules(container) {
        if rule.shape == CarrierShape::SerializedTurnMetadata {
            continue;
        }
        match projection_decision(identity, rule.relationship) {
            ProjectionDecision::Set(value) => {
                object.insert(rule.name.to_string(), value);
            }
            ProjectionDecision::Remove => {
                object.remove(rule.name);
            }
            ProjectionDecision::Preserve => {}
        }
    }
}

fn project_headers(headers: &mut HeaderMap, identity: &ResolvedRequestIdentity) -> Result<(), ()> {
    for rule in projection_rules(CarrierContainer::Header) {
        if rule.shape == CarrierShape::SerializedTurnMetadata {
            continue;
        }
        match projection_decision(identity, rule.relationship) {
            ProjectionDecision::Set(Value::String(value)) => {
                insert_header(headers, rule.name, &value)?;
            }
            ProjectionDecision::Set(_) => return Err(()),
            ProjectionDecision::Remove | ProjectionDecision::Preserve => {
                headers.remove(rule.name);
            }
        }
    }
    Ok(())
}

fn projection_decision(
    identity: &ResolvedRequestIdentity,
    relationship: Option<RelationshipCarrier>,
) -> ProjectionDecision {
    let string = |value: &str| ProjectionDecision::Set(Value::String(value.to_string()));
    match relationship {
        Some(RelationshipCarrier::Installation) => string(&identity.installation_id),
        Some(RelationshipCarrier::Session) => string(&identity.session_id),
        Some(RelationshipCarrier::Thread) => string(&identity.thread_id),
        Some(RelationshipCarrier::Window) => string(&identity.window_id()),
        Some(RelationshipCarrier::RequestKind) => string(&identity.request_kind),
        Some(RelationshipCarrier::ParentThread) => optional_string(&identity.parent_thread_id),
        Some(RelationshipCarrier::ForkedFromThread) => {
            optional_string(&identity.forked_from_thread_id)
        }
        Some(RelationshipCarrier::Turn) if identity.memory() => ProjectionDecision::Remove,
        Some(RelationshipCarrier::Turn) if identity.prewarm() => string(""),
        Some(RelationshipCarrier::Turn) => optional_string(&identity.turn_id),
        Some(RelationshipCarrier::RootTurn | RelationshipCarrier::ParentTurn)
            if identity.memory() || identity.prewarm() =>
        {
            ProjectionDecision::Remove
        }
        Some(RelationshipCarrier::RootTurn) => optional_string(&identity.root_turn_id),
        Some(RelationshipCarrier::ParentTurn) => optional_string(&identity.parent_turn_id),
        Some(RelationshipCarrier::TurnStartedAt) if identity.memory() || identity.prewarm() => {
            ProjectionDecision::Remove
        }
        Some(RelationshipCarrier::TurnStartedAt) => identity
            .turn_started_at_unix_ms
            .map_or(ProjectionDecision::Remove, |value| {
                ProjectionDecision::Set(Value::from(value))
            }),
        Some(RelationshipCarrier::Subagent) => ProjectionDecision::Preserve,
        _ => ProjectionDecision::Preserve,
    }
}

fn optional_string(value: &Option<String>) -> ProjectionDecision {
    value
        .as_deref()
        .map_or(ProjectionDecision::Remove, |value| {
            ProjectionDecision::Set(Value::String(value.to_string()))
        })
}

fn parse_object(raw: &str) -> Option<Map<String, Value>> {
    serde_json::from_str::<Value>(raw)
        .ok()?
        .as_object()
        .cloned()
}

fn insert_header(headers: &mut HeaderMap, name: &'static str, value: &str) -> Result<(), ()> {
    let value = HeaderValue::from_str(value).map_err(|_| ())?;
    headers.insert(HeaderName::from_static(name), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_one_identity_into_body_header_and_nested_metadata() {
        let mut headers = HeaderMap::new();
        let mut body = serde_json::json!({
            "prompt_cache_key": "conflict",
            "client_metadata": {
                "session_id": "conflict",
                "x-codex-turn-metadata": "{\"workspaces\":{\"/tmp\":{}},\"session_id\":\"conflict\"}"
            }
        })
        .as_object()
        .expect("body")
        .clone();
        let identity = ResolvedRequestIdentity {
            installation_id: "11111111-1111-4111-8111-111111111111".to_string(),
            session_id: "01900000-0000-7000-8000-000000000001".to_string(),
            thread_id: "01900000-0000-7000-8000-000000000001".to_string(),
            parent_thread_id: None,
            forked_from_thread_id: None,
            turn_id: Some("01900000-0000-7000-8000-000000000002".to_string()),
            root_turn_id: Some("01900000-0000-7000-8000-000000000002".to_string()),
            parent_turn_id: None,
            window_number: 3,
            request_kind: "turn".to_string(),
            turn_started_at_unix_ms: Some(1_700_000_000_000),
        };
        apply(&mut headers, &mut body, &identity).expect("projection");
        assert_eq!(body["prompt_cache_key"], identity.session_id);
        assert_eq!(body["client_metadata"]["thread_id"], identity.thread_id);
        let nested: Value = serde_json::from_str(
            body["client_metadata"][TURN_METADATA_HEADER]
                .as_str()
                .expect("nested"),
        )
        .expect("nested JSON");
        assert_eq!(nested["session_id"], identity.session_id);
        assert_eq!(nested["thread_id"], identity.thread_id);
        assert_eq!(nested["window_id"], identity.window_id());
        assert!(nested["workspaces"].get("/tmp").is_some());
        let header: Value = serde_json::from_str(
            headers[TURN_METADATA_HEADER]
                .to_str()
                .expect("header metadata"),
        )
        .expect("header JSON");
        assert_eq!(header["turn_id"], identity.turn_id.unwrap());
    }
}
