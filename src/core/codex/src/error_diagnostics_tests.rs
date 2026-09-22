use super::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Default)]
pub(crate) struct Capture(pub Arc<Mutex<Vec<u8>>>);
impl io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

async fn broken_response(stall: bool) -> reqwest::Error {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nshort")
            .await
            .unwrap();
        if stall {
            std::future::pending::<()>().await;
        }
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .read_timeout(std::time::Duration::from_millis(50))
        .build()
        .unwrap();
    let response = client
        .get(format!(
            "http://{address}/synthetic-private?token=synthetic-private"
        ))
        .send()
        .await
        .unwrap();
    let error = response.bytes().await.unwrap_err();
    server.abort();
    let _ = server.await;
    error
}

#[tokio::test]
async fn real_truncated_and_timed_out_bodies_have_safe_different_details() {
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let error = broken_response(false).await;
    let truncated = ErrorDetails::observe(&error);
    assert!(matches!(truncated.kind, "body" | "decode"), "{truncated:?}");
    assert!(
        truncated.http_kind == "incomplete_message" || truncated.io_kind == "unexpected_eof",
        "{truncated:?}"
    );
    truncated.log("req_truncated", "upstream_body");
    let mut reader = crate::response_sse_reader::SseReader::new(
        Box::pin(futures_util::stream::iter([Err(error)])),
        4096,
    )
    .with_request_id("req_reader");
    assert_eq!(
        reader.next_event().await,
        Err(crate::response_sse_reader::SseReadError::Upstream)
    );
    drop(reader);
    let timed = ErrorDetails::observe(&broken_response(true).await);
    assert_eq!(timed.kind, "timeout", "{timed:?}");
    timed.log("req_timeout", "upstream_body");
    let text = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    for expected in [
        "transport_error",
        "upstream_body",
        "req_truncated",
        "req_timeout",
        "source_depth",
        "http_error",
        "req_reader",
        "http_sse_observation",
        "upstream_read_error",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert!(!text.contains("synthetic-private"));
    assert!(!text.contains("127.0.0.1"));
}

#[tokio::test]
async fn real_h2_reset_preserves_remote_reason_code() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut connection = h2::server::handshake(socket).await.unwrap();
        let (_, mut send) = connection.accept().await.unwrap().unwrap();
        let mut body = send.send_response(http::Response::new(()), false).unwrap();
        body.send_data(bytes::Bytes::from_static(b"synthetic-private"), false)
            .unwrap();
        body.send_reset(h2::Reason::INTERNAL_ERROR);
        while connection.accept().await.is_some() {}
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .http2_prior_knowledge()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    let result = match client
        .get(format!("http://{address}/synthetic-private"))
        .send()
        .await
    {
        Ok(response) => response.bytes().await,
        Err(error) => Err(error),
    };
    let details = ErrorDetails::observe(&result.unwrap_err());
    server.abort();
    let _ = server.await;
    assert_eq!(details.h2_code, 2, "{details:?}");
    assert!(details.h2_reset && details.h2_remote, "{details:?}");
}

#[tokio::test]
async fn real_tls_handshake_failure_is_identifiable_without_peer_data() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut hello = [0; 4096];
        assert!(socket.read(&mut hello).await.unwrap() > 0);
        let _ = socket
            .write_all(b"HTTP/1.1 400 synthetic-private\r\n\r\n")
            .await;
    });
    let error = reqwest::Client::builder()
        .no_proxy()
        .use_native_tls()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap()
        .get(format!("https://{address}/synthetic-private"))
        .send()
        .await
        .unwrap_err();
    server.await.unwrap();
    let details = ErrorDetails::observe(&error);
    assert_eq!(details.kind, "connect");
    assert_ne!(details.tls_kind, "none", "{details:?}");
    assert!(!format!("{details:?}").contains("synthetic-private"));
}

#[tokio::test]
async fn refused_connect_is_distinct_from_a_response_body_failure() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{address}/synthetic-private"))
        .send()
        .await
        .unwrap_err();
    let details = ErrorDetails::observe(&error);
    assert_eq!(details.kind, "connect");
    assert_eq!(details.io_kind, "connection_refused");
    assert_ne!(details.os_code, 0);
}

#[test]
fn protocol_and_tls_codes_do_not_format_error_text() {
    let details = ErrorDetails::observe(&h2::Error::from(h2::Reason::CANCEL));
    assert_eq!(details.h2_code, 8);
    #[cfg(target_os = "linux")]
    {
        let stack = openssl::pkey::PKey::private_key_from_pem(b"synthetic-private").unwrap_err();
        let details = ErrorDetails::observe(&stack);
        assert_eq!(details.tls_kind, "openssl");
        assert_ne!(details.tls_library_code, 0);
        assert!(!format!("{details:?}").contains("synthetic-private"));
    }
}

#[test]
fn pathological_sources_are_bounded_without_invoking_display() {
    #[derive(Debug)]
    struct Cycle;
    impl std::fmt::Display for Cycle {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("must not format errors")
        }
    }
    impl Error for Cycle {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(self)
        }
    }
    let details = ErrorDetails::observe(&Cycle);
    assert_eq!(details.source_depth, 16);
    assert!(details.source_truncated);
}
