use super::{OperationKind, ResponsesWebSocketState};

impl ResponsesWebSocketState {
    /// ContextStore has already validated the public Key/thread/socket and Lite request.
    /// Check the upstream response again immediately before sending the translated control.
    pub(crate) fn request_interrupt(&mut self, response_id: &str) -> anyhow::Result<()> {
        let active = self
            .active
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no active response"))?;
        anyhow::ensure!(
            active.kind == OperationKind::PublicCreate && active.pending_compaction.is_none(),
            "cannot interrupt setup or compaction"
        );
        active.interruption.request(response_id)
    }
}
