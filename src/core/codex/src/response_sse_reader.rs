//! Bounded SSE framing with independent event and meaningful-output deadlines.
use crate::response_sse_diagnostics::SseDiagnostics;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use mini_sub2api_protocol_v1::sse_progress::{self, Class};
use std::borrow::Cow;
use std::pin::Pin;
use std::time::Duration;
use tokio::time::Instant;

pub(crate) type UpstreamByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

#[derive(Clone, Copy)]
pub(crate) struct SseTimeouts {
    pub idle: Duration,
    pub output_idle: Duration,
    pub terminal_tail: Duration,
}

impl Default for SseTimeouts {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(300),
            output_idle: Duration::from_secs(300),
            terminal_tail: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SseReadError {
    Upstream,
    EventTooLarge,
    IdleTimeout,
    FirstOutputTimeout,
    OutputIdleTimeout,
}

pub(crate) struct SseReader {
    upstream: Option<UpstreamByteStream>,
    buffer: Vec<u8>,
    pending: Bytes,
    maximum: usize,
    event_bytes: usize,
    line_length: u8,
    line_cr: bool,
    stream_start: bool,
    timeouts: SseTimeouts,
    idle_deadline: Instant,
    output_deadline: Instant,
    has_output: bool,
    terminal_deadline: Option<Instant>,
    diagnostics: SseDiagnostics,
}

impl SseReader {
    pub(crate) fn new(upstream: UpstreamByteStream, maximum: usize) -> Self {
        Self::with_timeouts(upstream, maximum, SseTimeouts::default())
    }

    pub(crate) fn with_timeouts(
        upstream: UpstreamByteStream,
        maximum: usize,
        timeouts: SseTimeouts,
    ) -> Self {
        let now = Instant::now();
        Self {
            upstream: Some(upstream),
            buffer: Vec::new(),
            pending: Bytes::new(),
            maximum,
            event_bytes: 0,
            line_length: 0,
            line_cr: false,
            stream_start: true,
            idle_deadline: now + timeouts.idle,
            output_deadline: now + timeouts.output_idle,
            has_output: false,
            terminal_deadline: None,
            diagnostics: SseDiagnostics::new(),
            timeouts,
        }
    }

    pub(crate) fn with_request_id(mut self, request_id: &str) -> Self {
        self.diagnostics.request_id = Some(request_id.to_owned());
        self
    }

    pub(crate) fn close(&mut self) {
        self.upstream = None;
        self.buffer.clear();
        self.pending = Bytes::new();
    }

    pub(crate) fn processing(&mut self, stage: &'static str) {
        self.diagnostics.processing_stage = stage;
    }

    pub(crate) fn observe_error(
        &mut self,
        error: &(dyn std::error::Error + 'static),
        phase: &'static str,
    ) {
        self.diagnostics.observe_error(error, phase);
    }

    pub(crate) fn terminal(&mut self, value: &serde_json::Value) {
        let kind = value.get("type").and_then(serde_json::Value::as_str);
        self.diagnostics.terminal_kind = match kind {
            Some("response.completed") => "completed",
            Some("response.failed") => "failed",
            Some("response.incomplete") => "incomplete",
            Some("error") => "error",
            _ => return,
        };
        self.diagnostics.provider_code = crate::request_diagnostics::provider_code(value);
    }

    pub(crate) fn aggregated_terminal(&mut self, kind: &'static str, value: &serde_json::Value) {
        self.diagnostics.terminal_kind = kind;
        self.diagnostics.provider_code = crate::request_diagnostics::provider_code(value);
    }

    pub(crate) async fn next_event(&mut self) -> Result<Option<Vec<u8>>, SseReadError> {
        loop {
            self.diagnostics.report_if_due();
            if self.upstream.is_some() && Instant::now() >= self.deadline() {
                if self.terminal_deadline.is_some() {
                    // Stop receiving new bytes, but validate the bounded tail already received.
                    self.upstream = None;
                    self.diagnostics.read_end = "terminal_tail_closed";
                } else {
                    let error = if self.output_deadline < self.idle_deadline {
                        if self.has_output {
                            self.diagnostics.read_end = "output_idle_timeout";
                            SseReadError::OutputIdleTimeout
                        } else {
                            self.diagnostics.read_end = "first_output_timeout";
                            SseReadError::FirstOutputTimeout
                        }
                    } else {
                        self.diagnostics.read_end = "event_idle_timeout";
                        SseReadError::IdleTimeout
                    };
                    self.close();
                    return Err(error);
                }
            }
            if !self.pending.is_empty() {
                let available = self
                    .maximum
                    .saturating_sub(self.event_bytes)
                    .min(self.pending.len());
                if available == 0 {
                    self.diagnostics.read_end = "event_too_large";
                    self.close();
                    return Err(SseReadError::EventTooLarge);
                }
                let mut take = 0;
                let mut complete = false;
                for byte in self.pending[..available].iter().copied() {
                    take += 1;
                    if self.line_cr && byte == b'\n' {
                        self.line_cr = false;
                        continue;
                    }
                    self.line_cr = byte == b'\r';
                    if matches!(byte, b'\r' | b'\n') {
                        self.buffer.push(b'\n');
                        let empty = self.line_length == 0;
                        self.line_length = 0;
                        if empty {
                            complete = true;
                            break;
                        }
                    } else {
                        self.buffer.push(byte);
                        self.line_length = self.line_length.saturating_add(1).min(2);
                        if self.stream_start && self.buffer.len() == 3 {
                            self.stream_start = false;
                            if self.buffer == [0xef, 0xbb, 0xbf] {
                                self.buffer.clear();
                                self.line_length = 0;
                            }
                        }
                    }
                }
                self.event_bytes += take;
                self.pending = self.pending.slice(take..);
                if complete {
                    self.stream_start = false;
                    return Ok(Some(self.take_event()));
                }
                continue;
            }
            let deadline = self.deadline().min(self.diagnostics.report_deadline());
            let Some(upstream) = self.upstream.as_mut() else {
                return Ok((!self.buffer.is_empty()).then(|| self.take_event()));
            };
            match tokio::time::timeout_at(deadline, upstream.next()).await {
                Ok(Some(Ok(bytes))) => {
                    self.diagnostics.received(bytes.len());
                    self.pending = bytes;
                }
                Ok(Some(Err(error))) => {
                    self.diagnostics.read_end = "upstream_read_error";
                    self.diagnostics.observe_error(&error, "upstream_body");
                    self.close();
                    return Err(SseReadError::Upstream);
                }
                Ok(None) => {
                    self.diagnostics.read_end = "eof";
                    self.upstream = None;
                }
                Err(_) => continue,
            }
        }
    }

    fn deadline(&self) -> Instant {
        self.terminal_deadline
            .unwrap_or(self.idle_deadline.min(self.output_deadline))
    }

    fn take_event(&mut self) -> Vec<u8> {
        self.event_bytes = 0;
        let event = std::mem::take(&mut self.buffer);
        if let Ok(text) = std::str::from_utf8(&event)
            && let Some(data) = data_payload(text)
            && !data.trim().is_empty()
            && data.trim() != "[DONE]"
        {
            let now = Instant::now();
            let class = sse_progress::classify(data.as_bytes());
            self.diagnostics.event(Some(class));
            self.idle_deadline = now + self.timeouts.idle;
            if class.is_output() {
                self.has_output = true;
                self.output_deadline = now + self.timeouts.output_idle;
            }
            if self.terminal_deadline.is_none() && class == Class::Terminal {
                self.terminal_deadline = Some(now + self.timeouts.terminal_tail);
            }
        } else {
            self.diagnostics.event(None);
        }
        event
    }
}

pub(crate) fn data_payload(text: &str) -> Option<Cow<'_, str>> {
    let mut parts = text.split(['\r', '\n']).filter_map(|line| {
        line.strip_suffix('\r')
            .unwrap_or(line)
            .strip_prefix("data:")
            .map(|value| value.strip_prefix(' ').unwrap_or(value))
    });
    let first = parts.next()?;
    let Some(second) = parts.next() else {
        return Some(Cow::Borrowed(first));
    };
    let mut joined = String::with_capacity(first.len() + second.len() + 1);
    joined.push_str(first);
    joined.push('\n');
    joined.push_str(second);
    for part in parts {
        joined.push('\n');
        joined.push_str(part);
    }
    Some(Cow::Owned(joined))
}

#[cfg(test)]
#[path = "response_sse_reader_tests.rs"]
mod tests;
