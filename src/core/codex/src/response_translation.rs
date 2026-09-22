use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;

use crate::request_compaction::PendingCompaction;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_store::RequestStateStore;
use crate::request_state_store::ResponseEdit;
use crate::request_state_types::WireIdOwner;
use crate::response_id_cache::ResponseCacheSlot;

#[cfg(test)]
#[path = "response_state_performance_tests.rs"]
mod performance_tests;

#[path = "response_failure_tail.rs"]
mod failure_tail;
pub(crate) use failure_tail::SseFailureTail;

#[derive(Clone)]
pub(crate) struct ResponseStateContext {
    account_ref: String,
    state_namespace: String,
    downstream_scope: String,
    store: RequestStateStore,
    owner: Arc<Mutex<Option<WireIdOwner>>>,
    default_compaction: Option<PendingCompaction>,
    operation: Arc<Mutex<Option<crate::subscription_context::Operation>>>,
    identity_cache: Arc<Mutex<ResponseCacheSlot>>,
}

impl ResponseStateContext {
    pub(crate) fn new(
        account_ref: &str,
        state_namespace: &str,
        downstream_scope: &str,
        store: &RequestStateStore,
        identity: Option<&ResolvedRequestIdentity>,
        pending_compaction: Option<&PendingCompaction>,
    ) -> Self {
        Self {
            account_ref: account_ref.to_string(),
            state_namespace: state_namespace.to_string(),
            downstream_scope: downstream_scope.to_string(),
            store: store.clone(),
            owner: Arc::new(Mutex::new(identity.map(owner_from_identity))),
            default_compaction: pending_compaction.cloned(),
            operation: Arc::new(Mutex::new(None)),
            identity_cache: Arc::new(Mutex::new(ResponseCacheSlot::default())),
        }
    }

    pub(crate) fn with_operation(
        self,
        operation: Option<crate::subscription_context::Operation>,
    ) -> Self {
        {
            let mut slot = self.identity_cache.lock().expect("new cache lock");
            slot.cache = None;
            slot.enabled = operation.is_some();
        }
        *self.operation.lock().expect("new operation lock") = operation;
        self
    }

    pub(crate) fn update_operation(
        &self,
        operation: Option<crate::subscription_context::Operation>,
    ) -> Result<()> {
        let enabled = operation.is_some();
        *self
            .operation
            .lock()
            .map_err(|_| anyhow::anyhow!("operation state unavailable"))? = operation;
        let mut slot = self
            .identity_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("response identity cache unavailable"))?;
        slot.cache = None;
        slot.enabled = enabled;
        Ok(())
    }

    pub(crate) fn update_identity(&self, identity: Option<&ResolvedRequestIdentity>) -> Result<()> {
        *self
            .owner
            .lock()
            .map_err(|_| anyhow::anyhow!("response identity owner lock poisoned"))? =
            identity.map(owner_from_identity);
        self.identity_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("response identity cache unavailable"))?
            .cache = None;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn translate_value(&self, value: Value) -> Result<Value> {
        let pending = (value.get("type").and_then(Value::as_str) == Some("response.completed"))
            .then_some(self.default_compaction.as_ref())
            .flatten();
        self.translate_value_with_compaction(value, pending, None, None)
            .await
    }

    pub(crate) async fn translate_terminal_value(
        &self,
        value: Value,
        completed: bool,
    ) -> Result<Value> {
        let pending = completed
            .then_some(self.default_compaction.as_ref())
            .flatten();
        // This caller already proved the terminal kind. Use the schema's response envelope even
        // when the provider omits output/usage/object, so its ID cannot bypass response aliasing.
        let mut envelope = self
            .translate_value_with_compaction(
                serde_json::json!({"response": value}),
                pending,
                Some(completed),
                None,
            )
            .await?;
        let translated = envelope["response"].take();
        Ok(translated)
    }

    async fn translate_value_with_compaction(
        &self,
        value: Value,
        pending_compaction: Option<&PendingCompaction>,
        terminal: Option<bool>,
        failure_tail: Option<SseFailureTail>,
    ) -> Result<Value> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| anyhow::anyhow!("response identity owner lock poisoned"))?
            .clone();
        let operation = self
            .operation
            .lock()
            .map_err(|_| anyhow::anyhow!("operation state unavailable"))?
            .clone();
        if let Some(operation) = &operation
            && value.get("type").and_then(Value::as_str) == Some("response.metadata")
            && let Some(headers) = value.get("headers").and_then(Value::as_object)
            && let Some(token) = headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("x-codex-turn-state"))
                .and_then(|(_, value)| value.as_str())
        {
            self.store.contexts.learn_response_turn(operation, token)?;
        }
        let pending_compaction = match pending_compaction {
            Some(pending)
                if self.store.contexts.accepts_compaction(
                    operation.as_ref(),
                    pending,
                    &value,
                )? =>
            {
                Some(pending.clone())
            }
            _ => None,
        };
        let validation_store = self.store.contexts.clone();
        let validation_operation = operation.clone();
        let failed_tail = failure_tail.is_some();
        let cache = if failed_tail
            || terminal.is_some()
            || !crate::response_delta_ids::DeltaIds::eligible(&value)
        {
            None
        } else {
            let mut cache = self
                .identity_cache
                .lock()
                .map_err(|_| anyhow::anyhow!("response identity cache unavailable"))?;
            if cache.enabled && cache.cache.is_none() {
                cache.cache = self.store.response_cache();
            }
            cache.cache.clone()
        };
        let mut translated = self
            .store
            .translate_response(
                &self.state_namespace,
                &self.account_ref,
                &self.downstream_scope,
                ResponseEdit {
                    value,
                    owner,
                    compaction: pending_compaction,
                    cache,
                },
                move |value| {
                    if let Some(tail) = failure_tail {
                        tail.validate(value)?;
                    } else {
                        validation_store.validate_event(
                            validation_operation.as_ref(),
                            value,
                            terminal,
                        )?;
                    }
                    Ok(())
                },
            )
            .await?;
        if let Some(operation) = operation {
            if !failed_tail && (terminal.is_some() || translated.get("type").is_some()) {
                self.store
                    .contexts
                    .observe(&operation, &translated, terminal)?;
            }
            // Publish private continuation state before applying the caller's optional-output policy.
            operation
                .0
                .reasoning_visibility
                .filter_response(&mut translated);
        }
        Ok(translated)
    }

    pub(crate) async fn translate_text_with_compaction(
        &self,
        text: String,
        maximum: usize,
        pending_compaction: Option<&PendingCompaction>,
    ) -> Result<String> {
        let value = serde_json::from_str::<Value>(&text)?;
        let completed = value.get("type").and_then(Value::as_str) == Some("response.completed");
        let pending = completed
            .then_some(pending_compaction.or(self.default_compaction.as_ref()))
            .flatten();
        let value = self
            .translate_value_with_compaction(value, pending, None, None)
            .await?;
        let encoded = serde_json::to_string(&value)?;
        anyhow::ensure!(encoded.len() <= maximum, "translated response is too large");
        Ok(encoded)
    }
}

fn owner_from_identity(identity: &ResolvedRequestIdentity) -> WireIdOwner {
    WireIdOwner {
        session_id: identity.session_id.clone(),
        thread_id: identity.thread_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_state_store::RequestStateStore;

    #[tokio::test]
    async fn aggregated_terminal_commits_only_when_kind_is_completed() {
        for requires_compaction_item in [false, true] {
            let temp = tempfile::tempdir().expect("tempdir");
            let store = RequestStateStore::new(temp.path().to_path_buf());
            let (pending, thread_id) = store
                .edit(
                    "namespace-terminal",
                    "acct_terminal",
                    "scope-terminal",
                    move |editor| {
                        let conversation_key = editor.lookup("conversation", "terminal-session");
                        let marker_key = editor.lookup("compaction", "terminal-operation");
                        let conversation = editor.conversation(&conversation_key)?;
                        let target = editor.begin_compaction(&marker_key, &conversation.id)?;
                        Ok((
                            PendingCompaction {
                                marker_key,
                                thread_id: conversation.id.clone(),
                                target_window: target,
                                requires_compaction_item,
                            },
                            conversation.id,
                        ))
                    },
                )
                .await
                .expect("pending compaction");
            let context = ResponseStateContext::new(
                "acct_terminal",
                "namespace-terminal",
                "scope-terminal",
                &store,
                None,
                Some(&pending),
            );
            for _ in ["response.failed", "response.incomplete"] {
                context
                    .translate_terminal_value(
                        serde_json::json!({"id":"resp_not_completed","output":[]}),
                        false,
                    )
                    .await
                    .expect("translate non-completed terminal");
            }
            assert_eq!(window(&store, &thread_id).await, 0);
            context
            .translate_terminal_value(serde_json::json!({"id":"resp_completed","output":[{"type":"compaction","encrypted_content":"synthetic"}]}), true)
            .await
            .expect("translate completed terminal");
            // Local summaries can complete without item events. V2 final output alone is not proof.
            assert_eq!(
                window(&store, &thread_id).await,
                u64::from(!requires_compaction_item)
            );
        }
    }

    async fn window(store: &RequestStateStore, thread_id: &str) -> u64 {
        let thread_id = thread_id.to_string();
        store
            .edit(
                "namespace-terminal",
                "acct_terminal",
                "scope-terminal",
                move |editor| {
                    editor
                        .window_number(&thread_id)
                        .ok_or_else(|| anyhow::anyhow!("missing terminal thread"))
                },
            )
            .await
            .expect("terminal window")
    }
}
