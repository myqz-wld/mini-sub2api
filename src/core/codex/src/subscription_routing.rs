//! Provider routing tokens are opaque, memory-only headers, not persisted logical IDs.
use crate::request_normalizer::EmulationTransport;
use crate::subscription_context::{ContextStore, Operation};

pub(crate) const MAX_ROUTING_TOKEN_BYTES: usize = 64 * 1024;

pub(crate) fn validate_token(token: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        token.len() <= MAX_ROUTING_TOKEN_BYTES,
        "invalid routing token size"
    );
    http::HeaderValue::from_bytes(token.as_bytes())?.to_str()?;
    Ok(())
}

pub(crate) fn metadata_token(event: &serde_json::Value) -> Option<&str> {
    use serde_json::Value;
    if event.get("type").and_then(Value::as_str) != Some("response.metadata") {
        return None;
    }
    let mut value = event
        .get("headers")?
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-codex-turn-state"))?
        .1;
    // Native header metadata accepts a string or the first value of an array.
    loop {
        match value {
            Value::String(token) => return Some(token),
            Value::Array(values) => value = values.first()?,
            _ => return None,
        }
    }
}

impl ContextStore {
    pub(crate) fn learn_turn(&self, operation: &Operation, token: &str) -> anyhow::Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("context state unavailable"))?;
        let Some(active) = inner.operations.get(&operation.0.id) else {
            return Ok(());
        };
        let Some(turn) = active
            .record
            .identity
            .turn_id
            .as_deref()
            .filter(|turn| !turn.is_empty())
        else {
            return Ok(());
        };
        let (scope, session, key) = (
            active.scope.clone(),
            active.record.identity.session_id.clone(),
            format!("{}:{turn}", active.record.branch),
        );
        if inner
            .scopes
            .get(&scope)
            .is_some_and(|scope| scope.routing.contains_key(&key))
        {
            return Ok(()); // Native OnceLock: later values cannot replace the first token.
        }
        validate_token(token)?;
        anyhow::ensure!(
            inner.make_room(
                &self.limits,
                &scope,
                &session,
                1024 + key.len() + token.len()
            ),
            "routing metadata capacity unavailable"
        );
        inner
            .scopes
            .entry(scope)
            .or_default()
            .routing
            .insert(key, token.to_string());
        Ok(())
    }

    pub(crate) fn learn_response_turn(
        &self,
        operation: &Operation,
        token: &str,
    ) -> anyhow::Result<()> {
        // Native HTTP learns only response headers; metadata events carry WS routing state.
        if operation.0.transport != EmulationTransport::WebSocket {
            return Ok(());
        }
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| anyhow::anyhow!("context state unavailable"))?;
            if let Some(active) = inner.operations.get(&operation.0.id)
                && active.record.identity.request_kind == "prewarm"
                && active.record.socket.is_some()
            {
                if active.record.startup_token.is_some() {
                    return Ok(());
                }
                let (scope, session) = (
                    active.scope.clone(),
                    active.record.identity.session_id.clone(),
                );
                validate_token(token)?;
                anyhow::ensure!(
                    inner.make_room(&self.limits, &scope, &session, token.len()),
                    "routing metadata capacity unavailable"
                );
                let active = inner
                    .operations
                    .get_mut(&operation.0.id)
                    .expect("active prewarm");
                active.reserved = active.reserved.saturating_add(token.len());
                active.record.startup_token = Some(token.to_string());
                return Ok(());
            }
        }
        self.learn_turn(operation, token)
    }

    pub(crate) fn turn_token(&self, operation: &Operation) -> Option<String> {
        let inner = self.inner.lock().ok()?;
        let active = inner.operations.get(&operation.0.id)?;
        let turn = active.record.identity.turn_id.as_ref()?;
        inner
            .scopes
            .get(&active.scope)?
            .routing
            .get(&format!("{}:{turn}", active.record.branch))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_metadata_matches_native_header_value_shapes() {
        for (value, expected) in [
            (serde_json::json!("token"), Some("token")),
            (serde_json::json!(["first", "later"]), Some("first")),
            (serde_json::json!([[""]]), Some("")),
            (serde_json::json!([]), None),
            (serde_json::json!(null), None),
            (serde_json::json!([null, "later"]), None),
        ] {
            let event = serde_json::json!({"type":"response.metadata","headers":{"X-Codex-Turn-State":value}});
            assert_eq!(metadata_token(&event), expected);
        }
    }

    #[test]
    fn routing_tokens_use_header_rules_and_a_separate_bound() {
        assert!(validate_token("").is_ok()); // Preserve a present empty native header distinctly.
        for length in [780, 4096, MAX_ROUTING_TOKEN_BYTES] {
            assert!(validate_token(&"x".repeat(length)).is_ok());
        }
        for token in [
            "x".repeat(MAX_ROUTING_TOKEN_BYTES + 1),
            "bad\r\nheader".into(),
            "bad\0header".into(),
        ] {
            assert!(validate_token(&token).is_err());
        }
        assert!(crate::request_state_types::validate_wire_id(&"x".repeat(780)).is_err());
    }
}
