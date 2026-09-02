use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use uuid::Uuid;

pub(crate) const REQUEST_STATE_VERSION: u32 = 1;
pub(crate) const INITIAL_REQUEST_STATE_REVISION: u64 = 1;
pub(crate) const MAX_REQUEST_STATE_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_OWNERS: usize = 64;
pub(crate) const MAX_SCOPES: usize = 32;
pub(crate) const MAX_SCOPED_INSTALLATIONS: usize = 2_048;
pub(crate) const MAX_CONVERSATIONS: usize = 256;
pub(crate) const MAX_CHILD_THREADS: usize = 1_024;
pub(crate) const MAX_TURNS: usize = 4_096;
pub(crate) const MAX_GENERATED_ITEMS: usize = 8_192;
pub(crate) const MAX_COMPACTION_MARKERS: usize = 4_096;
pub(crate) const MAX_WIRE_ID_PAIRS: usize = 32_768;
pub(crate) const MAX_SCOPE_INSTALLATIONS: usize = 64;
pub(crate) const MAX_SCOPE_CONVERSATIONS: usize = 64;
pub(crate) const MAX_SCOPE_CHILD_THREADS: usize = 256;
pub(crate) const MAX_SCOPE_TURNS: usize = 512;
pub(crate) const MAX_SCOPE_GENERATED_ITEMS: usize = 1_024;
pub(crate) const MAX_SCOPE_COMPACTION_MARKERS: usize = 512;
pub(crate) const MAX_SCOPE_WIRE_ID_PAIRS: usize = 4_096;
pub(crate) const MAX_WIRE_ID_BYTES: usize = 512;
pub(crate) const DETAIL_RETENTION_DAYS: i64 = 30;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PersistedRequestState {
    pub(crate) version: u32,
    pub(crate) revision: u64,
    pub(crate) installation_id: String,
    pub(crate) owners: BTreeSet<String>,
    pub(crate) scopes: BTreeMap<String, ScopeState>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScopeState {
    pub(crate) last_seen_day: i64,
    pub(crate) scoped_installations: BTreeMap<String, IdentityEntry>,
    pub(crate) conversations: BTreeMap<String, ConversationEntry>,
    pub(crate) child_threads: BTreeMap<String, ChildThreadEntry>,
    pub(crate) turns: BTreeMap<String, TurnEntry>,
    pub(crate) generated_items: BTreeMap<String, GeneratedItemEntry>,
    pub(crate) compaction_markers: BTreeMap<String, CompactionMarkerEntry>,
    pub(crate) wire_ids: BTreeMap<String, WireIdEntry>,
    pub(crate) wire_upstream_index: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct IdentityEntry {
    pub(crate) id: String,
    pub(crate) last_seen_day: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConversationEntry {
    pub(crate) id: String,
    pub(crate) window_number: u64,
    pub(crate) current_turn_id: Option<String>,
    pub(crate) last_seen_day: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ChildThreadEntry {
    pub(crate) id: String,
    pub(crate) session_id: String,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) window_number: u64,
    pub(crate) current_turn_id: Option<String>,
    pub(crate) last_seen_day: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TurnEntry {
    pub(crate) id: String,
    pub(crate) thread_id: String,
    pub(crate) root_turn_id: String,
    pub(crate) parent_turn_id: Option<String>,
    pub(crate) started_at_unix_ms: i64,
    pub(crate) last_seen_day: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GeneratedItemEntry {
    pub(crate) id: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) create_time_micros: Option<i64>,
    pub(crate) last_seen_day: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CompactionMarkerEntry {
    pub(crate) thread_id: String,
    pub(crate) window_number: u64,
    pub(crate) last_seen_day: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WireIdDomain {
    Installation,
    Session,
    Thread,
    Turn,
    Response,
    Conversation,
    Stream,
    Item,
    Call,
    Approval,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WireIdOrigin {
    Downstream,
    Upstream,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WireIdEntry {
    pub(crate) domain: WireIdDomain,
    pub(crate) origin: WireIdOrigin,
    pub(crate) downstream_id: String,
    pub(crate) upstream_id: String,
    pub(crate) upstream_lookup: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner: Option<WireIdOwner>,
    pub(crate) last_seen_day: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WireIdOwner {
    pub(crate) session_id: String,
    pub(crate) thread_id: String,
}

impl PersistedRequestState {
    pub(crate) fn new(owners: BTreeSet<String>) -> Self {
        Self {
            version: REQUEST_STATE_VERSION,
            revision: INITIAL_REQUEST_STATE_REVISION,
            installation_id: Uuid::new_v4().to_string(),
            owners,
            scopes: BTreeMap::new(),
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.version == REQUEST_STATE_VERSION,
            "unsupported request state version"
        );
        anyhow::ensure!(
            self.revision >= INITIAL_REQUEST_STATE_REVISION,
            "invalid request state revision"
        );
        validate_uuid_version(&self.installation_id, 4, "installation")?;
        anyhow::ensure!(!self.owners.is_empty(), "request state has no owners");
        anyhow::ensure!(
            self.owners.len() <= MAX_OWNERS,
            "too many request state owners"
        );
        anyhow::ensure!(
            self.scopes.len() <= MAX_SCOPES,
            "too many request state scopes"
        );
        for owner in &self.owners {
            validate_account_ref(owner)?;
        }
        for (scope_key, scope) in &self.scopes {
            validate_lookup_key(scope_key)?;
            scope.validate()?;
        }
        validate_global_caps(self)
    }
}

impl ScopeState {
    pub(crate) fn new(day: i64) -> Self {
        Self {
            last_seen_day: day,
            scoped_installations: BTreeMap::new(),
            conversations: BTreeMap::new(),
            child_threads: BTreeMap::new(),
            turns: BTreeMap::new(),
            generated_items: BTreeMap::new(),
            compaction_markers: BTreeMap::new(),
            wire_ids: BTreeMap::new(),
            wire_upstream_index: BTreeMap::new(),
        }
    }

    fn validate(&self) -> Result<()> {
        validate_day(self.last_seen_day)?;
        validate_map_keys(&self.scoped_installations)?;
        validate_map_keys(&self.conversations)?;
        validate_map_keys(&self.child_threads)?;
        validate_map_keys(&self.turns)?;
        validate_map_keys(&self.generated_items)?;
        validate_map_keys(&self.compaction_markers)?;
        validate_map_keys(&self.wire_ids)?;
        validate_map_keys(&self.wire_upstream_index)?;
        anyhow::ensure!(
            self.scoped_installations.len() <= MAX_SCOPE_INSTALLATIONS,
            "too many scoped installations"
        );
        anyhow::ensure!(
            self.conversations.len() <= MAX_SCOPE_CONVERSATIONS,
            "too many scoped conversations"
        );
        anyhow::ensure!(
            self.child_threads.len() <= MAX_SCOPE_CHILD_THREADS,
            "too many scoped child threads"
        );
        anyhow::ensure!(self.turns.len() <= MAX_SCOPE_TURNS, "too many scoped turns");
        anyhow::ensure!(
            self.generated_items.len() <= MAX_SCOPE_GENERATED_ITEMS,
            "too many scoped generated items"
        );
        anyhow::ensure!(
            self.compaction_markers.len() <= MAX_SCOPE_COMPACTION_MARKERS,
            "too many scoped compaction markers"
        );
        anyhow::ensure!(
            self.wire_ids.len() <= MAX_SCOPE_WIRE_ID_PAIRS,
            "too many scoped wire ID pairs"
        );
        anyhow::ensure!(
            self.wire_upstream_index.len() == self.wire_ids.len(),
            "wire ID index cardinality mismatch"
        );
        for entry in self.scoped_installations.values() {
            validate_uuid_version(&entry.id, 4, "scoped installation")?;
            validate_day(entry.last_seen_day)?;
        }
        for entry in self.conversations.values() {
            validate_uuid_version(&entry.id, 7, "conversation")?;
            validate_optional_uuid_v7(entry.current_turn_id.as_deref(), "conversation turn")?;
            validate_day(entry.last_seen_day)?;
        }
        for entry in self.child_threads.values() {
            validate_uuid_version(&entry.id, 7, "child thread")?;
            validate_uuid_version(&entry.session_id, 7, "child session")?;
            if let Some(parent) = &entry.parent_thread_id {
                validate_uuid_version(parent, 7, "parent thread")?;
            }
            validate_optional_uuid_v7(entry.current_turn_id.as_deref(), "child thread turn")?;
            validate_day(entry.last_seen_day)?;
        }
        validate_thread_relationships(self)?;
        for entry in self.turns.values() {
            validate_uuid_version(&entry.id, 7, "turn")?;
            validate_uuid_version(&entry.thread_id, 7, "turn thread")?;
            validate_uuid_version(&entry.root_turn_id, 7, "root turn")?;
            if let Some(parent) = &entry.parent_turn_id {
                validate_uuid_version(parent, 7, "parent turn")?;
            }
            anyhow::ensure!(entry.started_at_unix_ms >= 0, "invalid turn start time");
            validate_day(entry.last_seen_day)?;
        }
        validate_turn_relationships(self)?;
        for (thread_id, current_turn_id) in self
            .conversations
            .values()
            .map(|entry| (&entry.id, entry.current_turn_id.as_deref()))
            .chain(
                self.child_threads
                    .values()
                    .map(|entry| (&entry.id, entry.current_turn_id.as_deref())),
            )
        {
            if let Some(current_turn_id) = current_turn_id {
                anyhow::ensure!(
                    self.turns
                        .values()
                        .any(|turn| { turn.id == current_turn_id && turn.thread_id == *thread_id }),
                    "current turn pointer is dangling"
                );
            }
        }
        for entry in self.generated_items.values() {
            validate_prefixed_uuid_v7(&entry.id)?;
            if let Some(turn_id) = &entry.turn_id {
                validate_uuid_version(turn_id, 7, "item turn")?;
                anyhow::ensure!(
                    self.turns.values().any(|turn| turn.id == *turn_id),
                    "generated item turn is dangling"
                );
            }
            if let Some(create_time) = entry.create_time_micros {
                anyhow::ensure!(create_time >= 0, "invalid item create time");
            }
            validate_day(entry.last_seen_day)?;
        }
        for entry in self.compaction_markers.values() {
            validate_uuid_version(&entry.thread_id, 7, "compaction thread")?;
            anyhow::ensure!(
                thread_belongs_to_session(self, &entry.thread_id).is_some(),
                "compaction thread is dangling"
            );
            validate_day(entry.last_seen_day)?;
        }
        for (downstream_lookup, entry) in &self.wire_ids {
            validate_wire_id(&entry.downstream_id)?;
            validate_wire_id(&entry.upstream_id)?;
            anyhow::ensure!(
                entry.downstream_id != entry.upstream_id,
                "wire ID pair is not pseudonymized"
            );
            validate_lookup_key(&entry.upstream_lookup)?;
            if let Some(owner) = &entry.owner {
                anyhow::ensure!(
                    entry.domain == WireIdDomain::Response,
                    "only response wire IDs may own request identity"
                );
                validate_uuid_version(&owner.session_id, 7, "wire owner session")?;
                validate_uuid_version(&owner.thread_id, 7, "wire owner thread")?;
                anyhow::ensure!(
                    thread_belongs_to_session(self, &owner.thread_id)
                        .is_some_and(|session| session == owner.session_id),
                    "wire ID owner is dangling"
                );
            }
            validate_day(entry.last_seen_day)?;
            anyhow::ensure!(
                self.wire_upstream_index.get(&entry.upstream_lookup) == Some(downstream_lookup),
                "wire ID reverse index mismatch"
            );
        }
        for (upstream_lookup, downstream_lookup) in &self.wire_upstream_index {
            validate_lookup_key(upstream_lookup)?;
            validate_lookup_key(downstream_lookup)?;
            anyhow::ensure!(
                self.wire_ids
                    .get(downstream_lookup)
                    .is_some_and(|entry| entry.upstream_lookup == *upstream_lookup),
                "wire ID reverse index is dangling"
            );
        }
        Ok(())
    }
}

fn validate_thread_relationships(scope: &ScopeState) -> Result<()> {
    for child in scope.child_threads.values() {
        anyhow::ensure!(
            scope
                .conversations
                .values()
                .any(|conversation| conversation.id == child.session_id),
            "child session is dangling"
        );
        if let Some(parent_id) = &child.parent_thread_id {
            anyhow::ensure!(parent_id != &child.id, "child thread is its own parent");
            anyhow::ensure!(
                thread_belongs_to_session(scope, parent_id)
                    .is_some_and(|session| session == child.session_id),
                "parent thread is dangling or crosses sessions"
            );
        }
        let mut current = child.parent_thread_id.as_deref();
        for _ in 0..=scope.child_threads.len() {
            let Some(parent_id) = current else {
                break;
            };
            if parent_id == child.session_id {
                current = None;
                break;
            }
            let parent = scope
                .child_threads
                .values()
                .find(|candidate| candidate.id == parent_id)
                .ok_or_else(|| anyhow::anyhow!("parent thread is dangling"))?;
            current = parent.parent_thread_id.as_deref();
        }
        anyhow::ensure!(current.is_none(), "child thread ancestry contains a cycle");
    }
    Ok(())
}

fn validate_turn_relationships(scope: &ScopeState) -> Result<()> {
    for turn in scope.turns.values() {
        let session = thread_belongs_to_session(scope, &turn.thread_id)
            .ok_or_else(|| anyhow::anyhow!("turn thread is dangling"))?;
        let root = scope
            .turns
            .values()
            .find(|candidate| candidate.id == turn.root_turn_id)
            .ok_or_else(|| anyhow::anyhow!("root turn is dangling"))?;
        anyhow::ensure!(
            root.root_turn_id == root.id,
            "root turn does not identify itself"
        );
        anyhow::ensure!(
            thread_belongs_to_session(scope, &root.thread_id) == Some(session),
            "root turn crosses sessions"
        );
        if let Some(parent_id) = &turn.parent_turn_id {
            anyhow::ensure!(parent_id != &turn.id, "turn is its own parent");
            let parent = scope
                .turns
                .values()
                .find(|candidate| candidate.id == *parent_id)
                .ok_or_else(|| anyhow::anyhow!("parent turn is dangling"))?;
            anyhow::ensure!(
                parent.root_turn_id == turn.root_turn_id,
                "parent turn crosses roots"
            );
            anyhow::ensure!(
                thread_belongs_to_session(scope, &parent.thread_id) == Some(session),
                "parent turn crosses sessions"
            );
        }
        let mut current = turn.parent_turn_id.as_deref();
        for _ in 0..=scope.turns.len() {
            let Some(parent_id) = current else {
                break;
            };
            let parent = scope
                .turns
                .values()
                .find(|candidate| candidate.id == parent_id)
                .ok_or_else(|| anyhow::anyhow!("parent turn is dangling"))?;
            current = parent.parent_turn_id.as_deref();
        }
        anyhow::ensure!(current.is_none(), "turn ancestry contains a cycle");
    }
    Ok(())
}

fn thread_belongs_to_session<'a>(scope: &'a ScopeState, thread_id: &str) -> Option<&'a str> {
    if let Some(conversation) = scope
        .conversations
        .values()
        .find(|conversation| conversation.id == thread_id)
    {
        return Some(conversation.id.as_str());
    }
    scope
        .child_threads
        .values()
        .find(|thread| thread.id == thread_id)
        .map(|thread| thread.session_id.as_str())
}

fn validate_global_caps(state: &PersistedRequestState) -> Result<()> {
    let totals = state
        .scopes
        .values()
        .fold([0_usize; 7], |mut totals, scope| {
            totals[0] += scope.scoped_installations.len();
            totals[1] += scope.conversations.len();
            totals[2] += scope.child_threads.len();
            totals[3] += scope.turns.len();
            totals[4] += scope.generated_items.len();
            totals[5] += scope.compaction_markers.len();
            totals[6] += scope.wire_ids.len();
            totals
        });
    for (actual, maximum, description) in [
        (totals[0], MAX_SCOPED_INSTALLATIONS, "scoped installations"),
        (totals[1], MAX_CONVERSATIONS, "conversations"),
        (totals[2], MAX_CHILD_THREADS, "child threads"),
        (totals[3], MAX_TURNS, "turns"),
        (totals[4], MAX_GENERATED_ITEMS, "generated items"),
        (totals[5], MAX_COMPACTION_MARKERS, "compaction markers"),
        (totals[6], MAX_WIRE_ID_PAIRS, "wire ID pairs"),
    ] {
        anyhow::ensure!(actual <= maximum, "too many request state {description}");
    }
    Ok(())
}

fn validate_map_keys<T>(map: &BTreeMap<String, T>) -> Result<()> {
    for key in map.keys() {
        validate_lookup_key(key)?;
    }
    Ok(())
}

pub(crate) fn validate_lookup_key(key: &str) -> Result<()> {
    let encoded = key.strip_prefix("lk_").unwrap_or_default();
    anyhow::ensure!(encoded.len() == 43, "invalid request state lookup key");
    anyhow::ensure!(
        encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "invalid request state lookup key"
    );
    Ok(())
}

pub(crate) fn validate_account_ref(account_ref: &str) -> Result<()> {
    let suffix = account_ref.strip_prefix("acct_").unwrap_or_default();
    anyhow::ensure!(
        !suffix.is_empty()
            && suffix.len() <= 128
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "invalid request state owner"
    );
    Ok(())
}

fn validate_uuid_version(value: &str, version: usize, description: &str) -> Result<()> {
    let parsed =
        Uuid::parse_str(value).map_err(|_| anyhow::anyhow!("invalid {description} UUID"))?;
    anyhow::ensure!(
        parsed.get_version_num() == version,
        "invalid {description} UUID version"
    );
    Ok(())
}

fn validate_optional_uuid_v7(value: Option<&str>, description: &str) -> Result<()> {
    if let Some(value) = value {
        validate_uuid_version(value, 7, description)?;
    }
    Ok(())
}

fn validate_prefixed_uuid_v7(value: &str) -> Result<()> {
    let (prefix, uuid) = value.split_once('_').unwrap_or_default();
    anyhow::ensure!(
        !prefix.is_empty()
            && prefix.len() <= 32
            && prefix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
        "invalid generated item prefix"
    );
    validate_uuid_version(uuid, 7, "generated item")
}

pub(crate) fn validate_wire_id(value: &str) -> Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= MAX_WIRE_ID_BYTES
            && !value.chars().any(char::is_control),
        "invalid persisted wire ID"
    );
    Ok(())
}

fn validate_day(day: i64) -> Result<()> {
    anyhow::ensure!(day >= 0, "invalid request state activity day");
    Ok(())
}
