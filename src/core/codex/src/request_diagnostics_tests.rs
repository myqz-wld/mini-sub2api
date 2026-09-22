use super::*;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
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
impl Capture {
    fn logs(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let writer = self.clone();
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish()
    }
}

#[tokio::test(start_paused = true)]
async fn waiting_progress_does_not_retry_and_cancellation_retains_stage() {
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    let mut trace = HttpTrace::new("req_waiting");
    let mut starts = 0;
    trace
        .wait("credential_resolution", async {
            starts += 1;
            tokio::time::sleep(Duration::from_secs(181)).await;
        })
        .await;
    assert_eq!(starts, 1);
    drop(trace);
    let logs = capture.logs();
    assert_eq!(logs.matches("core_http_progress").count(), 3, "{logs}");
    assert!(logs.contains("core_http_canceled"));
    assert!(logs.contains("credential_resolution"));
    assert!(!logs.contains("response_ready"));
}

#[tokio::test(start_paused = true)]
async fn silent_reader_reports_progress_without_renewing_idle_deadline() {
    use crate::response_sse_reader::{SseReadError, SseReader};
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    let mut reader = SseReader::new(Box::pin(futures_util::stream::pending()), 4096)
        .with_request_id("req_silent");
    let started = Instant::now();
    assert_eq!(reader.next_event().await, Err(SseReadError::IdleTimeout));
    assert_eq!(started.elapsed(), Duration::from_secs(300));
    drop(reader);
    let logs = capture.logs();
    assert_eq!(logs.matches("http_sse_progress").count(), 5, "{logs}");
    assert_eq!(logs.matches("read_end=\"reading\"").count(), 5);
    assert_eq!(logs.matches("http_sse_observation").count(), 1);
    assert!(logs.contains("event_idle_timeout"));
    assert!(logs.contains("events=0"));
}

#[test]
fn metadata_uses_defaults_and_allowlists_without_private_labels() {
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    for effort in [None, Some("max"), Some("synthetic-private-effort\n")] {
        let span = tracing::info_span!(
            "http_request",
            requested_model = tracing::field::Empty,
            requested_effort = tracing::field::Empty,
            effective_model = tracing::field::Empty,
            effective_effort = tracing::field::Empty
        );
        let _entered = span.enter();
        let mut body = serde_json::json!({"model":"synthetic-private/gpt-5.6-luna", "input":"synthetic-private"});
        if let Some(effort) = effort {
            body["reasoning"] = serde_json::json!({"effort":effort});
        }
        crate::request_defaults::merge_request_defaults(
            body.as_object_mut().unwrap(),
            crate::request_defaults::model_profile("gpt-5.6-luna"),
            true,
        );
        tracing::info!(event = "settings_ready");
    }
    let logs = capture.logs();
    assert!(logs.contains("effective_effort=\"medium\""), "{logs}");
    assert!(logs.contains("effective_effort=\"max\""));
    assert!(logs.contains("effective_effort=\"other\""));
    assert!(logs.contains("requested_effort=\"unspecified\""));
    assert!(!logs.contains("synthetic-private"));
    assert_eq!(
        provider_code(&serde_json::json!({"response":{"error":{"code":"synthetic-private"}}})),
        "other"
    );
    assert_eq!(
        provider_code(
            &serde_json::json!({"response":{"incomplete_details":{"reason":"max_output_tokens"}}})
        ),
        "max_output_tokens"
    );
}
