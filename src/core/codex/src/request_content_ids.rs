//! Session/content-scoped identities for caller declarations, never opaque payload fields.
use anyhow::Result;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use crate::lifecycle_carriers::{
    CarrierContainer, CarrierDirection, CarrierShape, RequestWireMapping, request_wire_mapping,
    wire_rules,
};
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;

#[derive(Clone, Default)]
pub(crate) struct WireBindings {
    references: BTreeMap<(WireIdDomain, String), String>,
    // Keep every occurrence, even when a later declaration reuses its caller ID.
    pub(crate) origins: BTreeMap<(WireIdDomain, String), String>,
}

pub(crate) fn bindings_cost(bindings: &WireBindings) -> usize {
    bindings
        .references
        .iter()
        .chain(bindings.origins.iter())
        .map(|((_, raw), alias)| raw.len() + alias.len() + 128)
        .sum()
}

pub(crate) struct ContentIds<'a> {
    pub(crate) bindings: WireBindings,
    pub(crate) session: &'a str,
    pub(crate) generated: &'a BTreeSet<String>,
}

impl ContentIds<'_> {
    pub(crate) fn rewrite(
        &mut self,
        editor: &mut RequestStateEditor<'_>,
        object: &mut Map<String, Value>,
        container: CarrierContainer,
    ) -> Result<()> {
        if container == CarrierContainer::TopLevel {
            for ((domain, alias), raw) in &self.bindings.origins {
                editor.record_content_origin(*domain, raw, alias)?;
            }
        }
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if container == CarrierContainer::Item {
            self.declarations(editor, object, &kind)?;
        }
        for rule in wire_rules(CarrierDirection::Request, container) {
            match rule.shape {
                CarrierShape::Scalar | CarrierShape::TypedItemId => {
                    if let Some(domain @ (WireIdDomain::Item | WireIdDomain::Call)) = rule.domain
                        && request_wire_mapping(rule, Some(&kind))
                            == RequestWireMapping::RequireExisting
                    {
                        self.reference(object, rule.name, domain);
                    }
                }
                CarrierShape::ItemObject
                | CarrierShape::CallerObject
                | CarrierShape::ItemPassthroughMetadataObject => {
                    let child = match rule.shape {
                        CarrierShape::ItemObject => CarrierContainer::Item,
                        CarrierShape::CallerObject => CarrierContainer::Caller,
                        _ => CarrierContainer::ItemPassthroughMetadata,
                    };
                    if let Some(value) = object.get_mut(rule.name).and_then(Value::as_object_mut) {
                        self.rewrite(editor, value, child)?;
                    }
                }
                CarrierShape::ItemArray | CarrierShape::SafetyCheckArray => {
                    let child = if rule.shape == CarrierShape::ItemArray {
                        CarrierContainer::Item
                    } else {
                        CarrierContainer::SafetyCheck
                    };
                    if let Some(values) = object.get_mut(rule.name).and_then(Value::as_array_mut) {
                        for value in values.iter_mut().filter_map(Value::as_object_mut) {
                            self.rewrite(editor, value, child)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn declarations(
        &mut self,
        editor: &mut RequestStateEditor<'_>,
        item: &mut Map<String, Value>,
        kind: &str,
    ) -> Result<()> {
        let call_definition = wire_rules(CarrierDirection::Request, CarrierContainer::Item)
            .find(|rule| rule.name == "call_id")
            .is_some_and(|rule| {
                request_wire_mapping(rule, Some(kind)) == RequestWireMapping::Allocate
            });
        if call_definition && needs_mapping(editor, item, "call_id", WireIdDomain::Call)? {
            // Call identity is defined by the invocation, never by its eventual result.
            let mut content = item.clone();
            content.remove("call_id");
            let fingerprint = fingerprint(content);
            self.declaration(
                editor,
                item,
                "call_id",
                WireIdDomain::Call,
                "call",
                &fingerprint,
            )?;
        } else if !call_definition {
            self.reference(item, "call_id", WireIdDomain::Call);
        }
        if kind != "item_reference"
            && item
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| !self.generated.contains(id))
            && needs_mapping(editor, item, "id", WireIdDomain::Item)?
        {
            // Include the resolved call identity for call/result items so otherwise identical
            // results of different invocations cannot acquire the same item identity.
            let fingerprint = fingerprint(item.clone());
            let prefix = crate::responses_lite::item_id_prefix(kind).unwrap_or("item");
            self.declaration(editor, item, "id", WireIdDomain::Item, prefix, &fingerprint)?;
        }
        Ok(())
    }

    fn declaration(
        &mut self,
        editor: &mut RequestStateEditor<'_>,
        item: &mut Map<String, Value>,
        field: &str,
        domain: WireIdDomain,
        prefix: &str,
        fingerprint: &[u8; 32],
    ) -> Result<()> {
        let Some(raw) = item
            .get(field)
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
        else {
            return Ok(());
        };
        let domain_key = if domain == WireIdDomain::Call {
            b"call".as_slice()
        } else {
            b"item".as_slice()
        };
        let key = editor.derived_lookup(
            "local-wire-content-v1",
            &[
                self.session.as_bytes(),
                domain_key,
                raw.as_bytes(),
                fingerprint,
            ],
        );
        let generated = editor.generated_item(&key, prefix, None, false)?;
        let alias = editor.wire_from_upstream(domain, &generated.id)?;
        editor.record_content_origin(domain, &raw, &alias)?;
        self.bindings
            .references
            .insert((domain, raw.clone()), alias.clone());
        self.bindings.origins.insert((domain, alias.clone()), raw);
        item.insert(field.into(), alias.into());
        Ok(())
    }

    fn reference(&self, object: &mut Map<String, Value>, field: &str, domain: WireIdDomain) {
        if let Some(raw) = object.get(field).and_then(Value::as_str)
            && let Some(alias) = self.bindings.references.get(&(domain, raw.to_string()))
        {
            object.insert(field.into(), alias.clone().into());
        }
    }
}

fn needs_mapping(
    editor: &mut RequestStateEditor<'_>,
    item: &Map<String, Value>,
    field: &str,
    domain: WireIdDomain,
) -> Result<bool> {
    let Some(raw) = item
        .get(field)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Ok(false);
    };
    // Only a ledger-backed public alias has provider provenance. Prefixes are insufficient.
    Ok(!editor.is_public_wire_alias(domain, raw)?)
}

fn fingerprint(mut item: Map<String, Value>) -> [u8; 32] {
    item.remove("id");
    item.remove("internal_chat_message_metadata_passthrough");
    Sha256::digest(crate::subscription_index::candidate_key(&Value::Object(
        item,
    )))
    .into()
}
