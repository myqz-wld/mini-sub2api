use serde_json::Value;

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
