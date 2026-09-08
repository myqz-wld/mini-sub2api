//! Memory-only content interning and response-boundary radix lookup. Never log these values.
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Weak};

#[path = "subscription_history_match.rs"]
mod history_match;
pub(crate) use history_match::{candidate_key, completion_items_compatible, ids_compatible};

pub(crate) fn canonical(value: &Value) -> Vec<u8> {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let ordered: BTreeMap<_, _> = object.iter().collect();
                Value::Object(
                    ordered
                        .into_iter()
                        .map(|(k, v)| (k.clone(), sorted(v)))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_vec(&sorted(value)).expect("JSON value encoding")
}

pub(crate) struct Key {
    pub(crate) id: u64,
    bytes: Vec<u8>,
}

pub(crate) struct Item {
    pub(crate) value: Value,
    pub(crate) key: Arc<Key>,
    bytes: Vec<u8>,
}

impl Item {
    pub(crate) fn cost(&self) -> usize {
        // JSON tree allocations, encoded verification bytes and index references are charged.
        self.bytes
            .len()
            .saturating_mul(4)
            .saturating_add(self.key.bytes.len())
            .saturating_add(256)
    }
}

#[derive(Default)]
pub(crate) struct Interner {
    keys: HashMap<[u8; 32], Vec<Weak<Key>>>,
    items: HashMap<[u8; 32], Vec<Weak<Item>>>,
    next: u64,
}

impl Interner {
    fn key(&mut self, bytes: Vec<u8>) -> Arc<Key> {
        let bucket = self.keys.entry(Sha256::digest(&bytes).into()).or_default();
        bucket.retain(|entry| entry.strong_count() > 0);
        if let Some(key) = bucket
            .iter()
            .filter_map(Weak::upgrade)
            .find(|key| key.bytes == bytes)
        {
            return key;
        }
        self.next = self
            .next
            .checked_add(1)
            .expect("interner identity exhausted");
        let key = Arc::new(Key {
            id: self.next,
            bytes,
        });
        bucket.push(Arc::downgrade(&key));
        key
    }

    pub(crate) fn intern(&mut self, value: Value) -> Arc<Item> {
        let bytes = canonical(&value);
        let digest = Sha256::digest(&bytes).into();
        if let Some(item) = self
            .items
            .get(&digest)
            .into_iter()
            .flatten()
            .filter_map(Weak::upgrade)
            .find(|item| item.bytes == bytes)
        {
            return item;
        }
        let key = self.key(candidate_key(&value));
        let item = Arc::new(Item { value, key, bytes });
        self.items
            .entry(digest)
            .or_default()
            .push(Arc::downgrade(&item));
        item
    }

    pub(crate) fn lookup(&self, value: &Value) -> Option<u64> {
        let bytes = candidate_key(value);
        self.keys
            .get(&<[u8; 32]>::from(Sha256::digest(&bytes)))?
            .iter()
            .filter_map(Weak::upgrade)
            .find(|key| key.bytes == bytes)
            .map(|key| key.id)
    }

    pub(crate) fn sweep(&mut self) {
        self.keys.retain(|_, bucket| {
            bucket.retain(|entry| entry.strong_count() > 0);
            !bucket.is_empty()
        });
        self.items.retain(|_, bucket| {
            bucket.retain(|entry| entry.strong_count() > 0);
            !bucket.is_empty()
        });
    }

    pub(crate) fn cost(&self) -> usize {
        self.items
            .values()
            .flatten()
            .filter_map(Weak::upgrade)
            .map(|item| item.cost())
            .sum::<usize>()
            + (self.items.len() + self.keys.len()) * 128
    }
}

#[derive(Default)]
pub(crate) struct Radix {
    edge: Vec<u64>,
    terminals: Vec<String>,
    children: BTreeMap<u64, Radix>,
}

impl Radix {
    pub(crate) fn insert(&mut self, path: &[u64], response: String) {
        let mut node = self;
        let mut remaining = path;
        loop {
            let common = node
                .edge
                .iter()
                .zip(remaining)
                .take_while(|(a, b)| a == b)
                .count();
            if common < node.edge.len() {
                let suffix = node.edge.split_off(common);
                let child = Self {
                    edge: suffix,
                    terminals: std::mem::take(&mut node.terminals),
                    children: std::mem::take(&mut node.children),
                };
                node.children.insert(child.edge[0], child);
            }
            if common == remaining.len() {
                if !node.terminals.contains(&response) {
                    node.terminals.push(response);
                }
                return;
            }
            remaining = &remaining[common..];
            node = node.children.entry(remaining[0]).or_insert_with(|| Self {
                edge: remaining.to_vec(),
                terminals: Vec::new(),
                children: BTreeMap::new(),
            });
        }
    }

    /// All completed terminals along the path; eligibility is filtered before longest selection.
    pub(crate) fn prefixes(&self, path: &[u64]) -> Vec<(usize, String)> {
        let mut result = Vec::new();
        let mut node = self;
        let mut offset = 0;
        loop {
            if !path[offset..].starts_with(&node.edge) {
                break;
            }
            offset += node.edge.len();
            result.extend(node.terminals.iter().map(|id| (offset, id.clone())));
            let Some(child) = path.get(offset).and_then(|key| node.children.get(key)) else {
                break;
            };
            node = child;
        }
        result
    }

    pub(crate) fn cost(&self) -> usize {
        let mut pending = vec![self];
        let mut cost = 0;
        while let Some(node) = pending.pop() {
            cost += 96
                + node.edge.capacity() * 8
                + node
                    .terminals
                    .iter()
                    .map(|id| id.capacity() + 24)
                    .sum::<usize>();
            pending.extend(node.children.values());
        }
        cost
    }
}

pub(crate) struct History {
    pub(crate) parent: Option<Arc<History>>,
    pub(crate) items: Vec<Arc<Item>>,
    pub(crate) len: usize,
}

impl History {
    pub(crate) fn extend(parent: Option<Arc<Self>>, items: Vec<Arc<Item>>) -> Arc<Self> {
        let len = parent.as_ref().map_or(0, |parent| parent.len) + items.len();
        Arc::new(Self { parent, items, len })
    }

    pub(crate) fn items(&self) -> Vec<&Arc<Item>> {
        let mut blocks = Vec::new();
        let mut current = Some(self);
        while let Some(block) = current {
            blocks.push(block);
            current = block.parent.as_deref();
        }
        blocks
            .into_iter()
            .rev()
            .flat_map(|block| block.items.iter())
            .collect()
    }

    pub(crate) fn retained_cost(
        &self,
        blocks: &mut HashSet<usize>,
        mut items: Option<&mut HashSet<usize>>,
    ) -> usize {
        let mut cost = 0;
        let mut node = Some(self);
        while let Some(block) = node {
            if !blocks.insert(block as *const Self as usize) {
                break;
            }
            cost += 64 + block.items.capacity() * std::mem::size_of::<Arc<Item>>();
            if let Some(seen) = items.as_mut() {
                for item in &block.items {
                    if seen.insert(Arc::as_ptr(item) as usize) {
                        cost += item.cost();
                    }
                }
            }
            node = block.parent.as_deref();
        }
        cost
    }

    pub(crate) fn values(&self) -> Vec<Value> {
        self.items()
            .into_iter()
            .map(|item| item.value.clone())
            .collect()
    }
}

pub(crate) fn settings_for_format(
    object: &Map<String, Value>,
    format: crate::subscription_request::Format,
    transport: crate::request_normalizer::EmulationTransport,
) -> Value {
    let mut result = object.clone();
    crate::request_normalizer::filter_subscription_fields(&mut result, transport);
    let lite = format == crate::subscription_request::Format::Lite;
    let model = object.get("model").and_then(Value::as_str).unwrap_or("");
    let mut profile = crate::request_defaults::model_profile(model);
    profile.responses_lite |= lite;
    crate::request_defaults::merge_request_defaults(&mut result, profile, false);
    crate::codex_instructions::normalize_base(&mut result);
    if !lite {
        result
            .entry("tools")
            .or_insert_with(|| Value::Array(Vec::new()));
    }
    result.retain(|key, _| {
        !matches!(
            key.as_str(),
            "input"
                | "previous_response_id"
                | "client_metadata"
                | "type"
                | "stream"
                | "generate"
                | "stream_options"
                | "access_programs"
                | "store"
        )
    });
    Value::Object(result)
}

pub(crate) fn setup_hash(settings: &Value) -> [u8; 32] {
    Sha256::digest(canonical(&serde_json::json!({
        "instructions": settings.get("instructions"), "tools": settings.get("tools")
    })))
    .into()
}

#[cfg(test)]
#[path = "subscription_index_tests.rs"]
mod tests;

impl Drop for History {
    fn drop(&mut self) {
        let mut parent = self.parent.take();
        while let Some(node) = parent {
            match Arc::try_unwrap(node) {
                Ok(mut history) => parent = history.parent.take(),
                Err(_) => break,
            }
        }
    }
}

impl Drop for Radix {
    fn drop(&mut self) {
        let mut pending: Vec<_> = std::mem::take(&mut self.children).into_values().collect();
        while let Some(mut child) = pending.pop() {
            pending.extend(std::mem::take(&mut child.children).into_values());
        }
    }
}
