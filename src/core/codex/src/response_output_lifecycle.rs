//! Bounded identities of started output items; never retain partial content or argument bodies.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type Fingerprint = [u8; 32];

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum Locator {
    Index(usize),
    Id(Fingerprint),
}

#[derive(Clone, Copy, Default)]
struct Identity {
    id: Option<Fingerprint>,
    kind: Option<Fingerprint>,
}

impl Identity {
    fn item(item: &Value) -> Self {
        Self {
            id: fingerprint(item.get("id")),
            kind: fingerprint(item.get("type")),
        }
    }

    fn merge(&mut self, other: Self) -> anyhow::Result<()> {
        for (previous, next) in [(&mut self.id, other.id), (&mut self.kind, other.kind)] {
            if let (Some(previous), Some(next)) = (*previous, next) {
                anyhow::ensure!(previous == next, "output item identity changed");
            }
            *previous = previous.or(next);
        }
        Ok(())
    }

    fn matches(self, final_item: &Value) -> bool {
        let other = Self::item(final_item);
        self.id.is_none_or(|id| Some(id) == other.id)
            && self.kind.is_none_or(|kind| Some(kind) == other.kind)
    }
}

fn fingerprint(value: Option<&Value>) -> Option<Fingerprint> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|s| Sha256::digest(s.as_bytes()).into())
}

#[derive(Default)]
pub(crate) struct OutputLifecycle {
    pending: BTreeMap<Locator, Identity>,
    by_id: BTreeMap<Fingerprint, Locator>,
    unverified: bool,
}

impl OutputLifecycle {
    pub(crate) fn is_output_event(event: &Value) -> bool {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        kind.starts_with("response.")
            && !matches!(
                kind,
                "response.created"
                    | "response.in_progress"
                    | "response.completed"
                    | "response.failed"
                    | "response.incomplete"
                    | "response.metadata"
            )
            && (event.get("output_index").is_some()
                || event.get("item_id").is_some()
                || kind.starts_with("response.output_item.")
                || kind.starts_with("response.content_part.")
                || kind.ends_with(".delta")
                || kind == "response.output_text.done")
    }

    pub(crate) fn observe(&mut self, event: &Value, maximum: usize) -> anyhow::Result<()> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        if matches!(kind, "response.created" | "response.in_progress") {
            if let Some(items) = event.pointer("/response/output").and_then(Value::as_array) {
                for (index, item) in items.iter().enumerate() {
                    self.start(Some(index), Identity::item(item), maximum)?;
                }
            }
            return Ok(());
        }
        if !Self::is_output_event(event) {
            return Ok(());
        }
        let index = event
            .get("output_index")
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|i| usize::try_from(i).ok())
                    .ok_or_else(|| anyhow::anyhow!("invalid output item index"))
            })
            .transpose()?;
        let identity = if matches!(
            kind,
            "response.output_item.added" | "response.output_item.done"
        ) {
            let item = event
                .get("item")
                .filter(|item| item.is_object())
                .ok_or_else(|| anyhow::anyhow!("invalid output item"))?;
            Identity::item(item)
        } else {
            Identity {
                id: fingerprint(event.get("item_id")),
                kind: None,
            }
        };
        if kind == "response.output_item.done" && !unfinished(&event["item"]) {
            self.finish(index, identity)
        } else {
            self.start(index, identity, maximum)
        }
    }

    fn existing_id(&self, identity: Identity) -> Option<Locator> {
        let id = identity.id?;
        self.by_id.get(&id).copied()
    }

    fn locate(&self, index: Option<usize>, identity: Identity) -> anyhow::Result<Option<Locator>> {
        let existing = self.existing_id(identity);
        if let (Some(index), Some(Locator::Index(previous))) = (index, existing) {
            anyhow::ensure!(index == previous, "output item index changed");
        }
        Ok(index
            .map(Locator::Index)
            .or(existing)
            .or(identity.id.map(Locator::Id)))
    }

    fn start(
        &mut self,
        index: Option<usize>,
        mut identity: Identity,
        maximum: usize,
    ) -> anyhow::Result<()> {
        let Some(key) = self.locate(index, identity)? else {
            self.unverified = true;
            return Ok(());
        };
        if let Some(previous) = self
            .existing_id(identity)
            .filter(|previous| *previous != key)
            && let Some(previous) = self.pending.remove(&previous)
        {
            identity.merge(previous)?;
        }
        if let Some(previous) = self.pending.get_mut(&key) {
            previous.merge(identity)?;
            identity = *previous;
        } else if self.pending.len() < maximum {
            self.pending.insert(key, identity);
        } else {
            // A bounded proof cannot silently forget a pending item and later authorize success.
            self.unverified = true;
            return Ok(());
        }
        if let Some(id) = identity.id {
            self.by_id.insert(id, key);
        }
        Ok(())
    }

    fn finish(&mut self, index: Option<usize>, identity: Identity) -> anyhow::Result<()> {
        let Some(key) = self.locate(index, identity)? else {
            return Ok(());
        };
        let other = self.existing_id(identity).filter(|other| *other != key);
        for key in [Some(key), other].into_iter().flatten() {
            if let Some(previous) = self.pending.get(&key) {
                anyhow::ensure!(
                    previous.id.is_none_or(|id| Some(id) == identity.id)
                        && previous.kind.is_none_or(|kind| Some(kind) == identity.kind),
                    "finished output item does not match its start"
                );
            }
            if let Some(previous) = self.pending.remove(&key)
                && let Some(id) = previous.id
            {
                self.by_id.remove(&id);
            }
        }
        Ok(())
    }

    pub(crate) fn validate_completed(&self, response: &Value) -> anyhow::Result<()> {
        super::validate_terminal(response, std::iter::empty(), true)?;
        anyhow::ensure!(
            !self.unverified,
            "output completion evidence is unavailable"
        );
        anyhow::ensure!(
            response
                .get("status")
                .is_none_or(|status| status.as_str() == Some("completed")),
            "completed response has an unfinished status"
        );
        let output = response.get("output").and_then(Value::as_array);
        if let Some(items) = output {
            anyhow::ensure!(
                items.iter().all(|item| !unfinished(item)),
                "completed response contains unfinished output"
            );
        }
        // Index an ID-only pending item once, rather than scanning the footer for every item.
        let mut final_by_id: BTreeMap<_, Option<usize>> = self
            .pending
            .keys()
            .filter_map(|key| {
                if let Locator::Id(id) = key {
                    Some((*id, None))
                } else {
                    None
                }
            })
            .collect();
        if !final_by_id.is_empty()
            && let Some(items) = output
        {
            for (index, item) in items.iter().enumerate() {
                if let Some(id) = fingerprint(item.get("id"))
                    && let Some(found) = final_by_id.get_mut(&id)
                {
                    anyhow::ensure!(
                        found.replace(index).is_none(),
                        "ambiguous final output identity"
                    );
                }
            }
        }
        for (locator, identity) in &self.pending {
            let final_item = match locator {
                Locator::Index(index) => output.and_then(|items| items.get(*index)),
                Locator::Id(id) => final_by_id
                    .get(id)
                    .copied()
                    .flatten()
                    .and_then(|index| output.and_then(|items| items.get(index))),
            };
            anyhow::ensure!(
                final_item.is_some_and(|item| identity.matches(item)),
                "completed response omitted unfinished output"
            );
        }
        Ok(())
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.pending.len() * 192 + self.by_id.len() * 128
    }
}

fn unfinished(item: &Value) -> bool {
    matches!(
        item.get("status").and_then(Value::as_str),
        Some("in_progress" | "incomplete" | "queued" | "searching" | "generating" | "interpreting")
    )
}

#[cfg(test)]
#[path = "response_output_lifecycle_tests.rs"]
mod tests;
