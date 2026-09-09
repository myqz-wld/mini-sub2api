use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct CompletionFingerprint {
    digest: [u8; 32],
    id: Option<[u8; 32]>,
    message: bool,
}

impl CompletionFingerprint {
    pub(crate) fn new(item: &Value) -> Self {
        Self {
            digest: Sha256::digest(crate::subscription_index::completion_key(item)).into(),
            id: item
                .get("id")
                .map(|id| Sha256::digest(crate::subscription_index::canonical(id)).into()),
            message: item.get("type").and_then(Value::as_str) == Some("message"),
        }
    }

    fn compatible(&self, final_item: &Value) -> bool {
        let final_item = Self::new(final_item);
        self.digest == final_item.digest
            && ((self.message && self.id.is_none()) || self.id == final_item.id)
    }
}

pub(crate) fn validate_terminal(
    response: &Value,
    observed: impl Iterator<Item = (usize, CompletionFingerprint)>,
    all_observed: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(response.is_object(), "terminal response is not an object");
    if let Some(output) = response.get("output") {
        anyhow::ensure!(
            output
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_object)),
            "terminal output is not an array of items"
        );
    }
    if let Some(output) = populated(response.get("output")) {
        for (index, item) in observed {
            anyhow::ensure!(
                output
                    .get(index)
                    .is_some_and(|final_item| item.compatible(final_item)),
                "terminal output conflicts with a completed item"
            );
        }
    } else if all_observed {
        anyhow::ensure!(
            observed
                .map(|(index, _)| index)
                .enumerate()
                .all(|(expected, index)| expected == index),
            "terminal output is missing completed items"
        );
    }
    Ok(())
}

// Codex can finish a stream with an absent or empty output footer after publishing item-done
// events. Those events remain the output. A populated footer is a second complete representation,
// not an additional suffix; malformed non-array footers are not classified as metadata-only.
pub(crate) fn metadata_only(output: Option<&Value>) -> bool {
    output.is_none() || matches!(output, Some(Value::Array(items)) if items.is_empty())
}

pub(crate) fn populated(output: Option<&Value>) -> Option<&Vec<Value>> {
    output
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
}
