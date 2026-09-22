use super::*;

pub(super) fn edit_locked<R, F>(
    accounts_dir: &Path,
    account_namespace: &str,
    owner_account_ref: &str,
    downstream_scope: &str,
    now_unix_ms: i64,
    contexts: &crate::subscription_context::ContextStore,
    operation: F,
) -> Result<R>
where
    F: FnOnce(&mut RequestStateEditor<'_>) -> Result<R>,
{
    let state_ref = LookupKeyFactory::account_state_ref(account_namespace);
    let _lock = lock_state(accounts_dir, &state_ref)?;
    edit_under_lock(
        accounts_dir,
        account_namespace,
        owner_account_ref,
        downstream_scope,
        now_unix_ms,
        contexts,
        operation,
    )
}

// The caller holds the account file lock through load, validation and atomic persistence.
pub(super) fn edit_under_lock<R, F>(
    accounts_dir: &Path,
    account_namespace: &str,
    owner_account_ref: &str,
    downstream_scope: &str,
    now_unix_ms: i64,
    contexts: &crate::subscription_context::ContextStore,
    operation: F,
) -> Result<R>
where
    F: FnOnce(&mut RequestStateEditor<'_>) -> Result<R>,
{
    let state_ref = LookupKeyFactory::account_state_ref(account_namespace);
    let path = state_path(accounts_dir, &state_ref);
    let existing = read_optional_state(&path)?;
    let created = existing.is_none();
    let mut state = match existing {
        Some(state) => state,
        None => {
            let mut owners = owners_for_state_ref(
                accounts_dir,
                &LookupKeyFactory::account_state_ref(account_namespace),
            )?;
            owners.insert(owner_account_ref.to_string());
            PersistedRequestState::new(owners)
        }
    };
    // Existing state has already been fully validated by read_optional_state.
    if created {
        state.validate()?;
    }
    let keys = LookupKeyFactory::new(account_namespace, downstream_scope);
    let day = now_unix_ms / 86_400_000;
    let mut editor =
        RequestStateEditor::new(&mut state, keys, owner_account_ref, day, now_unix_ms)?;
    let output = operation(&mut editor)?;
    let mut summary = editor.finish();
    contexts.protect_aliases(&state, &mut summary.protected)?;
    let mut changed = summary.changed | state.prune(day, &summary.protected)?;
    if !created && !changed {
        return Ok(output);
    }
    let mut bytes = serde_json::to_vec(&state).context("encoding request state")?;
    while bytes.len() as u64 > MAX_REQUEST_STATE_BYTES {
        anyhow::ensure!(
            state.evict_one(&summary.protected),
            "request state cannot fit within the size limit"
        );
        changed = true;
        bytes = serde_json::to_vec(&state).context("encoding pruned request state")?;
    }
    if created || changed {
        if !created {
            state.revision = next_revision(state.revision)?;
            bytes = serde_json::to_vec(&state).context("encoding revised request state")?;
        }
        state.validate()?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_REQUEST_STATE_BYTES,
            "request state is too large"
        );
        write_bytes_atomically(accounts_dir, &path, &bytes)?;
    }
    Ok(output)
}
