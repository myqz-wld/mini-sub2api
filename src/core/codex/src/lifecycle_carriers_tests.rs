use super::*;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_lookup::LookupKeyFactory;
use crate::request_state_types::PersistedRequestState;
use crate::request_wire_ids::translate_request_ids;
use crate::response_wire_ids::translate_response_ids;
use std::collections::BTreeSet;

#[test]
fn carrier_contract_has_no_duplicate_placements() {
    let mut seen = BTreeSet::new();
    for rule in all_rules() {
        let key = format!(
            "{:?}/{:?}/{:?}/{}/{:?}/{:?}",
            rule.direction, rule.use_case, rule.container, rule.name, rule.shape, rule.relationship
        );
        assert!(seen.insert(key.clone()), "duplicate carrier rule: {key}");
    }
}

#[test]
fn reversible_scalar_carriers_have_an_explicit_domain() {
    for rule in all_rules().filter(|rule| rule.use_case == CarrierUse::Wire) {
        let owns_identifier = matches!(
            rule.shape,
            CarrierShape::Scalar
                | CarrierShape::Conversation
                | CarrierShape::TypedItemId
                | CarrierShape::OwnedResponseId
                | CarrierShape::TerminalResponseId
                | CarrierShape::Window
        );
        assert_eq!(
            rule.domain.is_some(),
            owns_identifier,
            "wire carrier has inconsistent domain ownership: {rule:?}"
        );
        assert_eq!(rule.action, CarrierAction::ReversibleWireId);
    }
}

#[test]
fn every_persisted_wire_domain_is_owned_by_the_contract() {
    let domains = all_rules()
        .filter(|rule| rule.use_case == CarrierUse::Wire)
        .filter_map(|rule| rule.domain)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        domains,
        BTreeSet::from([
            WireIdDomain::Installation,
            WireIdDomain::Session,
            WireIdDomain::Thread,
            WireIdDomain::Turn,
            WireIdDomain::Response,
            WireIdDomain::Conversation,
            WireIdDomain::Stream,
            WireIdDomain::Item,
            WireIdDomain::Call,
            WireIdDomain::Approval,
        ])
    );
}

#[test]
fn evidence_priorities_are_unique_per_relationship() {
    for relationship in [
        RelationshipCarrier::Installation,
        RelationshipCarrier::Session,
        RelationshipCarrier::ResponsesConversation,
        RelationshipCarrier::Thread,
        RelationshipCarrier::ParentThread,
        RelationshipCarrier::ForkedFromThread,
        RelationshipCarrier::Turn,
        RelationshipCarrier::RootTurn,
        RelationshipCarrier::ParentTurn,
        RelationshipCarrier::Window,
        RelationshipCarrier::RequestKind,
        RelationshipCarrier::Subagent,
        RelationshipCarrier::ItemTurn,
        RelationshipCarrier::ClientRequest,
    ] {
        let mut priorities = BTreeSet::new();
        for rule in evidence_rules(relationship) {
            assert!(
                priorities.insert(rule.priority),
                "duplicate evidence priority for {relationship:?}: {}",
                rule.priority
            );
        }
        assert!(
            !priorities.is_empty(),
            "missing evidence for {relationship:?}"
        );
    }
}

#[test]
fn turn_metadata_order_and_visibility_are_table_owned() {
    let names = turn_metadata_rules()
        .map(|rule| rule.name)
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "installation_id",
            "session_id",
            "thread_id",
            "agent_name",
            "turn_id",
            "window_id",
            "request_kind",
            "forked_from_thread_id",
            "parent_thread_id",
            "parent_turn_id",
            "root_turn_id",
            "subagent_kind",
            "thread_source",
            "sandbox",
            "sandbox_mode",
            "auto_review_enabled",
            "node_repl_auto_review_required",
            "node_repl_disabled",
            "workspaces",
            "tool_namespaces_info",
            "turn_started_at_unix_ms",
            "compaction",
        ]
    );
    let body_only = turn_metadata_rules()
        .filter(|rule| !rule.header_visible())
        .map(|rule| rule.name)
        .collect::<Vec<_>>();
    assert_eq!(body_only, ["tool_namespaces_info"]);
}

#[test]
fn response_header_policy_is_exact_and_default_deny() {
    for name in ["x-request-id", "openai-request-id", "request-id"] {
        assert_eq!(
            response_header_action(name),
            CarrierAction::GatewayRequestAlias
        );
    }
    assert_eq!(
        response_header_action("x-codex-turn-state"),
        CarrierAction::Opaque
    );
    assert_eq!(
        response_header_action("X-Unknown-Provider-ID"),
        CarrierAction::PublicStrip
    );
    assert_eq!(
        response_header_action(TURN_METADATA_HEADER),
        CarrierAction::PublicStrip
    );
}

#[test]
fn opaque_boundaries_are_container_specific() {
    let is_opaque = |direction, container, name| {
        rules_for(direction, CarrierUse::OpaqueBoundary, container)
            .any(|rule| rule.name == name && rule.shape == CarrierShape::Opaque)
    };
    assert!(is_opaque(
        CarrierDirection::Request,
        CarrierContainer::Item,
        "arguments"
    ));
    assert!(is_opaque(
        CarrierDirection::Request,
        CarrierContainer::Item,
        "output"
    ));
    assert!(is_opaque(
        CarrierDirection::Response,
        CarrierContainer::Item,
        "content"
    ));
    assert!(
        wire_rules(CarrierDirection::Response, CarrierContainer::TopLevel)
            .any(|rule| rule.name == "output" && rule.shape == CarrierShape::ItemArray)
    );
}

#[test]
fn request_and_response_carriers_round_trip_through_one_contract() {
    let mut state =
        PersistedRequestState::new(BTreeSet::from(["acct_carrier_roundtrip".to_string()]));
    let mut editor = RequestStateEditor::new(
        &mut state,
        LookupKeyFactory::new("namespace", "scope"),
        "acct_carrier_roundtrip",
        1,
        86_400_000,
    )
    .expect("editor");
    let mut request = serde_json::json!({
        "response_id":"resp_down",
        "previous_response_id":"resp_previous_down",
        "stream_id":"stream_down",
        "conversation":{"id":"conv_down"},
        "input":[{
            "type":"function_call_output",
            "id":"item_down",
            "call_id":"call_down",
            "approval_request_id":"approval_down",
            "caller":{"caller_id":"caller_down"},
            "pending_safety_checks":[{"id":"safety_down"}],
            "output":{"file_id":"file_opaque","opaque_id":"opaque_down"}
        }]
    })
    .as_object()
    .expect("request")
    .clone();
    translate_request_ids(&mut editor, &mut request, &BTreeSet::new()).expect("request translate");

    let mut response = serde_json::json!({
        "object":"response",
        "id":request["response_id"],
        "previous_response_id":request["previous_response_id"],
        "stream_id":request["stream_id"],
        "conversation":request["conversation"],
        "output":[{
            "id":request["input"][0]["id"],
            "call_id":request["input"][0]["call_id"],
            "approval_request_id":request["input"][0]["approval_request_id"],
            "caller":request["input"][0]["caller"],
            "pending_safety_checks":request["input"][0]["pending_safety_checks"],
            "content":{"file_id":"file_opaque","opaque_id":"opaque_up"}
        }]
    });
    translate_response_ids(&mut editor, &mut response, None).expect("response translate");

    assert_eq!(response["id"], "resp_down");
    assert_eq!(response["previous_response_id"], "resp_previous_down");
    assert_eq!(response["stream_id"], "stream_down");
    assert_eq!(response["conversation"]["id"], "conv_down");
    assert_eq!(response["output"][0]["id"], "item_down");
    assert_eq!(response["output"][0]["call_id"], "call_down");
    assert_eq!(
        response["output"][0]["approval_request_id"],
        "approval_down"
    );
    assert_eq!(response["output"][0]["caller"]["caller_id"], "caller_down");
    assert_eq!(
        response["output"][0]["pending_safety_checks"][0]["id"],
        "safety_down"
    );
    assert_eq!(response["output"][0]["content"]["file_id"], "file_opaque");
    assert_eq!(response["output"][0]["content"]["opaque_id"], "opaque_up");
}

#[test]
fn special_shapes_preserve_generated_items_and_translate_windows_only() {
    let mut state = PersistedRequestState::new(BTreeSet::from(["acct_shapes".to_string()]));
    let mut editor = RequestStateEditor::new(
        &mut state,
        LookupKeyFactory::new("namespace", "scope"),
        "acct_shapes",
        1,
        86_400_000,
    )
    .expect("editor");
    editor
        .bind_wire_pair(WireIdDomain::Thread, "thread_down", "thread_up")
        .expect("thread pair");
    let mut request = serde_json::json!({
        "conversation":"conversation_down",
        "input":[{
            "id":"msg_generated",
            "content":[{"type":"input_text","text":"opaque"}]
        }]
    })
    .as_object()
    .expect("request")
    .clone();
    translate_request_ids(
        &mut editor,
        &mut request,
        &BTreeSet::from(["msg_generated".to_string()]),
    )
    .expect("request translate");
    assert_eq!(request["input"][0]["id"], "msg_generated");
    assert_ne!(request["conversation"], "conversation_down");

    let nested = serde_json::json!({
        "thread_id":"thread_up",
        "window_id":"thread_up:9",
        "workspaces":{"/tmp":{"opaque_id":"keep"}}
    })
    .to_string();
    let mut response = serde_json::json!({
        "type":"response.completed",
        "client_metadata":{
            "x-codex-window-id":"thread_up:9",
            "x-codex-turn-metadata":nested,
            "external_id":"external_keep"
        }
    });
    translate_response_ids(&mut editor, &mut response, None).expect("response translate");
    assert_eq!(
        response["client_metadata"]["x-codex-window-id"],
        "thread_down:9"
    );
    let nested: serde_json::Value = serde_json::from_str(
        response["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("serialized turn metadata"),
    )
    .expect("turn metadata JSON");
    assert_eq!(nested["thread_id"], "thread_down");
    assert_eq!(nested["window_id"], "thread_down:9");
    assert_eq!(nested["workspaces"]["/tmp"]["opaque_id"], "keep");
    assert_eq!(response["client_metadata"]["external_id"], "external_keep");
}
