//! Manual, loopback-free CPU benchmark; fixtures contain only synthetic identity pairs.
use super::*;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_lookup::LookupKeyFactory;
use crate::request_state_types::{PersistedRequestState, WireIdDomain};
use std::collections::BTreeSet;
use std::time::Instant;

const NAMESPACE: &str = "synthetic-benchmark-namespace";
const OWNER: &str = "acct_benchmark";
const SCOPE: &str = "synthetic-benchmark-key";

fn process_read_characters() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/io")
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("rchar: ")?.trim().parse().ok())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn large_state() -> (tempfile::TempDir, RequestStateStore, usize) {
    let temporary = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temporary.path().into());
    let now = chrono::Utc::now().timestamp_millis();
    let mut state = PersistedRequestState::new(BTreeSet::from([OWNER.into()]));
    let mut editor = RequestStateEditor::new(
        &mut state,
        LookupKeyFactory::new(NAMESPACE, SCOPE),
        OWNER,
        now / 86_400_000,
        now,
    )
    .unwrap();
    let padding = "x".repeat(192);
    for index in 0..18_000 {
        editor
            .bind_wire_pair(
                WireIdDomain::Item,
                &format!("item_synthetic_down_{index}_{padding}"),
                &format!("item_synthetic_up_{index}_{padding}"),
            )
            .unwrap();
    }
    for index in 0..2 {
        editor
            .bind_wire_pair(
                WireIdDomain::Item,
                &format!("item_public_hot_{index}"),
                &format!("item_provider_hot_{index}"),
            )
            .unwrap();
    }
    editor.finish();
    state.validate().unwrap();
    let bytes = serde_json::to_vec(&state).unwrap();
    let path = store.state_path_for_test(NAMESPACE);
    std::fs::write(&path, &bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    (temporary, store, bytes.len())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual resource benchmark; run in release mode"]
async fn benchmark_large_identity_state_deltas() {
    let (_temporary, store, bytes) = large_state();
    for concurrency in [1, 2] {
        let mut contexts = Vec::new();
        for index in 0..concurrency {
            let context = ResponseStateContext::new(OWNER, NAMESPACE, SCOPE, &store, None, None);
            let event = serde_json::json!({"type":"response.output_text.delta",
                "item_id":format!("item_provider_hot_{index}"), "delta":"synthetic text"});
            context.translate_value(event.clone()).await.unwrap();
            contexts.push((index, context, event));
        }
        let read_before = process_read_characters();
        let started = Instant::now();
        let metrics_before = store.response_cache_metrics();
        let mut tasks = Vec::new();
        for (index, context, event) in contexts {
            tasks.push(tokio::spawn(async move {
                for _ in 0..12 {
                    let result = context.translate_value(event.clone()).await.unwrap();
                    assert_eq!(result["item_id"], format!("item_public_hot_{index}"));
                    assert_eq!(result["delta"], "synthetic text");
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        let elapsed = started.elapsed();
        let metrics_after = store.response_cache_metrics();
        assert_eq!(
            metrics_after.0, 0,
            "request caches must release their budget"
        );
        assert_eq!(metrics_after.1 - metrics_before.1, concurrency * 12);
        assert_eq!(metrics_after.2, metrics_before.2);
        let read_delta = read_before
            .zip(process_read_characters())
            .map(|(before, after)| after - before);
        if let Some(read_delta) = read_delta {
            assert!(
                read_delta < bytes as u64,
                "warmed deltas must not reread the ledger"
            );
        }
        println!(
            "identity_cache_resources reserved_bytes={} released_bytes={} read_characters={read_delta:?}",
            metrics_before.0, metrics_after.0
        );
        println!(
            "identity_delta_benchmark state_bytes={bytes} concurrency={concurrency} events={} elapsed_ms={:.3} milliseconds_per_event={:.3}",
            concurrency * 12,
            elapsed.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1000.0 / (concurrency * 12) as f64
        );
    }
}
