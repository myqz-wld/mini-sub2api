//! Native inter-agent input semantics, distinct from assistant output messages.
use serde_json::Value;

pub(crate) struct AgentMessage<'a> {
    author: &'a str,
    recipient: &'a str,
    content: &'a [Value],
}

impl<'a> AgentMessage<'a> {
    pub(crate) fn read(item: &'a Value) -> Option<Self> {
        if item.get("type")?.as_str()? != "agent_message" {
            return None;
        }
        let message = Self {
            author: item.get("author")?.as_str()?,
            recipient: item.get("recipient")?.as_str()?,
            content: item.get("content")?.as_array()?,
        };
        for part in message.content {
            match part.get("type")?.as_str()? {
                "input_text" => part.get("text")?.as_str()?,
                "encrypted_content" => part.get("encrypted_content")?.as_str()?,
                _ => return None,
            };
        }
        Some(message)
    }

    fn first_text(&self) -> Option<&str> {
        let first = self.content.first()?;
        match first.get("type")?.as_str()? {
            "input_text" => first.get("text")?.as_str(),
            _ => None,
        }
    }

    fn new_task(&self) -> bool {
        self.first_text()
            .is_some_and(|text| text.starts_with("Message Type: NEW_TASK\n"))
    }

    // Codex compact_remote_v2's per-agent retention predicate. Ciphertext remains
    // opaque; history.rs estimates its visible bytes as ceil(encoded_bytes * 9/16).
    pub(crate) fn retain_for_compaction(&self) -> bool {
        let first = self.first_text().unwrap_or_default();
        let descendant = self
            .author
            .strip_prefix(self.recipient)
            .is_some_and(|suffix| suffix.starts_with('/'));
        if first.starts_with("Message Type: FINAL_ANSWER\n")
            || (descendant
                && (first.starts_with("Message Type: MESSAGE\n")
                    || first.starts_with("Message Type: CHANNEL_POST\n")))
        {
            return false;
        }
        let bytes = self.content.iter().fold(
            self.author.len().saturating_add(self.recipient.len()),
            |bytes, part| {
                let size = if let Some(text) = part.get("text").and_then(Value::as_str)
                    && part["type"] == "input_text"
                {
                    text.len()
                } else {
                    part["encrypted_content"]
                        .as_str()
                        .expect("validated agent content")
                        .len()
                        .saturating_mul(9)
                        .div_ceil(16)
                };
                bytes.saturating_add(size)
            },
        );
        bytes.div_ceil(4) <= 10_000
    }
}

// Only the canonical, visible NEW_TASK envelope signals new work. Ordinary
// MESSAGE/FINAL_ANSWER mail must not split a running tool loop into new turns.
pub(crate) fn starts_turn(item: &Value) -> bool {
    item.get("role").and_then(Value::as_str) == Some("user")
        || AgentMessage::read(item).is_some_and(|message| message.new_task())
}

#[cfg(test)]
#[path = "agent_message_tests.rs"]
mod tests;
