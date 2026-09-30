//! A successful prewarm may hand its first routing value to one business turn.
use crate::subscription_context::{ContextStore, Operation, Scope};
use serde_json::Value;

#[derive(Clone)]
pub(crate) struct StartupRouting {
    operation: String,
    turn: Option<String>,
    pub(crate) token: Option<String>,
}

impl StartupRouting {
    pub(crate) fn new(operation: String) -> Self {
        Self {
            operation,
            turn: None,
            token: None,
        }
    }

    pub(crate) fn cost(&self) -> usize {
        self.operation.len()
            + self.turn.as_ref().map_or(0, String::len)
            + self.token.as_ref().map_or(0, String::len)
    }
}

impl Scope {
    pub(crate) fn adopt_startup(&mut self, socket: &str, thread: &str, turn: Option<&str>) {
        for record in self
            .records
            .values_mut()
            .filter(|record| record.socket.as_deref() == Some(socket) && record.completed)
        {
            let Some(startup) = &mut record.startup else {
                continue;
            };
            let compatible = record.identity.thread_id == thread
                && turn.is_some()
                && startup
                    .turn
                    .as_deref()
                    .is_none_or(|bound| Some(bound) == turn);
            if !compatible {
                record.startup = None;
                continue;
            }
            let turn = turn.unwrap();
            startup.turn.get_or_insert_with(|| turn.to_string());
            if let Some(token) = startup.token.take() {
                self.routing.entry(turn.to_string()).or_insert(token);
            }
        }
    }
}

impl ContextStore {
    pub(crate) fn learn_completed_startup(
        &self,
        operation: &Operation,
        token: &str,
    ) -> anyhow::Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("context state unavailable"))?;
        let candidate = inner.scopes.get(&operation.0.scope).and_then(|scope| {
            scope.records.iter().find(|(_, record)| {
                record.completed
                    && record
                        .socket
                        .as_ref()
                        .is_some_and(|socket| inner.sockets.contains(socket))
                    && record
                        .startup
                        .as_ref()
                        .is_some_and(|startup| startup.operation == operation.0.id)
            })
        });
        let Some((id, record)) = candidate else {
            return Ok(());
        };
        let startup = record.startup.as_ref().unwrap();
        if startup.token.is_some()
            || startup
                .turn
                .as_ref()
                .is_some_and(|turn| inner.scopes[&operation.0.scope].routing.contains_key(turn))
        {
            return Ok(());
        }
        let (id, session, turn) = (
            id.clone(),
            record.identity.session_id.clone(),
            startup.turn.clone(),
        );
        crate::subscription_routing::validate_token(token)?;
        let extra = token.len() + turn.as_ref().map_or(0, |turn| 1024 + turn.len());
        anyhow::ensure!(
            inner.make_room(&self.limits, &operation.0.scope, &session, extra),
            "routing metadata capacity unavailable"
        );
        let scope = inner
            .scopes
            .get_mut(&operation.0.scope)
            .expect("retained startup scope");
        if let Some(turn) = turn {
            scope
                .routing
                .entry(turn)
                .or_insert_with(|| token.to_string());
        } else if let Some(record) = scope.records.get_mut(&id) {
            record.startup.as_mut().unwrap().token = Some(token.to_string());
        }
        Ok(())
    }

    pub(crate) fn learn_response_metadata(
        &self,
        operation: &Operation,
        event: &Value,
    ) -> anyhow::Result<()> {
        let Some(token) = crate::subscription_routing::metadata_token(event) else {
            return Ok(());
        };
        if let Some(id) = event
            .pointer("/response/id")
            .or_else(|| event.get("response_id"))
            .and_then(Value::as_str)
        {
            let inner = self
                .inner
                .lock()
                .map_err(|_| anyhow::anyhow!("context state unavailable"))?;
            let scope = inner.scopes.get(&operation.0.scope);
            let known = scope.and_then(|scope| scope.records.get(id));
            if let Some(active) = inner.operations.get(&operation.0.id) {
                let current = active.response_id.as_deref();
                if current != Some(id) {
                    let startup = known.is_some_and(|record| {
                        record.completed
                            && record.socket == active.record.socket
                            && record.identity.thread_id == active.record.identity.thread_id
                            && record.startup.as_ref().is_some_and(|startup| {
                                active.record.identity.turn_id.as_ref().is_some_and(|turn| {
                                    startup.turn.as_deref()
                                        == Some(format!("{}:{turn}", active.record.branch).as_str())
                                })
                            })
                    });
                    if !startup && (known.is_some() || current.is_some()) {
                        return Ok(());
                    }
                }
            } else if !known.is_some_and(|record| {
                record
                    .startup
                    .as_ref()
                    .is_some_and(|startup| startup.operation == operation.0.id)
            }) {
                return Ok(());
            }
        }
        self.learn_response_turn(operation, token)
    }
}
