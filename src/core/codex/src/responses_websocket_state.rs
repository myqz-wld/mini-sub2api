use crate::request_compaction::PendingCompaction;
use crate::request_profile::CallerKind;
use crate::request_profile::UpstreamProfile;
use crate::responses_websocket_projection::encoded_len_within;
use crate::responses_websocket_projection::equivalent_items;
use crate::responses_websocket_projection::output_encoded_len;
use crate::responses_websocket_projection::reusable_item;
use crate::responses_websocket_reuse::RequestSnapshot;
use crate::responses_websocket_reuse::ReuseBaseline;
use crate::responses_websocket_reuse::has_explicit_state_carrier;
use crate::responses_websocket_reuse::incremental_input;
use crate::responses_websocket_reuse::lite_prewarm_prefix;
use crate::responses_websocket_reuse::request_snapshot;
use serde_json::Value;

const DEFAULT_MAX_OUTPUT_ITEMS: usize = 1024;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

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

struct PlannedOperation {
    kind: OperationKind,
    request: Option<RequestSnapshot>,
    pending_compaction: Option<PendingCompaction>,
}

struct ActiveOperation {
    kind: OperationKind,
    request: Option<RequestSnapshot>,
    output: Vec<Value>,
    output_bytes: usize,
    reusable: bool,
    pending_compaction: Option<PendingCompaction>,
}

/// Pure, socket-local continuation state. Values held here must never be logged or persisted.
pub(crate) struct ResponsesWebSocketState {
    caller: CallerKind,
    profile: UpstreamProfile,
    baseline: Option<ReuseBaseline>,
    planned: Option<PlannedOperation>,
    active: Option<ActiveOperation>,
    setup_phase: OperationPhase,
    public_phase: OperationPhase,
    max_output_items: usize,
    max_output_bytes: usize,
}

impl ResponsesWebSocketState {
    pub(crate) fn new(caller: CallerKind, profile: UpstreamProfile) -> Self {
        Self {
            caller,
            profile,
            baseline: None,
            planned: None,
            active: None,
            setup_phase: OperationPhase::Idle,
            public_phase: OperationPhase::Idle,
            max_output_items: DEFAULT_MAX_OUTPUT_ITEMS,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    #[cfg(test)]
    pub(super) fn with_output_limits(
        caller: CallerKind,
        profile: UpstreamProfile,
        max_output_items: usize,
        max_output_bytes: usize,
    ) -> Self {
        Self {
            max_output_items,
            max_output_bytes,
            ..Self::new(caller, profile)
        }
    }

    pub(crate) fn setup_phase(&self) -> OperationPhase {
        self.setup_phase
    }

    pub(crate) fn public_phase(&self) -> OperationPhase {
        self.public_phase
    }

    pub(crate) fn public_create_attempted(&self) -> bool {
        matches!(
            self.public_phase,
            OperationPhase::Attempted
                | OperationPhase::ResponseObserved
                | OperationPhase::Completed
                | OperationPhase::Failed
        )
    }

    #[cfg(test)]
    pub(crate) fn plan_hidden_setup(
        &mut self,
        request: &Value,
        mode: PrewarmMode,
    ) -> Option<HiddenSetupPlan> {
        self.plan_hidden_setup_with_synthesized_ids(request, mode, &[])
    }

    pub(crate) fn plan_hidden_setup_with_synthesized_ids(
        &mut self,
        request: &Value,
        mode: PrewarmMode,
        synthesized_item_ids: &[String],
    ) -> Option<HiddenSetupPlan> {
        if !self.automatic_reuse_enabled()
            || self.setup_phase != OperationPhase::Idle
            || self.baseline.is_some()
            || self.planned.is_some()
            || self.active.is_some()
            || has_explicit_state_carrier(request)
        {
            return None;
        }

        // Validate the eventual public input before sending any synthesized setup frame.
        request_snapshot(request, synthesized_item_ids)?;
        let mut frame = request.as_object()?.clone();
        let input = frame.get("input")?.as_array()?;
        let prefix = match mode {
            PrewarmMode::Ordinary => Vec::new(),
            PrewarmMode::ResponsesLite => lite_prewarm_prefix(input)?,
        };
        frame.insert("input".to_string(), Value::Array(prefix));
        frame.insert("generate".to_string(), Value::Bool(false));
        let frame = Value::Object(frame);
        let request = request_snapshot(&frame, synthesized_item_ids)?;

        self.planned = Some(PlannedOperation {
            kind: OperationKind::HiddenSetup,
            request: Some(request),
            pending_compaction: None,
        });
        self.setup_phase = OperationPhase::Planned;
        Some(HiddenSetupPlan { frame })
    }

    pub(crate) fn plan_public_create(&mut self, request: &Value) -> PublicCreatePlan {
        self.plan_public_create_with_synthesized_ids(request, &[])
    }

    pub(crate) fn plan_public_create_with_synthesized_ids(
        &mut self,
        request: &Value,
        synthesized_item_ids: &[String],
    ) -> PublicCreatePlan {
        self.plan_public_create_with_state(request, synthesized_item_ids, None)
    }

    pub(crate) fn plan_public_create_with_state(
        &mut self,
        request: &Value,
        synthesized_item_ids: &[String],
        pending_compaction: Option<PendingCompaction>,
    ) -> PublicCreatePlan {
        self.abandon_pending_operation();
        let explicit_state = has_explicit_state_carrier(request);
        let automatic = self.automatic_reuse_enabled() && !explicit_state;
        let request_snapshot = automatic
            .then(|| request_snapshot(request, synthesized_item_ids))
            .flatten();
        let mut frame = request.clone();

        let mode = if explicit_state {
            self.baseline = None;
            PublicCreateMode::ExplicitState
        } else if !automatic {
            self.baseline = None;
            if self.profile == UpstreamProfile::BareOpenAi {
                PublicCreateMode::Passthrough
            } else {
                PublicCreateMode::Full
            }
        } else if let (Some(baseline), Some(current)) = (&self.baseline, &request_snapshot) {
            if let Some(delta) = incremental_input(baseline, current) {
                if let Some(object) = frame.as_object_mut() {
                    object.insert(
                        "previous_response_id".to_string(),
                        Value::String(baseline.response_id.clone()),
                    );
                    object.insert("input".to_string(), Value::Array(delta));
                    PublicCreateMode::Incremental
                } else {
                    self.baseline = None;
                    PublicCreateMode::Full
                }
            } else {
                self.baseline = None;
                PublicCreateMode::Full
            }
        } else {
            self.baseline = None;
            PublicCreateMode::Full
        };

        self.planned = Some(PlannedOperation {
            kind: OperationKind::PublicCreate,
            request: request_snapshot,
            pending_compaction,
        });
        self.public_phase = OperationPhase::Planned;
        PublicCreatePlan { frame, mode }
    }

    pub(crate) fn mark_hidden_setup_attempted(&mut self) -> bool {
        self.activate(OperationKind::HiddenSetup)
    }

    pub(crate) fn mark_public_create_attempted(&mut self) -> bool {
        self.activate(OperationKind::PublicCreate)
    }

    pub(crate) fn observe_server_event(&mut self, event: &Value) -> EventDisposition {
        self.observe_server_event_with_compaction(event).disposition
    }

    pub(crate) fn observe_server_event_with_compaction(
        &mut self,
        event: &Value,
    ) -> ObservedServerEvent {
        let Some(kind) = self.active.as_ref().map(|active| active.kind) else {
            return ObservedServerEvent {
                disposition: EventDisposition::Unassociated,
                completed_compaction: None,
            };
        };
        let disposition = match kind {
            OperationKind::HiddenSetup => EventDisposition::ConsumeHiddenSetup,
            OperationKind::PublicCreate => EventDisposition::ForwardPublic,
        };
        self.set_phase(kind, OperationPhase::ResponseObserved);

        let Some(event_type) = event
            .as_object()
            .and_then(|object| object.get("type"))
            .and_then(Value::as_str)
        else {
            if let Some(active) = &mut self.active {
                active.reusable = false;
            }
            return ObservedServerEvent {
                disposition,
                completed_compaction: None,
            };
        };

        let mut completed_compaction = None;
        match event_type {
            "response.output_item.done" => self.observe_output_item(event),
            "response.completed" => completed_compaction = self.complete_active(event),
            "response.failed" | "response.incomplete" | "error" => self.fail_active(kind),
            _ => {}
        }
        ObservedServerEvent {
            disposition,
            completed_compaction,
        }
    }

    pub(crate) fn fail_hidden_setup(&mut self) {
        self.fail_operation(OperationKind::HiddenSetup);
    }

    pub(crate) fn fail_public_create(&mut self) {
        self.fail_operation(OperationKind::PublicCreate);
    }

    pub(crate) fn reset_for_reconnect(&mut self) {
        self.baseline = None;
        self.planned = None;
        self.active = None;
        self.setup_phase = OperationPhase::Idle;
        self.public_phase = OperationPhase::Idle;
    }

    pub(crate) fn reset(&mut self) {
        self.reset_for_reconnect();
    }

    fn automatic_reuse_enabled(&self) -> bool {
        self.caller == CallerKind::Bare && self.profile.uses_subscription_transport()
    }

    fn activate(&mut self, expected: OperationKind) -> bool {
        let Some(planned) = self.planned.take() else {
            return false;
        };
        if planned.kind != expected || self.active.is_some() {
            self.planned = Some(planned);
            return false;
        }
        self.active = Some(ActiveOperation {
            kind: planned.kind,
            request: planned.request,
            output: Vec::new(),
            output_bytes: 0,
            reusable: true,
            pending_compaction: planned.pending_compaction,
        });
        self.set_phase(expected, OperationPhase::Attempted);
        true
    }

    fn observe_output_item(&mut self, event: &Value) {
        let item = event.as_object().and_then(|object| object.get("item"));
        let max_output_items = self.max_output_items;
        let max_output_bytes = self.max_output_bytes;
        let Some(active) = &mut self.active else {
            return;
        };
        if !active.reusable {
            return;
        }
        let Some(item) = item.filter(|item| reusable_item(item)) else {
            abandon_output(active);
            return;
        };
        let remaining = max_output_bytes.saturating_sub(active.output_bytes);
        let Some(encoded) = encoded_len_within(item, remaining) else {
            abandon_output(active);
            return;
        };
        if active.output.len() >= max_output_items {
            abandon_output(active);
            return;
        }
        active.output.push(item.clone());
        active.output_bytes = active.output_bytes.saturating_add(encoded);
    }

    fn complete_active(&mut self, event: &Value) -> Option<PendingCompaction> {
        let mut active = self.active.take()?;
        let response = event
            .as_object()
            .and_then(|object| object.get("response"))
            .and_then(Value::as_object);
        let response_id = response
            .and_then(|response| response.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());

        if let Some(output) = response
            .and_then(|response| response.get("output"))
            .and_then(Value::as_array)
        {
            if !output.iter().all(reusable_item)
                || output.len() > self.max_output_items
                || output_encoded_len(output, self.max_output_bytes).is_none()
            {
                abandon_output(&mut active);
            } else if active.output.is_empty() {
                active.output.clone_from(output);
            } else if !equivalent_items(&active.output, output) {
                abandon_output(&mut active);
            }
        }

        self.baseline = match (active.reusable, active.request, response_id) {
            (true, Some(request), Some(response_id)) => Some(ReuseBaseline {
                request,
                response_id: response_id.to_string(),
                output: active.output,
            }),
            _ => None,
        };
        self.set_phase(active.kind, OperationPhase::Completed);
        active.pending_compaction
    }

    fn fail_active(&mut self, kind: OperationKind) {
        self.active = None;
        self.planned = None;
        self.baseline = None;
        self.set_phase(kind, OperationPhase::Failed);
    }

    fn fail_operation(&mut self, kind: OperationKind) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.kind == kind)
        {
            self.active = None;
        }
        if self
            .planned
            .as_ref()
            .is_some_and(|planned| planned.kind == kind)
        {
            self.planned = None;
        }
        self.baseline = None;
        self.set_phase(kind, OperationPhase::Failed);
    }

    fn abandon_pending_operation(&mut self) {
        let kind = self
            .active
            .as_ref()
            .map(|active| active.kind)
            .or_else(|| self.planned.as_ref().map(|planned| planned.kind));
        if let Some(kind) = kind {
            self.fail_operation(kind);
        }
    }

    fn set_phase(&mut self, kind: OperationKind, phase: OperationPhase) {
        match kind {
            OperationKind::HiddenSetup => self.setup_phase = phase,
            OperationKind::PublicCreate => self.public_phase = phase,
        }
    }
}

fn abandon_output(active: &mut ActiveOperation) {
    active.output.clear();
    active.output_bytes = 0;
    active.reusable = false;
}

#[cfg(test)]
#[path = "responses_websocket_state_tests.rs"]
mod tests;
