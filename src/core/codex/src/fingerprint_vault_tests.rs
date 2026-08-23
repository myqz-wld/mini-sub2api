use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn explicit_off_mode_persists_without_disclosing_installation_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-secret".to_string(),
            "http://127.0.0.1:43123/v1/responses".to_string(),
            FingerprintMode::Off,
        )
        .await
        .expect("create key");

    let safe = vault
        .fingerprint_metadata(&metadata.account_ref)
        .await
        .expect("fingerprint metadata");
    assert_eq!(safe.mode, FingerprintMode::Off);
    assert_eq!(safe.revision, 1);
    let serialized = serde_json::to_string(&safe).expect("safe metadata");
    assert!(!serialized.contains("installation"));
}

#[tokio::test]
async fn concurrent_legacy_materialization_converges_on_one_uuid() {
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
    std::fs::remove_file(&sidecar).expect("simulate legacy credential");

    let first_vault = vault.clone();
    let first_ref = metadata.account_ref.clone();
    let second_vault = vault.clone();
    let second_ref = metadata.account_ref.clone();
    let (first, second) = tokio::join!(
        async move {
            first_vault
                .fingerprint_snapshot(&first_ref)
                .await
                .expect("first materialization")
        },
        async move {
            second_vault
                .fingerprint_snapshot(&second_ref)
                .await
                .expect("second materialization")
        }
    );
    assert!(first.installation_id() == second.installation_id());
    assert_eq!(first.mode(), FingerprintMode::Device);
    assert_eq!(first.revision(), 1);
    assert!(sidecar.is_file());
}

#[tokio::test]
async fn corrupt_or_semantically_invalid_sidecar_fails_closed() {
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

    for invalid in [
        b"not-json".to_vec(),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "revision": 0,
            "mode": "device",
            "installationId": Uuid::new_v4().to_string()
        }))
        .expect("invalid revision fixture"),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "revision": 1,
            "mode": "device",
            "installationId": Uuid::nil().to_string()
        }))
        .expect("non-v4 fixture"),
    ] {
        std::fs::write(&sidecar, &invalid).expect("write corrupt sidecar");
        assert!(vault.lock_record(&metadata.account_ref).await.is_err());
        assert!(std::fs::read(&sidecar).expect("unchanged sidecar") == invalid);
    }
}

#[tokio::test]
async fn mode_changes_increment_revision_without_rotating_device() {
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
    let initial = vault
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("initial fingerprint");

    let off = vault
        .set_fingerprint_mode(&metadata.account_ref, FingerprintMode::Off)
        .await
        .expect("switch off");
    assert_eq!(off.mode, FingerprintMode::Off);
    assert_eq!(off.revision, 2);
    let after_off = vault
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("off fingerprint");
    assert!(initial.installation_id() == after_off.installation_id());

    let no_op = vault
        .set_fingerprint_mode(&metadata.account_ref, FingerprintMode::Off)
        .await
        .expect("same-mode no-op");
    assert_eq!(no_op.revision, 2);
    let device = vault
        .set_fingerprint_mode(&metadata.account_ref, FingerprintMode::Device)
        .await
        .expect("switch device");
    assert_eq!(device.mode, FingerprintMode::Device);
    assert_eq!(device.revision, 3);
    let final_snapshot = vault
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("final fingerprint");
    assert!(initial.installation_id() == final_snapshot.installation_id());
}

#[tokio::test]
async fn revision_overflow_fails_without_changing_sidecar() {
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
    let current = vault
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("fingerprint");
    let sidecar = fingerprint::sidecar_path(&vault.accounts_dir, &metadata.account_ref);
    let exhausted = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "revision": u64::MAX,
        "mode": "device",
        "installationId": current.installation_id()
    }))
    .expect("exhausted fixture");
    std::fs::write(&sidecar, &exhausted).expect("write exhausted revision");

    assert!(
        vault
            .set_fingerprint_mode(&metadata.account_ref, FingerprintMode::Off)
            .await
            .is_err()
    );
    assert!(std::fs::read(sidecar).expect("unchanged sidecar") == exhausted);
}

#[tokio::test]
async fn secret_record_rewrite_keeps_the_sidecar_device() {
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
        .expect("record");
    let installation_id = locked.fingerprint().installation_id().to_string();
    let record = locked.record.clone();
    drop(locked);

    write_record_atomically(
        &vault.accounts_dir,
        &record_path(&vault.accounts_dir, &metadata.account_ref).expect("record path"),
        &record,
    )
    .expect("simulate older binary rewrite");
    let reopened = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("reopened record");
    assert!(installation_id == reopened.fingerprint().installation_id());
}

#[tokio::test]
async fn copying_record_and_sidecar_preserves_device_on_restore() {
    let source = tempfile::tempdir().expect("source tempdir");
    let source_vault = Vault::open(source.path().join("vault")).expect("source vault");
    let metadata = source_vault
        .create_api_key(
            "upstream-secret".to_string(),
            "http://127.0.0.1:43123/v1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("create key");
    let installation_id = source_vault
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("source fingerprint")
        .installation_id()
        .to_string();

    let restored = tempfile::tempdir().expect("restored tempdir");
    let restored_state = restored.path().join("vault");
    let restored_accounts = restored_state.join("accounts");
    std::fs::create_dir_all(&restored_accounts).expect("restored accounts");
    std::fs::copy(
        record_path(&source_vault.accounts_dir, &metadata.account_ref).expect("source record"),
        record_path(&restored_accounts, &metadata.account_ref).expect("restored record"),
    )
    .expect("copy record");
    std::fs::copy(
        fingerprint::sidecar_path(&source_vault.accounts_dir, &metadata.account_ref),
        fingerprint::sidecar_path(&restored_accounts, &metadata.account_ref),
    )
    .expect("copy sidecar");

    let restored_vault = Vault::open(restored_state).expect("open restored vault");
    let restored_fingerprint = restored_vault
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("restored fingerprint");
    assert!(installation_id == restored_fingerprint.installation_id());
}

#[tokio::test]
async fn removing_then_recreating_a_credential_allocates_a_new_device() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let first = vault
        .create_api_key(
            "first-upstream-secret".to_string(),
            "http://127.0.0.1:43123/v1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("first key");
    let first_device = vault
        .fingerprint_snapshot(&first.account_ref)
        .await
        .expect("first fingerprint")
        .installation_id()
        .to_string();
    vault
        .remove(&first.account_ref, RemovalKind::ServiceOnly)
        .await
        .expect("remove first");

    let second = vault
        .create_api_key(
            "second-upstream-secret".to_string(),
            "http://127.0.0.1:43123/v1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("second key");
    let second_device = vault
        .fingerprint_snapshot(&second.account_ref)
        .await
        .expect("second fingerprint")
        .installation_id()
        .to_string();
    assert!(first.account_ref != second.account_ref);
    assert!(first_device != second_device);
}

#[test]
fn startup_cleans_creation_crash_sidecar_without_a_record() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let account_ref = "acct_creation_crash";
    let sidecar = fingerprint::sidecar_path(&vault.accounts_dir, account_ref);
    fingerprint::create(&vault.accounts_dir, account_ref, FingerprintMode::Device)
        .expect("orphan sidecar");
    assert!(sidecar.is_file());
    drop(vault);

    Vault::open(temp.path().to_path_buf()).expect("reopen and recover");
    assert!(!sidecar.exists());
}

#[tokio::test]
async fn startup_cleans_sidecar_after_older_binary_removal_receipt() {
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
    let record = record_path(&vault.accounts_dir, &metadata.account_ref).expect("record path");
    let receipt = RemovalReceipt {
        account_ref: metadata.account_ref.clone(),
        kind: RemovalKind::ServiceOnly,
        completed_at: Utc::now(),
    };
    write_json_atomically(
        &vault.accounts_dir,
        &receipt_path(&vault.accounts_dir, &metadata.account_ref).expect("receipt path"),
        &receipt,
    )
    .expect("older receipt");
    std::fs::remove_file(record).expect("older record removal");
    assert!(sidecar.is_file());
    drop(vault);

    let recovered = Vault::open(temp.path().to_path_buf()).expect("reopen and recover");
    assert!(!sidecar.exists());
    assert_eq!(
        recovered
            .removal_receipt(&metadata.account_ref)
            .await
            .expect("receipt")
            .expect("present receipt"),
        receipt
    );
}
