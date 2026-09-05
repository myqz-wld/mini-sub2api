use super::*;
use std::assert_eq;

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
async fn duplicate_api_keys_use_isolated_account_ref_namespaces_and_remove_independently() {
    let temp = TempDir::new().expect("temp dir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let first = vault
        .create_api_key(
            "same-secret".to_string(),
            "http://127.0.0.1:1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("first API key credential");
    let second = vault
        .create_api_key(
            "same-secret".to_string(),
            "http://127.0.0.1:1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("second API key credential");
    let logical_key = LookupKeyFactory::new(&first.account_ref, SCOPE)
        .identity("conversation", "same-downstream-session");
    let first_id = vault
        .request_state()
        .edit_at(&first.account_ref, &first.account_ref, SCOPE, DAY_MS, {
            let logical_key = logical_key.clone();
            move |editor| editor.conversation(&logical_key).map(|entry| entry.id)
        })
        .await
        .expect("first API key state");
    let second_id = vault
        .request_state()
        .edit_at(
            &second.account_ref,
            &second.account_ref,
            SCOPE,
            DAY_MS,
            move |editor| editor.conversation(&logical_key).map(|entry| entry.id),
        )
        .await
        .expect("second API key state");
    assert_ne!(first_id, second_id);

    let first_path = vault
        .request_state()
        .state_path_for_test(&first.account_ref);
    let second_path = vault
        .request_state()
        .state_path_for_test(&second.account_ref);
    assert_ne!(first_path, second_path);
    assert!(first_path.is_file());
    assert!(second_path.is_file());

    Vault::open(temp.path().to_path_buf()).expect("active API key state survives startup cleanup");
    assert!(first_path.is_file());
    assert!(second_path.is_file());

    vault
        .remove(&first.account_ref, RemovalKind::ServiceOnly)
        .await
        .expect("remove first API key state owner");
    assert!(!first_path.exists());
    assert!(second_path.is_file());
    vault
        .remove(&second.account_ref, RemovalKind::ServiceOnly)
        .await
        .expect("remove second API key state owner");
    assert!(!second_path.exists());
}

#[tokio::test]
async fn corrupt_api_key_state_does_not_block_final_credential_removal() {
    let temp = TempDir::new().expect("temp dir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let credential = vault
        .create_api_key(
            "api-key-secret".to_string(),
            "http://127.0.0.1:1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("API key credential");
    vault
        .request_state()
        .edit_at(
            &credential.account_ref,
            &credential.account_ref,
            SCOPE,
            DAY_MS,
            |editor| editor.installation_id(FingerprintMode::Device, None),
        )
        .await
        .expect("API key state");
    let state_path = vault
        .request_state()
        .state_path_for_test(&credential.account_ref);
    fs::write(&state_path, b"{corrupt-api-key-state").expect("corrupt API key state");

    vault
        .remove(&credential.account_ref, RemovalKind::ServiceOnly)
        .await
        .expect("remove credential despite corrupt state");
    assert!(!state_path.exists());
    assert!(vault.lock_record(&credential.account_ref).await.is_err());
}

#[tokio::test]
async fn startup_removes_api_key_state_after_its_credential_record_is_orphaned() {
    let temp = TempDir::new().expect("temp dir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let credential = vault
        .create_api_key(
            "orphan-secret".to_string(),
            "http://127.0.0.1:1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("API key credential");
    vault
        .request_state()
        .edit_at(
            &credential.account_ref,
            &credential.account_ref,
            SCOPE,
            DAY_MS,
            |editor| editor.installation_id(FingerprintMode::Device, None),
        )
        .await
        .expect("API key state");
    let state_path = vault
        .request_state()
        .state_path_for_test(&credential.account_ref);
    assert!(state_path.is_file());
    fs::remove_file(
        temp.path()
            .join("accounts")
            .join(format!("{}.json", credential.account_ref)),
    )
    .expect("simulate orphaned credential record");

    Vault::open(temp.path().to_path_buf()).expect("startup cleanup");
    assert!(!state_path.exists());
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
    let oversized = crate::request_state_types::MAX_REQUEST_STATE_BYTES + 1;
    fs::File::create(&path)
        .expect("fixture")
        .set_len(oversized)
        .expect("sparse oversized fixture");
    let error = store
        .edit_at(NAMESPACE, OWNER, SCOPE, DAY_MS, |editor| {
            editor.installation_id(FingerprintMode::Device, None)
        })
        .await
        .expect_err("oversized state must fail");
    assert!(format!("{error:#}").contains("too large"));
    assert_eq!(
        fs::metadata(path).expect("oversized metadata").len(),
        oversized
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
