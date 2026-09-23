use crate::request_compaction::PendingCompaction;
use crate::request_profile::CallerKind;
use crate::request_profile::UpstreamProfile;
use crate::responses_websocket_reuse::RequestSnapshot;
use crate::responses_websocket_reuse::ReuseBaseline;
use crate::responses_websocket_reuse::has_explicit_state_carrier;
use crate::responses_websocket_reuse::incremental_input;
use crate::responses_websocket_reuse::lite_prewarm_prefix;
use crate::responses_websocket_reuse::request_snapshot;
use serde_json::Value;

#[path = "responses_websocket_state_output.rs"]
mod output;
use output::abandon_output;

#[path = "responses_websocket_state_types.rs"]
mod types;
pub(crate) use types::*;

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
    compaction_output: crate::request_compaction::CompactionOutput,
    output_lifecycle: crate::response_output::OutputLifecycle,
    reusable: bool,
    pending_compaction: Option<PendingCompaction>,
}

/// Pure, socket-local continuation state. Values held here must never be logged or persisted.
pub(crate) struct ResponsesWebSocketState {
    caller: CallerKind,
    profile: UpstreamProfile,
    baseline: Option<ReuseBaseline>,
    setup_turn_state: Option<String>,
    planned: Option<PlannedOperation>,
    active: Option<ActiveOperation>,
    setup_phase: OperationPhase,
    public_phase: OperationPhase,
    rebuilt_reference: bool,
    max_output_items: usize,
    max_output_bytes: usize,
}

impl ResponsesWebSocketState {
    pub(crate) fn new(caller: CallerKind, profile: UpstreamProfile) -> Self {
        Self {
            caller,
            profile,
            baseline: None,
            setup_turn_state: None,
            planned: None,
            active: None,
            setup_phase: OperationPhase::Idle,
            public_phase: OperationPhase::Idle,
            rebuilt_reference: false,
            max_output_items: crate::inference_limits::get().output_items,
            max_output_bytes: crate::inference_limits::get().output_bytes,
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

    pub(crate) fn mark_rebuilt_reference(&mut self, reconstructed: bool) {
        self.rebuilt_reference = reconstructed;
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
            || self.rebuilt_reference
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
        let mut frame = Value::Object(frame);
        let request = request_snapshot(&frame, synthesized_item_ids)?;

        self.planned = Some(PlannedOperation {
            kind: OperationKind::HiddenSetup,
            request: Some(request),
            pending_compaction: None,
        });
        self.setup_phase = OperationPhase::Planned;
        if let Some(object) = frame.as_object_mut() {
            crate::request_normalizer::finalize_wire_order(
                object,
                crate::request_normalizer::EmulationTransport::WebSocket,
            );
        }
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
        let explicit_state =
            std::mem::take(&mut self.rebuilt_reference) || has_explicit_state_carrier(request);
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
            if self.profile == UpstreamProfile::ApiKeyPassthrough {
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

        if self.profile.emulates_codex()
            && let Some(object) = frame.as_object_mut()
        {
            crate::request_normalizer::finalize_wire_order(
                object,
                crate::request_normalizer::EmulationTransport::WebSocket,
            );
        }
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
        if kind == OperationKind::HiddenSetup
            && self.setup_turn_state.is_none()
            && let Some(token) = crate::subscription_routing::metadata_token(event)
            && crate::subscription_routing::validate_token(token).is_ok()
        {
            self.setup_turn_state = Some(token.to_string());
        }
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

        if let Some(active) = self.active.as_mut()
            && active
                .output_lifecycle
                .observe(event, self.max_output_items)
                .is_err()
        {
            self.fail_active(kind);
            return ObservedServerEvent {
                disposition,
                completed_compaction: None,
            };
        }
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
        self.rebuilt_reference = false;
        self.setup_turn_state = None;
        self.baseline = None;
        self.planned = None;
        self.active = None;
        self.setup_phase = OperationPhase::Idle;
        self.public_phase = OperationPhase::Idle;
    }

    pub(crate) fn reset(&mut self) {
        self.reset_for_reconnect();
    }

    pub(crate) fn setup_turn_state(&self) -> Option<&str> {
        (self.setup_phase == OperationPhase::Completed)
            .then_some(self.setup_turn_state.as_deref())
            .flatten()
    }

    pub(crate) fn expire_baseline(&mut self) {
        self.baseline = None;
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.setup_turn_state.as_ref().map_or(0, String::len)
            + self.baseline.as_ref().map_or(0, |b| {
                b.request.cost()
                    + b.output
                        .iter()
                        .map(|v| serde_json::to_vec(v).map_or(0, |b| b.len() * 4 + 64))
                        .sum::<usize>()
            })
            + self
                .planned
                .as_ref()
                .and_then(|p| p.request.as_ref())
                .map_or(0, RequestSnapshot::cost)
            + self.active.as_ref().map_or(0, |a| {
                a.request.as_ref().map_or(0, RequestSnapshot::cost)
                    + a.output_bytes * 4
                    + a.output_lifecycle.retained_bytes()
            })
    }

    pub(crate) fn abandon_cached_bodies(&mut self) {
        self.baseline = None;
        if let Some(active) = &mut self.active {
            active.request = None;
            abandon_output(active);
        }
        if let Some(planned) = &mut self.planned {
            planned.request = None;
        }
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
            compaction_output: Default::default(),
            output_lifecycle: Default::default(),
            reusable: true,
            pending_compaction: planned.pending_compaction,
        });
        self.set_phase(expected, OperationPhase::Attempted);
        true
    }

    fn fail_active(&mut self, kind: OperationKind) {
        if kind == OperationKind::HiddenSetup {
            self.setup_turn_state = None;
        }
        self.active = None;
        self.planned = None;
        self.baseline = None;
        self.set_phase(kind, OperationPhase::Failed);
    }

    fn fail_operation(&mut self, kind: OperationKind) {
        if kind == OperationKind::HiddenSetup {
            self.setup_turn_state = None;
        }
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

#[cfg(test)]
#[path = "responses_websocket_state_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "responses_websocket_boundary_tests.rs"]
mod boundary_tests;
