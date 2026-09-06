use serde_json::Map;
use serde_json::Value;
use uuid::Uuid;

use crate::request_identity_evidence::RequestIdentityEvidence;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingCompaction {
    pub(crate) marker_key: String,
    pub(crate) thread_id: String,
    pub(crate) target_window: u64,
    pub(crate) requires_compaction_item: bool,
}

impl PendingCompaction {
    pub(crate) const fn committed_base(&self) -> u64 {
        self.target_window.saturating_sub(1)
    }

    pub(crate) fn accepts_items(&self, output: &[Value]) -> bool {
        if !self.requires_compaction_item {
            // Native local Responses compaction builds an assistant summary, including an empty
            // fallback. Its completion contract is distinct from remote compaction V2.
            return true;
        }
        let mut compacted = output
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("compaction"));
        compacted.next().is_some_and(|item| {
            item.get("encrypted_content")
                .and_then(Value::as_str)
                .is_some()
        }) && compacted.next().is_none()
    }

    pub(crate) fn accepts_response(
        &self,
        response: &Value,
        observed: Option<&std::collections::BTreeMap<usize, Value>>,
    ) -> bool {
        if let Some(output) = response.get("output") {
            return output
                .as_array()
                .is_some_and(|items| self.accepts_items(items));
        }
        let items: Vec<_> = observed
            .into_iter()
            .flat_map(|output| output.values().cloned())
            .collect();
        self.accepts_items(&items)
    }
}

pub(crate) fn requires_compaction_item(object: &Map<String, Value>) -> bool {
    let turn = object
        .get("client_metadata")
        .and_then(|metadata| metadata.get("x-codex-turn-metadata"))
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let local = turn
        .as_ref()
        .and_then(|turn| turn.get("compaction"))
        .and_then(|compaction| compaction.get("implementation"))
        .and_then(Value::as_str)
        == Some("responses");
    !local
        || object
            .get("input")
            .and_then(Value::as_array)
            .is_some_and(|input| {
                input.iter().any(|item| {
                    item.get("type").and_then(Value::as_str) == Some("compaction_trigger")
                })
            })
}

pub(crate) fn operation_anchor(
    object: &Map<String, Value>,
    evidence: &RequestIdentityEvidence,
    has_explicit_turn: bool,
    turn_key: &str,
) -> Vec<u8> {
    if has_explicit_turn {
        return turn_key.as_bytes().to_vec();
    }
    if let Some(item_id) = evidence
        .items
        .iter()
        .rev()
        .find_map(|item| item.id.as_deref())
    {
        return item_id.as_bytes().to_vec();
    }
    if let Some(previous) = object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return previous.as_bytes().to_vec();
    }
    Uuid::now_v7().as_bytes().to_vec()
}
