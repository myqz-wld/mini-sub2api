//! Valid requests share context; each physical socket still owns one active response.
use super::ContextPlan;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_normalizer::StatefulPrepareError as Error;
use crate::subscription_context::Inner;

impl Inner {
    pub(super) fn execution_branch(
        &self,
        plan: &ContextPlan,
        identity: &ResolvedRequestIdentity,
    ) -> Result<(String, String), Error> {
        if plan.socket.as_ref().is_some_and(|socket| {
            self.operations.values().any(|active| {
                active.scope == plan.scope && active.record.socket.as_ref() == Some(socket)
            })
        }) {
            return Err(Error::StateUnavailable);
        }
        let lane_for = |branch: &str| {
            format!(
                "{}:{}:{}:{branch}",
                identity.session_id,
                identity.thread_id,
                identity.turn_id.as_deref().unwrap_or("")
            )
        };
        let occupied = |lane: &str| {
            self.operations
                .values()
                .any(|active| active.scope == plan.scope && active.lane == lane)
        };
        let mut branch = plan
            .branch
            .clone()
            .unwrap_or_else(|| identity.thread_id.clone());
        let mut lane = lane_for(&branch);
        if occupied(&lane) {
            // The caller holds the admission mutex through insertion. Branches use the same
            // format as equivalent anonymous histories. Admission carries this turn's already
            // learned routing value into a conflict fork without sharing execution ownership.
            branch = uuid::Uuid::now_v7().to_string();
            lane = lane_for(&branch);
            if occupied(&lane) {
                return Err(Error::StateUnavailable);
            }
        }
        Ok((branch, lane))
    }
}
