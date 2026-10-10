//! Structured writer identity, separate from the capped caller-kind label.

use std::sync::OnceLock;

fn clean(value: &str) -> Option<String> {
    let text: String = value
        .chars()
        .filter(char::is_ascii)
        .filter(|c| !c.is_control())
        .collect();
    let text: String = text.trim().chars().take(128).collect();
    (!text.is_empty()).then_some(text)
}

fn headers(
    env: impl Fn(&str) -> Option<String>,
    hostname: Option<String>,
    producer: &str,
) -> Vec<(&'static str, String)> {
    let get = |key| env(key).filter(|v| !v.is_empty());
    let in_session = get("CLAUDECODE").as_deref() == Some("1") || get("CODEX_HOME").is_some();
    let harness = get("QUIPU_HARNESS").unwrap_or_else(|| {
        if get("CLAUDECODE").as_deref() == Some("1") {
            "claude"
        } else if get("CODEX_HOME").is_some() {
            "codex"
        } else if producer == crate::quipu_label::DAEMON || producer == crate::quipu_label::MCP {
            "service"
        } else {
            "cli"
        }
        .into()
    });
    let agent = get("QUIPU_AGENT").or_else(|| {
        if in_session {
            get("SHANTY_AGENT")
        } else {
            Some(producer.into())
        }
    });
    let session = get("QUIPU_SESSION").or_else(|| match harness.as_str() {
        "codex" => get("CODEX_SESSION_ID").or_else(|| get("CODEX_THREAD_ID")),
        "claude" => get("CLAUDE_CODE_SESSION_ID"),
        _ => None,
    });
    let model = get("QUIPU_MODEL").or_else(|| {
        matches!(harness.as_str(), "codex" | "claude")
            .then(|| get("SHANTY_MODEL"))
            .flatten()
    });
    [
        ("X-Quipu-Agent", agent),
        ("X-Quipu-Harness", Some(harness)),
        ("X-Quipu-Model", model),
        ("X-Quipu-Session", session),
        ("X-Quipu-Host", get("QUIPU_HOST").or(hostname)),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.as_deref().and_then(clean).map(|v| (key, v)))
    .collect()
}

pub(crate) fn apply(mut request: ureq::Request, producer: &str) -> ureq::Request {
    // Host identity is machinery, never a guessed literal. Cache the command
    // result; if the platform cannot supply it, provenance remains partial.
    static HOST: OnceLock<Option<String>> = OnceLock::new();
    let hostname = HOST.get_or_init(|| {
        let output = std::process::Command::new("hostname").output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).ok())
            .flatten()
    });
    for (key, value) in headers(|key| std::env::var(key).ok(), hostname.clone(), producer) {
        request = request.set(key, &value);
    }
    request
}

/// Preserve declared headers, filling an omitted session from this hook's input.
/// This is request attribution only; authentication and its session markers are
/// deliberately unchanged. An unknown hook session remains absent.
pub(crate) fn with_hook_session(request: ureq::Request, session: Option<&str>) -> ureq::Request {
    if request.header("X-Quipu-Session").is_some() {
        return request;
    }
    match session.and_then(clean) {
        Some(session) => request.set("X-Quipu-Session", &session),
        None => request,
    }
}

#[cfg(test)]
#[path = "quipu_provenance_test.rs"]
mod tests;
