use super::*;
use crate::request_state_types::WireIdDomain;
use std::sync::atomic::Ordering;

const NS: &str = "synthetic-namespace";
const OWNER: &str = "acct_cache";
const SCOPE: &str = "synthetic-key";

fn event() -> Value {
    serde_json::json!({"type":"response.reasoning_summary_text.delta", "item_id":"item_up", "delta":"synthetic"})
}
fn response(value: Value, cache: &Option<SharedResponseCache>) -> ResponseEdit {
    ResponseEdit {
        value,
        owner: None,
        compaction: None,
        cache: cache.clone(),
    }
}
async fn translate(
    store: &RequestStateStore,
    cache: &Option<SharedResponseCache>,
) -> Result<Value> {
    store
        .translate_response(NS, OWNER, SCOPE, response(event(), cache), |_| Ok(()))
        .await
}

#[tokio::test]
async fn independent_store_writes_and_in_place_corruption_invalidate_cached_pairs() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let cache = store.response_cache();
    let initial = translate(&store, &cache).await.unwrap();
    let second = RequestStateStore::new(temp.path().into());
    second
        .edit(NS, OWNER, SCOPE, |editor| {
            editor.wire_from_upstream(WireIdDomain::Item, "item_other")
        })
        .await
        .unwrap();
    assert_eq!(translate(&store, &cache).await.unwrap(), initial);
    assert_eq!(store.response_cache_budget.hits.load(Ordering::Relaxed), 0);
    let path = store.state_path_for_test(NS);
    let valid = std::fs::read(&path).unwrap();
    let mut corrupt = valid.clone();
    corrupt[0] = b'!';
    std::fs::write(&path, &corrupt).unwrap();
    assert!(translate(&store, &cache).await.is_err());
    std::fs::write(&path, valid).unwrap();
    assert_eq!(translate(&store, &cache).await.unwrap(), initial);
}

#[tokio::test]
async fn replacement_deletion_and_revocation_cannot_return_cached_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let cache = store.response_cache();
    let initial = translate(&store, &cache).await.unwrap();
    let path = store.state_path_for_test(NS);
    let replacement = temp.path().join("synthetic-replacement");
    std::fs::write(&replacement, b"{}").unwrap();
    std::fs::rename(replacement, &path).unwrap();
    assert!(translate(&store, &cache).await.is_err());
    std::fs::remove_file(&path).unwrap();
    let recreated = translate(&store, &cache).await.unwrap();
    assert_ne!(initial["item_id"], recreated["item_id"]);
    store
        .remove_credential_owner(&RequestStateStore::state_ref_for_namespace(NS), OWNER)
        .unwrap();
    assert!(!path.exists());
    let after_removal = translate(&store, &cache).await.unwrap();
    assert_ne!(recreated["item_id"], after_removal["item_id"]);
    assert_eq!(store.response_cache_budget.hits.load(Ordering::Relaxed), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn nonprivate_permissions_are_repaired_and_symlinks_never_hit_cache() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let cache = store.response_cache();
    let initial = translate(&store, &cache).await.unwrap();
    let path = store.state_path_for_test(NS);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(translate(&store, &cache).await.unwrap(), initial);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    assert_eq!(store.response_cache_budget.hits.load(Ordering::Relaxed), 0);
    let target = temp.path().join("synthetic-target");
    std::fs::rename(&path, &target).unwrap();
    symlink(&target, &path).unwrap();
    assert!(translate(&store, &cache).await.is_err());
}

#[test]
fn day_rollover_uses_full_transaction_and_refreshes_retention() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let cache = store.response_cache();
    let day = 20_000 * 86_400_000;
    let first = store
        .translate_response_locked(NS, OWNER, SCOPE, day, response(event(), &cache), |_| Ok(()))
        .unwrap();
    let path = store.state_path_for_test(NS);
    store
        .translate_response_locked(NS, OWNER, SCOPE, day, response(event(), &cache), |_| Ok(()))
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        store
            .translate_response_locked(
                NS,
                OWNER,
                SCOPE,
                day + 1,
                response(event(), &cache),
                |_| Ok(())
            )
            .unwrap(),
        first
    );
    assert_eq!(store.response_cache_budget.hits.load(Ordering::Relaxed), 1);
    assert_eq!(
        store
            .translate_response_locked(
                NS,
                OWNER,
                SCOPE,
                day + 86_400_000,
                response(event(), &cache),
                |_| Ok(())
            )
            .unwrap(),
        first
    );
    assert_eq!(store.response_cache_budget.hits.load(Ordering::Relaxed), 1);
    assert_ne!(std::fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn keys_namespaces_and_response_owners_cannot_share_cached_relationships() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let cache = store.response_cache();
    let initial = translate(&store, &cache).await.unwrap();
    let other_key = store
        .translate_response(
            NS,
            OWNER,
            "synthetic-other-key",
            response(event(), &cache),
            |_| Ok(()),
        )
        .await
        .unwrap();
    let other_namespace = store
        .translate_response(
            "synthetic-other-namespace",
            OWNER,
            SCOPE,
            response(event(), &cache),
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_ne!(initial["item_id"], other_key["item_id"]);
    assert_ne!(initial["item_id"], other_namespace["item_id"]);
    let owners = store
        .edit(NS, OWNER, SCOPE, |editor| {
            let a = editor.lookup("conversation", "synthetic-a");
            let b = editor.lookup("conversation", "synthetic-b");
            Ok([editor.conversation(&a)?.id, editor.conversation(&b)?.id])
        })
        .await
        .unwrap();
    for (index, owner) in owners.into_iter().enumerate() {
        let mut input = response(event(), &cache);
        input.value["response_id"] = "resp_up".into();
        input.owner = Some(WireIdOwner {
            session_id: owner.clone(),
            thread_id: owner,
        });
        let result = store
            .translate_response(NS, OWNER, SCOPE, input, |_| Ok(()))
            .await;
        assert_eq!(result.is_ok(), index == 0);
    }
}

#[tokio::test]
async fn a_replacement_during_full_validation_cannot_seed_unvalidated_file_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    store
        .edit(NS, OWNER, SCOPE, |editor| {
            editor.bind_wire_pair(WireIdDomain::Item, "down_persisted", "item_up")
        })
        .await
        .unwrap();
    let cache = store.response_cache();
    let path = store.state_path_for_test(NS);
    store
        .translate_response(NS, OWNER, SCOPE, response(event(), &cache), move |_| {
            std::fs::write(&path, b"corrupt synthetic replacement")?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(translate(&store, &cache).await.is_err());
}
