use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use uuid::Uuid;

#[path = "request_state_validation.rs"]
mod validation;
pub(crate) use validation::validate_account_ref;
pub(crate) use validation::validate_lookup_key;
pub(crate) use validation::validate_wire_id;

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
}
