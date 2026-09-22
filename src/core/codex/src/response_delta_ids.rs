//! Cache only flat delta shapes; all other carrier shapes keep the complete translator.
use crate::lifecycle_carriers::{CarrierContainer, CarrierDirection, CarrierShape, wire_rules};
use crate::request_state_types::{WireIdDomain, validate_wire_id};
use serde_json::Value;

pub(crate) struct DeltaId {
    pub field: &'static str,
    pub domain: WireIdDomain,
    pub upstream: String,
}

pub(crate) struct DeltaIds(pub Vec<DeltaId>);

impl DeltaIds {
    pub fn eligible(value: &Value) -> bool {
        matches!(
            value.get("type").and_then(Value::as_str),
            Some(
                "response.output_text.delta"
                    | "response.refusal.delta"
                    | "response.reasoning_text.delta"
                    | "response.reasoning_summary_text.delta"
                    | "response.reasoning.delta"
            )
        )
    }

    pub fn inspect(value: &Value) -> Option<Self> {
        if !Self::eligible(value) {
            return None;
        }
        let object = value.as_object()?;
        let mut fields = Vec::new();
        for rule in wire_rules(CarrierDirection::Response, CarrierContainer::TopLevel) {
            let Some(value) = object.get(rule.name) else {
                continue;
            };
            match rule.shape {
                CarrierShape::Scalar
                | CarrierShape::TypedItemId
                | CarrierShape::OwnedResponseId => {
                    // The ordinary translator also leaves non-string/empty scalar carriers alone.
                    let Some(raw) = value.as_str().filter(|raw| !raw.is_empty()) else {
                        continue;
                    };
                    validate_wire_id(raw).ok()?;
                    if fields.len() == 16 {
                        return None;
                    }
                    fields.push(DeltaId {
                        field: rule.name,
                        domain: rule.domain?,
                        upstream: raw.into(),
                    });
                }
                // A typed delta is not an untyped terminal response; its root id is opaque.
                CarrierShape::TerminalResponseId => {}
                _ => return None,
            }
        }
        Some(Self(fields))
    }
}
