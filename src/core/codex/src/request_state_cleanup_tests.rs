use super::*;
use crate::fingerprint::FingerprintMode;
use crate::vault::{CredentialMaterial, Vault};

#[tokio::test]
async fn cleanup_rechecks_owners_after_a_stale_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let vault = Vault::open(temp.path().to_path_buf()).unwrap();
    let accounts = temp.path().join("accounts");
    let store = vault.request_state();
    let namespace = "cleanup-race";
    store
        .edit(namespace, "acct_orphan", "scope", |editor| {
            editor.installation_id(FingerprintMode::Device, None)
        })
        .await
        .unwrap();
    // Pause at the actual cleanup scan/lock boundary, then publish a new real credential and state.
    let stale = active_state_refs(&accounts).unwrap();
    let owner = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: "synthetic".into(),
                access_token: "synthetic".into(),
                refresh_token: "synthetic".into(),
                account_id: namespace.into(),
                access_expires_at: None,
                issuer: "http://127.0.0.1:1".into(),
                client_id: "synthetic".into(),
            },
            "http://127.0.0.1:1/responses".into(),
            FingerprintMode::Device,
        )
        .await
        .unwrap();
    let identity = store
        .edit(namespace, &owner.account_ref, "scope", |editor| {
            let key = editor.lookup("conversation", "session");
            Ok(editor.conversation(&key)?.id)
        })
        .await
        .unwrap();
    let path = store.state_path_for_test(namespace);
    let before = std::fs::read(&path).unwrap();
    cleanup_unowned_request_states(&accounts, &stale).unwrap();
    assert!(path.exists(), "cleanup deleted newly owned state");
    assert!(
        std::fs::read(&path).unwrap() == before,
        "cleanup changed live state"
    );
    let after = store
        .edit(namespace, &owner.account_ref, "scope", |editor| {
            let key = editor.lookup("conversation", "session");
            Ok(editor.conversation(&key)?.id)
        })
        .await
        .unwrap();
    assert_eq!(identity, after);
}

#[tokio::test]
async fn cleanup_preserves_state_when_new_owner_evidence_is_unreadable() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().to_path_buf());
    store
        .edit("corrupt-owner", "acct_old", "scope", |editor| {
            editor.installation_id(FingerprintMode::Device, None)
        })
        .await
        .unwrap();
    let stale = active_state_refs(temp.path()).unwrap();
    std::fs::write(temp.path().join("acct_new.json"), b"{invalid").unwrap();
    assert!(cleanup_unowned_request_states(temp.path(), &stale).is_err());
    assert!(store.state_path_for_test("corrupt-owner").exists());
}
