use super::*;
use clap::Parser;

#[test]
fn credential_creation_defaults_to_device() {
    for arguments in [
        vec!["core", "credential", "login"],
        vec![
            "core",
            "credential",
            "import-codex-auth",
            "--auth-file",
            "auth.json",
        ],
        vec!["core", "credential", "add-api-key", "--secret-stdin"],
    ] {
        let parsed = Cli::try_parse_from(arguments).expect("valid creation command");
        let Some(Command::Credential(credential)) = parsed.command else {
            panic!("expected credential command")
        };
        let mode = match credential.command {
            CredentialCommand::Login(args) => args.fingerprint_mode,
            CredentialCommand::ImportCodexAuth(args) => args.fingerprint_mode,
            CredentialCommand::AddApiKey(args) => args.fingerprint_mode,
            _ => panic!("expected creation command"),
        };
        assert_eq!(mode, FingerprintMode::Device);
    }
}

#[test]
fn credential_creation_accepts_explicit_off() {
    let parsed = Cli::try_parse_from([
        "core",
        "credential",
        "add-api-key",
        "--secret-stdin",
        "--fingerprint-mode",
        "off",
    ])
    .expect("valid explicit mode");
    let Some(Command::Credential(credential)) = parsed.command else {
        panic!("expected credential command")
    };
    let CredentialCommand::AddApiKey(args) = credential.command else {
        panic!("expected add-api-key")
    };
    assert_eq!(args.fingerprint_mode, FingerprintMode::Off);
}

#[test]
fn fingerprint_command_distinguishes_inspection_and_mutation() {
    let inspect = Cli::try_parse_from(["core", "credential", "fingerprint", "acct_test"])
        .expect("valid inspection");
    let Some(Command::Credential(credential)) = inspect.command else {
        panic!("expected credential command")
    };
    let CredentialCommand::Fingerprint(args) = credential.command else {
        panic!("expected fingerprint command")
    };
    assert_eq!(args.account_ref, "acct_test");
    assert_eq!(args.mode, None);

    let mutate = Cli::try_parse_from([
        "core",
        "credential",
        "fingerprint",
        "acct_test",
        "--mode",
        "device",
    ])
    .expect("valid mutation");
    let Some(Command::Credential(credential)) = mutate.command else {
        panic!("expected credential command")
    };
    let CredentialCommand::Fingerprint(args) = credential.command else {
        panic!("expected fingerprint command")
    };
    assert_eq!(args.mode, Some(FingerprintMode::Device));
}

#[test]
fn unsupported_fingerprint_mode_is_rejected() {
    assert!(
        Cli::try_parse_from([
            "core",
            "credential",
            "login",
            "--fingerprint-mode",
            "session",
        ])
        .is_err()
    );
}

#[tokio::test]
async fn fingerprint_command_updates_the_authoritative_sidecar() {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let credential = vault
        .create_api_key(
            "upstream-secret".to_string(),
            "http://127.0.0.1:43123/v1/responses".to_string(),
            FingerprintMode::Device,
        )
        .await
        .expect("credential");

    CredentialCommand::Fingerprint(FingerprintArgs {
        state_dir: Some(temp.path().to_path_buf()),
        account_ref: credential.account_ref.clone(),
        mode: Some(FingerprintMode::Off),
    })
    .run()
    .await
    .expect("core fingerprint command");

    let metadata = vault
        .fingerprint_metadata(&credential.account_ref)
        .await
        .expect("fingerprint metadata");
    assert_eq!(metadata.mode, FingerprintMode::Off);
    assert_eq!(metadata.revision, 2);
    assert!(
        !serde_json::to_string(&metadata)
            .expect("metadata JSON")
            .contains("installation")
    );
}
