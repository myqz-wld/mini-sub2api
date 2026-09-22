use super::*;
use crate::request_state_types::WireIdDomain;
use std::sync::atomic::Ordering;

const NS: &str = "synthetic-cache-namespace";
const ACCOUNT: &str = "acct_cache";
const SCOPE: &str = "synthetic-key-a";

fn delta(item: &str) -> Value {
    serde_json::json!({"type":"response.output_text.delta", "item_id":item,
        "delta":"synthetic private body", "opaque":{"id":"synthetic opaque id"}})
}

fn edit(value: Value, cache: &Option<SharedResponseCache>) -> ResponseEdit {
    ResponseEdit {
        value,
        owner: None,
        compaction: None,
        cache: cache.clone(),
    }
}

#[tokio::test]
async fn unchanged_deltas_hit_and_preserve_validation_and_opaque_fields() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let cache = store.response_cache();
    let first = store
        .translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &cache), |_| {
            Ok(())
        })
        .await
        .unwrap();
    let path = store.state_path_for_test(NS);
    store
        .translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &cache), |_| {
            Ok(())
        })
        .await
        .unwrap();
    let persisted = std::fs::read(&path).unwrap();
    let stamp = StateStamp::read(&path).unwrap();
    for _ in 0..8 {
        let expected = first.clone();
        let actual = store
            .translate_response(
                NS,
                ACCOUNT,
                SCOPE,
                edit(delta("item_up"), &cache),
                move |value| {
                    assert_eq!(value, &expected);
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert_eq!(actual, first);
    }
    assert_eq!(store.response_cache_budget.hits.load(Ordering::Relaxed), 8);
    assert_eq!(StateStamp::read(&path).unwrap(), stamp);
    assert_eq!(std::fs::read(&path).unwrap(), persisted);
    assert_eq!(first["opaque"]["id"], "synthetic opaque id");
    assert!(
        store
            .translate_response(
                NS,
                ACCOUNT,
                SCOPE,
                edit(delta("item_up"), &cache),
                |_| anyhow::bail!("synthetic validation failure")
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), persisted);
}

#[tokio::test]
async fn concurrent_request_caches_do_not_invalidate_each_other_on_private_reads() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let a = store.response_cache();
    let b = store.response_cache();
    store
        .translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &a), |_| Ok(()))
        .await
        .unwrap();
    store
        .translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &b), |_| Ok(()))
        .await
        .unwrap();
    store
        .translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &a), |_| Ok(()))
        .await
        .unwrap();
    let stamp = StateStamp::read(&store.state_path_for_test(NS)).unwrap();
    let hits = store.response_cache_budget.hits.load(Ordering::Relaxed);
    let left = store.translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &a), |_| Ok(()));
    let right =
        store.translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &b), |_| Ok(()));
    let (left, right) = tokio::join!(left, right);
    assert_eq!(left.unwrap(), right.unwrap());
    assert_eq!(
        store.response_cache_budget.hits.load(Ordering::Relaxed),
        hits + 2
    );
    assert_eq!(
        StateStamp::read(&store.state_path_for_test(NS)).unwrap(),
        stamp
    );
    drop(a);
    drop(b);
    assert_eq!(store.response_cache_budget.used(), 0);
}

#[tokio::test]
async fn unknown_id_commits_before_delivery_and_restart_preserves_mapping() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let cache = store.response_cache();
    let first = store
        .translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &cache), |_| {
            Ok(())
        })
        .await
        .unwrap();
    let reopened = RequestStateStore::new(temp.path().into());
    let id = first["item_id"].as_str().unwrap().to_owned();
    let restored = reopened
        .edit(NS, ACCOUNT, SCOPE, move |editor| {
            editor.required_wire_from_downstream(WireIdDomain::Item, &id, true)
        })
        .await
        .unwrap();
    assert_eq!(restored, "item_up");
    let before = std::fs::read(store.state_path_for_test(NS)).unwrap();
    assert!(
        store
            .translate_response(
                NS,
                ACCOUNT,
                SCOPE,
                edit(delta("item_invalid_new"), &cache),
                |_| anyhow::bail!("synthetic rejected frame")
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(store.state_path_for_test(NS)).unwrap(),
        before
    );
    assert_eq!(store.response_cache_budget.hits.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn partially_cached_frames_and_nested_carriers_use_the_complete_translator() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let cache = store.response_cache();
    let first = store
        .translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &cache), |_| {
            Ok(())
        })
        .await
        .unwrap();
    let mut combined = delta("item_up");
    combined["response_id"] = "resp_new".into();
    let translated = store
        .translate_response(NS, ACCOUNT, SCOPE, edit(combined, &cache), |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(translated["item_id"], first["item_id"]);
    assert_ne!(translated["response_id"], "resp_new");
    let mut nested = delta("item_up");
    nested["item"] = serde_json::json!({"id":"item_nested","type":"message"});
    let translated = store
        .translate_response(NS, ACCOUNT, SCOPE, edit(nested, &cache), |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(translated["item_id"], first["item_id"]);
    assert_ne!(translated["item"]["id"], "item_nested");
    assert_eq!(store.response_cache_budget.hits.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn persistence_failure_does_not_publish_a_cache_entry() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let cache = store.response_cache();
    store
        .translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &cache), |_| {
            Ok(())
        })
        .await
        .unwrap();
    let path = store.state_path_for_test(NS);
    let saved = std::fs::read(&path).unwrap();
    let obstacle = path.clone();
    let rejected = Arc::new(std::sync::Mutex::new(Value::Null));
    let observed = rejected.clone();
    let result = store
        .translate_response(
            NS,
            ACCOUNT,
            SCOPE,
            edit(delta("item_never_committed"), &cache),
            move |value| {
                *observed.lock().unwrap() = value["item_id"].clone();
                std::fs::remove_file(&obstacle)?;
                std::fs::create_dir(&obstacle)?;
                Ok(())
            },
        )
        .await;
    assert!(result.is_err());
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, saved).unwrap();
    let output = store
        .translate_response(
            NS,
            ACCOUNT,
            SCOPE,
            edit(delta("item_never_committed"), &cache),
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_ne!(output["item_id"], *rejected.lock().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_hits_respect_the_cross_process_file_lock() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let cache = store.response_cache();
    store
        .translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &cache), |_| {
            Ok(())
        })
        .await
        .unwrap();
    store
        .translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &cache), |_| {
            Ok(())
        })
        .await
        .unwrap();
    let locked = lock_state(temp.path(), &RequestStateStore::state_ref_for_namespace(NS)).unwrap();
    let pending =
        store.translate_response(NS, ACCOUNT, SCOPE, edit(delta("item_up"), &cache), |_| {
            Ok(())
        });
    tokio::pin!(pending);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut pending)
            .await
            .is_err()
    );
    std::fs::write(store.state_path_for_test(NS), b"invalid synthetic state").unwrap();
    drop(locked);
    assert!(pending.await.is_err());
}
