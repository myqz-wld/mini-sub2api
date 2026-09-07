use serde_json::Map;
use serde_json::Value;
use sha2::{Digest, Sha256};
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

    pub(crate) fn accepts_response(
        &self,
        response: &Value,
        observed: Option<&CompactionOutput>,
    ) -> bool {
        if !self.requires_compaction_item {
            // Native local summaries have a distinct completion contract, including empty output.
            return true;
        }
        let Some(observed) = observed.filter(|output| output.count == 1) else {
            return false;
        };
        let Some(fingerprint) = observed.fingerprint else {
            return false;
        };
        if crate::response_output::metadata_only(response.get("output")) {
            return true;
        }
        let output = response.get("output").expect("nonempty footer is present");
        let Some(items) = output.as_array() else {
            return false;
        };
        let mut compacted = items.iter().filter(|item| is_compaction(item));
        compacted.next().and_then(compaction_fingerprint) == Some(fingerprint)
            && compacted.next().is_none()
    }
}

/// Constant-size proof of actual item-done events, independent of retained output/body budgets.
#[derive(Default)]
pub(crate) struct CompactionOutput {
    count: usize,
    fingerprint: Option<[u8; 32]>,
}

impl CompactionOutput {
    pub(crate) fn matches_single(&self, item: &Value) -> bool {
        self.count == 1
            && self.fingerprint.is_some()
            && self.fingerprint == compaction_fingerprint(item)
    }

    pub(crate) fn observe(&mut self, item: &Value) {
        if is_compaction(item) {
            self.count = self.count.saturating_add(1);
            if self.count == 1 {
                self.fingerprint = compaction_fingerprint(item);
            }
        }
    }
}

fn is_compaction(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("compaction")
}

fn compaction_fingerprint(item: &Value) -> Option<[u8; 32]> {
    item.get("encrypted_content")
        .and_then(Value::as_str)
        .map(|payload| Sha256::digest(payload.as_bytes()).into())
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

#[cfg(test)]
mod footer_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_v2_footer_requires_one_actual_valid_done_event() {
        let pending = PendingCompaction {
            marker_key: "marker".into(),
            thread_id: "thread".into(),
            target_window: 1,
            requires_compaction_item: true,
        };
        let footer = json!({"output":[]});
        let item = json!({"type":"compaction","encrypted_content":"opaque-test-payload"});
        let mut proof = CompactionOutput::default();
        assert!(!pending.accepts_response(&footer, None));
        assert!(!pending.accepts_response(&footer, Some(&proof)));
        proof.observe(&item);
        assert!(pending.accepts_response(&footer, Some(&proof)));
        assert!(!pending.accepts_response(&json!({"output":null}), Some(&proof)));
        assert!(!pending.accepts_response(
            &json!({"output":[{"type":"compaction","encrypted_content":"different"}]}),
            Some(&proof)
        ));
        proof.observe(&item);
        assert!(!pending.accepts_response(&footer, Some(&proof)));
    }
}
