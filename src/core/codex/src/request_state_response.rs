//! Reuse validated delta ID pairs while preserving the complete transaction on a miss.
use super::*;
use crate::request_compaction::PendingCompaction;
use crate::request_state_types::WireIdOwner;
use crate::response_delta_ids::DeltaIds;
use crate::response_id_cache::{CacheIdentity, SharedResponseCache};
use crate::response_state_stamp::StateStamp;
use crate::response_wire_ids::translate_response_ids;
use serde_json::Value;

pub(crate) struct ResponseEdit {
    pub value: Value,
    pub owner: Option<WireIdOwner>,
    pub compaction: Option<PendingCompaction>,
    pub cache: Option<SharedResponseCache>,
}

impl RequestStateStore {
    #[cfg(test)]
    pub(crate) fn response_cache_metrics(&self) -> (usize, usize, usize) {
        use std::sync::atomic::Ordering;
        (
            self.response_cache_budget.used(),
            self.response_cache_budget.hits.load(Ordering::Relaxed),
            self.response_cache_budget.misses.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn response_cache(&self) -> Option<SharedResponseCache> {
        self.response_cache_budget.cache()
    }

    pub(crate) async fn translate_response<F>(
        &self,
        account_namespace: &str,
        owner_account_ref: &str,
        downstream_scope: &str,
        edit: ResponseEdit,
        validate: F,
    ) -> Result<Value>
    where
        F: FnOnce(&Value) -> Result<()> + Send + 'static,
    {
        validate_account_ref(owner_account_ref)?;
        anyhow::ensure!(
            !account_namespace.is_empty() && !downstream_scope.is_empty(),
            "empty response identity scope"
        );
        let store = self.clone();
        let namespace = account_namespace.to_owned();
        let account = owner_account_ref.to_owned();
        let scope = downstream_scope.to_owned();
        // One existing blocking/file-lock boundary, including cache hits. Never block an async worker.
        tokio::task::spawn_blocking(move || {
            store.translate_response_locked(
                &namespace,
                &account,
                &scope,
                Utc::now().timestamp_millis(),
                edit,
                validate,
            )
        })
        .await
        .context("response identity task failed")?
    }

    fn translate_response_locked<F>(
        &self,
        namespace: &str,
        account: &str,
        scope: &str,
        now: i64,
        mut edit: ResponseEdit,
        validate: F,
    ) -> Result<Value>
    where
        F: FnOnce(&Value) -> Result<()>,
    {
        anyhow::ensure!(now >= 0, "invalid response identity time");
        let state_ref = LookupKeyFactory::account_state_ref(namespace);
        let _lock = lock_state(&self.accounts_dir, &state_ref)?;
        let path = state_path(&self.accounts_dir, &state_ref);
        let day = now / 86_400_000;
        let identity = CacheIdentity::new(namespace, account, scope, edit.owner.as_ref());
        let fields = (edit.cache.is_some() && edit.compaction.is_none())
            .then(|| DeltaIds::inspect(&edit.value))
            .flatten();
        let before = if fields.is_some() {
            StateStamp::read(&path)?
        } else {
            None
        };
        if let (Some(cache), Some(fields)) = (&edit.cache, &fields) {
            let hit = cache
                .lock()
                .map_err(|_| anyhow::anyhow!("response identity cache unavailable"))?
                .translate(identity, before, day, fields, &mut edit.value);
            if hit {
                // Cache mutex is released before entering context/lifecycle validation.
                validate(&edit.value)?;
                return Ok(edit.value);
            }
        }
        let translated = edit_under_lock(
            &self.accounts_dir,
            namespace,
            account,
            scope,
            now,
            &self.contexts,
            move |editor| {
                translate_response_ids(editor, &mut edit.value, edit.owner.as_ref())?;
                validate(&edit.value)?;
                if let Some(pending) = edit.compaction {
                    editor.commit_compaction(
                        &pending.marker_key,
                        &pending.thread_id,
                        pending.target_window,
                    )?;
                }
                Ok(edit.value)
            },
        )?;
        if let (Some(cache), Some(fields)) = (&edit.cache, &fields) {
            // Do not attach previously read aliases to a stamp from an unrelated replacement.
            // After new writes, warm on the next stable read of the committed state.
            let after = StateStamp::read(&path)?;
            let stable = before.filter(|_| before == after);
            cache
                .lock()
                .map_err(|_| anyhow::anyhow!("response identity cache unavailable"))?
                .remember(identity, stable, day, fields, &translated);
        }
        Ok(translated)
    }
}

#[cfg(test)]
#[path = "request_state_response_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "request_state_response_invalidation_tests.rs"]
mod invalidation_tests;
