use super::*;
use crate::request_state_store::{RequestStateStore, ResponseEdit};
use serde_json::json;

#[test]
fn store_budget_bounds_fixed_cache_storage_and_reclaims_all_reservations() {
    let budget = Arc::new(CacheBudget::with_limit(BASE_BYTES * 3));
    let caches: Vec<_> = (0..3).map(|_| budget.cache().unwrap()).collect();
    assert_eq!(budget.used(), BASE_BYTES * 3);
    assert!(budget.cache().is_none());
    drop(caches);
    assert_eq!(budget.used(), 0);
    assert!(budget.cache().is_some());
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn id_storage_is_bounded_and_does_not_grow_with_delta_bodies() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    store
        .edit("synthetic", "acct_cache", "key", |editor| {
            for index in 0..100 {
                editor.bind_wire_pair(
                    WireIdDomain::Item,
                    &format!("down_{index}_{}", "x".repeat(200)),
                    &format!("up_{index}_{}", "x".repeat(200)),
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let budget = Arc::new(CacheBudget::default());
    let cache = budget.cache().unwrap();
    for index in 0..100 {
        let raw = format!("up_{index}_{}", "x".repeat(200));
        let output = store.translate_response("synthetic", "acct_cache", "key", ResponseEdit {
            value: json!({"type":"response.output_text.delta","item_id":raw,"delta":"synthetic"}),
            owner:None, compaction:None, cache:Some(cache.clone()),
        }, |_| Ok(())).await.unwrap();
        assert_eq!(
            output["item_id"],
            format!("down_{index}_{}", "x".repeat(200))
        );
        let guarded = cache.lock().unwrap();
        assert!(guarded.id_bytes <= MAX_ID_BYTES);
        assert!(guarded.entries.iter().flatten().count() <= MAX_ENTRIES);
        assert!(budget.used() <= BASE_BYTES + MAX_ID_BYTES);
    }
    let before = budget.used();
    for length in [1024, 1024 * 1024] {
        let output = store.translate_response("synthetic", "acct_cache", "key", ResponseEdit {
            value:json!({"type":"response.output_text.delta","item_id":format!("up_99_{}", "x".repeat(200)),"delta":"x".repeat(length)}),
            owner:None, compaction:None, cache:Some(cache.clone()),
        }, |_| Ok(())).await.unwrap();
        assert_eq!(output["delta"].as_str().unwrap().len(), length);
        assert_eq!(budget.used(), before);
    }
    assert_eq!(budget.hits.load(Ordering::Relaxed), 2);
    drop(cache);
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn exhausted_dynamic_budget_falls_back_without_losing_mappings() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let budget = Arc::new(CacheBudget::with_limit(BASE_BYTES));
    let cache = budget.cache();
    let mut previous = None;
    for _ in 0..3 {
        let output = store
            .translate_response(
                "synthetic",
                "acct_cache",
                "key",
                ResponseEdit {
                    value: json!({"type":"response.output_text.delta","item_id":"up","delta":"x"}),
                    owner: None,
                    compaction: None,
                    cache: cache.clone(),
                },
                |_| Ok(()),
            )
            .await
            .unwrap();
        if let Some(previous) = &previous {
            assert_eq!(&output, previous);
        }
        previous = Some(output);
        assert_eq!(budget.used(), BASE_BYTES);
    }
    assert_eq!(budget.hits.load(Ordering::Relaxed), 0);
    drop(cache);
    assert_eq!(budget.used(), 0);
}
