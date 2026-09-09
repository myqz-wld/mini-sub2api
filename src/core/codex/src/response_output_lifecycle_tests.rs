use super::*;
use serde_json::json;

fn item(id: &str) -> Value {
    json!({"type":"message","id":id,"status":"completed","content":[]})
}

#[test]
fn every_partial_output_carrier_requires_completion_evidence() {
    for kind in [
        "response.output_text.delta",
        "response.content_part.added",
        "response.refusal.delta",
        "response.reasoning_summary_text.delta",
        "response.reasoning_text.delta",
        "response.function_call_arguments.delta",
        "response.custom_tool_call_input.delta",
    ] {
        for locator in [
            json!({"output_index":0}),
            json!({"item_id":"msg_partial"}),
            json!({"output_index":0,"item_id":"msg_partial"}),
        ] {
            let mut lifecycle = OutputLifecycle::default();
            let mut event = locator;
            event["type"] = json!(kind);
            lifecycle.observe(&event, 8).unwrap();
            assert!(lifecycle.validate_completed(&json!({"output":[]})).is_err());
            assert!(
                lifecycle
                    .validate_completed(&json!({"output":[item("msg_partial")]}))
                    .is_ok()
            );
            let mut done = json!({"type":"response.output_item.done","item":item("msg_partial")});
            if event.get("output_index").is_some() {
                done["output_index"] = event["output_index"].clone();
            }
            lifecycle.observe(&done, 8).unwrap();
            assert!(lifecycle.validate_completed(&json!({})).is_ok());
        }
    }
}

#[test]
fn created_output_and_explicitly_unfinished_status_cannot_disappear() {
    let mut lifecycle = OutputLifecycle::default();
    lifecycle
        .observe(
            &json!({"type":"response.created","response":{"output":[item("msg_partial")]}}),
            8,
        )
        .unwrap();
    assert!(lifecycle.validate_completed(&json!({"output":[]})).is_err());
    assert!(
        lifecycle
            .validate_completed(&json!({"output":[item("msg_other")]}))
            .is_err()
    );
    for status in ["in_progress", "incomplete", "queued", "generating"] {
        let mut partial = item("msg_partial");
        partial["status"] = json!(status);
        let mut lifecycle = OutputLifecycle::default();
        lifecycle
            .observe(
                &json!({"type":"response.output_item.done","output_index":0,"item":partial}),
                8,
            )
            .unwrap();
        assert!(lifecycle.validate_completed(&json!({"output":[]})).is_err());
        assert!(
            lifecycle
                .validate_completed(&json!({"output":[partial]}))
                .is_err()
        );
    }
}

#[test]
fn completion_cannot_borrow_another_items_identity_or_index() {
    let start =
        json!({"type":"response.output_item.added","output_index":1,"item":item("msg_partial")});
    for done in [
        json!({"type":"response.output_item.done","output_index":0,"item":item("msg_partial")}),
        json!({"type":"response.output_item.done","output_index":1,"item":item("msg_other")}),
        json!({"type":"response.output_item.done","output_index":1,"item":{"type":"reasoning","id":"msg_partial"}}),
    ] {
        let mut lifecycle = OutputLifecycle::default();
        lifecycle.observe(&start, 8).unwrap();
        assert!(lifecycle.observe(&done, 8).is_err());
    }
    let mut lifecycle = OutputLifecycle::default();
    lifecycle
        .observe(
            &json!({"type":"response.output_text.delta","item_id":"msg_partial","delta":"text"}),
            8,
        )
        .unwrap();
    lifecycle.observe(&start, 8).unwrap();
    assert_eq!(lifecycle.pending.len(), 1);
    assert_eq!(lifecycle.by_id.len(), 1);
    lifecycle.observe(&json!({"type":"response.output_item.done","output_index":1,"item":item("msg_partial")}), 8).unwrap();
    assert!(lifecycle.pending.is_empty());
    assert!(lifecycle.by_id.is_empty());
    lifecycle
        .observe(
            &json!({"type":"response.output_text.delta","item_id":"msg_partial","delta":"text"}),
            8,
        )
        .unwrap();
    assert!(
        lifecycle
            .validate_completed(&json!({"output":[item("msg_partial"), item("msg_partial")]}))
            .is_err()
    );
}

#[test]
fn memory_limits_and_missing_locators_never_turn_missing_proof_into_success() {
    let mut lifecycle = OutputLifecycle::default();
    for index in 0..3 {
        lifecycle.observe(&json!({"type":"response.output_text.delta","output_index":index,"item_id":format!("msg_{index}")}), 2).unwrap();
    }
    assert_eq!(lifecycle.pending.len(), 2);
    assert_eq!(lifecycle.by_id.len(), 2);
    for index in 0..3 {
        lifecycle.observe(&json!({"type":"response.output_item.done","output_index":index,"item":item(&format!("msg_{index}"))}), 2).unwrap();
    }
    assert!(lifecycle.validate_completed(&json!({"output":[]})).is_err());
    let mut lifecycle = OutputLifecycle::default();
    lifecycle
        .observe(
            &json!({"type":"response.output_text.delta","delta":"unlocated"}),
            2,
        )
        .unwrap();
    assert!(
        lifecycle
            .validate_completed(&json!({"output":[item("msg_0")]}))
            .is_err()
    );
}
