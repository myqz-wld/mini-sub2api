//! Subscription execution state. Body blocks never enter the persisted identity store.
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_normalizer::StatefulPrepareError as Error;
use crate::subscription_index::{History, Interner, Radix};
use crate::subscription_request::{Dependencies, Format};
use mini_sub2api_protocol_v1::limits::InferenceLimits;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

pub(crate) const HISTORY_TTL: Duration = Duration::from_secs(3 * 60 * 60);
pub(crate) const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct ContextStore {
    pub(crate) inner: Arc<Mutex<Inner>>,
    pub(crate) limits: Arc<InferenceLimits>,
}

pub(crate) struct Inner {
    pub(crate) scopes: HashMap<String, Scope>,
    pub(crate) operations: HashMap<String, Active>,
    pub(crate) reservations: HashMap<String, Pending>,
    pub(crate) sockets: HashSet<String>,
    pub(crate) ttl: Duration,
    pub(crate) baselines: HashMap<String, TrackedBaseline>,
    busy_baseline_bytes: usize,
}

pub(crate) struct TrackedBaseline {
    pub(crate) scope: String,
    pub(crate) session: String,
    pub(crate) state: Weak<Mutex<crate::responses_websocket_state::ResponsesWebSocketState>>,
}

#[derive(Default)]
pub(crate) struct Scope {
    pub(crate) records: HashMap<String, Record>,
    pub(crate) sessions: HashMap<String, Session>,
    pub(crate) aliases: HashMap<String, String>,
    pub(crate) turns: HashMap<String, String>,
    pub(crate) routing: HashMap<String, String>,
    pub(crate) bindings: HashMap<String, String>,
    pub(crate) index: HashMap<Option<String>, Radix>,
    pub(crate) checkpoints: HashMap<u64, Vec<String>>,
    pub(crate) interner: Interner,
    pub(crate) settings_pool: HashMap<[u8; 32], Vec<Weak<Value>>>,
}

pub(crate) struct Session {
    pub(crate) explicit: bool,
    pub(crate) last_business: Instant,
}

#[derive(Clone)]
pub(crate) struct Record {
    pub(crate) identity: ResolvedRequestIdentity,
    pub(crate) branch: String,
    pub(crate) caller_format: Format,
    pub(crate) upstream_format: Format,
    pub(crate) setup_hash: [u8; 32],
    pub(crate) settings: Option<Arc<Value>>,
    pub(crate) history: Option<Arc<History>>,
    pub(crate) dependencies: Dependencies,
    pub(crate) lineage: crate::subscription_prepare::HistoryLineage,
    pub(crate) socket: Option<String>,
    pub(crate) completed: bool,
    // Startup routing is published only with a completed prewarm and consumed by the first turn
    // on that same socket/thread. It is live connection metadata, never persisted body history.
    pub(crate) startup_token: Option<String>,
    pub(crate) compaction: Option<crate::request_compaction::PendingCompaction>,
    // Set only by accepted output replacement; never inferred from caller input.
    pub(crate) compaction_key: Option<u64>,
    pub(crate) last_used: Instant,
}

pub(crate) struct Pending {
    pub(crate) scope: String,
    pub(crate) session: String,
    pub(crate) reserved: usize,
}

pub(crate) struct Publication {
    pub(crate) raw_session: Option<String>,
    pub(crate) selected_session: Option<String>,
    pub(crate) raw_turn: Option<String>,
}

pub(crate) struct Active {
    pub(crate) scope: String,
    pub(crate) record: Record,
    pub(crate) publication: Option<Publication>,
    pub(crate) lane: String,
    pub(crate) reserved: usize,
    pub(crate) output: BTreeMap<usize, Value>,
    pub(crate) observed_items: BTreeMap<usize, crate::response_output::CompletionFingerprint>,
    pub(crate) output_lifecycle: crate::response_output::OutputLifecycle,
    pub(crate) compaction_output: crate::request_compaction::CompactionOutput,
    pub(crate) dependencies_available: bool,
    pub(crate) output_bytes: usize,
    pub(crate) buffer_charge: usize,
    pub(crate) output_available: bool,
    pub(crate) response_id: Option<String>,
}

/// The last handle releases admission and assembly pins on errors, cancellation and disconnect.
#[derive(Clone)]
pub(crate) struct Operation(pub(crate) Arc<Lease>);
pub(crate) struct Lease {
    pub(crate) id: String,
    pub(crate) store: Weak<Mutex<Inner>>,
    pub(crate) reasoning_visibility: crate::reasoning_visibility::ReasoningVisibility,
}

pub(crate) struct SocketLease {
    store: ContextStore,
    pub(crate) id: String,
}
impl Drop for SocketLease {
    fn drop(&mut self) {
        self.store.socket_closed(&self.id);
    }
}

impl std::fmt::Debug for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SubscriptionOperation")
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(store) = self.store.upgrade()
            && let Ok(mut inner) = store.lock()
        {
            inner.operations.remove(&self.id);
            inner.reservations.remove(&self.id);
        }
    }
}

impl ContextStore {
    pub(crate) fn track_baseline(
        &self,
        scope: String,
        identity: &ResolvedRequestIdentity,
        state: &Arc<Mutex<crate::responses_websocket_state::ResponsesWebSocketState>>,
    ) {
        if let Some(socket) = &identity.connection_id
            && let Ok(mut inner) = self.inner.lock()
        {
            inner.baselines.insert(
                socket.clone(),
                TrackedBaseline {
                    scope,
                    session: identity.session_id.clone(),
                    state: Arc::downgrade(state),
                },
            );
        }
    }

    pub(crate) fn enforce_baseline_budget(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.expire(Instant::now());
            for tracked in inner.baselines.values() {
                if !inner.fits(&self.limits, &tracked.scope, &tracked.session, 0)
                    && let Some(state) = tracked.state.upgrade()
                    && let Ok(mut state) = state.try_lock()
                {
                    state.abandon_cached_bodies();
                }
            }
        }
    }

    pub(crate) fn open_socket(&self) -> Result<SocketLease, Error> {
        let id = uuid::Uuid::now_v7().to_string();
        self.socket_open(&id)?;
        Ok(SocketLease {
            store: self.clone(),
            id,
        })
    }

    pub(crate) fn learn_turn(&self, operation: &Operation, token: &str) -> anyhow::Result<()> {
        crate::request_state_types::validate_wire_id(token)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("context state unavailable"))?;
        let Some(active) = inner.operations.get(&operation.0.id) else {
            return Ok(());
        };
        let Some(turn) = &active.record.identity.turn_id else {
            return Ok(());
        };
        if turn.is_empty() {
            return Ok(());
        }
        let (scope, turn) = (
            active.scope.clone(),
            format!("{}:{}", active.record.branch, turn),
        );
        inner
            .scopes
            .entry(scope)
            .or_default()
            .routing
            .entry(turn)
            .or_insert_with(|| token.to_string());
        Ok(())
    }

    pub(crate) fn learn_response_turn(
        &self,
        operation: &Operation,
        token: &str,
    ) -> anyhow::Result<()> {
        crate::request_state_types::validate_wire_id(token)?;
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| anyhow::anyhow!("context state unavailable"))?;
            if let Some(active) = inner.operations.get_mut(&operation.0.id)
                && active.record.identity.request_kind == "prewarm"
                && active.record.socket.is_some()
            {
                active
                    .record
                    .startup_token
                    .get_or_insert_with(|| token.to_string());
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
            .get(&format!("{}:{}", active.record.branch, turn))
            .cloned()
    }
    pub(crate) fn new(limits: InferenceLimits) -> Self {
        let inner = Arc::new(Mutex::new(Inner {
            scopes: HashMap::new(),
            operations: HashMap::new(),
            reservations: HashMap::new(),
            sockets: HashSet::new(),
            baselines: HashMap::new(),
            busy_baseline_bytes: limits.session_bytes,
            ttl: HISTORY_TTL,
        }));
        // Only the weak store is retained by the sweeper; dropping the vault ends its lifetime.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(&inner);
            runtime.spawn(async move {
                let mut tick = tokio::time::interval(SWEEP_INTERVAL);
                loop {
                    tick.tick().await;
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    if let Ok(mut inner) = inner.lock() {
                        inner.expire(Instant::now());
                    }
                }
            });
        }
        Self {
            inner,
            limits: Arc::new(limits),
        }
    }

    pub(crate) fn socket_open(&self, socket: &str) -> Result<(), Error> {
        self.inner
            .lock()
            .map_err(|_| Error::StateUnavailable)?
            .sockets
            .insert(socket.into());
        Ok(())
    }

    pub(crate) fn socket_closed(&self, socket: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.sockets.remove(socket);
            inner.baselines.remove(socket);
            for scope in inner.scopes.values_mut() {
                scope.bindings.remove(socket);
                for record in scope.records.values_mut() {
                    if record.socket.as_deref() == Some(socket) {
                        record.socket = None;
                        record.startup_token = None;
                    }
                }
            }
        }
    }

    pub(crate) fn scope_key(namespace: &str, key: &str) -> String {
        crate::request_state_lookup::LookupKeyFactory::new(namespace, key).scope_key()
    }
}

#[path = "subscription_retention.rs"]
mod retention;

#[path = "subscription_checkpoint.rs"]
mod checkpoint;
pub(crate) use checkpoint::CheckpointAssociation;

impl Record {
    pub(crate) fn descriptor_cost(&self) -> usize {
        2048 + self.dependencies.cost()
            + self.lineage.cost
            + self.socket.as_ref().map_or(0, String::len)
            + self.startup_token.as_ref().map_or(0, String::len)
    }
}

impl Scope {
    pub(crate) fn prune_metadata(&mut self, now: Instant, ttl: Duration) {
        let sessions: HashSet<_> = self
            .records
            .values()
            .map(|r| r.identity.session_id.clone())
            .chain(self.bindings.values().cloned())
            .collect();
        // A failed first operation has no completed record. Its admitted session/turn still
        // owns the first routing token until idle expiry, including across socket reconnects.
        self.sessions.retain(|id, session| {
            sessions.contains(id) || now.saturating_duration_since(session.last_business) < ttl
        });
        let sessions: HashSet<_> = self.sessions.keys().cloned().collect();
        self.aliases.retain(|_, id| sessions.contains(id));
        self.turns.retain(|_, id| sessions.contains(id));
        let turns: HashSet<_> = self
            .records
            .values()
            .filter_map(|r| {
                r.identity
                    .turn_id
                    .as_ref()
                    .map(|turn| format!("{}:{}", r.branch, turn))
            })
            .collect();
        self.routing.retain(|id, _| {
            turns.contains(id)
                || id
                    .split_once(':')
                    .and_then(|(_, turn)| self.turns.get(turn))
                    .is_some_and(|owner| sessions.contains(owner))
        });
    }

    pub(crate) fn intern_settings(&mut self, value: Value, hash: [u8; 32]) -> Arc<Value> {
        let bucket = self.settings_pool.entry(hash).or_default();
        bucket.retain(|v| v.strong_count() > 0);
        if let Some(found) = bucket
            .iter()
            .filter_map(Weak::upgrade)
            .find(|found| **found == value)
        {
            return found;
        }
        let value = Arc::new(value);
        bucket.push(Arc::downgrade(&value));
        value
    }

    pub(crate) fn cost(&self) -> usize {
        let mut blocks = HashSet::new();
        let mut settings = HashSet::new();
        self.interner.cost()
            + self
                .records
                .values()
                .map(|record| {
                    record.descriptor_cost()
                        + record
                            .settings
                            .as_ref()
                            .filter(|v| settings.insert(Arc::as_ptr(v) as usize))
                            .map_or(0, |v| crate::subscription_index::canonical(v).len() * 4)
                        + record
                            .history
                            .as_ref()
                            .map_or(0, |h| h.retained_cost(&mut blocks, None))
                })
                .sum::<usize>()
            + self.index.values().map(Radix::cost).sum::<usize>()
            + self.checkpoint_index_cost(None)
            + self.settings_pool.len() * 128
            + (self.sessions.len()
                + self.aliases.len()
                + self.turns.len()
                + self.routing.len()
                + self.bindings.len())
                * 1024
    }

    pub(crate) fn session_cost(&self, session: &str) -> usize {
        let mut items = HashSet::new();
        let mut blocks = HashSet::new();
        let mut settings = HashSet::new();
        let mut cost = 1024;
        for record in self
            .records
            .values()
            .filter(|r| r.identity.session_id == session)
        {
            cost += record.descriptor_cost();
            cost += record
                .settings
                .as_ref()
                .filter(|v| settings.insert(Arc::as_ptr(v) as usize))
                .map_or(0, |v| crate::subscription_index::canonical(v).len() * 4);
            if let Some(history) = &record.history {
                cost += history.retained_cost(&mut blocks, Some(&mut items));
            }
        }
        cost + self.index.get(&Some(session.into())).map_or(0, Radix::cost)
            + self.checkpoint_index_cost(Some(session))
            + self
                .aliases
                .values()
                .chain(self.turns.values())
                .filter(|owner| *owner == session)
                .count()
                * 1024
    }
}
