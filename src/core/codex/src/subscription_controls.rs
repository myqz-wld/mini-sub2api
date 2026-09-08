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
        let object = value.as_object().ok_or(Error::InvalidRequest)?;
        let id = optional_id(object.get("response_id"))?;
        let inject = object.get("type").and_then(Value::as_str) == Some("response.inject");
        let mutates = inject || object.contains_key("item") || object.contains_key("input");
        if id.is_none() && !mutates {
            return Ok(());
        }
        let bound = binding.ok_or(Error::StateUnavailable)?;
        let mut inner = self.inner.lock().map_err(|_| Error::StateUnavailable)?;
        if let Some(id) = &id {
            let owner = inner
                .scopes
                .get(scope)
                .and_then(|s| s.records.get(id))
                .ok_or(Error::StateUnavailable)?;
            if owner.identity.session_id != bound.session_id
                || owner.identity.thread_id != bound.thread_id
                || owner.socket != bound.connection_id
            {
                return Err(Error::InvalidRequest);
            }
        }
        if !mutates {
            return Ok(());
        }
        // Missing response_id targets only the current bound operation. In particular, it cannot
        // leave that operation's old full history eligible for publication at completion.
        let mut candidates = inner.operations.iter().filter(|(_, op)| {
            op.scope == scope
                && op.record.identity.session_id == bound.session_id
                && op.record.identity.thread_id == bound.thread_id
                && op.record.socket == bound.connection_id
                && id
                    .as_ref()
                    .is_none_or(|id| op.response_id.as_ref() == Some(id))
        });
        let operation_id = candidates
            .next()
            .map(|(id, _)| id.clone())
            .ok_or(Error::StateUnavailable)?;
        if candidates.next().is_some() {
            return Err(Error::InvalidRequest);
        }
        let dependencies = if inject {
            let input = normalize_input(object.get("input"))?;
            let extra = serde_json::to_vec(&input)
                .map_err(|_| Error::InvalidRequest)?
                .len()
                .saturating_mul(6);
            if !inner.make_room(&self.limits, scope, &bound.session_id, extra) {
                return Err(Error::StateUnavailable);
            }
            let active = inner
                .operations
                .get_mut(&operation_id)
                .ok_or(Error::StateUnavailable)?;
            if !active.dependencies_available {
                return Err(Error::StateUnavailable);
            }
            let mut dependencies = active.record.dependencies.clone();
            dependencies.append(&input)?;
            active.reserved = active.reserved.saturating_add(extra);
            Some(dependencies)
        } else {
            None
        };
        let active = inner
            .operations
            .get_mut(&operation_id)
            .ok_or(Error::StateUnavailable)?;
        if let Some(dependencies) = dependencies {
            active.record.dependencies = dependencies;
        } else {
            // Generic control semantics do not establish the effective dependency graph.
            // Forward the control, but require a complete client replacement before local reuse.
            active.dependencies_available = false;
        }
        active.record.history = None;
        active.record.settings = None;
        active.output_available = false;
        active.output.clear();
        active.output_bytes = 0;
        active.reserved = active.reserved.saturating_sub(active.buffer_charge);
        active.buffer_charge = 0;
        let response_id = active.response_id.clone();
        if let Some(scope) = inner.scopes.get_mut(scope) {
            if let Some(record) = response_id.and_then(|id| scope.records.get_mut(&id)) {
                record.history = None;
                record.settings = None;
            }
            scope.rebuild_index();
        }
        Ok(())
    }
}
