//! Request metadata is recorded from the already parsed normalization object.
//! No second payload tree, free-form field, request body or credential is retained.
use serde_json::{Map, Value};
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

pub(crate) const PROGRESS_INTERVAL: Duration = Duration::from_secs(60);

pub(crate) fn provider_code(value: &Value) -> &'static str {
    let code = value
        .pointer("/error/code")
        .or_else(|| value.get("code"))
        .or_else(|| value.pointer("/response/error/code"))
        .or_else(|| value.pointer("/response/incomplete_details/reason"))
        .or_else(|| value.pointer("/incomplete_details/reason"))
        .and_then(Value::as_str);
    match code {
        None => "none",
        Some("rate_limit_exceeded") => "rate_limit_exceeded",
        Some("server_error") => "server_error",
        Some("invalid_request_error") => "invalid_request_error",
        Some("context_length_exceeded") => "context_length_exceeded",
        Some("max_output_tokens") => "max_output_tokens",
        Some("content_filter") => "content_filter",
        Some("insufficient_quota") => "insufficient_quota",
        Some("model_not_found") => "model_not_found",
        Some("invalid_api_key") => "invalid_api_key",
        Some(_) => "other",
    }
}

pub(crate) fn record_settings(object: &Map<String, Value>, effective: bool) {
    let model = object.get("model").and_then(Value::as_str);
    let model = model.map_or("missing", crate::request_defaults::diagnostic_model);
    let raw_effort = object.get("reasoning").and_then(|v| v.get("effort"));
    let effort = match raw_effort.and_then(Value::as_str) {
        Some(
            value @ ("none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
            | "persistent"),
        ) => value,
        None if raw_effort.is_none() => "unspecified",
        _ => "other",
    };
    let span = tracing::Span::current();
    if effective {
        span.record("effective_model", model);
        span.record("effective_effort", effort);
    } else {
        span.record("requested_model", model);
        span.record("requested_effort", effort);
    }
}

pub(crate) struct HttpTrace {
    request_id: String,
    started: Instant,
    phase: &'static str,
    next_report: Instant,
    finished: bool,
}

impl HttpTrace {
    pub fn new(request_id: &str) -> Self {
        let started = Instant::now();
        tracing::info!(event = "core_http_started", request_id);
        Self {
            request_id: request_id.into(),
            started,
            phase: "request_body",
            next_report: started + PROGRESS_INTERVAL,
            finished: false,
        }
    }

    pub fn phase(&mut self, phase: &'static str) {
        self.phase = phase;
    }

    // Poll the same future; progress reporting never retries or spawns a background task.
    pub async fn wait<T>(&mut self, phase: &'static str, future: impl Future<Output = T>) -> T {
        self.phase = phase;
        tokio::pin!(future);
        loop {
            tokio::select! {
                value = &mut future => return value,
                _ = tokio::time::sleep_until(self.next_report) => {
                    tracing::info!(event = "core_http_progress", request_id = self.request_id,
                        phase = self.phase, elapsed_ms = self.started.elapsed().as_millis() as u64);
                    self.next_report = Instant::now() + PROGRESS_INTERVAL;
                }
            }
        }
    }

    pub fn finish(
        &mut self,
        result: &Result<axum::http::Response<axum::body::Body>, crate::error::CoreFailure>,
    ) {
        self.finished = true;
        let (outcome, status, code) = match result {
            Ok(response) => ("response_ready", response.status().as_u16(), "none"),
            Err(error) => ("failed", error.status().as_u16(), error.code()),
        };
        tracing::info!(
            event = "core_http_response",
            request_id = self.request_id,
            phase = self.phase,
            outcome,
            http_status = status,
            error_code = code,
            elapsed_ms = self.started.elapsed().as_millis() as u64
        );
    }
}

impl Drop for HttpTrace {
    fn drop(&mut self) {
        if !self.finished {
            tracing::info!(
                event = "core_http_canceled",
                request_id = self.request_id,
                phase = self.phase,
                elapsed_ms = self.started.elapsed().as_millis() as u64
            );
        }
    }
}

#[cfg(test)]
#[path = "request_diagnostics_tests.rs"]
mod tests;
