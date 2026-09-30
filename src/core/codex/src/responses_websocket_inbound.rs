use super::*;
use crate::response_translation::SseFailureTail;
use serde_json::Value;
use tokio::time::{Duration, Instant};

const FAILURE_TAIL: Duration = Duration::from_secs(1);

struct FailedResponse {
    response_id: Option<String>,
    operation_id: Option<String>,
    generation: u64,
    state: Option<(ResponseStateContext, SseFailureTail)>,
    deadline: Instant,
}

#[derive(Default)]
pub(super) struct Inbound {
    failed: Option<FailedResponse>,
    response_id: Option<String>,
    retired: bool,
}

impl Inbound {
    pub(super) async fn next<S: futures_util::Stream + Unpin>(
        &mut self,
        stream: &mut S,
    ) -> Option<S::Item> {
        if let Some(failed) = &self.failed {
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(failed.deadline) => None,
                message = stream.next() => message,
            }
        } else if self.retired {
            None
        } else {
            stream.next().await
        }
    }

    pub(super) async fn translate(
        &mut self,
        text: String,
        state: Option<&ResponseStateContext>,
        continuation: &Arc<StdMutex<ResponsesWebSocketState>>,
        delivery: &WebSocketDeliveryTracker,
    ) -> anyhow::Result<Option<(String, Option<u64>)>> {
        let generation = delivery.generation();
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) if state.is_some() => {
                delivery.mark_response_observed();
                return Err(error.into());
            }
            Err(_) => return Ok(Some((text, None))),
        };
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if matches!(kind, "error" | "response.failed")
            || (kind == "response.incomplete"
                && !continuation_guard(continuation).completes_interruption(&value))
        {
            // Retire before observe_server_event releases the active lane. A deadline only
            // bounds how long we read its usage footer; it never makes this stream reusable.
            self.retired = true;
            delivery.retire();
        }
        let response_id = value
            .get("response")
            .and_then(|r| r.get("id"))
            .or_else(|| value.get("response_id"))
            .and_then(Value::as_str)
            .filter(|id| {
                !id.is_empty() && id.len() <= crate::request_state_types::MAX_WIRE_ID_BYTES
            });
        let current_operation = state
            .map(ResponseStateContext::operation_id)
            .transpose()?
            .flatten();
        if let Some(failed) = &self.failed {
            let same_operation = match (&failed.operation_id, &current_operation) {
                (Some(old), Some(current)) => old == current,
                _ => failed.generation == generation,
            };
            let matches_id = failed
                .response_id
                .as_deref()
                .is_some_and(|id| response_id == Some(id));
            let unbound_footer =
                failed.response_id.is_none() && same_operation && kind == "response.failed";
            if matches_id || unbound_footer || (same_operation && kind == "error") {
                let footer = kind == "response.failed";
                let translated = if let Some((context, tail)) = &failed.state {
                    encode(context.translate_sse_value(value, Some(tail)).await?)?
                } else {
                    text
                };
                let terminal = (footer && same_operation).then_some(failed.generation);
                if footer {
                    self.failed = None;
                }
                return Ok(Some((translated, terminal)));
            }
            anyhow::bail!("unassociated frame on a retired response stream");
        }

        let observed = observe_server_event(continuation, &value);
        if observed.disposition == EventDisposition::ConsumeHiddenSetup {
            return Ok(None);
        }
        delivery.mark_response_observed();
        if kind == "response.created" {
            self.response_id = response_id.map(str::to_string);
        }
        let terminal = matches!(
            kind,
            "response.completed" | "response.failed" | "response.incomplete" | "error"
        );
        let wait_for_footer = kind == "error"
            && !crate::response_failure::is_flex_event(&value)
            && !["status", "status_code"].iter().any(|key| {
                value
                    .get(*key)
                    .and_then(Value::as_u64)
                    .is_some_and(|status| (100..=599).contains(&status))
            });
        let translated = if wait_for_footer {
            let failed_response_id = self
                .response_id
                .take()
                .or_else(|| response_id.map(str::to_string));
            let frozen = state
                .map(ResponseStateContext::frozen_failure)
                .transpose()?
                .and_then(|(context, tail)| tail.map(|tail| (context, tail)));
            let translated = if let Some((context, tail)) = &frozen {
                encode(context.translate_sse_value(value, Some(tail)).await?)?
            } else {
                text
            };
            self.failed = Some(FailedResponse {
                response_id: failed_response_id,
                operation_id: current_operation,
                generation,
                state: frozen,
                deadline: Instant::now() + FAILURE_TAIL,
            });
            translated
        } else {
            let translated = match state {
                Some(state) => {
                    state
                        .translate_event_with_compaction(
                            value,
                            crate::inference_limits::get().output_bytes,
                            observed.completed_compaction.as_ref(),
                        )
                        .await?
                }
                None => text,
            };
            if terminal {
                self.response_id = None;
            }
            translated
        };
        Ok(Some((
            translated,
            (terminal && !wait_for_footer).then_some(generation),
        )))
    }
}

fn encode(value: Value) -> anyhow::Result<String> {
    let text = serde_json::to_string(&value)?;
    anyhow::ensure!(
        text.len() <= crate::inference_limits::get().output_bytes,
        "translated response is too large"
    );
    Ok(text)
}

#[cfg(test)]
#[path = "responses_websocket_performance_tests.rs"]
mod performance_tests;

#[cfg(test)]
#[path = "responses_websocket_retirement_tests.rs"]
mod retirement_tests;
