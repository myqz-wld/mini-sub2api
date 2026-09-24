//! Original caller evidence and deterministic structural validation, before emulation.
use crate::request_normalizer::{EmulationTransport, StatefulPrepareError as Error};
use crate::request_state_types::validate_wire_id;
use http::HeaderMap;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Format {
    Responses,
    Lite,
}

#[derive(Clone)]
pub(crate) struct Evidence {
    pub(crate) reasoning_visibility: crate::reasoning_visibility::ReasoningVisibility,
    pub(crate) transport: EmulationTransport,
    pub(crate) session: Option<String>,
    pub(crate) turn: Option<String>,
    pub(crate) previous: Option<String>,
    pub(crate) declared_format: Option<Format>,
    pub(crate) input: Vec<Value>,
}

pub(crate) fn optional_id(value: Option<&Value>) -> Result<Option<String>, Error> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
        Some(Value::String(value)) => {
            validate_wire_id(value).map_err(|_| Error::InvalidRequest)?;
            Ok(Some(value.clone()))
        }
        _ => Err(Error::InvalidRequest),
    }
}

fn metadata(raw: Option<&Value>) -> Result<Option<Map<String, Value>>, Error> {
    match raw {
        None => Ok(None),
        Some(Value::String(raw)) => serde_json::from_str::<Map<String, Value>>(raw)
            .map(Some)
            .map_err(|_| Error::InvalidRequest),
        _ => Err(Error::InvalidRequest),
    }
}

fn one(values: impl IntoIterator<Item = Option<String>>) -> Result<Option<String>, Error> {
    let values: BTreeSet<_> = values.into_iter().flatten().collect();
    if values.len() > 1 {
        return Err(Error::InvalidRequest);
    }
    Ok(values.into_iter().next())
}

pub(crate) fn selected_session(
    object: &Map<String, Value>,
    headers: &HeaderMap,
) -> Result<Option<String>, Error> {
    let mut direct = Vec::new();
    for header in headers.get_all("session-id") {
        let text = header.to_str().map_err(|_| Error::InvalidRequest)?;
        direct.push(optional_id(Some(&Value::String(text.to_string())))?);
    }
    let direct = one(direct)?;
    let flat = object.get("client_metadata").and_then(Value::as_object);
    let body = metadata(flat.and_then(|m| m.get("x-codex-turn-metadata")))?;
    let declared = optional_id(flat.and_then(|m| m.get("session_id")))?.or(optional_id(
        body.as_ref().and_then(|m| m.get("session_id")),
    )?);
    // Codex 0.156.0 uses session-id for cache affinity on root forks. The body's
    // explicit session remains the owner when the header carries prompt_cache_key.
    if let Some(session) = direct {
        if object.get("prompt_cache_key").and_then(Value::as_str) == Some(session.as_str())
            && let Some(declared) = declared
        {
            return Ok(Some(declared));
        }
        return Ok(Some(session));
    }
    if declared.is_some() {
        return Ok(declared);
    }
    let mut nested = vec![optional_id(
        body.as_ref().and_then(|m| m.get("session_id")),
    )?];
    for header in headers.get_all("x-codex-turn-metadata") {
        let value = Value::String(
            header
                .to_str()
                .map_err(|_| Error::InvalidRequest)?
                .to_string(),
        );
        let object = metadata(Some(&value))?;
        nested.push(optional_id(
            object.as_ref().and_then(|m| m.get("session_id")),
        )?);
    }
    one(nested)
}

pub(crate) fn normalize_input(value: Option<&Value>) -> Result<Vec<Value>, Error> {
    let items = match value {
        Some(Value::String(text)) => vec![serde_json::json!({"role":"user","content":text})],
        Some(Value::Array(items)) => items.clone(),
        None => Vec::new(),
        _ => return Err(Error::InvalidRequest),
    };
    items.into_iter().map(|item| {
        let mut object = item.as_object().cloned().ok_or(Error::InvalidRequest)?;
        if !object.contains_key("type") && object.contains_key("role") {
            object.insert("type".into(), Value::String("message".into()));
        }
        let kind = object.get("type").and_then(Value::as_str).ok_or(Error::InvalidRequest)?;
        if kind == "message" {
            let role = object.get("role").and_then(Value::as_str).ok_or(Error::InvalidRequest)?;
            if !matches!(role, "user" | "assistant" | "system" | "developer") {
                return Err(Error::InvalidRequest);
            }
            if let Some(Value::String(text)) = object.get("content") {
                let content = serde_json::json!([{
                    "type":if role == "assistant" {"output_text"} else {"input_text"}, "text":text
                }]);
                object.insert("content".into(), content);
            }
            if !object.get("content").is_some_and(Value::is_array) { return Err(Error::InvalidRequest); }
        }
        if let Some(id) = object.get("id") { optional_id(Some(id))?; }
        Ok(Value::Object(object))
    }).collect()
}

impl Evidence {
    pub(crate) fn read(
        object: &Map<String, Value>,
        headers: &HeaderMap,
        transport: EmulationTransport,
    ) -> Result<Self, Error> {
        if let Some(delivery) = object
            .get("stream_options")
            .and_then(Value::as_object)
            .and_then(|options| options.get("reasoning_summary_delivery"))
            && delivery.as_str() != Some("sequential_cutoff")
        {
            return Err(Error::InvalidRequest);
        }
        if let Some(programs) = object.get("access_programs") {
            let programs = programs.as_object().ok_or(Error::InvalidRequest)?;
            if programs.len() != 1
                || !programs
                    .get("cyber")
                    .and_then(Value::as_str)
                    .is_some_and(|program| {
                        matches!(program, "standard" | "daybreak_blue" | "daybreak_red")
                    })
            {
                return Err(Error::InvalidRequest);
            }
        }
        let previous = optional_id(object.get("previous_response_id"))?;
        // A blank explicit reference is malformed, unlike absent or null.
        if matches!(object.get("previous_response_id"), Some(Value::String(_)))
            && previous.is_none()
        {
            return Err(Error::InvalidRequest);
        }
        let input = normalize_input(object.get("input"))?;
        let prefixes: Vec<_> = input
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.get("type").and_then(Value::as_str) == Some("additional_tools")
            })
            .collect();
        let declared_format = if !prefixes.is_empty() {
            if prefixes.len() != 1 || prefixes[0].0 != 0 {
                return Err(Error::InvalidRequest);
            }
            let prefix = prefixes[0].1;
            if prefix.get("role").and_then(Value::as_str) != Some("developer") {
                return Err(Error::InvalidRequest);
            }
            let tools = prefix
                .get("tools")
                .and_then(Value::as_array)
                .ok_or(Error::InvalidRequest)?;
            validate_tools(tools)?;
            if let Some(top) = object.get("tools")
                && top != &Value::Array(tools.clone())
            {
                return Err(Error::InvalidRequest);
            }
            Some(Format::Lite)
        } else if previous.is_none() || object.contains_key("tools") {
            Some(Format::Responses)
        } else {
            None
        };
        if let Some(tools) = object.get("tools") {
            validate_tools(tools.as_array().ok_or(Error::InvalidRequest)?)?;
        }
        let flat = object.get("client_metadata").and_then(Value::as_object);
        let nested = metadata(flat.and_then(|m| m.get("x-codex-turn-metadata")))?;
        let mut turn = optional_id(flat.and_then(|m| m.get("turn_id")))?
            .or(optional_id(nested.as_ref().and_then(|m| m.get("turn_id")))?);
        if turn.is_none() {
            let mut header_turns = Vec::new();
            for header in headers.get_all("x-codex-turn-metadata") {
                let value = Value::String(
                    header
                        .to_str()
                        .map_err(|_| Error::InvalidRequest)?
                        .to_string(),
                );
                let object = metadata(Some(&value))?;
                header_turns.push(optional_id(object.as_ref().and_then(|m| m.get("turn_id")))?);
            }
            turn = one(header_turns)?;
        }
        Ok(Self {
            reasoning_visibility: crate::reasoning_visibility::ReasoningVisibility::read(object)?,
            transport,
            session: selected_session(object, headers)?,
            turn,
            previous,
            declared_format,
            input,
        })
    }
}

fn validate_tools(tools: &[Value]) -> Result<(), Error> {
    for tool in tools {
        let object = tool.as_object().ok_or(Error::InvalidRequest)?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or(Error::InvalidRequest)?;
        if matches!(kind, "function" | "custom" | "namespace")
            && object
                .get("name")
                .and_then(Value::as_str)
                .is_none_or(|name| name.trim().is_empty())
        {
            return Err(Error::InvalidRequest);
        }
        if matches!(kind, "function" | "tool_search")
            && let Some(parameters) = object.get("parameters")
            && !crate::responses_lite::valid_tool_parameters(parameters)
        {
            return Err(Error::InvalidRequest);
        }
        if kind == "namespace" {
            validate_tools(
                object
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or(Error::InvalidRequest)?,
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Default, Eq, PartialEq)]
pub(crate) struct Dependencies {
    pub(crate) calls: BTreeMap<String, bool>,
    pub(crate) items: BTreeSet<String>,
}

pub(crate) fn uses_direct_call_reference(kind: &str) -> bool {
    matches!(
        kind,
        "function_call" | "custom_tool_call" | "function_call_output" | "custom_tool_call_output"
    )
}

impl Dependencies {
    pub(crate) fn append(&mut self, items: &[Value]) -> Result<(), Error> {
        for item in items {
            let kind = item
                .get("type")
                .and_then(Value::as_str)
                .ok_or(Error::InvalidRequest)?;
            if kind == "item_reference" {
                let id = optional_id(item.get("id"))?.ok_or(Error::InvalidRequest)?;
                if !self.items.contains(&id) {
                    return Err(Error::StateUnavailable);
                }
                continue;
            }
            if let Some(id) = optional_id(item.get("id"))? {
                self.items.insert(id);
            }
            let call = optional_id(item.get("call_id"))?;
            let named_output = kind == "function_call_output"
                && item.get("call_id").is_none()
                && item
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| !name.trim().is_empty())
                && item
                    .get("output")
                    .is_some_and(|v| v.is_string() || v.is_array());
            if uses_direct_call_reference(kind) && call.is_none() && !named_output {
                return Err(Error::InvalidRequest);
            }
            if let Some(call) = call {
                if kind.ends_with("_output") {
                    let consumed = self.calls.get_mut(&call).ok_or(Error::StateUnavailable)?;
                    if *consumed {
                        return Err(Error::InvalidRequest);
                    }
                    *consumed = true;
                } else if kind.ends_with("_call") {
                    // Deliberate history repetition is preserved; a new declaration starts its occurrence.
                    self.calls.insert(call, false);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn append_output(&mut self, items: &[Value]) -> Result<(), Error> {
        let consumed: Vec<_> = self
            .calls
            .iter()
            .filter(|(_, done)| **done)
            .map(|(id, _)| id.clone())
            .collect();
        self.append(items)?;
        // A final copy of an already observed call must not undo an in-flight injected result.
        for id in consumed {
            self.calls.insert(id, true);
        }
        Ok(())
    }

    pub(crate) fn awaiting_tools(&self) -> bool {
        self.calls.values().any(|consumed| !consumed)
    }
    pub(crate) fn cost(&self) -> usize {
        self.calls
            .keys()
            .chain(self.items.iter())
            .map(|id| id.len() + 96)
            .sum()
    }
}
