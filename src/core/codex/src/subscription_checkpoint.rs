//! A verified checkpoint can locate identity without proving an unchanged request prefix.
use super::{Error, ResolvedRequestIdentity, Scope};
use crate::subscription_prepare::HistoryLineage;
use serde_json::Value;

pub(crate) struct CheckpointAssociation {
    pub(crate) identity: ResolvedRequestIdentity,
    pub(crate) branch: String,
    pub(crate) lineage: HistoryLineage,
}

impl Scope {
    pub(crate) fn rebuild_index(&mut self) {
        self.index.clear();
        self.checkpoints = Default::default();
        for (id, record) in &self.records {
            if !record.completed {
                continue;
            }
            let Some(history) = &record.history else {
                continue;
            };
            let items = history.items();
            let checkpoint = record
                .compaction_key
                .filter(|key| items.iter().any(|item| item.key.id == *key));
            let anonymous = self
                .sessions
                .get(&record.identity.session_id)
                .is_some_and(|session| !session.explicit);
            if anonymous && let Some(key) = checkpoint {
                self.checkpoints.entry(key).or_default().push(id.clone());
            }
            // A real completed checkpoint also establishes a window with no user/assistant item.
            // Arbitrary caller checkpoints and generic empty/setup-only input cannot do so.
            if checkpoint.is_none()
                && !items.iter().any(|item| {
                    matches!(
                        item.value.get("role").and_then(Value::as_str),
                        Some("user" | "assistant")
                    )
                })
            {
                continue;
            }
            let path: Vec<_> = items.iter().map(|item| item.lookup_key.id).collect();
            self.index
                .entry(Some(record.identity.session_id.clone()))
                .or_default()
                .insert(&path, id.clone());
            if anonymous {
                self.index
                    .entry(None)
                    .or_default()
                    .insert(&path, id.clone());
            }
        }
    }

    pub(crate) fn checkpoint_association(
        &self,
        input: &[Value],
    ) -> Result<Option<CheckpointAssociation>, Error> {
        // A later unknown checkpoint supersedes earlier checkpoints; never search backwards for
        // a convenient older owner. lookup verifies canonical bytes after the fingerprint lookup.
        let key = input
            .iter()
            .rfind(|item| item.get("type").and_then(Value::as_str) == Some("compaction"))
            .and_then(|item| self.interner.lookup(item));
        let Some(ids) = key.and_then(|key| self.checkpoints.get(&key)) else {
            return Ok(None);
        };
        let mut source: Option<&super::Record> = None;
        for record in ids.iter().filter_map(|id| self.records.get(id)) {
            if !record.completed
                || record.history.is_none()
                || self
                    .sessions
                    .get(&record.identity.session_id)
                    .is_none_or(|session| session.explicit)
            {
                continue;
            }
            if let Some(first) = source {
                if first.identity.session_id != record.identity.session_id
                    || first.identity.thread_id != record.identity.thread_id
                    || first.identity.window_number != record.identity.window_number
                    || first.identity.forked_from_thread_id != record.identity.forked_from_thread_id
                    || first.branch != record.branch
                    || first.lineage.turns != record.lineage.turns
                {
                    return Err(Error::StateUnavailable);
                }
            } else {
                source = Some(record);
            }
        }
        Ok(source.map(|record| CheckpointAssociation {
            identity: record.identity.clone(),
            branch: record.branch.clone(),
            lineage: record.lineage.clone(),
        }))
    }

    pub(crate) fn checkpoint_index_cost(&self, session: Option<&str>) -> usize {
        if session.is_none() {
            return self.checkpoints.capacity() * 64
                + self
                    .checkpoints
                    .values()
                    .map(|ids| {
                        ids.capacity() * std::mem::size_of::<String>()
                            + ids.iter().map(String::capacity).sum::<usize>()
                    })
                    .sum::<usize>();
        }
        self.checkpoints
            .values()
            .flatten()
            .filter(|id| {
                session.is_none_or(|session| {
                    self.records
                        .get(*id)
                        .is_some_and(|record| record.identity.session_id == session)
                })
            })
            .map(|id| id.capacity() + 256)
            .sum()
    }
}
