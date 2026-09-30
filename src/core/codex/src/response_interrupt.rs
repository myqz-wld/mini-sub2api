//! Correlate the native one-shot interrupt with the active response, without retaining content.
use serde_json::Value;

#[cfg(test)]
#[path = "response_interrupt_fixtures.rs"]
pub(crate) mod fixtures;

pub(crate) fn control_id(value: &Value) -> anyhow::Result<&str> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("invalid interrupt"))?;
    anyhow::ensure!(
        object.len() == 3
            && value["type"] == "response.interrupt"
            && value["mode"] == "discard_partial_items",
        "invalid interrupt shape"
    );
    let id = value["response_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("interrupt response is missing"))?;
    crate::request_state_types::validate_wire_id(id)?;
    Ok(id)
}

pub(crate) fn is_control(text: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Kind {
        #[serde(rename = "type")]
        kind: String,
    }
    serde_json::from_str::<Kind>(text).is_ok_and(|value| value.kind == "response.interrupt")
}

pub(crate) fn interrupted(event: &Value) -> bool {
    event["type"] == "response.incomplete"
        && event
            .pointer("/response/incomplete_details/reason")
            .and_then(Value::as_str)
            == Some("interrupted")
        && event
            .pointer("/response/status")
            .is_none_or(|status| status.as_str() == Some("incomplete"))
}

#[derive(Default)]
pub(crate) struct InterruptState {
    response_id: Option<String>,
    requested: bool,
}

impl InterruptState {
    pub(crate) fn request(&mut self, id: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.requested && self.response_id.as_deref() == Some(id),
            "interrupt does not target the active response"
        );
        self.requested = true;
        Ok(())
    }

    pub(crate) fn requested(&self) -> bool {
        self.requested
    }

    pub(crate) fn observe(&mut self, event: &Value) -> anyhow::Result<()> {
        let kind = event["type"].as_str().unwrap_or_default();
        if kind == "response.created" {
            if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                crate::request_state_types::validate_wire_id(id)?;
                anyhow::ensure!(
                    self.response_id.as_deref().is_none_or(|known| known == id),
                    "response changed during inference"
                );
                self.response_id = Some(id.into());
            }
        } else if matches!(
            kind,
            "response.interrupt.accepted" | "response.output_item.interrupted"
        ) {
            anyhow::ensure!(
                self.requested
                    && self
                        .response_id
                        .as_deref()
                        .is_some_and(|id| { event["response_id"].as_str() == Some(id) }),
                "unassociated interruption event"
            );
        }
        Ok(())
    }

    pub(crate) fn completes(&self, event: &Value) -> bool {
        self.requested
            && interrupted(event)
            && self
                .response_id
                .as_deref()
                .is_some_and(|id| event.pointer("/response/id").and_then(Value::as_str) == Some(id))
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.response_id.as_ref().map_or(0, String::len)
    }
}
