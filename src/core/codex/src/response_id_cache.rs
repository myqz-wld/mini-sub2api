//! Bounded request-owned ID pairs. No event body or complete ledger is retained.
use crate::request_state_types::{WireIdDomain, WireIdOwner, validate_wire_id};
use crate::response_delta_ids::DeltaIds;
use crate::response_state_stamp::StateStamp;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const MAX_ENTRIES: usize = 64;
const MAX_ID_BYTES: usize = 16 * 1024;
const STORE_BUDGET_BYTES: usize = 2 * 1024 * 1024;
const BASE_BYTES: usize = std::mem::size_of::<ResponseIdCache>() + 128;
pub(crate) type SharedResponseCache = Arc<Mutex<ResponseIdCache>>;

pub(crate) struct ResponseCacheSlot {
    pub cache: Option<SharedResponseCache>,
    pub enabled: bool,
}

impl Default for ResponseCacheSlot {
    fn default() -> Self {
        Self {
            cache: None,
            enabled: true,
        }
    }
}

pub(crate) struct CacheBudget {
    used: AtomicUsize,
    maximum: usize,
    #[cfg(test)]
    pub hits: AtomicUsize,
    #[cfg(test)]
    pub misses: AtomicUsize,
}

impl Default for CacheBudget {
    fn default() -> Self {
        Self::with_limit(STORE_BUDGET_BYTES)
    }
}

impl CacheBudget {
    pub(crate) fn with_limit(maximum: usize) -> Self {
        Self {
            used: AtomicUsize::new(0),
            maximum,
            #[cfg(test)]
            hits: AtomicUsize::new(0),
            #[cfg(test)]
            misses: AtomicUsize::new(0),
        }
    }
    fn reserve(&self, bytes: usize) -> bool {
        self.used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= self.maximum)
            })
            .is_ok()
    }
    fn release(&self, bytes: usize) {
        self.used.fetch_sub(bytes, Ordering::Relaxed);
    }
    pub fn cache(self: &Arc<Self>) -> Option<SharedResponseCache> {
        if !self.reserve(BASE_BYTES) {
            return None;
        }
        Some(Arc::new(Mutex::new(ResponseIdCache {
            entries: std::array::from_fn(|_| None),
            next: 0,
            id_bytes: 0,
            epoch: None,
            budget: Arc::clone(self),
        })))
    }
    #[cfg(test)]
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct CacheIdentity([u8; 32]);

impl CacheIdentity {
    pub fn new(namespace: &str, account: &str, scope: &str, owner: Option<&WireIdOwner>) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"response-id-cache-v1");
        for part in [namespace, account, scope] {
            digest.update((part.len() as u64).to_le_bytes());
            digest.update(part.as_bytes());
        }
        digest.update([u8::from(owner.is_some())]);
        if let Some(owner) = owner {
            for part in [&owner.session_id, &owner.thread_id] {
                digest.update((part.len() as u64).to_le_bytes());
                digest.update(part.as_bytes());
            }
        }
        Self(digest.finalize().into())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct CacheEpoch {
    identity: CacheIdentity,
    stamp: StateStamp,
    day: i64,
}

struct Pair {
    field: &'static str,
    domain: WireIdDomain,
    upstream: String,
    downstream: String,
}

impl Pair {
    fn bytes(&self) -> usize {
        // Include conservative allocator overhead for both strings, beyond inline fixed slots.
        self.upstream.capacity() + self.downstream.capacity() + 64
    }
}

pub(crate) struct ResponseIdCache {
    entries: [Option<Pair>; MAX_ENTRIES],
    next: usize,
    id_bytes: usize,
    epoch: Option<CacheEpoch>,
    budget: Arc<CacheBudget>,
}

impl ResponseIdCache {
    fn clear(&mut self) {
        for entry in &mut self.entries {
            *entry = None;
        }
        self.budget.release(self.id_bytes);
        self.id_bytes = 0;
        self.next = 0;
        self.epoch = None;
    }

    pub fn translate(
        &mut self,
        identity: CacheIdentity,
        stamp: Option<StateStamp>,
        day: i64,
        fields: &DeltaIds,
        value: &mut Value,
    ) -> bool {
        let epoch = stamp.map(|stamp| CacheEpoch {
            identity,
            stamp,
            day,
        });
        if epoch.is_none() || self.epoch != epoch {
            self.clear();
            #[cfg(test)]
            self.budget.misses.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        // Resolve every carrier before modifying any field; a miss leaves the original event intact.
        let mut replacements = Vec::with_capacity(fields.0.len());
        for field in &fields.0 {
            let Some(pair) = self.entries.iter().flatten().find(|pair| {
                pair.field == field.field
                    && pair.domain == field.domain
                    && pair.upstream == field.upstream
            }) else {
                #[cfg(test)]
                self.budget.misses.fetch_add(1, Ordering::Relaxed);
                return false;
            };
            replacements.push((field.field, pair.downstream.clone()));
        }
        for (field, replacement) in replacements {
            value[field] = Value::String(replacement);
        }
        #[cfg(test)]
        self.budget.hits.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub fn remember(
        &mut self,
        identity: CacheIdentity,
        stamp: Option<StateStamp>,
        day: i64,
        fields: &DeltaIds,
        translated: &Value,
    ) {
        let Some(stamp) = stamp else {
            self.clear();
            return;
        };
        let epoch = CacheEpoch {
            identity,
            stamp,
            day,
        };
        if self.epoch != Some(epoch) {
            self.clear();
            self.epoch = Some(epoch);
        }
        for field in &fields.0 {
            let Some(downstream) = translated.get(field.field).and_then(Value::as_str) else {
                continue;
            };
            if validate_wire_id(downstream).is_err() {
                continue;
            }
            if self.entries.iter().flatten().any(|pair| {
                pair.field == field.field
                    && pair.domain == field.domain
                    && pair.upstream == field.upstream
            }) {
                continue;
            }
            let pair = Pair {
                field: field.field,
                domain: field.domain,
                upstream: field.upstream.clone(),
                downstream: downstream.into(),
            };
            let bytes = pair.bytes();
            if bytes > MAX_ID_BYTES {
                continue;
            }
            while self.id_bytes + bytes > MAX_ID_BYTES || self.entries[self.next].is_some() {
                if let Some(removed) = self.entries[self.next].take() {
                    self.id_bytes -= removed.bytes();
                    self.budget.release(removed.bytes());
                }
                if self.id_bytes + bytes > MAX_ID_BYTES {
                    self.next = (self.next + 1) % MAX_ENTRIES;
                }
            }
            if !self.budget.reserve(bytes) {
                continue;
            }
            self.id_bytes += bytes;
            self.entries[self.next] = Some(pair);
            self.next = (self.next + 1) % MAX_ENTRIES;
        }
    }
}

impl Drop for ResponseIdCache {
    fn drop(&mut self) {
        self.budget.release(BASE_BYTES + self.id_bytes);
    }
}

#[cfg(test)]
#[path = "response_id_cache_tests.rs"]
mod tests;
