use std::process::Command;
use std::sync::OnceLock;

static TERMINAL_TOKEN: OnceLock<String> = OnceLock::new();

pub(crate) fn user_agent_token() -> String {
    TERMINAL_TOKEN
        .get_or_init(|| detect(&read_env, &tmux_client_info))
        .clone()
}

fn detect(
    get: &dyn Fn(&str) -> Option<String>,
    tmux_info: &dyn Fn() -> (Option<String>, Option<String>),
) -> String {
    let tmux_active = non_empty(get, "TMUX").is_some() || non_empty(get, "TMUX_PANE").is_some();
    if let Some(program) = non_empty(get, "TERM_PROGRAM") {
        if program.eq_ignore_ascii_case("tmux") && tmux_active {
            let (term_type, term_name) = tmux_info();
            if let Some(term_type) = term_type.and_then(non_whitespace) {
                let mut parts = term_type.split_whitespace();
                let program = parts.next().unwrap_or_default();
                let version = parts.next();
                return sanitize(match version {
                    Some(version) => format!("{program}/{version}"),
                    None => program.to_string(),
                });
            }
            if let Some(term_name) = term_name.and_then(non_whitespace) {
                return sanitize(term_name);
            }
        }
        let version = non_empty(get, "TERM_PROGRAM_VERSION");
        return sanitize(match version {
            Some(version) => format!("{program}/{version}"),
            None => program,
        });
    }

    if get("WEZTERM_VERSION").is_some() {
        return named_with_version("WezTerm", non_empty(get, "WEZTERM_VERSION"));
    }
    if ["ITERM_SESSION_ID", "ITERM_PROFILE", "ITERM_PROFILE_NAME"]
        .iter()
        .any(|name| get(name).is_some())
    {
        return "iTerm.app".to_string();
    }
    if get("TERM_SESSION_ID").is_some() {
        return "Apple_Terminal".to_string();
    }
    if get("KITTY_WINDOW_ID").is_some() || get("TERM").is_some_and(|term| term.contains("kitty")) {
        return "kitty".to_string();
    }
    if get("ALACRITTY_SOCKET").is_some() || get("TERM").is_some_and(|term| term == "alacritty") {
        return "Alacritty".to_string();
    }
    if get("KONSOLE_VERSION").is_some() {
        return named_with_version("Konsole", non_empty(get, "KONSOLE_VERSION"));
    }
    if get("GNOME_TERMINAL_SCREEN").is_some() {
        return "gnome-terminal".to_string();
    }
    if get("VTE_VERSION").is_some() {
        return named_with_version("VTE", non_empty(get, "VTE_VERSION"));
    }
    if get("WT_SESSION").is_some() {
        return "WindowsTerminal".to_string();
    }
    non_empty(get, "TERM")
        .map(sanitize)
        .unwrap_or_else(|| "unknown".to_string())
}

fn read_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn non_empty(get: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    get(name).and_then(non_whitespace)
}

fn non_whitespace(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn named_with_version(name: &str, version: Option<String>) -> String {
    version.map_or_else(|| name.to_string(), |version| format!("{name}/{version}"))
}

fn sanitize(value: String) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '/') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn tmux_client_info() -> (Option<String>, Option<String>) {
    (
        tmux_display_message("#{client_termtype}"),
        tmux_display_message("#{client_termname}"),
    )
}

fn tmux_display_message(format: &str) -> Option<String> {
    let output = Command::new("tmux")
        .args(["display-message", "-p", format])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .and_then(non_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn detected(values: &[(&str, &str)], tmux: (Option<&str>, Option<&str>)) -> String {
        let values = values.iter().copied().collect::<HashMap<_, _>>();
        detect(
            &|name| values.get(name).map(|value| (*value).to_string()),
            &|| (tmux.0.map(str::to_string), tmux.1.map(str::to_string)),
        )
    }

    #[test]
    fn matches_codex_terminal_detection_precedence() {
        assert_eq!(
            detected(
                &[("TERM_PROGRAM", "WezTerm"), ("TERM_PROGRAM_VERSION", "1.2")],
                (None, None)
            ),
            "WezTerm/1.2"
        );
        assert_eq!(
            detected(&[("ITERM_SESSION_ID", "")], (None, None)),
            "iTerm.app"
        );
        assert_eq!(
            detected(
                &[("TERM_PROGRAM", "tmux"), ("TMUX", "active")],
                (Some("ghostty 1.0"), Some("xterm-256color"))
            ),
            "ghostty/1.0"
        );
        assert_eq!(
            detected(&[("TERM", "xterm-256color")], (None, None)),
            "xterm-256color"
        );
    }
}
