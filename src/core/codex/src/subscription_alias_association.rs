//! Known public output aliases can locate identity, never an incremental history baseline.
use crate::request_normalizer::StatefulPrepareError as Error;
use crate::subscription_context::{CheckpointAssociation, Record, Scope};
use crate::subscription_index::{ids_compatible, metadata_compatible};
use serde_json::Value;
use std::collections::BTreeSet;

impl Scope {
    pub(super) fn alias_association(
        &self,
        input: &[Value],
    ) -> Result<Option<CheckpointAssociation>, Error> {
        let eligible = |record: &&Record| {
            record.completed
                && record.history.is_some()
                && self
                    .sessions
                    .get(&record.identity.session_id)
                    .is_some_and(|session| !session.explicit)
        };
        // Only provider-emitted aliases are evidence. Caller-chosen local IDs are not locators.
        let emitted: BTreeSet<_> = self
            .records
            .values()
            .filter(eligible)
            .flat_map(|record| record.emitted_aliases.iter())
            .collect();
        let anchors: Vec<_> = input
            .iter()
            .filter(|item| {
                let model_output = item.get("role").and_then(Value::as_str) == Some("assistant")
                    || matches!(
                        item.get("type").and_then(Value::as_str),
                        Some(
                            "function_call"
                                | "custom_tool_call"
                                | "local_shell_call"
                                | "web_search_call"
                                | "image_generation_call"
                                | "tool_search_call"
                        )
                    );
                model_output
                    && ["id", "call_id"].iter().any(|field| {
                        item.get(*field).and_then(Value::as_str).is_some_and(|id| {
                            emitted.contains(&(field.to_string(), id.to_string()))
                        })
                    })
            })
            .collect();
        if anchors.is_empty() {
            return Ok(None);
        }
        let business: Vec<_> = input.iter().skip_while(|item| setup_item(item)).collect();
        let keys: Vec<_> = business
            .iter()
            .map(|item| self.interner.lookup_history(item))
            .collect();
        let mut qualified = Vec::new();
        let mut longest = 0;
        for record in self.records.values().filter(eligible) {
            let history = record.history.as_ref().expect("eligible history");
            let items = history.items();
            let saved: Vec<_> = items
                .iter()
                .skip_while(|item| setup_item(&item.value))
                .collect();
            let hidden = history.hidden_reasoning();
            // Only leading setup can differ. A source alias never authorizes replacing old tasks.
            if saved.is_empty()
                || saved.len() > business.len()
                || !saved
                    .iter()
                    .zip(&business)
                    .zip(&keys)
                    .all(|((saved, caller), key)| {
                        *key == Some(saved.lookup_key.id)
                            && ids_compatible(caller, &saved.value)
                            && metadata_compatible(caller, &saved.value)
                            && crate::subscription_index::hidden_ciphertext_compatible(
                                caller,
                                &saved.value,
                                hidden.contains(&saved.key.id),
                            )
                    })
            {
                continue;
            }
            // Every known supplied anchor must belong to this same complete candidate history.
            if !anchors.iter().all(|anchor| {
                items.iter().any(|saved| {
                    ["id", "call_id"].iter().any(|field| {
                        anchor
                            .get(*field)
                            .and_then(Value::as_str)
                            .is_some_and(|id| {
                                emitted.contains(&(field.to_string(), id.to_string()))
                                    && saved.value.get(*field).and_then(Value::as_str) == Some(id)
                            })
                    }) && ids_compatible(anchor, &saved.value)
                        && metadata_compatible(anchor, &saved.value)
                        && self.interner.lookup_history(anchor) == Some(saved.lookup_key.id)
                })
            }) {
                continue;
            }
            if saved.len() > longest {
                qualified.clear();
                longest = saved.len();
            }
            if saved.len() == longest {
                qualified.push(record);
            }
        }
        let owner = qualified.first().copied();
        if let Some(owner) = owner
            && qualified.iter().any(|record| {
                owner.identity.session_id != record.identity.session_id
                    || owner.identity.thread_id != record.identity.thread_id
                    || owner.branch != record.branch
            })
        {
            return Err(Error::StateUnavailable);
        }
        Ok(owner.map(|record| CheckpointAssociation {
            identity: record.identity.clone(),
            branch: record.branch.clone(),
            lineage: record.lineage.clone(),
        }))
    }
}

fn setup_item(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("additional_tools")
        || (item.get("type").and_then(Value::as_str) == Some("message")
            && matches!(
                item.get("role").and_then(Value::as_str),
                Some("developer" | "system")
            ))
}
