//! Completed-history association is independent of current request settings and WS reuse.
use super::{Error, Record, local_dependencies};
use crate::subscription_context::Scope;
use crate::subscription_index::{hidden_ciphertext_compatible, ids_compatible};
use serde_json::Value;

pub(super) struct HistoryMatch<'a> {
    pub(super) record: &'a Record,
    pub(super) length: usize,
    pub(super) equivalent_count: usize,
}

impl Scope {
    pub(super) fn match_history(
        &self,
        session: &Option<String>,
        input: &[Value],
    ) -> Result<Option<HistoryMatch<'_>>, Error> {
        let path: Vec<_> = input
            .iter()
            .map_while(|item| self.interner.lookup_history(item))
            .collect();
        let candidates = self
            .index
            .get(session)
            .map(|index| index.prefixes(&path))
            .unwrap_or_default();
        let mut length = 0;
        let mut qualified = Vec::new();
        for (candidate_len, id) in candidates {
            let Some(record) = self.records.get(&id) else {
                continue;
            };
            let Some(history) = &record.history else {
                continue;
            };
            let hidden = history.hidden_reasoning();
            if !record.completed
                || !history.items().iter().zip(input).all(|(saved, caller)| {
                    ids_compatible(caller, &saved.value)
                        && hidden_ciphertext_compatible(
                            caller,
                            &saved.value,
                            hidden.contains(&saved.key.id),
                        )
                })
            {
                continue;
            }
            let mut dependencies = local_dependencies(record);
            if dependencies.append(&input[candidate_len..]).is_err() {
                continue;
            }
            // An ineligible longer terminal cannot veto an eligible shorter context.
            if candidate_len > length {
                qualified.clear();
                length = candidate_len;
            }
            if candidate_len == length {
                qualified.push(record);
            }
        }
        let Some(first) = qualified.first().copied() else {
            return Ok(None);
        };
        let first_context = first.history.as_ref().expect("indexed history").items();
        if qualified.iter().skip(1).any(|record| {
            record.dependencies.calls != first.dependencies.calls
                || !record
                    .history
                    .as_ref()
                    .expect("indexed history")
                    .items()
                    .iter()
                    .zip(&first_context)
                    .all(|(a, b)| a.key.id == b.key.id)
        }) {
            return Err(Error::StateUnavailable);
        }
        // Different historical instructions/tools/models do not contradict equivalent history.
        // This grants neither current-setting inheritance nor a reusable upstream reference.
        Ok(Some(HistoryMatch {
            record: first,
            length,
            equivalent_count: qualified.len(),
        }))
    }
}
