//! Bounded SSE framing and event-level deadlines; network heartbeats do not extend idle time.
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;
use std::pin::Pin;
use std::time::Duration;
use tokio::time::Instant;

pub(crate) type UpstreamByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

#[derive(Clone, Copy)]
pub(crate) struct SseTimeouts {
    pub idle: Duration,
    pub terminal_tail: Duration,
}

impl Default for SseTimeouts {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(300),
            terminal_tail: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SseReadError {
    Upstream,
    EventTooLarge,
    IdleTimeout,
}

pub(crate) struct SseReader {
    upstream: Option<UpstreamByteStream>,
    buffer: Vec<u8>,
    pending: Bytes,
    maximum: usize,
    line_length: u8,
    line_cr: bool,
    timeouts: SseTimeouts,
    idle_deadline: Instant,
    terminal_deadline: Option<Instant>,
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
        Self {
            upstream: Some(upstream),
            buffer: Vec::new(),
            pending: Bytes::new(),
            maximum,
            line_length: 0,
            line_cr: false,
            idle_deadline: Instant::now() + timeouts.idle,
            terminal_deadline: None,
            timeouts,
        }
    }

    pub(crate) fn close(&mut self) {
        self.upstream = None;
        self.buffer.clear();
        self.pending = Bytes::new();
    }

    pub(crate) async fn next_event(&mut self) -> Result<Option<Vec<u8>>, SseReadError> {
        loop {
            if self.upstream.is_some() && Instant::now() >= self.deadline() {
                if self.terminal_deadline.is_some() {
                    // Stop receiving new bytes, but validate the bounded tail already received.
                    self.upstream = None;
                } else {
                    self.close();
                    tracing::warn!(
                        event = "http_sse_idle_timeout",
                        "SSE event idle deadline reached"
                    );
                    return Err(SseReadError::IdleTimeout);
                }
            }
            if !self.pending.is_empty() {
                let available = self
                    .maximum
                    .saturating_sub(self.buffer.len())
                    .min(self.pending.len());
                if available == 0 {
                    self.close();
                    return Err(SseReadError::EventTooLarge);
                }
                let mut end = None;
                for (index, byte) in self.pending[..available].iter().copied().enumerate() {
                    if byte == b'\n' {
                        let empty =
                            self.line_length == 0 || (self.line_length == 1 && self.line_cr);
                        self.line_length = 0;
                        self.line_cr = false;
                        if empty {
                            end = Some(index + 1);
                            break;
                        }
                    } else {
                        if self.line_length == 0 {
                            self.line_cr = byte == b'\r';
                        }
                        self.line_length = self.line_length.saturating_add(1).min(2);
                    }
                }
                let take = end.unwrap_or(available);
                self.buffer.extend_from_slice(&self.pending[..take]);
                self.pending = self.pending.slice(take..);
                if end.is_some() {
                    return Ok(Some(self.take_event()));
                }
                continue;
            }
            let Some(upstream) = self.upstream.as_mut() else {
                return Ok((!self.buffer.is_empty()).then(|| self.take_event()));
            };
            let deadline = self.terminal_deadline.unwrap_or(self.idle_deadline);
            match tokio::time::timeout_at(deadline, upstream.next()).await {
                Ok(Some(Ok(bytes))) => self.pending = bytes,
                Ok(Some(Err(_))) => {
                    self.close();
                    return Err(SseReadError::Upstream);
                }
                Ok(None) => self.upstream = None,
                Err(_) => continue,
            }
        }
    }

    fn deadline(&self) -> Instant {
        self.terminal_deadline.unwrap_or(self.idle_deadline)
    }

    fn take_event(&mut self) -> Vec<u8> {
        let event = std::mem::take(&mut self.buffer);
        if let Ok(text) = std::str::from_utf8(&event)
            && let Some(data) = data_payload(text)
            && !data.trim().is_empty()
            && data.trim() != "[DONE]"
        {
            self.idle_deadline = Instant::now() + self.timeouts.idle;
            if self.terminal_deadline.is_none()
                && let Ok(meta) = serde_json::from_str::<EventType>(&data)
                && meta.0
            {
                self.terminal_deadline = Some(Instant::now() + self.timeouts.terminal_tail);
            }
        }
        event
    }
}

struct EventType(bool);

impl<'de> Deserialize<'de> for EventType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EventVisitor;
        impl<'de> Visitor<'de> for EventVisitor {
            type Value = EventType;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an SSE event object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<EventType, M::Error> {
                let mut terminal = false;
                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if key == "type" {
                        // Match the consumer's last-key-wins JSON semantics without allocating
                        // a second response/output tree just to inspect its event type.
                        let kind = map.next_value::<serde_json::Value>()?;
                        terminal = matches!(
                            kind.as_str(),
                            Some(
                                "response.completed"
                                    | "response.failed"
                                    | "response.incomplete"
                                    | "error"
                            )
                        );
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(EventType(terminal))
            }
        }
        deserializer.deserialize_map(EventVisitor)
    }
}

pub(crate) fn data_payload(text: &str) -> Option<Cow<'_, str>> {
    let mut parts = text.lines().filter_map(|line| {
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
