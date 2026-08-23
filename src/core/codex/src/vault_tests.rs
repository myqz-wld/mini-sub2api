use super::*;
use pretty_assertions::assert_eq;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use uuid::Version;

#[tokio::test]
async fn api_key_record_is_private_and_round_trips() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-secret".to_string(),
            "http://127.0.0.1:43123/v1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("create key");

    let locked = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("lock record");
    assert_eq!(locked.record.metadata(), metadata);
    assert_eq!(locked.fingerprint().mode(), FingerprintMode::Device);
    assert_eq!(locked.fingerprint().revision(), 1);
    let installation_id =
        Uuid::parse_str(locked.fingerprint().installation_id()).expect("valid installation id");
    assert_eq!(installation_id.get_version(), Some(Version::Random));
    assert!(format!("{:?}", locked.fingerprint()).contains("<redacted>"));
    assert!(!format!("{:?}", locked.fingerprint()).contains(&installation_id.to_string()));
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

        let fingerprint_path =
            fingerprint::sidecar_path(&temp.path().join("accounts"), &metadata.account_ref);
        let fingerprint_mode = std::fs::metadata(fingerprint_path)
            .expect("fingerprint metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(fingerprint_mode, 0o600);
    }
}

#[test]
fn second_instance_lock_is_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let _first = vault.acquire_instance_lock().expect("first lock");
    assert!(vault.acquire_instance_lock().is_err());
}

#[test]
fn failed_atomic_record_replace_removes_private_temp_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let accounts_dir = temp.path().join("accounts");
    std::fs::create_dir(&accounts_dir).expect("accounts dir");
    let error = write_json_atomically(
        &accounts_dir,
        &accounts_dir,
        &serde_json::json!({"nonSecret": true}),
    )
    .expect_err("rename over directory must fail");
    assert!(error.to_string().contains("replacing credential record"));
    let files = std::fs::read_dir(&accounts_dir)
        .expect("account directory")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert!(
        files
            .iter()
            .all(|name| !name.to_string_lossy().starts_with(".credential-"))
    );
}

#[tokio::test]
async fn removal_is_idempotent_and_leaves_a_non_secret_receipt() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-secret".to_string(),
            "http://127.0.0.1:43123/v1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("create key");
    let sidecar = fingerprint::sidecar_path(&vault.accounts_dir, &metadata.account_ref);
    assert!(sidecar.is_file());

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
    assert!(!sidecar.exists());

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
