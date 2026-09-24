//! Bounded diagnostics for synchronous request construction. Never retain caller strings.
use crate::request_normalizer::EmulationTransport;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;

const MAX_FIELDS: usize = 16;
type Key = (&'static str, &'static str, &'static str);
#[derive(Default)]
struct Counts {
    fields: BTreeMap<Key, u64>,
    overflow: u64,
}
thread_local! {
    static COUNTS: RefCell<Option<Counts>> = const { RefCell::new(None) };
}

// The closure must be synchronous: no task-local state crosses an await or a worker thread.
// Admission and construction each emit at most 16 field events and one overflow event.
pub(crate) fn scope<T>(
    transport: EmulationTransport,
    role: &'static str,
    model: &'static str,
    stage: &'static str,
    build: impl FnOnce() -> T,
) -> T {
    struct Guard(Option<Counts>);
    impl Drop for Guard {
        fn drop(&mut self) {
            COUNTS.with(|cell| *cell.borrow_mut() = self.0.take());
        }
    }
    let _guard = Guard(COUNTS.with(|cell| cell.replace(Some(Counts::default()))));
    let result = build();
    let counts = COUNTS.with(|cell| cell.take()).unwrap_or_default();
    let transport = match transport {
        EmulationTransport::Http => "http",
        EmulationTransport::WebSocket => "websocket",
    };
    for ((path, field, reason), count) in counts.fields {
        tracing::info!(
            event = "codex_ignored_field",
            profile = "subscription_1560",
            transport,
            role,
            model,
            stage,
            path,
            field,
            reason,
            count
        );
    }
    if counts.overflow != 0 {
        tracing::info!(
            event = "codex_ignored_fields_overflow",
            profile = "subscription_1560",
            transport,
            role,
            model,
            stage,
            count = counts.overflow
        );
    }
    result
}

// Called before removing/replacing data. The aggregate is flushed before construction returns
// to the sender, including rejected requests. Names and paths here are closed protocol labels.
pub(crate) fn record(path: &'static str, field: &'static str, reason: &'static str) {
    COUNTS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let Some(counts) = cell.as_mut() else { return };
        let key = (path, field, reason);
        if let Some(count) = counts.fields.get_mut(&key) {
            *count = count.saturating_add(1);
        } else if counts.fields.len() < MAX_FIELDS {
            counts.fields.insert(key, 1);
        } else {
            counts.overflow = counts.overflow.saturating_add(1);
        }
    });
}

pub(crate) fn remove(
    object: &mut Map<String, Value>,
    name: &'static str,
    path: &'static str,
    reason: &'static str,
) {
    if object.contains_key(name) {
        record(path, name, reason);
        object.shift_remove(name);
    }
}

pub(crate) fn retain(object: &mut Map<String, Value>, fields: &[&str], path: &'static str) {
    object.retain(|name, _| {
        let keep = fields.contains(&name.as_str());
        if !keep {
            record(path, safe_field(name), "unsupported_field");
        }
        keep
    });
}

fn safe_field(name: &str) -> &'static str {
    // Unknown names can contain secrets, even when they look like valid identifiers.
    const KNOWN: &[&str] = &[
        "max_tool_calls",
        "top_logprobs",
        "background",
        "prompt",
        "stream_id",
        "metadata",
        "context_management",
        "conversation",
        "moderation",
        "prompt_cache_options",
        "max_output_tokens",
        "max_completion_tokens",
        "max_tokens",
        "temperature",
        "top_p",
        "frequency_penalty",
        "presence_penalty",
        "prompt_cache_retention",
        "safety_identifier",
        "truncation",
        "user",
        "agent",
        "status",
        "caller",
        "created_by",
        "description",
        "const",
        "minimum",
        "maximum",
        "default",
        "title",
        "format",
        "prefixItems",
        "minLength",
        "maxLength",
        "pattern",
        "additionalItems",
        "uniqueItems",
        "maxItems",
        "generate_summary",
        "mode",
        "include_obfuscation",
        "annotations",
        "logprobs",
        "prompt_cache_breakpoint",
        "namespace",
        "analytics_enabled",
    ];
    KNOWN
        .iter()
        .copied()
        .find(|field| *field == name)
        .unwrap_or("unknown")
}
