//! Preserve first-assigned input metadata in caller-form, memory-only continuation history.
use super::ContextPlan;
use serde_json::{Map, Value};

const META: &str = "internal_chat_message_metadata_passthrough";

impl ContextPlan {
    pub(crate) fn restore_input_metadata(&mut self) {
        if self.evidence.previous.is_some() {
            return;
        }
        let Some(history) = self
            .baseline
            .as_ref()
            .and_then(|base| base.history.as_ref())
        else {
            return;
        };
        let mut input = self.input().to_vec();
        let mut charge = 0;
        // The selected full-history baseline already passed ordered content/ID eligibility.
        for (caller, saved) in input.iter_mut().zip(history.items()) {
            charge += copy_missing(caller, &saved.value);
        }
        self.remember(input, charge);
    }

    pub(crate) fn capture_input_metadata(&mut self, projected: &Map<String, Value>) {
        let Some(wire) = projected.get("input").and_then(Value::as_array) else {
            return;
        };
        let mut input = self.input().to_vec();
        let mut charge = 0;
        // Rebuilt reference history and generated Lite prefixes precede the current caller input.
        let offset = wire.len().saturating_sub(input.len());
        for (caller, projected) in input.iter_mut().zip(&wire[offset..]) {
            if caller.get("type").and_then(Value::as_str) != Some("additional_tools") {
                charge += copy_missing(caller, projected);
            }
        }
        self.remember(input, charge);
    }

    fn remember(&mut self, input: Vec<Value>, charge: usize) {
        if charge > 0 {
            self.input_metadata_charge = self
                .input_metadata_charge
                .saturating_add(charge.saturating_mul(8));
            self.restored_input = Some(input);
        }
    }
}

fn copy_missing(caller: &mut Value, saved: &Value) -> usize {
    let Some(saved) = saved.get(META).and_then(Value::as_object) else {
        return 0;
    };
    let Some(caller) = caller.as_object_mut() else {
        return 0;
    };
    let mut charge = 0;
    for field in ["create_time", "turn_id"] {
        let valid = |value: &Value| {
            if field == "create_time" {
                value.is_number()
            } else {
                value.as_str().is_some_and(|turn| !turn.is_empty())
            }
        };
        let Some(value) = saved.get(field).filter(|value| valid(value)) else {
            continue;
        };
        let metadata = caller
            .entry(META)
            .or_insert_with(|| Value::Object(Map::new()));
        if !metadata.is_object() {
            *metadata = Value::Object(Map::new());
        }
        let metadata = metadata.as_object_mut().expect("metadata object");
        if !metadata.get(field).is_some_and(valid) {
            // Charge only the small added fields, without cloning/serializing full bodies again.
            charge += crate::json_size::encoded_len(value).expect("JSON metadata")
                + field.len()
                + META.len()
                + 8;
            metadata.insert(field.into(), value.clone());
        }
    }
    charge
}
