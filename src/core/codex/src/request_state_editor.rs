use anyhow::Result;
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::fingerprint::FingerprintMode;
use crate::request_state_lookup::LookupKeyFactory;
use crate::request_state_types::ChildThreadEntry;
use crate::request_state_types::ConversationEntry;
use crate::request_state_types::GeneratedItemEntry;
use crate::request_state_types::IdentityEntry;
use crate::request_state_types::MAX_OWNERS;
use crate::request_state_types::PersistedRequestState;
use crate::request_state_types::ScopeState;
use crate::request_state_types::TurnEntry;
use crate::request_state_types::validate_lookup_key;

#[path = "request_state_editor_compaction.rs"]
mod compaction;
#[path = "request_state_editor_existing.rs"]
mod existing;
#[path = "request_state_editor_wire.rs"]
mod wire;
pub(crate) use wire::RequiredWireReferenceUnavailable;

#[derive(Default)]
pub(crate) struct ProtectedStateKeys {
    pub(crate) scopes: BTreeSet<String>,
    pub(crate) installations: BTreeSet<(String, String)>,
    pub(crate) conversations: BTreeSet<(String, String)>,
    pub(crate) child_threads: BTreeSet<(String, String)>,
    pub(crate) turns: BTreeSet<(String, String)>,
    pub(crate) generated_items: BTreeSet<(String, String)>,
    pub(crate) compaction_markers: BTreeSet<(String, String)>,
    pub(crate) wire_ids: BTreeSet<(String, String)>,
}

pub(crate) struct EditSummary {
    pub(crate) changed: bool,
    pub(crate) protected: ProtectedStateKeys,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConversationAssignment {
    pub(crate) id: String,
    pub(crate) window_number: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ThreadAssignment {
    pub(crate) id: String,
    pub(crate) session_id: String,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) window_number: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TurnAssignment {
    pub(crate) id: String,
    pub(crate) root_turn_id: String,
    pub(crate) parent_turn_id: Option<String>,
    pub(crate) started_at_unix_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ItemAssignment {
    pub(crate) id: String,
    pub(crate) create_time_micros: Option<i64>,
}

pub(crate) struct RequestStateEditor<'a> {
    state: &'a mut PersistedRequestState,
    keys: LookupKeyFactory,
    scope_key: String,
    day: i64,
    now_unix_ms: i64,
    changed: bool,
    protected: ProtectedStateKeys,
}

impl<'a> RequestStateEditor<'a> {
    pub(crate) fn new(
        state: &'a mut PersistedRequestState,
        keys: LookupKeyFactory,
        owner_account_ref: &str,
        day: i64,
        now_unix_ms: i64,
    ) -> Result<Self> {
        let scope_key = keys.scope_key();
        validate_lookup_key(&scope_key)?;
        let mut changed = state.owners.insert(owner_account_ref.to_string());
        anyhow::ensure!(
            state.owners.len() <= MAX_OWNERS,
            "too many request state owners"
        );
        let scope = state.scopes.entry(scope_key.clone()).or_insert_with(|| {
            changed = true;
            ScopeState::new(day)
        });
        changed |= touch_day(&mut scope.last_seen_day, day);
        let mut protected = ProtectedStateKeys::default();
        protected.scopes.insert(scope_key.clone());
        Ok(Self {
            state,
            keys,
            scope_key,
            day,
            now_unix_ms,
            changed,
            protected,
        })
    }

    pub(crate) fn installation_id(
        &mut self,
        mode: FingerprintMode,
        scoped_lookup: Option<&str>,
    ) -> Result<String> {
        if mode == FingerprintMode::Device {
            return Ok(self.state.installation_id.clone());
        }
        let key =
            scoped_lookup.ok_or_else(|| anyhow::anyhow!("missing scoped installation key"))?;
        validate_lookup_key(key)?;
        let day = self.day;
        let scope_key = self.scope_key.clone();
        let existed = self.scope().scoped_installations.contains_key(key);
        let (id, touched) = {
            let scope = self.scope_mut();
            let entry = scope
                .scoped_installations
                .entry(key.to_string())
                .or_insert_with(|| IdentityEntry {
                    id: Uuid::new_v4().to_string(),
                    last_seen_day: day,
                });
            let touched = touch_day(&mut entry.last_seen_day, day);
            (entry.id.clone(), touched)
        };
        self.changed |= !existed || touched;
        self.protected
            .installations
            .insert((scope_key, key.to_string()));
        Ok(id)
    }

    pub(crate) fn lookup(&self, kind: &str, raw: &str) -> String {
        self.keys.identity(kind, raw)
    }

    pub(crate) fn derived_lookup(&self, kind: &str, components: &[&[u8]]) -> String {
        self.keys.derived(kind, components)
    }

    pub(crate) fn conversation(&mut self, key: &str) -> Result<ConversationAssignment> {
        validate_lookup_key(key)?;
        let day = self.day;
        let scope_key = self.scope_key.clone();
        let existed = self.scope().conversations.contains_key(key);
        let (assignment, touched) = {
            let scope = self.scope_mut();
            let entry = scope
                .conversations
                .entry(key.to_string())
                .or_insert_with(|| ConversationEntry {
                    id: Uuid::now_v7().to_string(),
                    window_number: 0,
                    current_turn_id: None,
                    last_seen_day: day,
                });
            let touched = touch_day(&mut entry.last_seen_day, day);
            (
                ConversationAssignment {
                    id: entry.id.clone(),
                    window_number: entry.window_number,
                },
                touched,
            )
        };
        self.changed |= !existed || touched;
        self.protected
            .conversations
            .insert((scope_key, key.to_string()));
        Ok(assignment)
    }

    pub(crate) fn child_thread(
        &mut self,
        key: &str,
        session_id: &str,
        parent_thread_id: Option<&str>,
    ) -> Result<ThreadAssignment> {
        validate_lookup_key(key)?;
        let day = self.day;
        let scope_key = self.scope_key.clone();
        let existed = self.scope().child_threads.contains_key(key);
        let (assignment, touched) = {
            let scope = self.scope_mut();
            let entry = scope
                .child_threads
                .entry(key.to_string())
                .or_insert_with(|| ChildThreadEntry {
                    id: Uuid::now_v7().to_string(),
                    session_id: session_id.to_string(),
                    parent_thread_id: parent_thread_id.map(str::to_string),
                    window_number: 0,
                    current_turn_id: None,
                    last_seen_day: day,
                });
            anyhow::ensure!(
                entry.session_id == session_id
                    && entry.parent_thread_id.as_deref() == parent_thread_id,
                "child thread relationship changed"
            );
            let touched = touch_day(&mut entry.last_seen_day, day);
            (
                ThreadAssignment {
                    id: entry.id.clone(),
                    session_id: entry.session_id.clone(),
                    parent_thread_id: entry.parent_thread_id.clone(),
                    window_number: entry.window_number,
                },
                touched,
            )
        };
        self.changed |= !existed || touched;
        self.protected
            .child_threads
            .insert((scope_key, key.to_string()));
        Ok(assignment)
    }

    pub(crate) fn existing_child_thread(&mut self, key: &str) -> Option<ThreadAssignment> {
        let day = self.day;
        let scope_key = self.scope_key.clone();
        let (assignment, touched) = {
            let entry = self.scope_mut().child_threads.get_mut(key)?;
            let touched = touch_day(&mut entry.last_seen_day, day);
            (
                ThreadAssignment {
                    id: entry.id.clone(),
                    session_id: entry.session_id.clone(),
                    parent_thread_id: entry.parent_thread_id.clone(),
                    window_number: entry.window_number,
                },
                touched,
            )
        };
        self.changed |= touched;
        self.protected
            .child_threads
            .insert((scope_key, key.to_string()));
        Some(assignment)
    }

    pub(crate) fn turn(
        &mut self,
        key: &str,
        thread_id: &str,
        root_turn_id: Option<&str>,
        parent_turn_id: Option<&str>,
    ) -> Result<TurnAssignment> {
        validate_lookup_key(key)?;
        let day = self.day;
        let now = self.now_unix_ms;
        let scope_key = self.scope_key.clone();
        let existed = self.scope().turns.contains_key(key);
        let (assignment, touched) = {
            let scope = self.scope_mut();
            let entry = scope.turns.entry(key.to_string()).or_insert_with(|| {
                let id = Uuid::now_v7().to_string();
                TurnEntry {
                    root_turn_id: root_turn_id.unwrap_or(&id).to_string(),
                    id,
                    thread_id: thread_id.to_string(),
                    parent_turn_id: parent_turn_id.map(str::to_string),
                    started_at_unix_ms: now,
                    last_seen_day: day,
                }
            });
            anyhow::ensure!(
                entry.thread_id == thread_id
                    && root_turn_id.is_none_or(|root| entry.root_turn_id == root)
                    && entry.parent_turn_id.as_deref() == parent_turn_id,
                "turn relationship changed"
            );
            let touched = touch_day(&mut entry.last_seen_day, day);
            (
                TurnAssignment {
                    id: entry.id.clone(),
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

    pub(crate) fn existing_turn(&mut self, key: &str) -> Option<TurnAssignment> {
        let day = self.day;
        let scope_key = self.scope_key.clone();
        let (assignment, touched) = {
            let entry = self.scope_mut().turns.get_mut(key)?;
            let touched = touch_day(&mut entry.last_seen_day, day);
            (
                TurnAssignment {
                    id: entry.id.clone(),
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

    pub(crate) fn generated_item(
        &mut self,
        key: &str,
        prefix: &str,
        turn_id: Option<&str>,
        add_create_time: bool,
    ) -> Result<ItemAssignment> {
        validate_lookup_key(key)?;
        validate_prefix(prefix)?;
        let day = self.day;
        let now_micros = self.now_unix_ms.saturating_mul(1_000);
        let scope_key = self.scope_key.clone();
        let existed = self.scope().generated_items.contains_key(key);
        let (assignment, touched) = {
            let scope = self.scope_mut();
            let entry = scope
                .generated_items
                .entry(key.to_string())
                .or_insert_with(|| GeneratedItemEntry {
                    id: format!("{prefix}_{}", Uuid::now_v7()),
                    turn_id: turn_id.map(str::to_string),
                    create_time_micros: add_create_time.then_some(now_micros),
                    last_seen_day: day,
                });
            anyhow::ensure!(
                entry.id.starts_with(&format!("{prefix}_")) && entry.turn_id.as_deref() == turn_id,
                "generated item relationship changed"
            );
            let touched = touch_day(&mut entry.last_seen_day, day);
            (
                ItemAssignment {
                    id: entry.id.clone(),
                    create_time_micros: entry.create_time_micros,
                },
                touched,
            )
        };
        self.changed |= !existed || touched;
        self.protected
            .generated_items
            .insert((scope_key, key.to_string()));
        Ok(assignment)
    }

    pub(crate) fn finish(self) -> EditSummary {
        EditSummary {
            changed: self.changed,
            protected: self.protected,
        }
    }

    fn scope(&self) -> &ScopeState {
        self.state
            .scopes
            .get(&self.scope_key)
            .expect("editor scope exists")
    }

    fn scope_mut(&mut self) -> &mut ScopeState {
        self.state
            .scopes
            .get_mut(&self.scope_key)
            .expect("editor scope exists")
    }
}

fn touch_day(current: &mut i64, day: i64) -> bool {
    replace_if_different(current, day)
}

fn replace_if_different<T: Eq>(current: &mut T, next: T) -> bool {
    if *current == next {
        false
    } else {
        *current = next;
        true
    }
}

fn validate_prefix(prefix: &str) -> Result<()> {
    anyhow::ensure!(
        !prefix.is_empty()
            && prefix.len() <= 32
            && prefix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
        "invalid generated item prefix"
    );
    Ok(())
}
