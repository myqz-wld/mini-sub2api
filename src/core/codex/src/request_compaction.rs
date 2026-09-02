use serde_json::Map;
use serde_json::Value;
use uuid::Uuid;

use crate::request_identity_evidence::RequestIdentityEvidence;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingCompaction {
    pub(crate) marker_key: String,
    pub(crate) thread_id: String,
    pub(crate) target_window: u64,
}

impl PendingCompaction {
    pub(crate) const fn committed_base(&self) -> u64 {
        self.target_window.saturating_sub(1)
    }
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
