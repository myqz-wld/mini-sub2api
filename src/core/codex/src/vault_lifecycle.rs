use super::*;

pub(super) fn read_optional_receipt(path: &Path) -> Result<Option<RemovalReceipt>> {
    match read_json_limited(path, 64 * 1024, "credential removal receipt") {
        Ok(receipt) => Ok(Some(receipt)),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|source| source.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn receipt_path(accounts_dir: &Path, account_ref: &str) -> Result<PathBuf> {
    validate_account_ref(account_ref)?;
    Ok(accounts_dir.join(format!("{account_ref}.removal.json")))
}

pub(super) fn complete_removal_locked(
    accounts_dir: &Path,
    record_path: &Path,
    account_ref: &str,
    requested_kind: RemovalKind,
) -> Result<RemovalReceipt> {
    let record = match read_record(record_path) {
        Ok(record) => {
            anyhow::ensure!(
                record.account_ref == account_ref,
                "credential record identity mismatch"
            );
            Some(record)
        }
        Err(error) if is_not_found(&error) => None,
        Err(error) => return Err(error),
    };
    let receipt_path = receipt_path(accounts_dir, account_ref)?;
    let record_state_ref = record
        .as_ref()
        .map(|record| RequestStateStore::state_ref_for_namespace(record.request_state_namespace()));
    let mut receipt = match read_optional_receipt(&receipt_path)? {
        Some(existing) => {
            anyhow::ensure!(
                existing.account_ref == account_ref,
                "credential removal receipt identity mismatch"
            );
            anyhow::ensure!(
                existing.kind == requested_kind
                    || (existing.kind == RemovalKind::OAuthRevoked
                        && requested_kind == RemovalKind::ServiceOnly),
                "credential was removed without an upstream OAuth revoke"
            );
            existing
        }
        None => {
            let receipt = RemovalReceipt {
                account_ref: account_ref.to_string(),
                kind: requested_kind,
                completed_at: Utc::now(),
                request_state_ref: record_state_ref.clone(),
            };
            write_json_atomically(accounts_dir, &receipt_path, &receipt)?;
            receipt
        }
    };
    if let Some(record_state_ref) = record_state_ref {
        match receipt.request_state_ref.as_deref() {
            Some(existing) => anyhow::ensure!(
                existing == record_state_ref,
                "credential removal receipt state identity mismatch"
            ),
            None => {
                receipt.request_state_ref = Some(record_state_ref);
                write_json_atomically(accounts_dir, &receipt_path, &receipt)?;
            }
        }
    }
    if let Some(state_ref) = receipt.request_state_ref.as_deref() {
        anyhow::ensure!(
            RequestStateStore::valid_state_ref(state_ref),
            "invalid credential removal state reference"
        );
        RequestStateStore::new(accounts_dir.to_path_buf())
            .remove_credential_owner(state_ref, account_ref)?;
    } else {
        match std::fs::remove_file(record_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("removing credential record"),
        }
    }
    fingerprint::remove_if_exists(accounts_dir, account_ref)?;
    sync_directory(accounts_dir);
    Ok(receipt)
}

pub(super) fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|source| source.downcast_ref::<std::io::Error>())
        .any(|source| source.kind() == std::io::ErrorKind::NotFound)
}
