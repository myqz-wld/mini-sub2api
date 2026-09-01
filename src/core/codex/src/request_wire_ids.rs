use anyhow::Result;
use serde_json::Map;
use serde_json::Value;
use std::collections::BTreeSet;

use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;

pub(crate) fn translate_request_ids(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    generated_upstream_item_ids: &BTreeSet<String>,
) -> Result<()> {
    translate_field(
        editor,
        object,
        "previous_response_id",
        WireIdDomain::Response,
    )?;
    translate_field(editor, object, "response_id", WireIdDomain::Response)?;
    translate_field(editor, object, "stream_id", WireIdDomain::Stream)?;
    translate_field(editor, object, "item_id", WireIdDomain::Item)?;
    translate_field(editor, object, "output_item_id", WireIdDomain::Item)?;
    translate_field(editor, object, "call_id", WireIdDomain::Call)?;
    translate_field(
        editor,
        object,
        "approval_request_id",
        WireIdDomain::Approval,
    )?;
    translate_field(editor, object, "approval_id", WireIdDomain::Approval)?;
    translate_conversation(editor, object)?;
    if let Some(item) = object.get_mut("item").and_then(Value::as_object_mut) {
        translate_item(editor, item, generated_upstream_item_ids)?;
    }
    if let Some(items) = object.get_mut("items").and_then(Value::as_array_mut) {
        for item in items {
            if let Some(item) = item.as_object_mut() {
                translate_item(editor, item, generated_upstream_item_ids)?;
            }
        }
    }
    if let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) {
        for item in items {
            let Some(item) = item.as_object_mut() else {
                continue;
            };
            translate_item(editor, item, generated_upstream_item_ids)?;
        }
    }
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
            *value = editor.wire_from_downstream(WireIdDomain::Conversation, value)?;
        }
        Value::Object(conversation) => {
            translate_field(editor, conversation, "id", WireIdDomain::Conversation)?;
        }
        _ => {}
    }
    Ok(())
}

fn translate_item(
    editor: &mut RequestStateEditor<'_>,
    item: &mut Map<String, Value>,
    generated_upstream_item_ids: &BTreeSet<String>,
) -> Result<()> {
    if let Some(id) = item.get("id").and_then(Value::as_str).map(str::to_string)
        && !id.is_empty()
        && !generated_upstream_item_ids.contains(&id)
    {
        item.insert(
            "id".to_string(),
            Value::String(editor.wire_from_downstream(WireIdDomain::Item, &id)?),
        );
    }
    for name in ["item_id", "output_item_id"] {
        translate_field(editor, item, name, WireIdDomain::Item)?;
    }
    translate_field(editor, item, "call_id", WireIdDomain::Call)?;
    translate_field(editor, item, "approval_request_id", WireIdDomain::Approval)?;
    translate_field(editor, item, "response_id", WireIdDomain::Response)?;
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
        Value::String(editor.wire_from_downstream(domain, &raw)?),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_state_editor::RequestStateEditor;
    use crate::request_state_lookup::LookupKeyFactory;
    use crate::request_state_types::PersistedRequestState;
    use std::collections::BTreeSet;

    #[test]
    fn translates_only_schema_owned_request_ids() {
        let mut owners = BTreeSet::new();
        owners.insert("acct_wire_test".to_string());
        let mut state = PersistedRequestState::new(owners);
        let keys = LookupKeyFactory::new("account", "scope");
        let mut editor = RequestStateEditor::new(&mut state, keys, "acct_wire_test", 1, 86_400_000)
            .expect("editor");
        let mut request = serde_json::json!({
            "previous_response_id":"resp_real",
            "conversation":{"id":"conv_real"},
            "item_id":"item_top_real",
            "input":[{
                "type":"function_call_output",
                "id":"fco_real",
                "call_id":"call_real",
                "output":{"file_id":"file_must_remain","opaque_id":"opaque_must_remain"}
            }]
        })
        .as_object()
        .expect("request")
        .clone();
        translate_request_ids(&mut editor, &mut request, &BTreeSet::new()).expect("translate");
        assert_ne!(request["previous_response_id"], "resp_real");
        assert_ne!(request["conversation"]["id"], "conv_real");
        assert_ne!(request["item_id"], "item_top_real");
        assert_ne!(request["input"][0]["id"], "fco_real");
        assert_ne!(request["input"][0]["call_id"], "call_real");
        assert_eq!(request["input"][0]["output"]["file_id"], "file_must_remain");
        assert_eq!(
            request["input"][0]["output"]["opaque_id"],
            "opaque_must_remain"
        );
    }
}
