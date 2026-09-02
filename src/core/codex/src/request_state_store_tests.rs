use super::*;
use crate::fingerprint::FingerprintMode;
use crate::request_state_lookup::LookupKeyFactory;
use crate::request_state_types::MAX_SCOPE_CONVERSATIONS;
use crate::request_state_types::PersistedRequestState;
use crate::request_state_types::WireIdDomain;
use crate::vault::CredentialMaterial;
use crate::vault::RemovalKind;
use crate::vault::Vault;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

const NAMESPACE: &str = "chatgpt-account-test";
const OWNER: &str = "acct_request_state_test";
const SCOPE: &str = "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const DAY_MS: i64 = 86_400_000;

fn store() -> (TempDir, RequestStateStore) {
    let temp = TempDir::new().expect("temp dir");
    let accounts = temp.path().join("accounts");
    fs::create_dir(&accounts).expect("accounts dir");
    (temp, RequestStateStore::new(accounts))
}

fn keys(scope: &str) -> LookupKeyFactory {
    LookupKeyFactory::new(NAMESPACE, scope)
}

fn persisted(store: &RequestStateStore) -> PersistedRequestState {
    let bytes = fs::read(store.state_path_for_test(NAMESPACE)).expect("request state");
    serde_json::from_slice(&bytes).expect("request state JSON")
}

#[tokio::test]
async fn assigns_true_uuid_versions_and_reuses_them_after_restart() {
    let (_temp, store) = store();
    let conversation_key = keys(SCOPE).identity("conversation", "real-conversation-id");
    let turn_key = keys(SCOPE).identity("turn", "real-turn-id");
    let first = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, {
            let conversation_key = conversation_key.clone();
            let turn_key = turn_key.clone();
            move |editor| {
                let installation = editor.installation_id(FingerprintMode::Device, None)?;
                let conversation = editor.conversation(&conversation_key)?;
                let turn = editor.turn(&turn_key, &conversation.id, None, None)?;
                Ok((installation, conversation.id, turn.id))
            }
        })
        .await
        .expect("first state edit");

    assert_eq!(
        Uuid::parse_str(&first.0)
            .expect("installation")
            .get_version_num(),
        4
    );
    assert_eq!(
        Uuid::parse_str(&first.1)
            .expect("conversation")
            .get_version_num(),
        7
    );
    assert_eq!(
        Uuid::parse_str(&first.2).expect("turn").get_version_num(),
        7
    );

    let reopened = RequestStateStore::new(store.accounts_dir.clone());
    let second = reopened
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, move |editor| {
            let installation = editor.installation_id(FingerprintMode::Device, None)?;
            let conversation = editor.conversation(&conversation_key)?;
            let turn = editor.turn(&turn_key, &conversation.id, None, None)?;
            Ok((installation, conversation.id, turn.id))
        })
        .await
        .expect("reopened state edit");
    assert_eq!(first, second);

    let state_path = reopened.state_path_for_test(NAMESPACE);
    let name = state_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("state filename");
    assert!(name.starts_with("rs_") && name.ends_with(".request-state.json"));
    assert!(!name.contains(NAMESPACE));
    assert_eq!(
        persisted(&reopened).version,
        crate::request_state_types::REQUEST_STATE_VERSION
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&state_path)
                .expect("state metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let text = fs::read_to_string(state_path).expect("state text");
    assert!(!text.contains("real-conversation-id"));
    assert!(!text.contains("real-turn-id"));
}

#[tokio::test]
async fn device_converges_across_scopes_while_off_stays_scoped() {
    let (_temp, store) = store();
    let other_scope = "psn_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
    let first_key = keys(SCOPE).identity("installation", "caller-device");
    let other_key = keys(other_scope).identity("installation", "caller-device");

    let device_one = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, move |editor| {
            editor.installation_id(FingerprintMode::Device, None)
        })
        .await
        .expect("device one");
    let device_two = store
        .edit_at(NAMESPACE, OWNER, other_scope, DAY_MS, move |editor| {
            editor.installation_id(FingerprintMode::Device, None)
        })
        .await
        .expect("device two");
    assert_eq!(device_one, device_two);

    let off_one = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, move |editor| {
            editor.installation_id(FingerprintMode::Off, Some(&first_key))
        })
        .await
        .expect("off one");
    let off_two = store
        .edit_at(NAMESPACE, OWNER, other_scope, DAY_MS, move |editor| {
            editor.installation_id(FingerprintMode::Off, Some(&other_key))
        })
        .await
        .expect("off two");
    assert_ne!(off_one, off_two);
    assert_ne!(off_one, device_one);
    assert_eq!(
        Uuid::parse_str(&off_one)
            .expect("off UUID")
            .get_version_num(),
        4
    );
}

#[tokio::test]
async fn late_compaction_retry_refreshes_and_protects_its_marker() {
    let (_temp, store) = store();
    let conversation_key = keys(SCOPE).identity("conversation", "compaction-session");
    let marker_key = keys(SCOPE).identity("compaction", "compaction-operation");
    let first_window = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, {
            let conversation_key = conversation_key.clone();
            let marker_key = marker_key.clone();
            move |editor| {
                let conversation = editor.conversation(&conversation_key)?;
                editor.apply_compaction(&marker_key, &conversation.id)
            }
        })
        .await
        .expect("first compaction");
    assert_eq!(first_window, 1);

    let late_day = 32 * DAY_MS;
    let late_retry = store
        .edit_at(NAMESPACE, OWNER, SCOPE, late_day, {
            let conversation_key = conversation_key.clone();
            let marker_key = marker_key.clone();
            move |editor| {
                let conversation = editor.conversation(&conversation_key)?;
                editor.apply_compaction(&marker_key, &conversation.id)
            }
        })
        .await
        .expect("late compaction retry");
    assert_eq!(late_retry, first_window);

    let state = persisted(&store);
    let scope = state.scopes.values().next().expect("persisted scope");
    assert_eq!(
        scope
            .compaction_markers
            .get(&marker_key)
            .expect("retained marker")
            .last_seen_day,
        32
    );

    let immediate_retry = store
        .edit_at(NAMESPACE, OWNER, SCOPE, late_day, move |editor| {
            let conversation = editor.conversation(&conversation_key)?;
            editor.apply_compaction(&marker_key, &conversation.id)
        })
        .await
        .expect("immediate compaction retry");
    assert_eq!(immediate_retry, first_window);
    let state = persisted(&store);
    let scope = state.scopes.values().next().expect("persisted scope");
    assert_eq!(scope.conversations.len(), 1);
    assert_eq!(
        scope.conversations.values().next().unwrap().window_number,
        1
    );
    assert_eq!(scope.compaction_markers.len(), 1);
}

#[tokio::test]
async fn wire_pairs_translate_transparently_in_both_directions() {
    let (_temp, store) = store();
    let upstream_alias = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, |editor| {
            editor.wire_from_downstream(WireIdDomain::Response, "resp_downstream_real")
        })
        .await
        .expect("downstream mapping");
    assert_ne!(upstream_alias, "resp_downstream_real");
    assert!(upstream_alias.starts_with("resp_"));

    let restored = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, {
            let upstream_alias = upstream_alias.clone();
            move |editor| editor.wire_from_upstream(WireIdDomain::Response, &upstream_alias)
        })
        .await
        .expect("restore caller ID");
    assert_eq!(restored, "resp_downstream_real");

    let downstream_alias = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, |editor| {
            editor.wire_from_upstream(WireIdDomain::Call, "call_provider_real")
        })
        .await
        .expect("provider mapping");
    assert_ne!(downstream_alias, "call_provider_real");
    let provider_restored = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, {
            let downstream_alias = downstream_alias.clone();
            move |editor| editor.wire_from_downstream(WireIdDomain::Call, &downstream_alias)
        })
        .await
        .expect("restore provider ID");
    assert_eq!(provider_restored, "call_provider_real");

    let provider_installation = Uuid::new_v4().to_string();
    let provider_session = Uuid::now_v7().to_string();
    let identity_aliases = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, {
            let provider_installation = provider_installation.clone();
            let provider_session = provider_session.clone();
            move |editor| {
                Ok((
                    editor
                        .wire_from_upstream(WireIdDomain::Installation, &provider_installation)?,
                    editor.wire_from_upstream(WireIdDomain::Session, &provider_session)?,
                ))
            }
        })
        .await
        .expect("identity aliases");
    assert_eq!(
        Uuid::parse_str(&identity_aliases.0)
            .expect("installation alias")
            .get_version_num(),
        4
    );
    assert_eq!(
        Uuid::parse_str(&identity_aliases.1)
            .expect("session alias")
            .get_version_num(),
        7
    );

    let text = fs::read_to_string(store.state_path_for_test(NAMESPACE)).expect("state text");
    assert!(text.contains("resp_downstream_real"));
    assert!(text.contains("call_provider_real"));
    assert!(!text.contains("request body must never be stored"));
}

#[tokio::test]
async fn stable_same_day_hits_do_not_rewrite_but_next_day_does() {
    let (_temp, store) = store();
    let conversation_key = keys(SCOPE).identity("conversation", "conversation");
    let first = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, {
            let conversation_key = conversation_key.clone();
            move |editor| editor.conversation(&conversation_key).map(|entry| entry.id)
        })
        .await
        .expect("first edit");
    let before = fs::read(store.state_path_for_test(NAMESPACE)).expect("before bytes");
    let before_revision = persisted(&store).revision;

    let same = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS + 1, {
            let conversation_key = conversation_key.clone();
            move |editor| editor.conversation(&conversation_key).map(|entry| entry.id)
        })
        .await
        .expect("same day edit");
    assert_eq!(first, same);
    assert_eq!(
        before,
        fs::read(store.state_path_for_test(NAMESPACE)).expect("same bytes")
    );

    store
        .edit_at(NAMESPACE, OWNER, SCOPE, 2 * DAY_MS, move |editor| {
            editor.conversation(&conversation_key).map(|_| ())
        })
        .await
        .expect("next day edit");
    assert_eq!(persisted(&store).revision, before_revision + 1);
}

#[tokio::test]
async fn concurrent_edits_do_not_lose_conversations() {
    let (_temp, store) = store();
    let store = Arc::new(store);
    let mut tasks = Vec::new();
    for index in 0..24 {
        let store = Arc::clone(&store);
        let key = keys(SCOPE).identity("conversation", &format!("conversation-{index}"));
        tasks.push(tokio::spawn(async move {
            store
                .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, move |editor| {
                    editor.conversation(&key).map(|entry| entry.id)
                })
                .await
        }));
    }
    let mut ids = BTreeSet::new();
    for task in tasks {
        ids.insert(task.await.expect("task").expect("edit"));
    }
    assert_eq!(ids.len(), 24);
    let state = persisted(&store);
    let scope_key = keys(SCOPE).scope_key();
    assert_eq!(state.scopes[&scope_key].conversations.len(), 24);
}

#[tokio::test]
async fn one_account_handles_many_sessions_and_evicted_identity_pairs_rebuild_cleanly() {
    let (_temp, store) = store();
    let mut assignments = Vec::new();
    for index in 0..=MAX_SCOPE_CONVERSATIONS {
        let raw = format!("conversation-capacity-{index}");
        let key = keys(SCOPE).identity("conversation", &raw);
        let assigned = store
            .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, {
                let raw = raw.clone();
                move |editor| {
                    let conversation = editor.conversation(&key)?;
                    editor.bind_wire_pair(WireIdDomain::Session, &raw, &conversation.id)?;
                    Ok(conversation.id)
                }
            })
            .await
            .expect("conversation edit");
        assignments.push((raw, assigned));
    }
    let state = persisted(&store);
    let scope_key = keys(SCOPE).scope_key();
    let scope = &state.scopes[&scope_key];
    assert_eq!(scope.conversations.len(), MAX_SCOPE_CONVERSATIONS);
    let (evicted_raw, evicted_id) = assignments
        .into_iter()
        .find(|(raw, _)| {
            let key = keys(SCOPE).identity("conversation", raw);
            !scope.conversations.contains_key(&key)
        })
        .expect("one evicted conversation");
    assert!(
        !scope.wire_ids.values().any(|entry| {
            entry.domain == WireIdDomain::Session && entry.upstream_id == evicted_id
        }),
        "evicted conversation left a stale reverse pair"
    );

    let key = keys(SCOPE).identity("conversation", &evicted_raw);
    let rebuilt = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, move |editor| {
            let conversation = editor.conversation(&key)?;
            editor.bind_wire_pair(WireIdDomain::Session, &evicted_raw, &conversation.id)?;
            Ok(conversation.id)
        })
        .await
        .expect("rebuild evicted conversation");
    assert_ne!(rebuilt, evicted_id);
    assert_eq!(
        persisted(&store).scopes[&scope_key].conversations.len(),
        MAX_SCOPE_CONVERSATIONS
    );
}

#[tokio::test]
async fn corrupt_state_fails_closed_without_replacement() {
    let (_temp, store) = store();
    store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, |editor| {
            editor
                .installation_id(FingerprintMode::Device, None)
                .map(|_| ())
        })
        .await
        .expect("materialize state");
    let path = store.state_path_for_test(NAMESPACE);
    fs::write(&path, b"{corrupt").expect("corrupt state");
    let error = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, |editor| {
            editor.installation_id(FingerprintMode::Device, None)
        })
        .await
        .expect_err("corruption must fail");
    assert!(format!("{error:#}").contains("decoding request state"));
    assert_eq!(
        fs::read(path).expect("preserved corrupt state"),
        b"{corrupt"
    );
}

#[tokio::test]
async fn detail_expires_after_thirty_days_but_conversation_identity_remains() {
    let (_temp, store) = store();
    let conversation_key = keys(SCOPE).identity("conversation", "long-lived-conversation");
    let turn_key = keys(SCOPE).identity("turn", "expiring-turn");
    let conversation_id = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, {
            let conversation_key = conversation_key.clone();
            move |editor| {
                let conversation = editor.conversation(&conversation_key)?;
                let turn = editor.turn(&turn_key, &conversation.id, None, None)?;
                editor.set_current_turn(&conversation.id, &turn.id)?;
                editor.wire_from_upstream(WireIdDomain::Response, "resp_expiring")?;
                Ok(conversation.id)
            }
        })
        .await
        .expect("initial detail");
    let initial = persisted(&store);
    let scope_key = keys(SCOPE).scope_key();
    assert_eq!(initial.scopes[&scope_key].wire_ids.len(), 1);

    let retained = store
        .edit_at(NAMESPACE, OWNER, SCOPE, 32 * DAY_MS, move |editor| {
            editor.conversation(&conversation_key).map(|entry| entry.id)
        })
        .await
        .expect("prune detail");
    assert_eq!(retained, conversation_id);
    let pruned = persisted(&store);
    assert!(pruned.scopes[&scope_key].wire_ids.is_empty());
    assert!(pruned.scopes[&scope_key].wire_upstream_index.is_empty());
    assert!(pruned.scopes[&scope_key].turns.is_empty());
    assert!(
        pruned.scopes[&scope_key]
            .conversations
            .values()
            .all(|conversation| conversation.current_turn_id.is_none())
    );
    assert_eq!(pruned.scopes[&scope_key].conversations.len(), 1);
}

#[tokio::test]
async fn duplicate_credentials_share_state_until_the_last_owner_is_removed() {
    let temp = TempDir::new().expect("temp dir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let first = vault
        .create_oauth(
            oauth_material("access-one"),
            "http://127.0.0.1:1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("first credential");
    let second = vault
        .create_oauth(
            oauth_material("access-two"),
            "http://127.0.0.1:1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("second credential");
    let shared_key = keys(SCOPE).identity("conversation", "shared-conversation");

    let first_id = vault
        .request_state()
        .edit_at(NAMESPACE, &first.account_ref, SCOPE, DAY_MS, {
            let shared_key = shared_key.clone();
            move |editor| editor.conversation(&shared_key).map(|entry| entry.id)
        })
        .await
        .expect("first owner state");
    let second_id = vault
        .request_state()
        .edit_at(
            NAMESPACE,
            &second.account_ref,
            SCOPE,
            DAY_MS,
            move |editor| editor.conversation(&shared_key).map(|entry| entry.id),
        )
        .await
        .expect("second owner state");
    assert_eq!(first_id, second_id);

    let path = vault.request_state().state_path_for_test(NAMESPACE);
    let shared: PersistedRequestState =
        serde_json::from_slice(&fs::read(&path).expect("shared state")).expect("shared JSON");
    assert_eq!(shared.owners.len(), 2);
    assert_eq!(shared.scopes.len(), 1);

    vault
        .remove(&first.account_ref, RemovalKind::ServiceOnly)
        .await
        .expect("remove first owner");
    let remaining: PersistedRequestState =
        serde_json::from_slice(&fs::read(&path).expect("remaining state")).expect("remaining JSON");
    assert_eq!(
        remaining.owners,
        BTreeSet::from([second.account_ref.clone()])
    );
    assert_eq!(remaining.scopes.len(), 1);

    vault
        .remove(&second.account_ref, RemovalKind::ServiceOnly)
        .await
        .expect("remove last owner");
    assert!(!path.exists());
}

#[tokio::test]
async fn concurrent_duplicate_owner_removals_leave_no_credential_or_state() {
    let temp = TempDir::new().expect("temp dir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let first = vault
        .create_oauth(
            oauth_material("concurrent-one"),
            "http://127.0.0.1:1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("first credential");
    let second = vault
        .create_oauth(
            oauth_material("concurrent-two"),
            "http://127.0.0.1:1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("second credential");
    for owner in [&first.account_ref, &second.account_ref] {
        vault
            .request_state()
            .edit_at(NAMESPACE, owner, SCOPE, DAY_MS, |editor| {
                editor.installation_id(FingerprintMode::Device, None)
            })
            .await
            .expect("owner state");
    }
    let state_path = vault.request_state().state_path_for_test(NAMESPACE);
    let first_ref = first.account_ref.clone();
    let second_ref = second.account_ref.clone();
    let (first_result, second_result) = tokio::join!(
        vault.remove(&first_ref, RemovalKind::ServiceOnly),
        vault.remove(&second_ref, RemovalKind::ServiceOnly),
    );
    first_result.expect("remove first");
    second_result.expect("remove second");
    assert!(!state_path.exists());
    for account_ref in [first_ref, second_ref] {
        assert!(
            !temp
                .path()
                .join("accounts")
                .join(format!("{account_ref}.json"))
                .exists()
        );
    }
}

#[tokio::test]
async fn corrupt_state_never_blocks_owner_removal_and_final_owner_deletes_it() {
    let temp = TempDir::new().expect("temp dir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let first = vault
        .create_oauth(
            oauth_material("corrupt-one"),
            "http://127.0.0.1:1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("first credential");
    let second = vault
        .create_oauth(
            oauth_material("corrupt-two"),
            "http://127.0.0.1:1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("second credential");
    for owner in [&first.account_ref, &second.account_ref] {
        vault
            .request_state()
            .edit_at(NAMESPACE, owner, SCOPE, DAY_MS, |editor| {
                editor.installation_id(FingerprintMode::Device, None)
            })
            .await
            .expect("owner state");
    }
    let state_path = vault.request_state().state_path_for_test(NAMESPACE);
    let corrupt = b"{not-valid-request-state";
    fs::write(&state_path, corrupt).expect("corrupt state");

    vault
        .remove(&first.account_ref, RemovalKind::ServiceOnly)
        .await
        .expect("corrupt state must not block non-final owner removal");
    assert_eq!(fs::read(&state_path).expect("preserved state"), corrupt);
    assert!(
        !temp
            .path()
            .join("accounts")
            .join(format!("{}.json", first.account_ref))
            .exists()
    );

    vault
        .remove(&second.account_ref, RemovalKind::ServiceOnly)
        .await
        .expect("corrupt state must not block final owner removal");
    assert!(!state_path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_state_is_rejected_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let (temp, store) = store();
    let victim = temp.path().join("victim");
    fs::write(&victim, b"do-not-touch").expect("victim");
    let state_path = store.state_path_for_test(NAMESPACE);
    symlink(&victim, &state_path).expect("state symlink");

    let error = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, |editor| {
            editor.installation_id(FingerprintMode::Device, None)
        })
        .await
        .expect_err("symlink must fail");
    assert!(format!("{error:#}").contains("not a regular file"));
    assert_eq!(fs::read(victim).expect("victim bytes"), b"do-not-touch");
}

#[tokio::test]
async fn oversized_state_is_rejected_and_preserved() {
    let (_temp, store) = store();
    let path = store.state_path_for_test(NAMESPACE);
    let oversized = vec![b'x'; crate::request_state_types::MAX_REQUEST_STATE_BYTES as usize + 1];
    fs::write(&path, &oversized).expect("oversized fixture");
    let error = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, |editor| {
            editor.installation_id(FingerprintMode::Device, None)
        })
        .await
        .expect_err("oversized state must fail");
    assert!(format!("{error:#}").contains("too large"));
    assert_eq!(
        fs::metadata(path).expect("oversized metadata").len(),
        oversized.len() as u64
    );
}

fn oauth_material(access_token: &str) -> CredentialMaterial {
    CredentialMaterial::CodexOAuth {
        id_token: "id-token".to_string(),
        access_token: access_token.to_string(),
        refresh_token: "refresh-token".to_string(),
        account_id: NAMESPACE.to_string(),
        access_expires_at: None,
        issuer: "http://127.0.0.1:1".to_string(),
        client_id: "request-state-test".to_string(),
    }
}
