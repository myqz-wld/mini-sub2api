use anyhow::Result;
use serde_json::Map;
use serde_json::Value;
use std::collections::BTreeSet;

use crate::lifecycle_carriers::CarrierContainer;
use crate::lifecycle_carriers::CarrierDirection;
use crate::lifecycle_carriers::CarrierRule;
use crate::lifecycle_carriers::CarrierShape;
use crate::lifecycle_carriers::RequestWireMapping;
use crate::lifecycle_carriers::request_wire_mapping;
use crate::lifecycle_carriers::wire_rules;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;

pub(crate) fn strip_inline_history_item_ids(object: &mut Map<String, Value>) {
    if object.get("store").and_then(Value::as_bool) != Some(false) {
        return;
    }
    let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in items.iter_mut().filter_map(Value::as_object_mut) {
        if item.get("type").and_then(Value::as_str) != Some("item_reference") {
            item.remove("id");
        }
    }
}

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
    let item_type = (container == CarrierContainer::Item)
        .then(|| {
            object
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .flatten();
    for rule in wire_rules(CarrierDirection::Request, container) {
        match rule.shape {
            CarrierShape::Scalar => {
                translate_field(editor, object, rule, item_type.as_deref())?;
            }
            CarrierShape::Conversation => {
                translate_conversation(editor, object, rule, item_type.as_deref())?;
            }
            CarrierShape::TypedItemId => {
                translate_typed_item_id(
                    editor,
                    object,
                    rule,
                    item_type.as_deref(),
                    generated_upstream_item_ids,
                )?;
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
    item_type: Option<&str>,
) -> Result<()> {
    let domain = required_domain(rule)?;
    let mapping = request_wire_mapping(rule, item_type);
    let Some(conversation) = object.get_mut(rule.name) else {
        return Ok(());
    };
    match conversation {
        Value::String(value) if !value.is_empty() => {
            *value = translate_id(editor, domain, value, mapping)?;
        }
        Value::Object(conversation) => {
            translate_named_field(editor, conversation, "id", domain, mapping)?;
        }
        _ => {}
    }
    Ok(())
}

fn translate_typed_item_id(
    editor: &mut RequestStateEditor<'_>,
    item: &mut Map<String, Value>,
    rule: &CarrierRule,
    item_type: Option<&str>,
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
        Value::String(translate_id(
            editor,
            required_domain(rule)?,
            &id,
            request_wire_mapping(rule, item_type),
        )?),
    );
    Ok(())
}

fn translate_field(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    rule: &CarrierRule,
    item_type: Option<&str>,
) -> Result<()> {
    translate_named_field(
        editor,
        object,
        rule.name,
        required_domain(rule)?,
        request_wire_mapping(rule, item_type),
    )
}

fn translate_named_field(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    name: &str,
    domain: WireIdDomain,
    mapping: RequestWireMapping,
) -> Result<()> {
    let Some(raw) = object.get(name).and_then(Value::as_str).map(str::to_string) else {
        return Ok(());
    };
    if raw.is_empty() {
        return Ok(());
    }
    object.insert(
        name.to_string(),
        Value::String(translate_id(editor, domain, &raw, mapping)?),
    );
    Ok(())
}

fn translate_id(
    editor: &mut RequestStateEditor<'_>,
    domain: WireIdDomain,
    raw: &str,
    mapping: RequestWireMapping,
) -> Result<String> {
    match mapping {
        RequestWireMapping::Allocate => editor.wire_from_downstream(domain, raw),
        RequestWireMapping::RequireExisting => {
            editor.required_wire_from_downstream(domain, raw, false)
        }
        RequestWireMapping::RequireProviderExisting => {
            editor.required_wire_from_downstream(domain, raw, true)
        }
        RequestWireMapping::Contextual => {
            anyhow::bail!("contextual wire mapping was not resolved for {domain:?}")
        }
    }
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
        let previous_response = editor
            .wire_from_upstream(WireIdDomain::Response, "resp_provider")
            .expect("response mapping");
        let conversation = editor
            .wire_from_upstream(WireIdDomain::Conversation, "conv_provider")
            .expect("conversation mapping");
        let item_upstream = editor
            .wire_from_downstream(WireIdDomain::Item, "item_top_real")
            .expect("item mapping");
        let call_upstream = editor
            .wire_from_downstream(WireIdDomain::Call, "call_real")
            .expect("call mapping");
        let mut request = serde_json::json!({
            "previous_response_id":previous_response,
            "conversation":{"id":conversation},
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
        assert_eq!(request["previous_response_id"], "resp_provider");
        assert_eq!(request["conversation"]["id"], "conv_provider");
        assert_eq!(request["item_id"], item_upstream);
        assert_ne!(request["input"][0]["id"], "fco_real");
        assert_eq!(request["input"][0]["call_id"], call_upstream);
        assert_eq!(request["input"][0]["output"]["file_id"], "file_must_remain");
        assert_eq!(
            request["input"][0]["output"]["opaque_id"],
            "opaque_must_remain"
        );
    }

    #[test]
    fn missing_required_reference_does_not_allocate_a_wire_mapping() {
        let mut state = PersistedRequestState::new(BTreeSet::from(["acct_missing".to_string()]));
        let keys = LookupKeyFactory::new("account", "scope");
        let mut editor = RequestStateEditor::new(&mut state, keys, "acct_missing", 1, 86_400_000)
            .expect("editor");
        let mut request = serde_json::json!({
            "previous_response_id":"resp_missing",
            "input":[]
        })
        .as_object()
        .expect("request")
        .clone();

        let error = translate_request_ids(&mut editor, &mut request, &BTreeSet::new())
            .expect_err("missing response mapping must fail closed");
        assert!(
            error
                .downcast_ref::<crate::request_state_editor::RequiredWireReferenceUnavailable>()
                .is_some()
        );
        assert!(state.scopes.values().all(|scope| scope.wire_ids.is_empty()));
    }

    #[test]
    fn provider_reference_rejects_a_downstream_allocated_mapping() {
        let mut state = PersistedRequestState::new(BTreeSet::from(["acct_origin".to_string()]));
        let keys = LookupKeyFactory::new("account", "scope");
        let mut editor = RequestStateEditor::new(&mut state, keys, "acct_origin", 1, 86_400_000)
            .expect("editor");
        editor
            .wire_from_downstream(WireIdDomain::Response, "resp_downstream")
            .expect("legacy downstream allocation");
        let mut request = serde_json::json!({
            "previous_response_id":"resp_downstream",
            "input":[]
        })
        .as_object()
        .expect("request")
        .clone();

        let error = translate_request_ids(&mut editor, &mut request, &BTreeSet::new()).expect_err(
            "downstream-created response mapping must not satisfy a provider reference",
        );
        assert!(
            error
                .downcast_ref::<crate::request_state_editor::RequiredWireReferenceUnavailable>()
                .is_some()
        );
    }

    #[test]
    fn output_call_reference_requires_an_existing_definition() {
        let mut state = PersistedRequestState::new(BTreeSet::from(["acct_call".to_string()]));
        let keys = LookupKeyFactory::new("account", "scope");
        let mut editor =
            RequestStateEditor::new(&mut state, keys, "acct_call", 1, 86_400_000).expect("editor");
        let mut request = serde_json::json!({
            "input":[{"type":"function_call_output","call_id":"call_missing","output":"ok"}]
        })
        .as_object()
        .expect("request")
        .clone();

        let error = translate_request_ids(&mut editor, &mut request, &BTreeSet::new())
            .expect_err("missing call definition must fail closed");
        assert!(
            error
                .downcast_ref::<crate::request_state_editor::RequiredWireReferenceUnavailable>()
                .is_some()
        );
    }

    #[test]
    fn request_local_call_definition_can_satisfy_a_later_output_reference() {
        let mut state = PersistedRequestState::new(BTreeSet::from(["acct_local".to_string()]));
        let keys = LookupKeyFactory::new("account", "scope");
        let mut editor =
            RequestStateEditor::new(&mut state, keys, "acct_local", 1, 86_400_000).expect("editor");
        let mut request = serde_json::json!({
            "input":[
                {"type":"function_call","id":"fc_down","call_id":"call_down","name":"f","arguments":"{}"},
                {"type":"function_call_output","id":"fco_down","call_id":"call_down","output":"ok"}
            ]
        })
        .as_object()
        .expect("request")
        .clone();

        translate_request_ids(&mut editor, &mut request, &BTreeSet::new())
            .expect("same-request definition should satisfy reference");
        assert_ne!(request["input"][0]["call_id"], "call_down");
        assert_eq!(
            request["input"][0]["call_id"],
            request["input"][1]["call_id"]
        );
    }
}
