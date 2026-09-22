use super::*;
use crate::response_sse_translation::{UpstreamByteStream, translated_sse_frames};
use futures_util::{StreamExt, stream};

fn prefix() -> Vec<Value> {
    vec![
        json!({"type":"response.created","response":{"id":"resp_cached"}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_cached","type":"message","role":"assistant","content":[]}}),
        json!({"type":"response.content_part.added","output_index":0,"item_id":"msg_cached","content_index":0,"part":{"type":"output_text","text":""}}),
        delta(),
        delta(),
    ]
}
fn delta() -> Value {
    json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_cached","content_index":0,"delta":"x"})
}
fn item(text: &str) -> Value {
    json!({"id":"msg_cached","type":"message","role":"assistant","content":[{"type":"output_text","text":text}]})
}
async fn context(store: &RequestStateStore) -> ResponseStateContext {
    let prepared = prepare(store, request(json!([input("synthetic start")])))
        .await
        .unwrap();
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

#[tokio::test]
async fn warmed_cache_keeps_terminal_integrity_and_rejects_late_deltas() {
    for valid in [false, true] {
        let (_temp, store) = store();
        let context = context(&store).await;
        for event in prefix() {
            context.translate_value(event).await.unwrap();
        }
        assert!(store.response_cache_metrics().1 > 0);
        context.translate_value(json!({"type":"response.output_text.done","output_index":0,"item_id":"msg_cached","content_index":0,"text":"xx"})).await.unwrap();
        context.translate_value(json!({"type":"response.content_part.done","output_index":0,"item_id":"msg_cached","content_index":0,"part":{"type":"output_text","text":"xx"}})).await.unwrap();
        context
            .translate_value(
                json!({"type":"response.output_item.done","output_index":0,"item":item("xx")}),
            )
            .await
            .unwrap();
        let terminal = json!({"type":"response.completed","response":{"id":"resp_cached","output":[item(if valid {"xx"} else {"wrong"})]}});
        assert_eq!(context.translate_value(terminal).await.is_ok(), valid);
        assert!(context.translate_value(delta()).await.is_err());
        context.update_operation(None).unwrap();
        assert_eq!(store.response_cache_metrics().0, 0);
        assert!(store.contexts.inner.lock().unwrap().operations.is_empty());
    }
}

#[tokio::test]
async fn dropping_a_live_sse_stream_releases_cache_even_with_a_retained_context_clone() {
    let (_temp, store) = store();
    let context = context(&store).await;
    let retained = context.clone();
    let frames = prefix()
        .into_iter()
        .map(|event| Ok(Bytes::from(format!("data: {event}\n\n"))));
    let upstream: UpstreamByteStream = Box::pin(stream::iter(frames).chain(stream::pending()));
    let mut translated = Box::pin(translated_sse_frames(upstream, context, 4096));
    for _ in 0..5 {
        assert!(
            translated
                .next()
                .await
                .unwrap()
                .unwrap()
                .data_ref()
                .is_some()
        );
    }
    assert!(store.response_cache_metrics().0 > 0);
    assert!(store.response_cache_metrics().1 > 0);
    drop(translated);
    assert_eq!(store.response_cache_metrics().0, 0);
    assert!(store.contexts.inner.lock().unwrap().operations.is_empty());
    // The retained context keeps its existing stateless behavior, without resurrecting a closed cache.
    retained.translate_value(delta()).await.unwrap();
    assert_eq!(store.response_cache_metrics().0, 0);
    drop(retained);
}

#[tokio::test]
async fn replacing_an_operation_through_the_builder_releases_existing_cache() {
    let (_temp, store) = store();
    let context = context(&store).await;
    for event in prefix() {
        context.translate_value(event).await.unwrap();
    }
    assert!(store.response_cache_metrics().0 > 0);
    let stateless = context.clone().with_operation(None);
    assert_eq!(store.response_cache_metrics().0, 0);
    stateless.translate_value(delta()).await.unwrap();
    assert_eq!(store.response_cache_metrics().0, 0);
}
