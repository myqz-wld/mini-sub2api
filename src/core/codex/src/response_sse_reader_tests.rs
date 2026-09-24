use super::*;
use futures_util::stream;

fn short_timeouts() -> SseTimeouts {
    SseTimeouts {
        idle: Duration::from_millis(100),
        output_idle: Duration::from_millis(100),
        terminal_tail: Duration::from_millis(50),
    }
}

fn delayed_repeat(value: &'static str, interval: Duration) -> UpstreamByteStream {
    Box::pin(stream::unfold((), move |_| async move {
        tokio::time::sleep(interval).await;
        Some((Ok(Bytes::from_static(value.as_bytes())), ()))
    }))
}

#[tokio::test(start_paused = true)]
async fn comments_empty_data_done_and_partial_events_cannot_extend_idle() {
    for value in [
        ": heartbeat\n\n",
        "data: \n\n",
        "data: [DONE]\n\n",
        "data: {",
    ] {
        let started = Instant::now();
        let mut reader = SseReader::with_timeouts(
            delayed_repeat(value, Duration::from_millis(10)),
            1024,
            short_timeouts(),
        );
        loop {
            match reader.next_event().await {
                Ok(Some(_)) => {}
                Err(SseReadError::IdleTimeout) => break,
                result => panic!("unexpected read result: {result:?}"),
            }
        }
        assert_eq!(started.elapsed(), Duration::from_millis(100));
        assert!(reader.upstream.is_none());
        assert!(reader.buffer.is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn framed_data_events_refresh_idle_without_a_total_duration_cap() {
    let mut reader = SseReader::with_timeouts(
        delayed_repeat(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
            Duration::from_millis(60),
        ),
        1024,
        short_timeouts(),
    );
    let started = Instant::now();
    for _ in 0..5 {
        assert!(reader.next_event().await.unwrap().is_some());
    }
    assert_eq!(started.elapsed(), Duration::from_millis(300));
}

#[tokio::test(start_paused = true)]
async fn status_unknown_and_empty_deltas_cannot_extend_first_output_or_output_idle() {
    for value in [
        "data: {\"type\":\"response.metadata\"}\n\n",
        "data: {\"type\":\"response.in_progress\"}\n\n",
        "data: {\"type\":\"synthetic.unknown\",\"delta\":\"x\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"\"}\n\n",
    ] {
        for initial_output in [false, true] {
            let mut reader = SseReader::with_timeouts(
                delayed_repeat(value, Duration::from_millis(20)),
                1024,
                short_timeouts(),
            );
            let started = Instant::now();
            if initial_output {
                tokio::time::advance(Duration::from_millis(30)).await;
                reader.buffer =
                    b"data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"x\"}\n\n"
                        .to_vec();
                reader.take_event();
            }
            loop {
                match reader.next_event().await {
                    Ok(Some(_)) => {}
                    Err(error) => {
                        assert_eq!(
                            error,
                            if initial_output {
                                SseReadError::OutputIdleTimeout
                            } else {
                                SseReadError::FirstOutputTimeout
                            }
                        );
                        break;
                    }
                    result => panic!("unexpected read: {result:?}"),
                }
            }
            assert_eq!(
                started.elapsed(),
                Duration::from_millis(if initial_output { 130 } else { 100 })
            );
            assert!(reader.upstream.is_none());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn each_output_class_extends_the_output_deadline() {
    for value in [
        "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"x\"}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
        "data: {\"type\":\"response.output_audio.delta\",\"delta\":\"x\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\"}}\n\n",
    ] {
        let mut reader = SseReader::with_timeouts(
            delayed_repeat(value, Duration::from_millis(60)),
            1024,
            short_timeouts(),
        );
        let started = Instant::now();
        for _ in 0..5 {
            assert!(reader.next_event().await.unwrap().is_some());
        }
        assert_eq!(started.elapsed(), Duration::from_millis(300));
    }
}

#[tokio::test(start_paused = true)]
async fn first_terminal_starts_a_tail_that_later_events_cannot_extend() {
    for terminal in [
        "response.completed",
        "response.failed",
        "response.incomplete",
        "error",
    ] {
        let first = Bytes::from(format!("data: {{\"type\":\"{terminal}\"}}\n\n"));
        let following = delayed_repeat(
            "data: {\"type\":\"response.failed\"}\n\n",
            Duration::from_millis(20),
        );
        let input = Box::pin(stream::once(async { Ok(first) }).chain(following));
        let mut reader = SseReader::with_timeouts(input, 1024, short_timeouts());
        let started = Instant::now();
        assert!(reader.next_event().await.unwrap().is_some());
        while reader.next_event().await.unwrap().is_some() {}
        assert_eq!(started.elapsed(), Duration::from_millis(50));
        assert!(reader.upstream.is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn terminal_tail_stops_a_silent_connection_and_returns_received_fragments_for_validation() {
    let prefix = "data: {\"type\":\"response.completed\"}\n\ndata: {broken";
    let input =
        stream::once(async { Ok(Bytes::from_static(prefix.as_bytes())) }).chain(stream::pending());
    let mut reader = SseReader::with_timeouts(Box::pin(input), 1024, short_timeouts());
    assert!(reader.next_event().await.unwrap().is_some());
    let started = Instant::now();
    assert_eq!(
        reader.next_event().await.unwrap().unwrap(),
        b"data: {broken"
    );
    assert_eq!(started.elapsed(), Duration::from_millis(50));
    assert!(reader.next_event().await.unwrap().is_none());
}

#[tokio::test]
async fn framing_and_final_eof_fragment_are_independent_of_chunking() {
    let wire = ": comment\r\ndata: {\"type\":\"response.created\"}\r\n\r\ndata: {\"type\":\"response.completed\"}";
    for size in [1, 2, 7, wire.len()] {
        let chunks: Vec<_> = wire
            .as_bytes()
            .chunks(size)
            .map(|b| Ok(Bytes::copy_from_slice(b)))
            .collect();
        let mut reader = SseReader::new(Box::pin(stream::iter(chunks)), wire.len());
        let mut events = Vec::new();
        while let Some(event) = reader.next_event().await.unwrap() {
            events.push(event);
        }
        assert_eq!(events.len(), 2);
        assert_eq!(events.concat(), wire.replace("\r\n", "\n").as_bytes());
    }
}

#[tokio::test]
async fn event_limit_preserves_verified_prefixes_and_releases_upstream() {
    let prefix = ": allowed\n\n";
    let wire = format!("{prefix}data: {}\n\n", "x".repeat(64));
    let mut reader = SseReader::new(Box::pin(stream::once(async { Ok(Bytes::from(wire)) })), 32);
    assert_eq!(
        reader.next_event().await.unwrap().unwrap(),
        prefix.as_bytes()
    );
    assert_eq!(reader.next_event().await, Err(SseReadError::EventTooLarge));
    assert!(reader.upstream.is_none());
}

#[test]
fn common_data_lines_are_borrowed_and_multiline_payloads_keep_newlines() {
    assert!(matches!(
        data_payload("data: {}\r\n\r\n"),
        Some(Cow::Borrowed("{}"))
    ));
    assert_eq!(data_payload(": comment\n\n"), None);
    assert_eq!(
        data_payload("data: {\n: comment\ndata: }\n\n").as_deref(),
        Some("{\n}")
    );
}

#[test]
fn terminal_detection_matches_json_consumers_without_inspecting_nested_types() {
    for (data, expected) in [
        (
            r#"{"type":"response.completed","response":{"type":"other"}}"#,
            true,
        ),
        (r#"{"type":"other","ty\u0070e":"response.completed"}"#, true),
        (r#"{"type":{},"type":"response.failed"}"#, true),
        (r#"{"type":"response.completed","type":null}"#, false),
        (r#"{"response":{"type":"response.completed"}}"#, false),
    ] {
        assert_eq!(
            sse_progress::classify(data.as_bytes()) == Class::Terminal,
            expected
        );
    }
}

#[tokio::test]
async fn bom_cr_and_mixed_delimiters_work_across_every_byte_boundary() {
    let wire = "\u{feff}data: {\"type\":\"response.created\"}\r\rdata: {\"type\":\"response.completed\"}\r\n\r\ndata: [DONE]\n\n";
    for size in 1..=wire.len() {
        let chunks: Vec<_> = wire
            .as_bytes()
            .chunks(size)
            .map(|b| Ok(Bytes::copy_from_slice(b)))
            .collect();
        let mut reader = SseReader::new(Box::pin(stream::iter(chunks)), 4096);
        let mut payloads = Vec::new();
        while let Some(event) = reader.next_event().await.unwrap() {
            payloads.push(
                data_payload(std::str::from_utf8(&event).unwrap())
                    .unwrap()
                    .into_owned(),
            );
        }
        assert_eq!(
            payloads,
            [
                "{\"type\":\"response.created\"}",
                "{\"type\":\"response.completed\"}",
                "[DONE]"
            ]
        );
    }
}
