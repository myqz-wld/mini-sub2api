//! Stable native Lite prefix identity, generated after the scoped thread has been resolved.
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use uuid::Uuid;

pub(crate) fn apply(
    object: &mut Map<String, Value>,
    thread: &str,
    prefixes: &[usize],
) -> anyhow::Result<BTreeSet<String>> {
    let namespace = Uuid::new_v5(&Uuid::NAMESPACE_OID, thread.as_bytes());
    let mut ids = BTreeSet::new();
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(ids);
    };
    for index in prefixes {
        let item = input
            .get_mut(*index)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow::anyhow!("Lite prefix is missing"))?;
        let (prefix, payload) =
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                (
                    "at",
                    serde_json::to_vec(
                        item.get("tools")
                            .ok_or_else(|| anyhow::anyhow!("Lite tools missing"))?,
                    )?,
                )
            } else {
                let content = item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow::anyhow!("Lite base missing"))?;
                anyhow::ensure!(content.len() == 1, "Lite base shape changed");
                let text = content[0]
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("Lite base text missing"))?;
                ("msg", text.as_bytes().to_vec())
            };
        let id = format!("{prefix}_{}", Uuid::new_v5(&namespace, &payload));
        item.insert("id".into(), Value::String(id.clone()));
        ids.insert(id);
    }
    Ok(ids)
}
