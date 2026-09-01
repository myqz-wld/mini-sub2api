use serde_json::Map;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RewriteReason {
    None,
    InvalidOrMissingMode,
    MissingSandbox,
    PlatformMismatch,
}

pub(crate) fn normalize(metadata: &mut Map<String, Value>) {
    let reason = normalize_for_platform(metadata, std::env::consts::OS);
    if reason != RewriteReason::None {
        tracing::debug!(
            event = "sandbox_metadata_rewrite_total",
            mismatch = (reason == RewriteReason::PlatformMismatch),
            reason = ?reason,
            sandbox_mode = metadata.get("sandbox_mode").and_then(|value| value.as_str()),
            sandbox = metadata.get("sandbox").and_then(|value| value.as_str()),
        );
    }
}

pub(crate) fn normalize_serialized(raw: &str, request_kind: &str) -> Option<String> {
    if request_kind == "memory" {
        return Some(raw.to_string());
    }
    let mut value = serde_json::from_str::<Value>(raw).ok()?;
    normalize(value.as_object_mut()?);
    crate::ascii_json::to_ascii_json_string(&value).ok()
}

fn normalize_for_platform(metadata: &mut Map<String, Value>, platform: &str) -> RewriteReason {
    let mode = metadata
        .get("sandbox_mode")
        .and_then(Value::as_str)
        .map(str::to_string);
    let expected = match mode.as_deref() {
        Some("danger-full-access") => ("danger-full-access", "none"),
        Some("external-sandbox") => ("external-sandbox", "external"),
        Some("read-only") => match platform_sandbox(platform) {
            Some(sandbox) => ("read-only", sandbox),
            None => ("danger-full-access", "none"),
        },
        Some("workspace-write") => match platform_sandbox(platform) {
            Some(sandbox) => ("workspace-write", sandbox),
            None => ("danger-full-access", "none"),
        },
        _ => ("danger-full-access", "none"),
    };
    let reason = if mode.as_deref() != Some(expected.0) {
        RewriteReason::InvalidOrMissingMode
    } else {
        match metadata.get("sandbox").and_then(Value::as_str) {
            None => RewriteReason::MissingSandbox,
            Some(actual) if actual != expected.1 => RewriteReason::PlatformMismatch,
            Some(_) => RewriteReason::None,
        }
    };
    insert_ordered(metadata, expected.0, expected.1);
    reason
}

fn insert_ordered(metadata: &mut Map<String, Value>, mode: &str, sandbox: &str) {
    let source = std::mem::take(metadata);
    let mut inserted = false;
    for (name, value) in source {
        if matches!(name.as_str(), "sandbox" | "sandbox_mode") {
            continue;
        }
        if !inserted
            && matches!(
                name.as_str(),
                "auto_review_enabled"
                    | "node_repl_auto_review_required"
                    | "node_repl_disabled"
                    | "workspaces"
                    | "tool_namespaces_info"
                    | "turn_started_at_unix_ms"
                    | "compaction"
            )
        {
            metadata.insert("sandbox".to_string(), sandbox.into());
            metadata.insert("sandbox_mode".to_string(), mode.into());
            inserted = true;
        }
        metadata.insert(name, value);
    }
    if !inserted {
        metadata.insert("sandbox".to_string(), sandbox.into());
        metadata.insert("sandbox_mode".to_string(), mode.into());
    }
}

fn platform_sandbox(platform: &str) -> Option<&'static str> {
    match platform {
        "macos" => Some("seatbelt"),
        "linux" | "android" => Some("seccomp"),
        "windows" => Some("windows_sandbox"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_platform_sandbox_and_keeps_workspace_metadata() {
        let mut metadata = serde_json::json!({
            "sandbox_mode":"workspace-write",
            "sandbox":"seatbelt",
            "workspaces":{"/caller/workspace":{"writable_roots":["/caller/workspace"]}}
        })
        .as_object()
        .expect("metadata")
        .clone();
        assert_eq!(
            normalize_for_platform(&mut metadata, "linux"),
            RewriteReason::PlatformMismatch
        );
        assert_eq!(metadata["sandbox_mode"], "workspace-write");
        assert_eq!(metadata["sandbox"], "seccomp");
        assert!(metadata["workspaces"].get("/caller/workspace").is_some());
    }

    #[test]
    fn preserves_permission_semantics_on_all_supported_platforms() {
        for (platform, sandbox) in [
            ("macos", "seatbelt"),
            ("linux", "seccomp"),
            ("windows", "windows_sandbox"),
        ] {
            let mut metadata = serde_json::json!({"sandbox_mode":"read-only"})
                .as_object()
                .expect("metadata")
                .clone();
            normalize_for_platform(&mut metadata, platform);
            assert_eq!(metadata["sandbox_mode"], "read-only");
            assert_eq!(metadata["sandbox"], sandbox);
        }
    }

    #[test]
    fn invalid_or_missing_pairs_fail_to_full_access_none() {
        for mut metadata in [
            Map::new(),
            serde_json::json!({"sandbox_mode":"future-mode","sandbox":"seatbelt"})
                .as_object()
                .expect("metadata")
                .clone(),
        ] {
            assert_eq!(
                normalize_for_platform(&mut metadata, "linux"),
                RewriteReason::InvalidOrMissingMode
            );
            assert_eq!(metadata["sandbox_mode"], "danger-full-access");
            assert_eq!(metadata["sandbox"], "none");
        }
    }

    #[test]
    fn danger_and_external_have_platform_independent_sandboxes() {
        for (mode, sandbox) in [
            ("danger-full-access", "none"),
            ("external-sandbox", "external"),
        ] {
            let mut metadata = serde_json::json!({"sandbox_mode":mode,"sandbox":"wrong"})
                .as_object()
                .expect("metadata")
                .clone();
            normalize_for_platform(&mut metadata, "macos");
            assert_eq!(metadata["sandbox_mode"], mode);
            assert_eq!(metadata["sandbox"], sandbox);
        }
    }
}
