//! Default-off read legend. It never contacts Quipu, rebuilds the index, or
//! invents a second context allocation. The pipeline must supply remaining bytes.
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::keywords::{Reply, Request};
use serde_json::Value;

pub fn run_post_read(config_override: Option<&Path>) -> anyhow::Result<()> {
    if std::env::var("YUPANA_READ_LEGEND").as_deref() != Ok("1") {
        return Ok(());
    }
    if let Some(value) = read_payload() {
        if let Some(output) = evaluate(&value, config_override)? {
            println!("{output}");
        }
    }
    Ok(())
}

pub(super) fn read_payload() -> Option<Value> {
    let mut raw = String::new();
    std::io::stdin()
        .take(256 * 1024 + 1)
        .read_to_string(&mut raw)
        .ok()?;
    if raw.len() > 256 * 1024 {
        return None;
    }
    serde_json::from_str(&raw).ok()
}

pub(super) fn evaluate(
    value: &Value,
    config_override: Option<&Path>,
) -> anyhow::Result<Option<Value>> {
    if std::env::var("YUPANA_READ_LEGEND").as_deref() != Ok("1") {
        return Ok(None);
    }
    let started = Instant::now();
    let Some(request) = request(value) else {
        return Ok(None);
    };
    let root = value["cwd"]
        .as_str()
        .map_or_else(|| std::env::current_dir().ok(), |p| Some(p.into()));
    let Some(root) = root else {
        return Ok(None);
    };
    let Ok(config) = crate::config::YupanaConfig::resolve(config_override, &root) else {
        return Ok(None);
    };
    // Read text must stay on loopback, even if a guard's configured daemon is
    // reachable on another host. No DNS lookup or remote failover for this hook.
    let host = &config.serve.bind_address;
    if !config.serve.use_daemon || !["127.0.0.1", "::1"].contains(&host.as_str()) {
        return Ok(None);
    }
    let timeout = Duration::from_millis(40);
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let host = if host == "::1" { "[::1]" } else { host };
    let result = agent
        .post(&format!(
            "http://{host}:{}/keywords",
            config.serve.mcp_http_port
        ))
        .timeout(timeout)
        .set("Content-Type", "application/json")
        .send_string(&serde_json::to_string(&request)?);
    let reply = result
        .ok()
        .and_then(|r| r.into_string().ok())
        .and_then(|r| serde_json::from_str::<Reply>(&r).ok());
    let Some(reply) = reply else {
        crate::metrics::emit(
            "read_legend_unknown",
            &[
                ("reason", "resident keywords unavailable".into()),
                ("session_id", request.session_id.into()),
            ],
        );
        return Ok(None);
    };
    if reply.context.len() > request.remaining_bytes.min(600) || reply.shown.len() > 5 {
        return Ok(None);
    }
    // Scoring contract shared with the frozen DP value scorer. Separate event
    // kind keeps read traffic out of action/guard evaluation denominators.
    crate::metrics::emit(
        "read_legend",
        &[
            ("timestamp", chrono::Utc::now().to_rfc3339().into()),
            ("session_id", request.session_id.into()),
            ("tool", value["tool_name"].clone()),
            ("ref", request.reference.into()),
            ("shown", serde_json::json!(reply.shown)),
            ("raw_hits", reply.raw_hits.into()),
            ("bytes", reply.context.len().into()),
            ("budget", request.remaining_bytes.into()),
            ("latency_us", (started.elapsed().as_micros() as u64).into()),
        ],
    );
    if !reply.context.is_empty() {
        return Ok(Some(serde_json::json!({"hookSpecificOutput": {
            "hookEventName": "PostToolUse", "additionalContext": reply.context
        }})));
    }
    Ok(None)
}

pub(crate) fn request(value: &Value) -> Option<Request> {
    if value["hook_event_name"] != "PostToolUse" {
        return None;
    }
    if value["tool_response"]["interrupted"] == true || value["tool_response"]["isError"] == true {
        return None;
    }
    let session_id = value["session_id"].as_str()?.to_string();
    if session_id.is_empty() {
        return None;
    }
    // Missing means no allocation, never "default 600". The orchestrating
    // pipeline reserves newline/separator costs before invoking this stage.
    let remaining_bytes = usize::try_from(value["remaining_context_bytes"].as_u64()?)
        .ok()?
        .min(600);
    if remaining_bytes == 0 {
        return None;
    }
    let input = &value["tool_input"];
    let reference = match value["tool_name"].as_str()? {
        "Read" => input["file_path"].as_str()?.to_string(),
        "Bash" => bash_reference(input["command"].as_str()?)?,
        _ => return None,
    };
    let text = response_text(&value["tool_response"], 0)?.to_string();
    if text.is_empty() || text.len() > 128 * 1024 {
        return None;
    }
    Some(Request {
        session_id,
        text,
        reference,
        remaining_bytes,
    })
}

fn response_text(value: &Value, depth: usize) -> Option<&str> {
    if depth > 5 {
        return None;
    }
    if let Some(text) = value.as_str() {
        return (!text.is_empty()).then_some(text);
    }
    for key in ["file", "stdout", "output", "content", "text"] {
        if let Some(text) = response_text(&value[key], depth + 1) {
            return Some(text);
        }
    }
    None
}

fn bash_reference(command: &str) -> Option<String> {
    // Recognize the measured read families, including flags/quoted filenames.
    // Restrict Bash file reads to markdown; other commands stay silent.
    for segment in command.split([';', '|', '&']) {
        let words = shell_words::split(segment).ok()?;
        let Some(tool) = words.first() else {
            continue;
        };
        if tool == "br" {
            let mut i = 1;
            if words.get(i).is_some_and(|s| s == "--db") {
                i += 2;
            }
            if words.get(i).is_some_and(|s| s == "show") {
                return words.get(i + 1).cloned();
            }
        }
        if ["cat", "head", "tail", "sed"].contains(&tool.as_str()) {
            if let Some(file) = words
                .iter()
                .skip(1)
                .find(|w| w.ends_with(".md") && !w.starts_with('-'))
            {
                return Some(file.clone());
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "post_read_test.rs"]
mod tests;
