use anyhow::Result;
use serde_json::Map;
use serde_json::Value;
use std::collections::BTreeSet;

use crate::lifecycle_carriers::CarrierContainer;
use crate::lifecycle_carriers::CarrierDirection;
use crate::lifecycle_carriers::CarrierRule;
use crate::lifecycle_carriers::CarrierShape;
use crate::lifecycle_carriers::wire_rules;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;

pub(crate) fn translate_request_ids(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    generated_upstream_item_ids: &BTreeSet<String>,
) -> Result<()> {
    translate_request_object(
        editor,
        object,
        CarrierContainer::TopLevel,
        generated_upstream_item_ids,
    )?;
    Ok(())
}

fn translate_request_object(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    container: CarrierContainer,
    generated_upstream_item_ids: &BTreeSet<String>,
) -> Result<()> {
    for rule in wire_rules(CarrierDirection::Request, container) {
        match rule.shape {
            CarrierShape::Scalar => translate_field(editor, object, rule)?,
            CarrierShape::Conversation => translate_conversation(editor, object, rule)?,
            CarrierShape::TypedItemId => {
                translate_typed_item_id(editor, object, rule, generated_upstream_item_ids)?;
            }
            CarrierShape::ItemObject => {
                if let Some(item) = object.get_mut(rule.name).and_then(Value::as_object_mut) {
                    translate_request_object(
                        editor,
                        item,
                        CarrierContainer::Item,
                        generated_upstream_item_ids,
                    )?;
                }
            }
            CarrierShape::ItemArray => {
                if let Some(items) = object.get_mut(rule.name).and_then(Value::as_array_mut) {
                    for item in items.iter_mut().filter_map(Value::as_object_mut) {
                        translate_request_object(
                            editor,
                            item,
                            CarrierContainer::Item,
                            generated_upstream_item_ids,
                        )?;
                    }
                }
            }
            CarrierShape::CallerObject => {
                if let Some(caller) = object.get_mut(rule.name).and_then(Value::as_object_mut) {
                    translate_request_object(
                        editor,
                        caller,
                        CarrierContainer::Caller,
                        generated_upstream_item_ids,
                    )?;
                }
            }
            CarrierShape::SafetyCheckArray => {
                if let Some(checks) = object.get_mut(rule.name).and_then(Value::as_array_mut) {
                    for check in checks.iter_mut().filter_map(Value::as_object_mut) {
                        translate_request_object(
                            editor,
                            check,
                            CarrierContainer::SafetyCheck,
                            generated_upstream_item_ids,
                        )?;
                    }
                }
            }
            unsupported => anyhow::bail!(
                "unsupported request carrier shape {unsupported:?} for {}",
                rule.name
            ),
        }
    }
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
            *value = editor.wire_from_downstream(domain, value)?;
        }
        Value::Object(conversation) => {
            translate_named_field(editor, conversation, "id", domain)?;
        }
        _ => {}
    }
    Ok(())
}

fn translate_typed_item_id(
    editor: &mut RequestStateEditor<'_>,
    item: &mut Map<String, Value>,
    rule: &CarrierRule,
    generated_upstream_item_ids: &BTreeSet<String>,
) -> Result<()> {
    let Some(id) = item
        .get(rule.name)
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(());
    };
    if id.is_empty() || generated_upstream_item_ids.contains(&id) {
        return Ok(());
    }
    item.insert(
        rule.name.to_string(),
        Value::String(editor.wire_from_downstream(required_domain(rule)?, &id)?),
    );
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
        Value::String(editor.wire_from_downstream(domain, &raw)?),
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
