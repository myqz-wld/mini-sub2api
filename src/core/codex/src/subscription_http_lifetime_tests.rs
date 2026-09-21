use super::*;
use crate::response_sse_reader::{SseReader, SseTimeouts, UpstreamByteStream};
use crate::response_sse_translation::translate_reader;
use futures_util::{Stream, StreamExt, stream};
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Poll};
use std::time::Duration;

struct WatchedUpstream {
    inner: UpstreamByteStream,
    closed: Arc<AtomicBool>,
}

impl Stream for WatchedUpstream {
    type Item = Result<Bytes, reqwest::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for WatchedUpstream {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

async fn context(store: &RequestStateStore, body: Value) -> ResponseStateContext {
    let prepared = prepare(store, body).await.unwrap();
    ResponseStateContext::new(
        OWNER,
        NAMESPACE,
        KEY,
        store,
        prepared.resolved_identity.as_ref(),
        None,
    )
    .with_operation(prepared.operation)
}

fn reader(prefix: &'static str, closed: Arc<AtomicBool>) -> SseReader {
    let chunks = stream::once(async move { Ok(Bytes::from_static(prefix.as_bytes())) })
        .chain(stream::pending());
    SseReader::with_timeouts(
        Box::pin(WatchedUpstream {
            inner: Box::pin(chunks),
            closed,
        }),
        4096,
        SseTimeouts {
            idle: Duration::from_millis(100),
            terminal_tail: Duration::from_millis(50),
        },
    )
}

#[tokio::test(start_paused = true)]
async fn idle_failure_releases_transport_and_operation_before_trailer_delivery_finishes() {
    let (_temp, store) = store();
    let context = context(&store, request(json!([input("start")]))).await;
    let retained_context = context.clone();
    let closed = Arc::new(AtomicBool::new(false));
    let mut frames = Box::pin(translate_reader(
        reader(": heartbeat\n\n", closed.clone()),
        context,
        4096,
    ));
    assert!(frames.next().await.unwrap().unwrap().data_ref().is_some());
    assert_eq!(store.contexts.inner.lock().unwrap().operations.len(), 1);
    let started = tokio::time::Instant::now();
    let failure = frames.next().await.unwrap().unwrap();
    assert_eq!(started.elapsed(), Duration::from_millis(100));
    assert_eq!(
        failure.trailers_ref().unwrap()[mini_sub2api_protocol_v1::RETRY_ADVICE_TRAILER],
        "never"
    );
    assert!(closed.load(Ordering::SeqCst));
    // Do not poll or drop the response stream yet: a slow downstream must not retain the lane.
    let inner = store.contexts.inner.lock().unwrap();
    assert!(inner.operations.is_empty());
    assert!(inner.reservations.is_empty());
    drop(inner);
    drop(retained_context);
}

#[tokio::test(start_paused = true)]
async fn error_tail_deadline_releases_reservation_without_canceling_a_retry() {
    let (_temp, store) = store();
    let mut body = request(json!([input("retry")]));
    body["client_metadata"] = json!({"session_id":"session","turn_id":"turn"});
    let context = context(&store, body.clone()).await;
    let closed = Arc::new(AtomicBool::new(false));
    let mut frames = Box::pin(translate_reader(
        reader(
            "data: {\"type\":\"error\",\"code\":\"synthetic\"}\n\n",
            closed.clone(),
        ),
        context,
        4096,
    ));
    assert!(frames.next().await.unwrap().unwrap().data_ref().is_some());
    assert_eq!(store.contexts.inner.lock().unwrap().reservations.len(), 1);
    let retry = prepare(&store, body).await.unwrap();
    let retry_id = retry.operation.as_ref().unwrap().0.id.clone();
    assert!(frames.next().await.is_none());
    assert!(closed.load(Ordering::SeqCst));
    let inner = store.contexts.inner.lock().unwrap();
    assert!(inner.reservations.is_empty());
    assert_eq!(inner.operations.len(), 1);
    assert!(inner.operations.contains_key(&retry_id));
}

#[tokio::test]
async fn dropping_an_unfinished_http_stream_releases_a_shared_operation() {
    let (_temp, store) = store();
    let context = context(&store, request(json!([input("cancel")]))).await;
    let retained_context = context.clone();
    let closed = Arc::new(AtomicBool::new(false));
    let frames = translate_reader(reader(": heartbeat\n\n", closed.clone()), context, 4096);
    assert_eq!(store.contexts.inner.lock().unwrap().operations.len(), 1);
    drop(frames);
    assert!(closed.load(Ordering::SeqCst));
    assert!(store.contexts.inner.lock().unwrap().operations.is_empty());
    drop(retained_context);
}
