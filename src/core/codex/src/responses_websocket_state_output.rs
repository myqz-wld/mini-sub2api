use super::{
    ActiveOperation, OperationPhase, PendingCompaction, ResponsesWebSocketState, ReuseBaseline,
};
use crate::responses_websocket_projection::{
    encoded_len_within, equivalent_items, output_encoded_len, reusable_item,
};
use serde_json::Value;

impl ResponsesWebSocketState {
    pub(super) fn observe_output_item(&mut self, event: &Value) {
        let item = event.as_object().and_then(|object| object.get("item"));
        let max_output_items = self.max_output_items;
        let max_output_bytes = self.max_output_bytes;
        let Some(active) = &mut self.active else {
            return;
        };
        if let Some(item) = item {
            active.compaction_output.observe(item);
        }
        if !active.reusable {
            return;
        }
        let Some(item) = item.filter(|item| reusable_item(item)) else {
            abandon_output(active);
            return;
        };
        let remaining = max_output_bytes.saturating_sub(active.output_bytes);
        let Some(encoded) = encoded_len_within(item, remaining) else {
            abandon_output(active);
            return;
        };
        if active.output.len() >= max_output_items {
            abandon_output(active);
            return;
        }
        active.output.push(item.clone());
        active.output_bytes = active.output_bytes.saturating_add(encoded);
    }

    pub(super) fn complete_active(&mut self, event: &Value) -> Option<PendingCompaction> {
        let mut active = self.active.take()?;
        let response = event
            .as_object()
            .and_then(|object| object.get("response"))
            .and_then(Value::as_object);
        let response_id = response
            .and_then(|response| response.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());

        if let Some(output) = response
            .and_then(|response| response.get("output"))
            .and_then(Value::as_array)
        {
            if !output.iter().all(reusable_item)
                || output.len() > self.max_output_items
                || output_encoded_len(output, self.max_output_bytes).is_none()
            {
                abandon_output(&mut active);
            } else if active.output.is_empty() {
                active.output.clone_from(output);
            } else if !equivalent_items(&active.output, output) {
                abandon_output(&mut active);
            }
        }

        if active.pending_compaction.as_ref().is_some_and(|pending| {
            !pending.accepts_response(
                event.get("response").unwrap_or(event),
                Some(&active.compaction_output),
            )
        }) {
            active.reusable = false;
        }
        self.baseline = match (active.reusable, active.request, response_id) {
            (true, Some(request), Some(response_id)) => Some(ReuseBaseline {
                request,
                response_id: response_id.to_string(),
                output: active.output,
            }),
            _ => None,
        };
        self.set_phase(active.kind, OperationPhase::Completed);
        active.pending_compaction
    }
}

pub(super) fn abandon_output(active: &mut ActiveOperation) {
    active.output.clear();
    active.output_bytes = 0;
    active.reusable = false;
}
