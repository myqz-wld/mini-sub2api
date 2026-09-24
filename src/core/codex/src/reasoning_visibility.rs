//! Subscription upstream state and caller-visible optional output are separate decisions.
use crate::request_normalizer::StatefulPrepareError;
use serde_json::{Map, Value};

pub(crate) const ENCRYPTED_REASONING: &str = "reasoning.encrypted_content";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ReasoningVisibility {
    #[default]
    Include,
    Hide,
}

impl ReasoningVisibility {
    pub(crate) fn read(object: &Map<String, Value>) -> Result<Self, StatefulPrepareError> {
        match object.get("include") {
            None => Ok(Self::Include),
            Some(Value::Null) => Ok(Self::Hide),
            Some(Value::Array(values)) if values.iter().all(Value::is_string) => Ok(
                if values
                    .iter()
                    .any(|v| v.as_str() == Some(ENCRYPTED_REASONING))
                {
                    Self::Include
                } else {
                    Self::Hide
                },
            ),
            _ => Err(StatefulPrepareError::InvalidRequest),
        }
    }

    pub(crate) fn filter_response(self, value: &mut Value) {
        if self == Self::Include {
            return;
        }
        // These are protocol containers, never arbitrary nested tool data or user text.
        remove_ciphertext(value);
        if let Some(output) = value.get_mut("output").and_then(Value::as_array_mut) {
            for item in output {
                remove_ciphertext(item);
            }
        }
        if let Some(item) = value.get_mut("item") {
            remove_ciphertext(item);
        }
        if let Some(response) = value.get_mut("response") {
            self.filter_response(response);
        }
    }
}

pub(crate) fn ciphertext(item: &Value) -> Option<&Value> {
    (item.get("type").and_then(Value::as_str) == Some("reasoning"))
        .then(|| item.get("encrypted_content"))
        .flatten()
}

pub(crate) fn remove_ciphertext(item: &mut Value) {
    if item.get("type").and_then(Value::as_str) == Some("reasoning")
        && let Some(item) = item.as_object_mut()
    {
        item.remove("encrypted_content");
    }
}

pub(crate) fn restore_hidden(caller: &mut Value, saved: &Value) {
    if ciphertext(caller).is_none_or(Value::is_null)
        && let Some(ciphertext) = ciphertext(saved)
        && let Some(caller) = caller.as_object_mut()
    {
        caller.insert("encrypted_content".into(), ciphertext.clone());
    }
}
