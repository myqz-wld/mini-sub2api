use super::*;
use serde_json::json;

fn origin() -> Origin {
    Origin {
        cell: Some("synthetic-cell".into()),
        item: Some("synthetic-item".into()),
        call: None,
        turn: None,
    }
}
fn request() -> Value {
    json!({"input":[{"id":"later-wait",budget::META:{"cell_id":"synthetic-cell","executed_tool_calls":[],"tool_calls_complete":true}}]})
}

#[tokio::test]
async fn revocations_survive_reload_and_are_key_and_source_thread_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().to_path_buf());
    store
        .edit("synthetic-account", "acct_budget", "key-a", |editor| {
            editor.revoke_tool_inventories(&[origin()], "source-thread");
            Ok(())
        })
        .await
        .unwrap();
    drop(store);
    let store = RequestStateStore::new(temp.path().to_path_buf());
    for (key, thread, revoked) in [
        ("key-a", "source-thread", true),
        ("key-b", "source-thread", false),
        ("key-a", "other-thread", false),
    ] {
        store
            .edit("synthetic-account", "acct_budget", key, move |editor| {
                let mut request = request();
                editor.filter_tool_completeness(&mut request, thread);
                assert_eq!(
                    request["input"][0][budget::META]
                        .get("tool_calls_complete")
                        .is_none(),
                    revoked
                );
                Ok(())
            })
            .await
            .unwrap();
    }
    let paths = std::fs::read_dir(temp.path()).unwrap();
    for path in paths
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|s| s == "json"))
    {
        let raw = std::fs::read_to_string(path).unwrap();
        assert!(!raw.contains("synthetic-cell") && !raw.contains("synthetic-item"));
    }
}

#[tokio::test]
async fn unresolved_origin_loss_remains_unknown_after_reload() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().to_path_buf());
    store
        .edit("synthetic-account", "acct_budget", "key-a", |editor| {
            let mut loss = origin();
            loss.turn = Some("imported-wire-alias-without-owner".into());
            editor.revoke_tool_inventories(&[loss], "source-thread");
            assert!(editor.state.tool_inventory_uncertain);
            Ok(())
        })
        .await
        .unwrap();
    drop(store);
    let store = RequestStateStore::new(temp.path().to_path_buf());
    let text = finalize_frame(
        &store,
        "synthetic-account",
        "acct_budget",
        "key-a",
        "source-thread",
        request(),
        4096,
    )
    .await
    .unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert!(
        value["input"][0][budget::META]
            .get("tool_calls_complete")
            .is_none()
    );
}

#[tokio::test]
async fn already_prepared_frame_rechecks_intervening_wire_loss_without_new_losses() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().to_path_buf());
    let b = store
        .edit("synthetic-account", "acct_budget", "key-a", |editor| {
            let mut b = request();
            editor.filter_tool_completeness(&mut b, "source-thread");
            assert_eq!(b["input"][0][budget::META]["tool_calls_complete"], true);
            Ok(b)
        })
        .await
        .unwrap();
    let mut a = request();
    a["instructions"] = "x".repeat(budget::MESSAGE_BYTES - 2048).into();
    a["input"][0][budget::META]["executed_tool_calls"] =
        json!([{"name":"probe","arguments":{"data":"x".repeat(4096)}}]);
    finalize_frame(
        &store,
        "synthetic-account",
        "acct_budget",
        "key-a",
        "source-thread",
        a,
        budget::MESSAGE_BYTES * 2,
    )
    .await
    .unwrap();
    let text = finalize_frame(
        &store,
        "synthetic-account",
        "acct_budget",
        "key-a",
        "source-thread",
        b,
        4096,
    )
    .await
    .unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert!(
        value["input"][0][budget::META]
            .get("tool_calls_complete")
            .is_none()
    );
}

#[tokio::test]
async fn exhausted_revocation_budget_keeps_completeness_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().to_path_buf());
    store
        .edit("synthetic-account", "acct_budget", "key-a", |editor| {
            for i in 0..=MAX_TOOL_INVENTORY_REVOCATIONS {
                editor.revoke_tool_inventories(
                    &[Origin {
                        cell: Some(format!("cell-{i}")),
                        item: None,
                        call: None,
                        turn: None,
                    }],
                    "thread",
                );
            }
            assert!(editor.state.tool_inventory_uncertain);
            assert!(editor.state.tool_inventory_revocations.is_empty());
            let mut request = request();
            editor.filter_tool_completeness(&mut request, "thread");
            assert!(
                request["input"][0][budget::META]
                    .get("tool_calls_complete")
                    .is_none()
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn legacy_one_anchor_revocations_remain_negative_across_upgrade_and_reload() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    store
        .edit("synthetic-account", "acct_budget", "key-a", |editor| {
            let legacy =
                editor.derived_lookup("tool-item-loss", &[b"source-thread", b"synthetic-item"]);
            editor.state.tool_inventory_revocations.insert(legacy);
            editor.state.tool_inventory_revision = 0;
            editor.changed = true;
            Ok(())
        })
        .await
        .unwrap();
    drop(store);
    for _ in 0..2 {
        let store = RequestStateStore::new(temp.path().into());
        store.edit("synthetic-account", "acct_budget", "key-a", |editor| {
            assert_eq!(editor.state.tool_inventory_revision, 1);
            assert!(editor.state.tool_inventory_uncertain);
            assert_eq!(editor.state.tool_inventory_revocations.len(), 1, "legacy evidence must not be erased");
            let mut replay = json!({"input":[{"call_id":"call-only-replay",budget::META:{"tool_calls_complete":true}}]});
            editor.filter_tool_completeness(&mut replay, "imported-thread");
            assert!(replay["input"][0][budget::META].get("tool_calls_complete").is_none());
            Ok(())
        }).await.unwrap();
    }
}

#[tokio::test]
async fn loss_without_any_usable_anchor_remains_unknown_after_reload() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    store
        .edit("synthetic-account", "acct_budget", "key-a", |editor| {
            editor.revoke_tool_inventories(
                &[Origin {
                    cell: None,
                    item: None,
                    call: None,
                    turn: None,
                }],
                "source-thread",
            );
            assert!(editor.state.tool_inventory_uncertain);
            Ok(())
        })
        .await
        .unwrap();
    drop(store);
    let store = RequestStateStore::new(temp.path().into());
    store
        .edit("synthetic-account", "acct_budget", "key-a", |editor| {
            let mut replay = request();
            editor.filter_tool_completeness(&mut replay, "source-thread");
            assert!(
                replay["input"][0][budget::META]
                    .get("tool_calls_complete")
                    .is_none()
            );
            Ok(())
        })
        .await
        .unwrap();
}
