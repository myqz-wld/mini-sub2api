use serde::{Deserialize, Serialize};

const DEFAULTS: &str = include_str!("../../go/limits.json");
pub const LIMITS_ENV: &str = "MINI_SUB2API_LIMITS";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InferenceLimits {
    pub request_bytes: usize,
    pub output_bytes: usize,
    pub output_items: usize,
    pub global_bytes: usize,
    pub key_bytes: usize,
    pub session_bytes: usize,
    pub session_records: usize,
}

impl Default for InferenceLimits {
    fn default() -> Self {
        serde_json::from_str(DEFAULTS).expect("inference defaults")
    }
}

impl InferenceLimits {
    pub fn parse(override_json: &str) -> Result<Self, &'static str> {
        let mut defaults: serde_json::Value = serde_json::from_str(DEFAULTS).expect("defaults");
        if !override_json.is_empty() {
            let overrides: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(override_json).map_err(|_| "invalid inference limits")?;
            defaults
                .as_object_mut()
                .expect("defaults object")
                .extend(overrides);
        }
        let result: Self =
            serde_json::from_value(defaults).map_err(|_| "invalid inference limits")?;
        if [
            result.request_bytes,
            result.output_bytes,
            result.output_items,
            result.global_bytes,
            result.key_bytes,
            result.session_bytes,
            result.session_records,
        ]
        .iter()
        .any(|v| *v == 0 || *v > 1 << 40)
        {
            return Err("inference limits must be between 1 and 2^40");
        }
        if result.session_bytes > result.key_bytes || result.key_bytes > result.global_bytes {
            return Err("inference byte budgets must satisfy session <= key <= global");
        }
        Ok(result)
    }

    pub fn load() -> Result<Self, &'static str> {
        Self::parse(&std::env::var(LIMITS_ENV).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_inference_limits() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../fixtures/limits.json")).unwrap();
        for case in cases {
            let result = InferenceLimits::parse(case["override"].as_str().unwrap());
            assert_eq!(result.is_ok(), case["valid"].as_bool().unwrap());
            if let Ok(value) = result {
                assert_eq!(
                    value.request_bytes as u64,
                    case["requestBytes"].as_u64().unwrap()
                );
            }
        }
    }
}
