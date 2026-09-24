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

#[test]
fn ignored_field_events_are_bounded_aggregated_and_private_before_send() {
    use crate::request_normalizer::{EmulationTransport, prepare_codex_overlay_for_test};
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    let mut body = serde_json::json!({"model":"private-model-label","input":[],"tools":[]});
    for key in [
        "max_tool_calls",
        "top_logprobs",
        "background",
        "prompt",
        "stream_id",
        "metadata",
        "context_management",
        "moderation",
        "prompt_cache_options",
        "max_output_tokens",
        "max_tokens",
        "temperature",
        "top_p",
        "frequency_penalty",
        "presence_penalty",
        "prompt_cache_retention",
        "safety_identifier",
        "truncation",
        "user",
    ] {
        body[key] = "synthetic-secret-value".into();
    }
    for i in 0..200 {
        body[format!("synthetic-secret-key-{i}")] = "synthetic-secret-value".into();
    }
    let prepared = prepare_codex_overlay_for_test(
        crate::request_profile::UpstreamProfile::CodexSubscription1560,
        EmulationTransport::Http,
        &http::HeaderMap::new(),
        bytes::Bytes::from(body.to_string()),
        1 << 20,
    )
    .unwrap();
    // The diagnostic scope has flushed before the caller can hand these bytes to the sender.
    let logs = capture.logs();
    assert!(logs.contains("codex_ignored_field"));
    assert!(logs.contains("codex_ignored_fields_overflow"));
    assert!(logs.lines().count() <= 17);
    assert!(!logs.contains("synthetic-secret"));
    assert!(!logs.contains("private-model-label"));
    assert!(
        !std::str::from_utf8(&prepared.body)
            .unwrap()
            .contains("synthetic-secret")
    );

    let before = capture.logs().lines().count();
    crate::ignored_fields::scope(
        EmulationTransport::Http,
        "model",
        "other",
        "construction",
        || {
            for _ in 0..200 {
                crate::ignored_fields::record("input[]", "status", "unsupported_field");
            }
        },
    );
    let logs = capture.logs();
    assert_eq!(logs.lines().count(), before + 1);
    assert!(logs.contains("count=200"));
}
