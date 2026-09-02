use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;

use crate::request_compaction::PendingCompaction;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_store::RequestStateStore;
use crate::request_state_types::WireIdOwner;
use crate::response_wire_ids::translate_response_ids;

#[derive(Clone)]
pub(crate) struct ResponseStateContext {
    account_ref: String,
    state_namespace: String,
    downstream_scope: String,
    store: RequestStateStore,
    owner: Arc<Mutex<Option<WireIdOwner>>>,
    default_compaction: Option<PendingCompaction>,
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
        }
    }

    pub(crate) fn update_identity(&self, identity: Option<&ResolvedRequestIdentity>) -> Result<()> {
        *self
            .owner
            .lock()
            .map_err(|_| anyhow::anyhow!("response identity owner lock poisoned"))? =
            identity.map(owner_from_identity);
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn translate_value(&self, value: Value) -> Result<Value> {
        let pending = (value.get("type").and_then(Value::as_str) == Some("response.completed"))
            .then_some(self.default_compaction.as_ref())
            .flatten();
        self.translate_value_with_compaction(value, pending).await
    }

    pub(crate) async fn translate_terminal_value(
        &self,
        value: Value,
        completed: bool,
    ) -> Result<Value> {
        let pending = completed
            .then_some(self.default_compaction.as_ref())
            .flatten();
        self.translate_value_with_compaction(value, pending).await
    }

    async fn translate_value_with_compaction(
        &self,
        mut value: Value,
        pending_compaction: Option<&PendingCompaction>,
    ) -> Result<Value> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| anyhow::anyhow!("response identity owner lock poisoned"))?
            .clone();
        let pending_compaction = pending_compaction.cloned();
        self.store
            .edit(
                &self.state_namespace,
                &self.account_ref,
                &self.downstream_scope,
                move |editor| {
                    translate_response_ids(editor, &mut value, owner.as_ref())?;
                    if let Some(pending) = pending_compaction {
                        editor.commit_compaction(
                            &pending.marker_key,
                            &pending.thread_id,
                            pending.target_window,
                        )?;
                    }
                    Ok(value)
                },
            )
            .await
    }

    pub(crate) async fn translate_text(&self, text: String, maximum: usize) -> Result<String> {
        self.translate_text_with_compaction(text, maximum, None)
            .await
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
        let value = self.translate_value_with_compaction(value, pending).await?;
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
        let temp = tempfile::tempdir().expect("tempdir");
        let store = RequestStateStore::new(temp.path().to_path_buf());
        let (pending, thread_id) = store
            .edit(
                "namespace-terminal",
                "acct_terminal",
                "scope-terminal",
                |editor| {
                    let conversation_key = editor.lookup("conversation", "terminal-session");
                    let marker_key = editor.lookup("compaction", "terminal-operation");
                    let conversation = editor.conversation(&conversation_key)?;
                    let target = editor.begin_compaction(&marker_key, &conversation.id)?;
                    Ok((
                        PendingCompaction {
                            marker_key,
                            thread_id: conversation.id.clone(),
                            target_window: target,
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
            .translate_terminal_value(serde_json::json!({"id":"resp_completed","output":[]}), true)
            .await
            .expect("translate completed terminal");
        assert_eq!(window(&store, &thread_id).await, 1);
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
