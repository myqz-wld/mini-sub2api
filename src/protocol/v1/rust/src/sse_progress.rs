//! Payload-free event classification shared with Go through the protocol fixtures.
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum Class {
    Status,
    Text,
    Reasoning,
    Tool,
    Media,
    Item,
    Terminal,
    Empty,
    Other,
    Invalid,
}

impl Class {
    pub const COUNT: usize = 10;
    pub const fn as_str(self) -> &'static str {
        [
            "status",
            "text",
            "reasoning",
            "tool",
            "media",
            "item",
            "terminal",
            "empty",
            "other",
            "invalid",
        ][self as usize]
    }
    pub const fn is_output(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Reasoning | Self::Tool | Self::Media | Self::Item
        )
    }
}

pub fn classify(data: &[u8]) -> Class {
    let Ok(event) = serde_json::from_slice::<Fields>(data) else {
        return Class::Invalid;
    };
    let payload = |class, present| if present { class } else { Class::Empty };
    match event.kind.as_str() {
        "response.completed" | "response.failed" | "response.incomplete" => Class::Terminal,
        "error"
        | "response.created"
        | "response.in_progress"
        | "response.queued"
        | "response.metadata"
        | "codex.response.metadata"
        | "responsesapi.websocket_timing"
        | "ping"
        | "heartbeat"
        | "keepalive"
        | "response.output_item.added"
        | "response.web_search_call.in_progress"
        | "response.web_search_call.searching"
        | "response.file_search_call.in_progress"
        | "response.file_search_call.searching"
        | "response.code_interpreter_call.in_progress"
        | "response.code_interpreter_call.interpreting"
        | "response.image_generation_call.in_progress"
        | "response.image_generation_call.generating" => Class::Status,
        "response.output_text.delta" | "response.refusal.delta" => {
            payload(Class::Text, event.delta)
        }
        "response.output_text.done" => payload(Class::Text, event.text),
        "response.refusal.done" => payload(Class::Text, event.refusal),
        "response.reasoning_text.delta"
        | "response.reasoning_summary_text.delta"
        | "response.reasoning.delta" => payload(Class::Reasoning, event.delta),
        "response.reasoning_text.done" | "response.reasoning_summary_text.done" => {
            payload(Class::Reasoning, event.text)
        }
        "response.function_call_arguments.delta"
        | "response.custom_tool_call_input.delta"
        | "response.code_interpreter_call_code.delta" => payload(Class::Tool, event.delta),
        "response.function_call_arguments.done" => payload(Class::Tool, event.arguments),
        "response.custom_tool_call_input.done" => payload(Class::Tool, event.input),
        "response.code_interpreter_call_code.done" => payload(Class::Tool, event.code),
        "response.audio.delta" | "response.output_audio.delta" => {
            payload(Class::Media, event.delta)
        }
        "response.image_generation_call.partial_image" => payload(Class::Media, event.image),
        "response.content_part.added" | "response.content_part.done" => {
            payload(Class::Text, event.part.text || event.part.refusal)
        }
        "response.reasoning_summary_part.added" | "response.reasoning_summary_part.done" => {
            payload(Class::Reasoning, event.part.text)
        }
        "response.output_item.done" => {
            if event.item.kind.is_empty()
                || matches!(
                    event.item.status.as_str(),
                    "in_progress"
                        | "incomplete"
                        | "queued"
                        | "searching"
                        | "generating"
                        | "interpreting"
                )
            {
                return Class::Empty;
            }
            match event.item.kind.as_str() {
                "reasoning" => Class::Reasoning,
                "function_call"
                | "custom_tool_call"
                | "local_shell_call"
                | "shell_call"
                | "code_interpreter_call"
                | "web_search_call"
                | "file_search_call"
                | "tool_search_call" => Class::Tool,
                "image_generation_call" => Class::Media,
                _ => Class::Item,
            }
        }
        _ => Class::Other,
    }
}

#[derive(Default)]
struct Fields {
    kind: String,
    delta: bool,
    text: bool,
    refusal: bool,
    arguments: bool,
    input: bool,
    code: bool,
    image: bool,
    item: Item,
    part: Part,
}

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;
        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = Fields;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an SSE event object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Fields, M::Error> {
                let mut out = Fields::default();
                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    // Match Go's field lookup; duplicate fields consistently use their last value.
                    match key.to_ascii_lowercase().as_str() {
                        "type" => out.kind = map.next_value::<ShortText>()?.0,
                        "delta" => out.delta = map.next_value::<TextPresence>()?.0,
                        "text" => out.text = map.next_value::<TextPresence>()?.0,
                        "refusal" => out.refusal = map.next_value::<TextPresence>()?.0,
                        "arguments" => out.arguments = map.next_value::<TextPresence>()?.0,
                        "input" => out.input = map.next_value::<TextPresence>()?.0,
                        "code" => out.code = map.next_value::<TextPresence>()?.0,
                        "partial_image_b64" => out.image = map.next_value::<TextPresence>()?.0,
                        "item" => out.item = map.next_value()?,
                        "part" => out.part = map.next_value()?,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(out)
            }
        }
        d.deserialize_map(FieldsVisitor)
    }
}

#[derive(Default)]
struct ShortText(String);
#[derive(Default)]
struct TextPresence(bool);
#[derive(Default)]
struct Item {
    kind: String,
    status: String,
}
#[derive(Default)]
struct Part {
    text: bool,
    refusal: bool,
}

trait Field: Default {
    fn string(_value: &str) -> Self {
        Self::default()
    }
    fn object<'de, M: MapAccess<'de>>(mut map: M) -> Result<Self, M::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Self::default())
    }
}

impl Field for ShortText {
    fn string(value: &str) -> Self {
        Self(if value.len() <= 128 {
            value.to_string()
        } else {
            String::new()
        })
    }
}
impl Field for TextPresence {
    fn string(value: &str) -> Self {
        Self(!value.is_empty())
    }
}
impl Field for Item {
    fn object<'de, M: MapAccess<'de>>(mut map: M) -> Result<Self, M::Error> {
        let mut out = Self::default();
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            match key.to_ascii_lowercase().as_str() {
                "type" => out.kind = map.next_value::<ShortText>()?.0,
                "status" => out.status = map.next_value::<ShortText>()?.0,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(out)
    }
}
impl Field for Part {
    fn object<'de, M: MapAccess<'de>>(mut map: M) -> Result<Self, M::Error> {
        let mut out = Self::default();
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            match key.to_ascii_lowercase().as_str() {
                "text" => out.text = map.next_value::<TextPresence>()?.0,
                "refusal" => out.refusal = map.next_value::<TextPresence>()?.0,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(out)
    }
}

fn field<'de, D: Deserializer<'de>, T: Field>(d: D) -> Result<T, D::Error> {
    struct FieldVisitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: Field> Visitor<'de> for FieldVisitor<T> {
        type Value = T;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an observed field")
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<T, E> {
            Ok(T::string(v))
        }
        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<T, E> {
            Ok(T::default())
        }
        fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<T, E> {
            Ok(T::default())
        }
        fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<T, E> {
            Ok(T::default())
        }
        fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<T, E> {
            Ok(T::default())
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<T, E> {
            Ok(T::default())
        }
        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::object(map)
        }
        fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<T, S::Error> {
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok(T::default())
        }
    }
    d.deserialize_any(FieldVisitor(std::marker::PhantomData))
}

macro_rules! observed_field {
    ($($kind:ty),+) => { $(impl<'de> Deserialize<'de> for $kind {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> { field(d) }
    })+ };
}
observed_field!(ShortText, TextPresence, Item, Part);

#[cfg(test)]
#[path = "sse_progress_tests.rs"]
mod tests;
