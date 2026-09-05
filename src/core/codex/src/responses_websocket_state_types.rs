use crate::request_compaction::PendingCompaction;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrewarmMode {
    Ordinary,
    ResponsesLite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicCreateMode {
    Passthrough,
    ExplicitState,
    Full,
    Incremental,
}

pub(crate) struct HiddenSetupPlan {
    pub(crate) frame: Value,
}

pub(crate) struct PublicCreatePlan {
    pub(crate) frame: Value,
    pub(crate) mode: PublicCreateMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationKind {
    HiddenSetup,
    PublicCreate,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum OperationPhase {
    #[default]
    Idle,
    Planned,
    Attempted,
    ResponseObserved,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EventDisposition {
    Unassociated,
    ConsumeHiddenSetup,
    ForwardPublic,
}

pub(crate) struct ObservedServerEvent {
    pub(crate) disposition: EventDisposition,
    pub(crate) completed_compaction: Option<PendingCompaction>,
}
