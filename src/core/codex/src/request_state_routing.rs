//! Bounded opaque turn routing lives separately from the identity-only ledger.
use super::*;
use crate::subscription_context::Operation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) const SUFFIX: &str = ".turn-state.json";
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const TTL_MS: i64 = 3 * 60 * 60 * 1000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutingState {
    version: u32,
    entries: BTreeMap<String, Entry>,
}

impl Default for RoutingState {
    fn default() -> Self {
        Self {
            version: 1,
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    scope: String,
    session: String,
    thread: String,
    turn: String,
    branch: String,
    token: String,
    seen_ms: i64,
}

impl Entry {
    fn key(&self) -> String {
        format!("{}:{}:{}", self.scope, self.branch, self.turn)
    }
    fn same_turn(&self, other: &Self) -> bool {
        self.scope == other.scope
            && self.session == other.session
            && self.thread == other.thread
            && self.turn == other.turn
    }
    fn validate(&self) -> Result<()> {
        for value in [
            &self.scope,
            &self.session,
            &self.thread,
            &self.turn,
            &self.branch,
        ] {
            crate::request_state_types::validate_wire_id(value)?;
        }
        anyhow::ensure!(
            !self.turn.is_empty() && self.seen_ms >= 0,
            "invalid routing identity"
        );
        crate::subscription_routing::validate_token(&self.token)
    }
}

fn snapshot(store: &RequestStateStore, operation: &Operation) -> Option<(Entry, bool)> {
    let inner = store.contexts.inner.lock().ok()?;
    let Some(active) = inner.operations.get(&operation.0.id) else {
        let scope = inner.scopes.get(&operation.0.scope)?;
        let (record, key) = scope.records.values().find_map(|record| {
            Some((
                record,
                record.startup.as_ref()?.routing_target(&operation.0.id)?,
            ))
        })?;
        let (branch, turn) = key.split_once(':')?;
        let token = scope.routing.get(key)?;
        return Some((
            Entry {
                scope: operation.0.scope.clone(),
                session: record.identity.session_id.clone(),
                thread: record.identity.thread_id.clone(),
                turn: turn.into(),
                branch: branch.into(),
                token: token.clone(),
                seen_ms: Utc::now().timestamp_millis(),
            },
            true,
        ));
    };
    let identity = &active.record.identity;
    let turn = identity.turn_id.as_ref().filter(|turn| !turn.is_empty())?;
    let token = inner.scopes.get(&active.scope).and_then(|scope| {
        scope
            .routing
            .get(&format!("{}:{turn}", active.record.branch))
    });
    Some((
        Entry {
            scope: active.scope.clone(),
            session: identity.session_id.clone(),
            thread: identity.thread_id.clone(),
            turn: turn.clone(),
            branch: active.record.branch.clone(),
            token: token.cloned().unwrap_or_default(),
            seen_ms: Utc::now().timestamp_millis(),
        },
        token.is_some(),
    ))
}

impl RequestStateStore {
    pub(crate) async fn learn_turn_state(
        &self,
        namespace: &str,
        operation: &Operation,
        token: &str,
    ) -> Result<()> {
        self.contexts.learn_turn(operation, token)?;
        self.persist_turn_state(namespace, operation, false).await
    }

    pub(crate) async fn restore_turn_state(
        &self,
        namespace: &str,
        operation: &Operation,
    ) -> Result<()> {
        self.persist_turn_state(namespace, operation, true).await
    }

    pub(crate) async fn persist_turn_state(
        &self,
        namespace: &str,
        operation: &Operation,
        restore: bool,
    ) -> Result<()> {
        let Some((entry, has_token)) = snapshot(self, operation) else {
            return Ok(());
        };
        if !restore && !has_token {
            return Ok(());
        }
        let state_ref = LookupKeyFactory::account_state_ref(namespace);
        let accounts_dir = self.accounts_dir.clone();
        let token = tokio::task::spawn_blocking(move || {
            update(&accounts_dir, &state_ref, entry, has_token, restore)
        })
        .await
        .context("routing state task failed")??;
        if restore && let Some(token) = token {
            self.contexts.learn_turn(operation, &token)?;
        }
        Ok(())
    }
}

fn update(
    accounts_dir: &Path,
    state_ref: &str,
    mut entry: Entry,
    has_token: bool,
    restore: bool,
) -> Result<Option<String>> {
    let _lock = lock_state(accounts_dir, state_ref)?;
    // Owner deletion and token writes share the identity lock; late responses cannot resurrect it.
    anyhow::ensure!(
        state_path(accounts_dir, state_ref).is_file(),
        "routing identity state unavailable"
    );
    let path = accounts_dir.join(format!("{state_ref}{SUFFIX}"));
    let mut state = read(&path)?;
    let before_len = state.entries.len();
    let now = entry.seen_ms;
    state
        .entries
        .retain(|_, value| now.saturating_sub(value.seen_ms) < TTL_MS);
    let key = entry.key();
    let mut selected = state
        .entries
        .get(&key)
        .filter(|value| value.same_turn(&entry))
        .cloned();
    if restore && !has_token && entry.branch == entry.thread {
        // Without a retained branch, only unanimous same-thread/turn routing can be recovered.
        for candidate in state
            .entries
            .values()
            .filter(|value| value.same_turn(&entry))
        {
            anyhow::ensure!(
                selected
                    .as_ref()
                    .is_none_or(|first| first.token == candidate.token),
                "ambiguous restored turn routing"
            );
            selected = Some(candidate.clone());
        }
    }
    if let Some(selected) = &selected {
        anyhow::ensure!(
            !has_token || entry.token == selected.token,
            "turn routing owner conflict"
        );
        entry.token.clone_from(&selected.token);
    } else if !has_token {
        if before_len != state.entries.len() {
            write(accounts_dir, &path, &mut state, None)?;
        }
        return Ok(None);
    }
    entry.validate()?;
    // Avoid an fsync for every metadata event. Refresh on admission at most once per sweep.
    let changed = state
        .entries
        .get(&key)
        .is_none_or(|old| now.saturating_sub(old.seen_ms) >= 30_000)
        || before_len != state.entries.len();
    let token = entry.token.clone();
    if changed {
        state.entries.insert(key.clone(), entry);
        write(accounts_dir, &path, &mut state, Some(&key))?;
    }
    Ok(Some(token))
}

fn read(path: &Path) -> Result<RoutingState> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RoutingState::default());
        }
        Err(error) => return Err(error).context("reading routing state metadata"),
    };
    anyhow::ensure!(
        metadata.file_type().is_file() && metadata.len() <= MAX_BYTES as u64,
        "invalid routing state file"
    );
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o7777 != 0o600 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let state: RoutingState = read_json_limited(path, MAX_BYTES as u64, "routing state")?;
    anyhow::ensure!(
        state.version == 1 && state.entries.len() <= MAX_ENTRIES,
        "unsupported routing state"
    );
    for (key, value) in &state.entries {
        value.validate()?;
        anyhow::ensure!(*key == value.key(), "invalid routing state key");
    }
    Ok(state)
}

fn write(
    accounts_dir: &Path,
    path: &Path,
    state: &mut RoutingState,
    protected: Option<&str>,
) -> Result<()> {
    loop {
        let bytes = serde_json::to_vec(state)?;
        if bytes.len() <= MAX_BYTES && state.entries.len() <= MAX_ENTRIES {
            return write_bytes_atomically(accounts_dir, path, &bytes);
        }
        let oldest = state
            .entries
            .iter()
            .filter(|(key, _)| Some(key.as_str()) != protected)
            .min_by_key(|(_, entry)| entry.seen_ms)
            .map(|(key, _)| key.clone())
            .ok_or_else(|| anyhow::anyhow!("routing state capacity unavailable"))?;
        state.entries.remove(&oldest);
    }
}

pub(super) fn remove(accounts_dir: &Path, state_ref: &str) -> Result<()> {
    remove_file_if_exists(
        accounts_dir,
        &accounts_dir.join(format!("{state_ref}{SUFFIX}")),
    )
}

#[cfg(test)]
#[path = "request_state_routing_tests.rs"]
mod tests;
