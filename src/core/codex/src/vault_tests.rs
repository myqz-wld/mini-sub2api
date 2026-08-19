use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn api_key_record_is_private_and_round_trips() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-secret".to_string(),
            "http://127.0.0.1:43123/v1/responses".to_string(),
        )
        .await
        .expect("create key");

    let locked = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("lock record");
    assert_eq!(locked.record.metadata(), metadata);
    match &locked.record.material {
        CredentialMaterial::OpenAiApiKey { api_key } => {
            assert_eq!(api_key, "upstream-secret")
        }
        CredentialMaterial::CodexOAuth { .. } => panic!("wrong material"),
    }
    drop(locked);

    let mut locked = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("lock for atomic update");
    locked.record.status = CredentialStatus::RequiresLogin;
    locked.persist().await.expect("atomic update");
    drop(locked);
    let account_files = std::fs::read_dir(temp.path().join("accounts"))
        .expect("account directory")
        .map(|entry| entry.expect("directory entry").file_name())
        .collect::<Vec<_>>();
    assert!(
        account_files
            .iter()
            .all(|name| !name.to_string_lossy().starts_with(".credential-"))
    );
    let locked = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("updated record");
    assert_eq!(locked.record.status, CredentialStatus::RequiresLogin);

    #[cfg(unix)]
    {
        let path = temp
            .path()
            .join("accounts")
            .join(format!("{}.json", metadata.account_ref));
        let mode = std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn second_instance_lock_is_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let _first = vault.acquire_instance_lock().expect("first lock");
    assert!(vault.acquire_instance_lock().is_err());
}

#[tokio::test]
async fn removal_is_idempotent_and_leaves_a_non_secret_receipt() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-secret".to_string(),
            "http://127.0.0.1:43123/v1/responses".to_string(),
        )
        .await
        .expect("create key");

    let first = vault
        .remove(&metadata.account_ref, RemovalKind::ServiceOnly)
        .await
        .expect("first removal");
    let second = vault
        .remove(&metadata.account_ref, RemovalKind::ServiceOnly)
        .await
        .expect("idempotent removal");
    assert_eq!(first, second);
    assert_eq!(first.kind, RemovalKind::ServiceOnly);
    assert!(vault.lock_record(&metadata.account_ref).await.is_err());

    let receipt = vault
        .removal_receipt(&metadata.account_ref)
        .await
        .expect("receipt read")
        .expect("receipt");
    assert_eq!(receipt, first);
    let receipt_path = temp
        .path()
        .join("accounts")
        .join(format!("{}.removal.json", metadata.account_ref));
    let contents = std::fs::read_to_string(receipt_path).expect("receipt contents");
    assert!(!contents.contains("upstream-secret"));
    assert!(
        vault
            .remove(&metadata.account_ref, RemovalKind::OAuthRevoked)
            .await
            .is_err()
    );
}
