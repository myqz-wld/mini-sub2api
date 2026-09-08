use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_normalizer::{EmulationTransport, StatefulPrepareError as Error};
use crate::subscription_context::{
    Active, CheckpointAssociation, ContextStore, Lease, Operation, Pending, Publication, Record,
    Session,
};
use crate::subscription_index::{History, canonical};
use crate::subscription_request::{Dependencies, Evidence, Format};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

#[path = "subscription_lineage.rs"]
mod lineage;
pub(crate) use lineage::HistoryLineage;

#[path = "subscription_history_selection.rs"]
mod history_selection;

pub(crate) struct ContextPlan {
    pub(crate) evidence: Evidence,
    pub(crate) session: Option<String>,
    pub(crate) turn: Option<String>,
    // Local history/reference evidence; upstream WS reuse has its own request/socket baseline.
    pub(crate) baseline: Option<Record>,
    pub(crate) checkpoint: Option<CheckpointAssociation>,
    pub(crate) branch: Option<String>,
    pub(crate) caller_format: Format,
    pub(crate) admission: Operation,
    pub(crate) dependencies: Dependencies,
    pub(crate) settings: Value,
    pub(crate) external_context: bool,
    pub(crate) scope: String,
    pub(crate) socket: Option<String>,
}

impl ContextStore {
    pub(crate) fn plan(
        &self,
        scope_key: String,
        object: &Map<String, Value>,
        evidence: Evidence,
        binding: Option<&ResolvedRequestIdentity>,
        socket: Option<&str>,
    ) -> Result<ContextPlan, Error> {
        let mut inner = self.inner.lock().map_err(|_| Error::StateUnavailable)?;
        inner.expire(Instant::now());
        let scope = inner.scopes.get(&scope_key);
        if let Some(bound) = binding
            && let Some(frame) =
                crate::subscription_request::selected_session(object, &http::HeaderMap::new())?
        {
            let frame = scope
                .and_then(|scope| scope.aliases.get(&frame))
                .unwrap_or(&frame);
            if *frame != bound.session_id {
                return Err(Error::InvalidRequest);
            }
        }
        let selected = evidence.session.as_ref().map(|raw| {
            scope
                .and_then(|scope| scope.aliases.get(raw))
                .cloned()
                .unwrap_or_else(|| raw.clone())
        });
        if let (Some(selected), Some(bound)) = (&selected, binding)
            && selected != &bound.session_id
        {
            return Err(Error::InvalidRequest);
        }
        let mut session = binding.map(|b| b.session_id.clone()).or(selected);
        let mut matched_len = 0;
        let mut independent_branch = false;
        let mut baseline = if let Some(previous) = &evidence.previous {
            let record = scope
                .and_then(|s| s.records.get(previous))
                .ok_or(Error::StateUnavailable)?;
            if !record.completed {
                return Err(Error::StateUnavailable);
            }
            if session
                .as_ref()
                .is_some_and(|session| session != &record.identity.session_id)
            {
                return Err(Error::InvalidRequest);
            }
            session = Some(record.identity.session_id.clone());
            Some(record.clone())
        } else if let Some(scope) = scope {
            if let Some(found) = scope.match_history(&session, &evidence.input)? {
                matched_len = found.length;
                independent_branch = found.equivalent_count > 1
                    && evidence.session.is_none()
                    && binding.is_none()
                    && evidence.turn.is_none();
                session = Some(found.record.identity.session_id.clone());
                Some(found.record.clone())
            } else {
                None
            }
        } else {
            None
        };

        let checkpoint = if baseline.is_none() && session.is_none() && evidence.previous.is_none() {
            scope
                .map(|scope| scope.checkpoint_association(&evidence.input))
                .transpose()?
                .flatten()
        } else {
            None
        };
        if let Some(source) = &checkpoint {
            session = Some(source.identity.session_id.clone());
        }

        let caller_format = evidence
            .declared_format
            .or_else(|| baseline.as_ref().map(|r| r.caller_format))
            .unwrap_or(Format::Responses);
        if evidence.previous.is_some()
            && let Some(base) = &baseline
            && caller_format != base.caller_format
            && evidence.declared_format == Some(Format::Lite)
        {
            return Err(Error::InvalidRequest);
        }
        let explicit_delta = evidence.previous.is_some();
        if explicit_delta
            && evidence
                .input
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("additional_tools"))
        {
            return Err(Error::InvalidRequest);
        }
        let suffix = if explicit_delta {
            &evidence.input[..]
        } else {
            &evidence.input[matched_len..]
        };
        let has_history = !explicit_delta || baseline.as_ref().is_some_and(|r| r.history.is_some());
        if evidence.transport == EmulationTransport::Http && !has_history {
            return Err(Error::StateUnavailable);
        }
        if explicit_delta && evidence.transport == EmulationTransport::WebSocket {
            let base = baseline.as_ref().expect("validated previous");
            let can_use_remote = socket.is_some()
                && base.socket.as_deref() == socket
                && socket.is_some_and(|id| inner.sockets.contains(id));
            if !can_use_remote && !has_history {
                return Err(Error::StateUnavailable);
            }
        }
        let mut dependencies = if explicit_delta || matched_len > 0 {
            baseline
                .as_ref()
                .map(|r| {
                    if explicit_delta {
                        r.dependencies.clone()
                    } else {
                        local_dependencies(r)
                    }
                })
                .unwrap_or_default()
        } else {
            Dependencies::default()
        };
        let awaiting_tools = dependencies.awaiting_tools();
        dependencies.append(suffix)?;
        let new_user = suffix
            .iter()
            .any(|item| item.get("role").and_then(Value::as_str) == Some("user"));
        let turn = evidence.turn.clone().or_else(|| {
            if !new_user || awaiting_tools {
                baseline
                    .as_ref()
                    .and_then(|r| r.identity.turn_id.clone())
                    .filter(|id| !id.is_empty())
            } else {
                None
            }
        });
        if let (Some(scope), Some(turn), Some(session)) = (scope, &turn, &session)
            && scope.turns.get(turn).is_some_and(|owner| owner != session)
        {
            return Err(Error::InvalidRequest);
        }
        // A full-history match establishes context and identity, never the current configuration.
        let mut effective_settings = crate::subscription_index::settings_for_format(
            object,
            caller_format,
            evidence.transport,
        );
        // Native Lite setup belongs to its input prefix. Ordinary callers keep their own format
        // and apply current caller-first defaults, even if the earlier upstream format was Lite.
        if explicit_delta
            && caller_format == Format::Lite
            && let Some(base) = baseline.as_ref().and_then(|r| r.settings.as_deref())
            && let (Some(base), Some(current)) =
                (base.as_object(), effective_settings.as_object_mut())
        {
            for (key, value) in base {
                current.entry(key).or_insert_with(|| value.clone());
            }
        }
        let branch = if independent_branch {
            Some(Uuid::now_v7().to_string())
        } else {
            baseline
                .as_ref()
                .map(|r| r.branch.clone())
                .or_else(|| checkpoint.as_ref().map(|source| source.branch.clone()))
        };
        let session = session.or_else(|| Some(Uuid::now_v7().to_string()));
        let reserved = canonical(&Value::Array(evidence.input.clone()))
            .len()
            .saturating_mul(8)
            .saturating_add(canonical(&effective_settings).len().saturating_mul(6))
            .saturating_add(self.limits.output_items.saturating_mul(2304))
            .saturating_add(8192)
            .saturating_add(dependencies.cost());
        if !inner.make_room(
            &self.limits,
            &scope_key,
            session.as_deref().expect("session"),
            reserved,
        ) {
            let target_lite = caller_format == Format::Lite
                || object
                    .get("model")
                    .and_then(Value::as_str)
                    .is_some_and(|model| {
                        crate::request_defaults::model_profile(model).responses_lite
                    });
            let remote = evidence.transport == EmulationTransport::WebSocket
                && explicit_delta
                && socket.is_some_and(|id| inner.sockets.contains(id))
                && baseline.as_ref().is_some_and(|base| {
                    base.socket.as_deref() == socket
                        && base.upstream_format
                            == if target_lite {
                                Format::Lite
                            } else {
                                Format::Responses
                            }
                        && (!target_lite
                            || base.setup_hash
                                == crate::subscription_index::setup_hash(&effective_settings))
                });
            if remote && let Some(base) = &mut baseline {
                base.history = None;
                base.settings = None;
            }
            if !remote
                || !inner.make_room(
                    &self.limits,
                    &scope_key,
                    session.as_deref().expect("session"),
                    reserved,
                )
            {
                return Err(Error::StateUnavailable);
            }
        }
        let id = Uuid::now_v7().to_string();
        inner.reservations.insert(
            id.clone(),
            Pending {
                scope: scope_key.clone(),
                session: session.clone().expect("session"),
                reserved,
            },
        );
        let admission = Operation(Arc::new(Lease {
            id,
            store: Arc::downgrade(&self.inner),
        }));
        Ok(ContextPlan {
            evidence,
            session,
            turn,
            baseline,
            checkpoint,
            branch,
            caller_format,
            admission,
            dependencies,
            settings: effective_settings,
            external_context: object.get("conversation").is_some_and(|v| !v.is_null()),
            scope: scope_key,
            socket: socket.map(str::to_string),
        })
    }
}

#[path = "subscription_admission.rs"]
mod admission;

fn local_dependencies(record: &Record) -> Dependencies {
    let mut dependencies = record.dependencies.clone();
    // A content-matched caller may omit non-reference IDs; the verified saved items still supply
    // them. Remote-only IDs removed by compaction must not qualify a full-history candidate.
    dependencies.items = record
        .history
        .as_ref()
        .map_or_else(Default::default, |history| {
            history
                .items()
                .iter()
                .filter_map(|item| {
                    item.value
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect()
        });
    dependencies
}

impl ContextPlan {
    pub(crate) fn identity_baseline(&self) -> Option<&ResolvedRequestIdentity> {
        self.baseline
            .as_ref()
            .map(|record| &record.identity)
            .or_else(|| self.checkpoint.as_ref().map(|source| &source.identity))
    }

    pub(crate) fn full_input(&self, store: &ContextStore) -> Result<Vec<Value>, Error> {
        if self.evidence.previous.is_none() {
            return Ok(self.evidence.input.clone());
        }
        let history = self
            .baseline
            .as_ref()
            .and_then(|r| r.history.as_ref())
            .ok_or(Error::StateUnavailable)?;
        let extra = history
            .items()
            .iter()
            .map(|item| item.cost())
            .sum::<usize>()
            .saturating_mul(2);
        let mut inner = store.inner.lock().map_err(|_| Error::StateUnavailable)?;
        let session = self.session.as_deref().ok_or(Error::StateUnavailable)?;
        if !inner.make_room(&store.limits, &self.scope, session, extra) {
            return Err(Error::StateUnavailable);
        }
        inner
            .reservations
            .get_mut(&self.admission.0.id)
            .ok_or(Error::StateUnavailable)?
            .reserved += extra;
        drop(inner);
        let mut full = history.values();
        // Lightweight live-WS dependencies may outlive items discarded by compaction. Full
        // sending must prove references against the actual replacement window, not those facts.
        let mut dependencies = Dependencies::default();
        dependencies.append(&full)?;
        dependencies.append(&self.evidence.input)?;
        full.extend(self.evidence.input.clone());
        Ok(full)
    }
}
