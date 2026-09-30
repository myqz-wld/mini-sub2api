//! Preserve bounded native provenance without trusting client-supplied source shapes.
use serde_json::{Value, json};

pub(super) fn normalize(call: &mut Value) {
    let Some(value) = call.get("tool_result_sources") else {
        return;
    };
    match bounded(value) {
        Some(sources) => call["tool_result_sources"] = sources,
        None => {
            call.as_object_mut()
                .unwrap()
                .shift_remove("tool_result_sources");
        }
    }
}

fn bounded(value: &Value) -> Option<Value> {
    let failed = || Some(json!([{"type":"parse_failed","id":""}]));
    let Some(sources) = value.as_array() else {
        return failed();
    };
    if sources.iter().any(|source| {
        source.get("type").and_then(Value::as_str).is_none()
            || source.get("id").and_then(Value::as_str).is_none()
    }) {
        return failed();
    }
    let mut unique = Vec::new();
    for source in sources {
        let kind = source["type"].as_str().unwrap();
        let id = source["id"].as_str().unwrap();
        let pair = (kind, id);
        if unique.contains(&pair) {
            continue;
        }
        if unique.len() == 32 || kind.len() > 128 || id.len() > 128 {
            return None;
        }
        unique.push(pair);
    }
    Some(Value::Array(
        unique
            .into_iter()
            .map(|(kind, id)| json!({"type":kind,"id":id}))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_limits_use_unique_pairs_and_utf8_bytes() {
        let source = |id: String| json!({"type":"file","id":id});
        let values: Vec<_> = (0..32).map(|i| source(format!("source-{i}"))).collect();
        assert_eq!(bounded(&json!(values)), Some(json!(values)));
        let mut too_many = values.clone();
        too_many.push(source("source-33".into()));
        assert!(bounded(&json!(too_many)).is_none());
        assert_eq!(
            bounded(&json!(vec![values[0].clone(); 40])),
            Some(json!([values[0]]))
        );
        for boundary in [
            "x".repeat(128),
            format!("{}aa", "界".repeat(42)),
            "🦀".repeat(32),
        ] {
            for field in ["id", "type"] {
                let mut valid = source("valid".into());
                valid[field] = boundary.clone().into();
                assert_eq!(
                    bounded(&json!([valid.clone()])),
                    Some(json!([valid.clone()]))
                );
                valid[field] = format!("{boundary}x").into();
                assert!(bounded(&json!([valid])).is_none());
            }
        }
        assert_eq!(bounded(&json!([])), Some(json!([])));
    }

    #[test]
    fn malformed_sources_have_a_bounded_parse_marker() {
        let marker = json!([{"type":"parse_failed","id":""}]);
        for invalid in [
            json!(null),
            json!({}),
            json!("invalid"),
            json!([{}]),
            json!([{"type":"url","url":"https://example.test"}]),
            json!([{"type":"file","id":1}]),
            json!([{"type":false,"id":"valid"}]),
            json!([null]),
        ] {
            assert_eq!(bounded(&invalid), Some(marker.clone()));
        }
        assert_eq!(bounded(&marker), Some(marker));
        assert_eq!(
            bounded(&json!([{"type":"custom-valid-kind","id":"valid","extra":"x".repeat(9000)}])),
            Some(json!([{"type":"custom-valid-kind","id":"valid"}]))
        );
    }
}
