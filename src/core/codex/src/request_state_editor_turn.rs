use super::{RequestStateEditor, TurnAssignment, touch_day};
use crate::request_state_types::{TurnEntry, validate_lookup_key};
use anyhow::Result;
use uuid::Uuid;

impl RequestStateEditor<'_> {
    #[cfg(test)]
    pub(crate) fn turn(
        &mut self,
        key: &str,
        thread_id: &str,
        root_turn_id: Option<&str>,
        parent_turn_id: Option<&str>,
    ) -> Result<TurnAssignment> {
        self.turn_with_id(key, thread_id, root_turn_id, parent_turn_id, None)
    }

    pub(crate) fn turn_with_id(
        &mut self,
        key: &str,
        thread_id: &str,
        root_turn_id: Option<&str>,
        parent_turn_id: Option<&str>,
        reserved_id: Option<&str>,
    ) -> Result<TurnAssignment> {
        validate_lookup_key(key)?;
        let day = self.day;
        let now = self.now_unix_ms;
        let scope_key = self.scope_key.clone();
        let existed = self.scope().turns.contains_key(key);
        let (assignment, touched) = {
            let scope = self.scope_mut();
            let entry = scope.turns.entry(key.to_string()).or_insert_with(|| {
                let id = reserved_id.map_or_else(|| Uuid::now_v7().to_string(), str::to_string);
                TurnEntry {
                    root_turn_id: root_turn_id.unwrap_or(&id).to_string(),
                    id,
                    thread_id: thread_id.to_string(),
                    inventory_source_thread_id: None,
                    classifier: false,
                    parent_turn_id: parent_turn_id.map(str::to_string),
                    started_at_unix_ms: now,
                    last_seen_day: day,
                }
            });
            anyhow::ensure!(
                !entry.classifier
                    && entry.thread_id == thread_id
                    && root_turn_id.is_none_or(|root| entry.root_turn_id == root)
                    && entry.parent_turn_id.as_deref() == parent_turn_id
                    && reserved_id.is_none_or(|id| entry.id == id),
                "turn relationship changed"
            );
            let touched = touch_day(&mut entry.last_seen_day, day);
            (
                TurnAssignment {
                    id: entry.id.clone(),
                    thread_id: entry.thread_id.clone(),
                    inventory_source_thread_id: entry.inventory_source_thread_id.clone(),
                    root_turn_id: entry.root_turn_id.clone(),
                    parent_turn_id: entry.parent_turn_id.clone(),
                    started_at_unix_ms: entry.started_at_unix_ms,
                },
                touched,
            )
        };
        self.changed |= !existed || touched;
        self.protected.turns.insert((scope_key, key.to_string()));
        Ok(assignment)
    }

    pub(crate) fn observe_turn_started_at(&mut self, key: &str, value: i64) -> Result<()> {
        anyhow::ensure!(value >= 0, "invalid caller turn start time");
        let entry = self
            .scope_mut()
            .turns
            .get_mut(key)
            .ok_or_else(|| anyhow::anyhow!("turn start owner is missing"))?;
        let changed = entry.started_at_unix_ms != value;
        // Explicit caller corrections also replace old gateway-generated values. Retention
        // still uses last_seen_day from the server clock, never this protocol timestamp.
        entry.started_at_unix_ms = value;
        self.changed |= changed;
        Ok(())
    }

    pub(crate) fn existing_turn(&mut self, key: &str) -> Option<TurnAssignment> {
        let day = self.day;
        let scope_key = self.scope_key.clone();
        let (assignment, touched) = {
            let entry = self.scope_mut().turns.get_mut(key)?;
            let touched = touch_day(&mut entry.last_seen_day, day);
            (
                TurnAssignment {
                    id: entry.id.clone(),
                    thread_id: entry.thread_id.clone(),
                    inventory_source_thread_id: entry.inventory_source_thread_id.clone(),
                    root_turn_id: entry.root_turn_id.clone(),
                    parent_turn_id: entry.parent_turn_id.clone(),
                    started_at_unix_ms: entry.started_at_unix_ms,
                },
                touched,
            )
        };
        self.changed |= touched;
        self.protected.turns.insert((scope_key, key.to_string()));
        Some(assignment)
    }
}
