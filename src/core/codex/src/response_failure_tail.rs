use super::ResponseStateContext;
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::subscription_context::{Operation, Pending};

/// Failed SSE validation stays charged to its existing lease without owning an execution lane.
/// It can validate a failure footer but can never publish history or commit compaction.
#[derive(Clone)]
pub(crate) struct SseFailureTail(Arc<FailedOperation>);

struct FailedOperation {
    facts: Mutex<FailureFacts>,
    operation: Operation,
}

struct FailureFacts {
    response_id: Option<String>,
    observed_items: BTreeMap<usize, crate::response_output::CompletionFingerprint>,
    dependencies_available: bool,
}

impl Drop for FailedOperation {
    fn drop(&mut self) {
        if let Some(store) = self.operation.0.store.upgrade()
            && let Ok(mut inner) = store.lock()
        {
            inner.reservations.remove(&self.operation.0.id);
        }
    }
}

impl SseFailureTail {
    pub(super) fn validate(&self, event: &Value) -> Result<()> {
        let kind = event.get("type").and_then(Value::as_str);
        anyhow::ensure!(
            matches!(kind, Some("error" | "response.failed")),
            "invalid event after SSE error"
        );
        let mut facts = self
            .0
            .facts
            .lock()
            .map_err(|_| anyhow::anyhow!("failed response state unavailable"))?;
        let response = event.get("response").unwrap_or(&Value::Null);
        if let Some(id) = response.get("id").and_then(Value::as_str) {
            if let Some(previous) = &facts.response_id {
                anyhow::ensure!(previous == id, "response ownership changed after SSE error");
            } else {
                facts.response_id = Some(id.into());
            }
        }
        if kind == Some("response.failed") {
            crate::response_output::validate_terminal(
                response,
                facts
                    .observed_items
                    .iter()
                    .map(|(index, item)| (*index, *item)),
                facts.dependencies_available,
            )?;
        }
        Ok(())
    }
}

impl ResponseStateContext {
    pub(crate) fn detach_sse_failure(&self) -> Result<Option<SseFailureTail>> {
        let operation = self
            .operation
            .lock()
            .map_err(|_| anyhow::anyhow!("operation state unavailable"))?
            .clone();
        let Some(operation) = operation else {
            return Ok(None);
        };
        let mut inner = self
            .store
            .contexts
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("context state unavailable"))?;
        let active = inner
            .operations
            .remove(&operation.0.id)
            .ok_or_else(|| anyhow::anyhow!("operation already terminated"))?;
        if let Some(scope) = inner.scopes.get_mut(&active.scope)
            && let Some(id) = &active.response_id
        {
            scope.records.remove(id);
        }
        // Move the verification facts and keep their existing budget until the footer/drop.
        inner.reservations.insert(
            operation.0.id.clone(),
            Pending {
                scope: active.scope.clone(),
                session: active.record.identity.session_id.clone(),
                reserved: active.reserved,
            },
        );
        Ok(Some(SseFailureTail(Arc::new(FailedOperation {
            facts: Mutex::new(FailureFacts {
                response_id: active.response_id,
                observed_items: active.observed_items,
                dependencies_available: active.dependencies_available,
            }),
            operation,
        }))))
    }

    pub(crate) async fn translate_sse_value(
        &self,
        value: Value,
        failure_tail: Option<&SseFailureTail>,
    ) -> Result<Value> {
        if failure_tail.is_none() {
            return self.translate_value(value).await;
        }
        self.translate_value_with_compaction(value, None, None, failure_tail.cloned())
            .await
    }
}
