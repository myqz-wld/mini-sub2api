use super::*;
use pretty_assertions::assert_eq;

const TEST_UPSTREAM: &str = "http://127.0.0.1:43123/v1/responses";

#[tokio::test]
async fn explicit_off_mode_persists_only_mode_and_revision() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-secret".to_string(),
            TEST_UPSTREAM.to_string(),
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
    let sidecar = std::fs::read_to_string(fingerprint::sidecar_path(
        &vault.accounts_dir,
        &metadata.account_ref,
    ))
    .expect("sidecar");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&sidecar).expect("sidecar JSON"),
        serde_json::json!({"version": 2, "revision": 1, "mode": "off"})
    );
}

#[tokio::test]
async fn concurrent_materialization_converges_on_one_mode_revision_sidecar() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-secret".to_string(),
            TEST_UPSTREAM.to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("create key");
    let sidecar = fingerprint::sidecar_path(&vault.accounts_dir, &metadata.account_ref);
    std::fs::remove_file(&sidecar).expect("simulate missing sidecar");

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
    assert_eq!(first.mode(), second.mode());
    assert_eq!(first.revision(), second.revision());
    assert_eq!(first.mode(), FingerprintMode::Device);
    assert_eq!(first.revision(), 1);
    assert!(sidecar.is_file());
}

#[tokio::test]
async fn old_corrupt_or_semantically_invalid_sidecar_fails_closed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-secret".to_string(),
            TEST_UPSTREAM.to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("create key");
    let sidecar = fingerprint::sidecar_path(&vault.accounts_dir, &metadata.account_ref);

    for invalid in [
        b"not-json".to_vec(),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "revision": 1,
            "mode": "device",
            "installationId": "11111111-1111-4111-8111-111111111111"
        }))
        .expect("old schema fixture"),
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "revision": 0,
            "mode": "device"
        }))
        .expect("invalid revision fixture"),
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "revision": 1,
            "mode": "device",
            "installationId": "state-must-not-return"
        }))
        .expect("unexpected field fixture"),
    ] {
        std::fs::write(&sidecar, &invalid).expect("write invalid sidecar");
        assert!(vault.lock_record(&metadata.account_ref).await.is_err());
        assert_eq!(std::fs::read(&sidecar).expect("unchanged sidecar"), invalid);
    }
}

#[tokio::test]
async fn mode_changes_increment_revision_without_identity_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-secret".to_string(),
            TEST_UPSTREAM.to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("create key");

    let off = vault
        .set_fingerprint_mode(&metadata.account_ref, FingerprintMode::Off)
        .await
        .expect("switch off");
    assert_eq!(off.mode, FingerprintMode::Off);
    assert_eq!(off.revision, 2);
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
}

#[tokio::test]
async fn revision_overflow_fails_without_changing_sidecar() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-secret".to_string(),
            TEST_UPSTREAM.to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("create key");
    let sidecar = fingerprint::sidecar_path(&vault.accounts_dir, &metadata.account_ref);
    let exhausted = serde_json::to_vec(&serde_json::json!({
        "version": 2,
        "revision": u64::MAX,
        "mode": "device"
    }))
    .expect("exhausted fixture");
    std::fs::write(&sidecar, &exhausted).expect("write exhausted revision");

    assert!(
        vault
            .set_fingerprint_mode(&metadata.account_ref, FingerprintMode::Off)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(sidecar).expect("unchanged sidecar"),
        exhausted
    );
}

#[tokio::test]
async fn secret_record_rewrite_and_restore_keep_mode_revision_sidecar() {
    let source = tempfile::tempdir().expect("source tempdir");
    let source_vault = Vault::open(source.path().join("vault")).expect("source vault");
    let metadata = source_vault
        .create_api_key(
            "upstream-secret".to_string(),
            TEST_UPSTREAM.to_string(),
            FingerprintMode::Off,
        )
        .await
        .expect("create key");
    let locked = source_vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("record");
    let record = locked.record.clone();
    drop(locked);
    write_record_atomically(
        &source_vault.accounts_dir,
        &record_path(&source_vault.accounts_dir, &metadata.account_ref).expect("record path"),
        &record,
    )
    .expect("rewrite record");

    let restored = tempfile::tempdir().expect("restored tempdir");
    let restored_state = restored.path().join("vault");
    let restored_accounts = restored_state.join("accounts");
    std::fs::create_dir_all(&restored_accounts).expect("restored accounts");
    for source_path in [
        record_path(&source_vault.accounts_dir, &metadata.account_ref).expect("source record"),
        fingerprint::sidecar_path(&source_vault.accounts_dir, &metadata.account_ref),
    ] {
        std::fs::copy(
            &source_path,
            restored_accounts.join(source_path.file_name().expect("file name")),
        )
        .expect("copy state file");
    }
    let restored_vault = Vault::open(restored_state).expect("open restored vault");
    let restored_fingerprint = restored_vault
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("restored fingerprint");
    assert_eq!(restored_fingerprint.mode(), FingerprintMode::Off);
    assert_eq!(restored_fingerprint.revision(), 1);
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
            TEST_UPSTREAM.to_string(),
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
