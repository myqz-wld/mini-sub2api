use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;

use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_store::RequestStateStore;
use crate::request_state_types::WireIdOwner;
use crate::response_wire_ids::translate_response_ids;

#[derive(Clone)]
pub(crate) struct ResponseStateContext {
    account_ref: String,
    account_namespace: String,
    downstream_scope: String,
    store: RequestStateStore,
    owner: Arc<Mutex<Option<WireIdOwner>>>,
}

impl ResponseStateContext {
    pub(crate) fn new(
        account_ref: &str,
        account_namespace: &str,
        downstream_scope: &str,
        store: &RequestStateStore,
        identity: Option<&ResolvedRequestIdentity>,
    ) -> Self {
        Self {
            account_ref: account_ref.to_string(),
            account_namespace: account_namespace.to_string(),
            downstream_scope: downstream_scope.to_string(),
            store: store.clone(),
            owner: Arc::new(Mutex::new(identity.map(owner_from_identity))),
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

    pub(crate) async fn translate_value(&self, mut value: Value) -> Result<Value> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| anyhow::anyhow!("response identity owner lock poisoned"))?
            .clone();
        self.store
            .edit(
                &self.account_namespace,
                &self.account_ref,
                &self.downstream_scope,
                move |editor| {
                    translate_response_ids(editor, &mut value, owner.as_ref())?;
                    Ok(value)
                },
            )
            .await
    }

    pub(crate) async fn translate_text(&self, text: String, maximum: usize) -> Result<String> {
        let value = serde_json::from_str::<Value>(&text)?;
        let value = self.translate_value(value).await?;
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
