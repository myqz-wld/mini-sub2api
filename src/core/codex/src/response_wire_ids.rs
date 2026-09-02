use anyhow::Result;
use serde_json::Map;
use serde_json::Value;

use crate::lifecycle_carriers::CarrierContainer;
use crate::lifecycle_carriers::CarrierDirection;
use crate::lifecycle_carriers::CarrierRule;
use crate::lifecycle_carriers::CarrierShape;
use crate::lifecycle_carriers::wire_rules;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;
use crate::request_state_types::WireIdOwner;

pub(crate) fn translate_response_ids(
    editor: &mut RequestStateEditor<'_>,
    value: &mut Value,
    owner: Option<&WireIdOwner>,
) -> Result<()> {
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    let event_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    let is_terminal_response = event_type.is_none()
        && (object.get("object").and_then(Value::as_str) == Some("response")
            || object.contains_key("output")
            || object.contains_key("usage"));
    translate_response_container(
        editor,
        object,
        CarrierContainer::TopLevel,
        owner,
        is_terminal_response,
    )?;
    Ok(())
}

fn translate_response_container(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    container: CarrierContainer,
    owner: Option<&WireIdOwner>,
    terminal_response: bool,
) -> Result<()> {
    for rule in wire_rules(CarrierDirection::Response, container) {
        match rule.shape {
            CarrierShape::Scalar | CarrierShape::TypedItemId => {
                translate_field(editor, object, rule)?;
            }
            CarrierShape::OwnedResponseId => {
                translate_response_field(editor, object, rule.name, owner)?;
            }
            CarrierShape::TerminalResponseId if terminal_response => {
                translate_response_field(editor, object, rule.name, owner)?;
            }
            CarrierShape::TerminalResponseId => {}
            CarrierShape::Conversation => translate_conversation(editor, object, rule)?,
            CarrierShape::ResponseObject => {
                if let Some(response) = object.get_mut(rule.name).and_then(Value::as_object_mut) {
                    translate_response_container(
                        editor,
                        response,
                        CarrierContainer::ResponseObject,
                        owner,
                        false,
                    )?;
                }
            }
            CarrierShape::ItemObject => {
                if let Some(item) = object.get_mut(rule.name).and_then(Value::as_object_mut) {
                    translate_response_container(
                        editor,
                        item,
                        CarrierContainer::Item,
                        owner,
                        false,
                    )?;
                }
            }
            CarrierShape::ItemArray => {
                if let Some(items) = object.get_mut(rule.name).and_then(Value::as_array_mut) {
                    for item in items.iter_mut().filter_map(Value::as_object_mut) {
                        translate_response_container(
                            editor,
                            item,
                            CarrierContainer::Item,
                            owner,
                            false,
                        )?;
                    }
                }
            }
            CarrierShape::IdentityMetadataObject => {
                if let Some(metadata) = object.get_mut(rule.name).and_then(Value::as_object_mut) {
                    translate_identity_metadata(editor, metadata)?;
                }
            }
            CarrierShape::CallerObject => {
                if let Some(caller) = object.get_mut(rule.name).and_then(Value::as_object_mut) {
                    translate_response_container(
                        editor,
                        caller,
                        CarrierContainer::Caller,
                        owner,
                        false,
                    )?;
                }
            }
            CarrierShape::SafetyCheckArray => {
                if let Some(checks) = object.get_mut(rule.name).and_then(Value::as_array_mut) {
                    for check in checks.iter_mut().filter_map(Value::as_object_mut) {
                        translate_response_container(
                            editor,
                            check,
                            CarrierContainer::SafetyCheck,
                            owner,
                            false,
                        )?;
                    }
                }
            }
            unsupported => anyhow::bail!(
                "unsupported response carrier shape {unsupported:?} for {}",
                rule.name
            ),
        }
    }
    Ok(())
}

fn translate_response_field(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    name: &str,
    owner: Option<&WireIdOwner>,
) -> Result<()> {
    let Some(raw) = object.get(name).and_then(Value::as_str).map(str::to_string) else {
        return Ok(());
    };
    if raw.is_empty() {
        return Ok(());
    }
    object.insert(
        name.to_string(),
        Value::String(editor.wire_from_upstream_response(&raw, owner)?),
    );
    Ok(())
}

fn translate_identity_metadata(
    editor: &mut RequestStateEditor<'_>,
    metadata: &mut Map<String, Value>,
) -> Result<()> {
    for rule in wire_rules(
        CarrierDirection::Response,
        CarrierContainer::IdentityMetadata,
    ) {
        match rule.shape {
            CarrierShape::Scalar => translate_field(editor, metadata, rule)?,
            CarrierShape::Window => translate_window(editor, metadata, rule)?,
            CarrierShape::SerializedTurnMetadata => {
                translate_serialized_turn_metadata(editor, metadata, rule)?;
            }
            unsupported => anyhow::bail!(
                "unsupported identity metadata shape {unsupported:?} for {}",
                rule.name
            ),
        }
    }
    Ok(())
}

fn translate_window(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    rule: &CarrierRule,
) -> Result<()> {
    let Some(raw) = object
        .get(rule.name)
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
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
        rule.name.to_string(),
        Value::String(format!("{thread}:{suffix}")),
    );
    Ok(())
}

fn translate_serialized_turn_metadata(
    editor: &mut RequestStateEditor<'_>,
    metadata: &mut Map<String, Value>,
    rule: &CarrierRule,
) -> Result<()> {
    let Some(raw) = metadata
        .get(rule.name)
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(());
    };
    let mut nested = serde_json::from_str::<Value>(&raw)?;
    let nested = nested
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("turn metadata is not an object"))?;
    translate_identity_metadata(editor, nested)?;
    metadata.insert(
        rule.name.to_string(),
        Value::String(serde_json::to_string(nested)?),
    );
    Ok(())
}

fn translate_conversation(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    rule: &CarrierRule,
) -> Result<()> {
    let domain = required_domain(rule)?;
    let Some(conversation) = object.get_mut(rule.name) else {
        return Ok(());
    };
    match conversation {
        Value::String(value) if !value.is_empty() => {
            *value = editor.wire_from_upstream(domain, value)?;
        }
        Value::Object(conversation) => {
            translate_named_field(editor, conversation, "id", domain)?;
        }
        _ => {}
    }
    Ok(())
}

fn translate_field(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    rule: &CarrierRule,
) -> Result<()> {
    translate_named_field(editor, object, rule.name, required_domain(rule)?)
}

fn translate_named_field(
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

fn required_domain(rule: &CarrierRule) -> Result<WireIdDomain> {
    rule.domain
        .ok_or_else(|| anyhow::anyhow!("carrier {} has no wire domain", rule.name))
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
        translate_response_ids(&mut editor, &mut event, None).expect("translate response");
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
