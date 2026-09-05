use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_normalizer::StatefulPrepareError as Error;
use crate::subscription_context::ContextStore;
use crate::subscription_request::{normalize_input, optional_id};
use serde_json::Value;

impl ContextStore {
    pub(crate) fn prepare_control(
        &self,
        scope: &str,
        binding: Option<&ResolvedRequestIdentity>,
        value: &Value,
    ) -> Result<(), Error> {
        let Some(object) = value.as_object() else {
            return Err(Error::InvalidRequest);
        };
        let id = optional_id(object.get("response_id"))?;
        let Some(id) = id else {
            return Ok(());
        };
        let bound = binding.ok_or(Error::StateUnavailable)?;
        let mut inner = self.inner.lock().map_err(|_| Error::StateUnavailable)?;
        let owner = inner
            .scopes
            .get(scope)
            .and_then(|s| s.records.get(&id))
            .ok_or(Error::StateUnavailable)?;
        if owner.identity.session_id != bound.session_id || owner.socket != bound.connection_id {
            return Err(Error::InvalidRequest);
        }
        if object.get("type").and_then(Value::as_str) == Some("response.inject") {
            let input = normalize_input(object.get("input"))?;
            let operation_id = inner
                .operations
                .iter()
                .find(|(_, op)| op.scope == scope && op.response_id.as_ref() == Some(&id))
                .map(|(id, _)| id.clone())
                .ok_or(Error::StateUnavailable)?;
            let extra = serde_json::to_vec(&input)
                .map_err(|_| Error::InvalidRequest)?
                .len()
                .saturating_mul(6);
            if !inner.make_room(&self.limits, scope, &bound.session_id, extra) {
                return Err(Error::StateUnavailable);
            }
            let operation = inner
                .operations
                .get_mut(&operation_id)
                .ok_or(Error::StateUnavailable)?;
            if !operation.dependencies_available {
                return Err(Error::StateUnavailable);
            }
            let mut dependencies = operation.record.dependencies.clone();
            dependencies.append(&input)?;
            operation.reserved = operation.reserved.saturating_add(extra);
            // Injection can interleave with generated items. Without an authoritative replacement
            // context, keep only the continuation facts; a later full request can materialize it.
            operation.record.dependencies = dependencies;
            operation.record.history = None;
            operation.record.settings = None;
            operation.output_available = false;
            operation.output.clear();
            operation.output_bytes = 0;
        } else if object.contains_key("item") || object.contains_key("input") {
            if let Some(record) = inner
                .scopes
                .get_mut(scope)
                .and_then(|s| s.records.get_mut(&id))
            {
                record.history = None;
                record.settings = None;
            }
            if let Some(scope) = inner.scopes.get_mut(scope) {
                scope.rebuild_index();
            }
        }
        Ok(())
    }
}
