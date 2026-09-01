use anyhow::Result;
use serde_json::Map;
use serde_json::Value;

use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;

pub(crate) fn translate_response_ids(
    editor: &mut RequestStateEditor<'_>,
    value: &mut Value,
) -> Result<()> {
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    let event_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    for (name, domain) in [
        ("response_id", WireIdDomain::Response),
        ("previous_response_id", WireIdDomain::Response),
        ("item_id", WireIdDomain::Item),
        ("output_item_id", WireIdDomain::Item),
        ("call_id", WireIdDomain::Call),
        ("approval_request_id", WireIdDomain::Approval),
        ("approval_id", WireIdDomain::Approval),
        ("stream_id", WireIdDomain::Stream),
        ("installation_id", WireIdDomain::Installation),
        ("session_id", WireIdDomain::Session),
        ("thread_id", WireIdDomain::Thread),
        ("parent_thread_id", WireIdDomain::Thread),
        ("forked_from_thread_id", WireIdDomain::Thread),
        ("turn_id", WireIdDomain::Turn),
        ("root_turn_id", WireIdDomain::Turn),
        ("parent_turn_id", WireIdDomain::Turn),
    ] {
        translate_field(editor, object, name, domain)?;
    }
    translate_conversation(editor, object)?;

    if let Some(response) = object.get_mut("response").and_then(Value::as_object_mut) {
        translate_response_object(editor, response)?;
    }
    if let Some(item) = object.get_mut("item").and_then(Value::as_object_mut) {
        translate_item(editor, item)?;
    }
    if let Some(items) = object.get_mut("output").and_then(Value::as_array_mut) {
        translate_items(editor, items)?;
    }
    if let Some(metadata) = object
        .get_mut("client_metadata")
        .and_then(Value::as_object_mut)
    {
        translate_identity_metadata(editor, metadata)?;
    }

    let is_terminal_response = event_type.is_none()
        && (object.get("object").and_then(Value::as_str) == Some("response")
            || object.contains_key("output")
            || object.contains_key("usage"));
    if is_terminal_response {
        translate_field(editor, object, "id", WireIdDomain::Response)?;
    }
    Ok(())
}

fn translate_response_object(
    editor: &mut RequestStateEditor<'_>,
    response: &mut Map<String, Value>,
) -> Result<()> {
    translate_field(editor, response, "id", WireIdDomain::Response)?;
    translate_field(
        editor,
        response,
        "previous_response_id",
        WireIdDomain::Response,
    )?;
    translate_field(editor, response, "stream_id", WireIdDomain::Stream)?;
    translate_conversation(editor, response)?;
    if let Some(items) = response.get_mut("output").and_then(Value::as_array_mut) {
        translate_items(editor, items)?;
    }
    if let Some(metadata) = response
        .get_mut("client_metadata")
        .and_then(Value::as_object_mut)
    {
        translate_identity_metadata(editor, metadata)?;
    }
    Ok(())
}

fn translate_items(editor: &mut RequestStateEditor<'_>, items: &mut [Value]) -> Result<()> {
    for item in items {
        if let Some(item) = item.as_object_mut() {
            translate_item(editor, item)?;
        }
    }
    Ok(())
}

fn translate_item(
    editor: &mut RequestStateEditor<'_>,
    item: &mut Map<String, Value>,
) -> Result<()> {
    for (name, domain) in [
        ("id", WireIdDomain::Item),
        ("item_id", WireIdDomain::Item),
        ("output_item_id", WireIdDomain::Item),
        ("call_id", WireIdDomain::Call),
        ("response_id", WireIdDomain::Response),
        ("approval_request_id", WireIdDomain::Approval),
        ("approval_id", WireIdDomain::Approval),
    ] {
        translate_field(editor, item, name, domain)?;
    }
    if let Some(caller) = item.get_mut("caller").and_then(Value::as_object_mut) {
        translate_field(editor, caller, "caller_id", WireIdDomain::Item)?;
    }
    for name in ["pending_safety_checks", "acknowledged_safety_checks"] {
        if let Some(checks) = item.get_mut(name).and_then(Value::as_array_mut) {
            for check in checks {
                if let Some(check) = check.as_object_mut() {
                    translate_field(editor, check, "id", WireIdDomain::Approval)?;
                }
            }
        }
    }
    if let Some(metadata) = item
        .get_mut("internal_chat_message_metadata_passthrough")
        .and_then(Value::as_object_mut)
    {
        translate_identity_metadata(editor, metadata)?;
    }
    Ok(())
}

fn translate_identity_metadata(
    editor: &mut RequestStateEditor<'_>,
    metadata: &mut Map<String, Value>,
) -> Result<()> {
    for (name, domain) in [
        ("x-codex-installation-id", WireIdDomain::Installation),
        ("installation_id", WireIdDomain::Installation),
        ("session_id", WireIdDomain::Session),
        ("conversation_id", WireIdDomain::Session),
        ("thread_id", WireIdDomain::Thread),
        ("parent_thread_id", WireIdDomain::Thread),
        ("forked_from_thread_id", WireIdDomain::Thread),
        ("x-codex-parent-thread-id", WireIdDomain::Thread),
        ("turn_id", WireIdDomain::Turn),
        ("root_turn_id", WireIdDomain::Turn),
        ("parent_turn_id", WireIdDomain::Turn),
    ] {
        translate_field(editor, metadata, name, domain)?;
    }
    for name in ["window_id", "x-codex-window-id"] {
        translate_window(editor, metadata, name)?;
    }
    if let Some(raw) = metadata
        .get("x-codex-turn-metadata")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        let mut nested = serde_json::from_str::<Value>(&raw)?;
        let nested = nested
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("turn metadata is not an object"))?;
        translate_identity_metadata(editor, nested)?;
        metadata.insert(
            "x-codex-turn-metadata".to_string(),
            Value::String(serde_json::to_string(nested)?),
        );
    }
    Ok(())
}

fn translate_window(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    name: &str,
) -> Result<()> {
    let Some(raw) = object.get(name).and_then(Value::as_str).map(str::to_string) else {
        return Ok(());
    };
    let Some((thread, suffix)) = raw.rsplit_once(':') else {
        return Ok(());
    };
    if thread.is_empty() || suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(());
    }
    let thread = editor.wire_from_upstream(WireIdDomain::Thread, thread)?;
    object.insert(
        name.to_string(),
        Value::String(format!("{thread}:{suffix}")),
    );
    Ok(())
}

fn translate_conversation(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
) -> Result<()> {
    let Some(conversation) = object.get_mut("conversation") else {
        return Ok(());
    };
    match conversation {
        Value::String(value) if !value.is_empty() => {
            *value = editor.wire_from_upstream(WireIdDomain::Conversation, value)?;
        }
        Value::Object(conversation) => {
            translate_field(editor, conversation, "id", WireIdDomain::Conversation)?;
        }
        _ => {}
    }
    Ok(())
}

fn translate_field(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    name: &str,
    domain: WireIdDomain,
) -> Result<()> {
    let Some(raw) = object.get(name).and_then(Value::as_str).map(str::to_string) else {
        return Ok(());
    };
    if raw.is_empty() {
        return Ok(());
    }
    object.insert(
        name.to_string(),
        Value::String(editor.wire_from_upstream(domain, &raw)?),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_state_lookup::LookupKeyFactory;
    use crate::request_state_types::PersistedRequestState;
    use std::collections::BTreeSet;

    #[test]
    fn translates_lifecycle_ids_but_not_external_or_opaque_ids() {
        let mut owners = BTreeSet::new();
        owners.insert("acct_response_wire".to_string());
        let mut state = PersistedRequestState::new(owners);
        let mut editor = RequestStateEditor::new(
            &mut state,
            LookupKeyFactory::new("namespace", "scope"),
            "acct_response_wire",
            1,
            86_400_000,
        )
        .expect("editor");
        editor
            .bind_wire_pair(WireIdDomain::Response, "resp_down", "resp_up")
            .expect("response pair");
        editor
            .bind_wire_pair(WireIdDomain::Turn, "turn_down", "turn_up")
            .expect("turn pair");
        let mut event = serde_json::json!({
            "type":"response.output_item.done",
            "response_id":"resp_up",
            "item":{
                "id":"item_provider",
                "call_id":"call_provider",
                "internal_chat_message_metadata_passthrough":{"turn_id":"turn_up"},
                "output":{"file_id":"file_keep","vector_store_id":"vs_keep","opaque_id":"opaque_keep"}
            }
        });
        translate_response_ids(&mut editor, &mut event).expect("translate response");
        assert_eq!(event["response_id"], "resp_down");
        assert_eq!(
            event["item"]["internal_chat_message_metadata_passthrough"]["turn_id"],
            "turn_down"
        );
        assert_ne!(event["item"]["id"], "item_provider");
        assert_ne!(event["item"]["call_id"], "call_provider");
        assert_eq!(event["item"]["output"]["file_id"], "file_keep");
        assert_eq!(event["item"]["output"]["vector_store_id"], "vs_keep");
        assert_eq!(event["item"]["output"]["opaque_id"], "opaque_keep");
    }
}
