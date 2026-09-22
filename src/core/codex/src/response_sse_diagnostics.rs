//! Bounded observations only: no event names, payloads, or provider identifiers.
use mini_sub2api_protocol_v1::sse_progress::Class;
use tokio::time::Instant;

pub(crate) struct SseDiagnostics {
    pub request_id: Option<String>,
    pub read_end: &'static str,
    pub processing_stage: &'static str,
    pub error_kind: &'static str,
    pub terminal_kind: &'static str,
    pub provider_code: &'static str,
    next_report: Instant,
    started: Instant,
    bytes: u64,
    events: u64,
    outputs: u64,
    heartbeats: u64,
    counts: [u64; Class::COUNT],
    first_byte_ms: i64,
    last_byte_ms: i64,
    first_event_ms: i64,
    last_event_ms: i64,
    first_output_ms: i64,
    last_output_ms: i64,
}

impl SseDiagnostics {
    pub fn new() -> Self {
        Self {
            request_id: None,
            read_end: "reader_dropped",
            processing_stage: "none",
            error_kind: "none",
            terminal_kind: "none",
            provider_code: "none",
            next_report: Instant::now() + crate::request_diagnostics::PROGRESS_INTERVAL,
            started: Instant::now(),
            bytes: 0,
            events: 0,
            outputs: 0,
            heartbeats: 0,
            counts: [0; Class::COUNT],
            first_byte_ms: -1,
            last_byte_ms: -1,
            first_event_ms: -1,
            last_event_ms: -1,
            first_output_ms: -1,
            last_output_ms: -1,
        }
    }

    fn elapsed_ms(&self) -> i64 {
        self.started.elapsed().as_millis().min(i64::MAX as u128) as i64
    }

    pub fn report_deadline(&self) -> Instant {
        self.next_report
    }

    pub fn report_if_due(&mut self) {
        if Instant::now() >= self.next_report {
            self.emit("http_sse_progress");
            self.next_report = Instant::now() + crate::request_diagnostics::PROGRESS_INTERVAL;
        }
    }

    pub fn observe_error(
        &mut self,
        error: &(dyn std::error::Error + 'static),
        phase: &'static str,
    ) {
        let details = crate::error_diagnostics::ErrorDetails::observe(error);
        self.error_kind = details.kind;
        if let Some(id) = &self.request_id {
            details.log(id, phase);
        }
    }

    pub fn received(&mut self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        self.bytes = self.bytes.saturating_add(bytes as u64);
        self.last_byte_ms = self.elapsed_ms();
        if self.first_byte_ms < 0 {
            self.first_byte_ms = self.last_byte_ms;
        }
    }

    pub fn event(&mut self, class: Option<Class>) {
        let Some(class) = class else {
            self.heartbeats = self.heartbeats.saturating_add(1);
            return;
        };
        self.events = self.events.saturating_add(1);
        self.counts[class as usize] = self.counts[class as usize].saturating_add(1);
        self.last_event_ms = self.elapsed_ms();
        if self.first_event_ms < 0 {
            self.first_event_ms = self.last_event_ms;
        }
        if class.is_output() {
            self.outputs = self.outputs.saturating_add(1);
            self.last_output_ms = self.last_event_ms;
            if self.first_output_ms < 0 {
                self.first_output_ms = self.last_output_ms;
            }
        }
    }
    fn emit(&self, event: &'static str) {
        let Some(request_id) = &self.request_id else {
            return;
        };
        tracing::info!(
            event,
            request_id,
            read_end = if event == "http_sse_progress" {
                "reading"
            } else {
                self.read_end
            },
            processing_stage = self.processing_stage,
            error_kind = self.error_kind,
            terminal_kind = self.terminal_kind,
            provider_code = self.provider_code,
            elapsed_ms = self.elapsed_ms(),
            bytes = self.bytes,
            events = self.events,
            outputs = self.outputs,
            heartbeats = self.heartbeats,
            status = self.counts[Class::Status as usize],
            text = self.counts[Class::Text as usize],
            reasoning = self.counts[Class::Reasoning as usize],
            tool = self.counts[Class::Tool as usize],
            media = self.counts[Class::Media as usize],
            item = self.counts[Class::Item as usize],
            terminal = self.counts[Class::Terminal as usize],
            empty = self.counts[Class::Empty as usize],
            other = self.counts[Class::Other as usize],
            invalid = self.counts[Class::Invalid as usize],
            first_byte_ms = self.first_byte_ms,
            last_byte_ms = self.last_byte_ms,
            first_event_ms = self.first_event_ms,
            last_event_ms = self.last_event_ms,
            first_output_ms = self.first_output_ms,
            last_output_ms = self.last_output_ms,
            byte_idle_ms = if self.last_byte_ms < 0 {
                self.elapsed_ms()
            } else {
                self.elapsed_ms() - self.last_byte_ms
            },
            output_idle_ms = if self.last_output_ms < 0 {
                self.elapsed_ms()
            } else {
                self.elapsed_ms() - self.last_output_ms
            },
            "HTTP SSE read observation"
        );
    }
}

impl Drop for SseDiagnostics {
    fn drop(&mut self) {
        self.emit("http_sse_observation");
    }
}

#[cfg(test)]
mod tests {
    use crate::response_sse_reader::SseReader;
    use bytes::Bytes;
    use futures_util::stream;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn reader_summary_keeps_counts_and_timings_without_provider_content() {
        let output = Capture(Arc::new(Mutex::new(Vec::new())));
        let writer = output.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let wire = concat!(
            ": synthetic-private-heartbeat\n\n",
            "data: {\"type\":\"synthetic-private-type\",\"secret\":\"synthetic-private-payload\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic-private-output\"}\n\n",
            "data: {\"type\":\"response.completed\"}\n\n"
        );
        let mut reader = SseReader::new(
            Box::pin(stream::iter([Ok(Bytes::from_static(wire.as_bytes()))])),
            4096,
        )
        .with_request_id("req_observation");
        while reader.next_event().await.unwrap().is_some() {}
        drop(reader);
        let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        for expected in [
            "http_sse_observation",
            "req_observation",
            "events=3",
            "outputs=1",
            "heartbeats=1",
            "other=1",
            "terminal=1",
            "first_output_ms=",
            "last_event_ms=",
            "read_end=\"eof\"",
        ] {
            assert!(logs.contains(expected), "missing {expected}: {logs}");
        }
        assert!(!logs.contains("synthetic-private"));
        assert!(!logs.contains("response.completed"));
    }
}
