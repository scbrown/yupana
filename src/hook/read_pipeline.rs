//! One shared allocation: DP keeps signpost; Yupana supplies the read legend.
//! # arming: library explicit harness configuration only; never auto-installed
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

pub fn run_post_read_pipeline(config: Option<&Path>) -> anyhow::Result<()> {
    let Some(payload) = super::post_read::read_payload() else {
        return Ok(());
    };
    let enabled = std::env::var("YUPANA_READ_LEGEND").as_deref() == Ok("1");
    let mut command =
        Command::new(std::env::var_os("YUPANA_DP_BIN").unwrap_or_else(|| "dp".into()));
    command.arg("signpost");
    if enabled {
        command.env("DP_LEGEND", "0");
    }
    let Some(prior) = invoke(
        &mut command,
        &serde_json::to_vec(&payload)?,
        Duration::from_secs(1),
    ) else {
        return Ok(());
    };
    let cap = std::env::var("DP_LEGEND_MAX_BYTES")
        .ok()
        .map_or(Some(600), |s| s.parse::<usize>().ok())
        .map(|n| n.min(600));
    let output = combine(payload, &prior, enabled, cap, |value| {
        super::post_read::evaluate(value, config).ok().flatten()
    });
    std::io::stdout().write_all(&output)?;
    Ok(())
}

/// Anonymous files prevent pipe deadlocks and cap accepted output. The child
/// receives no shell, and is killed and reaped on every error/deadline path.
fn invoke(command: &mut Command, raw: &[u8], timeout: Duration) -> Option<Vec<u8>> {
    let mut input = tempfile::tempfile().ok()?;
    input.write_all(raw).ok()?;
    input.seek(SeekFrom::Start(0)).ok()?;
    let mut output = tempfile::tempfile().ok()?;
    let mut child = command
        .stdin(input)
        .stdout(output.try_clone().ok()?)
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(1));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    }?;
    if !status.success() || output.metadata().ok()?.len() > 64 * 1024 {
        return None;
    }
    output.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = Vec::new();
    output.take(64 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= 64 * 1024).then_some(bytes)
}

fn context(value: &Value) -> Option<&str> {
    let hook = value.get("hookSpecificOutput")?;
    if hook.get("hookEventName")?.as_str()? != "PostToolUse" {
        return None;
    }
    hook.get("additionalContext")
        .map_or(Some(""), Value::as_str)
}

fn combine(
    mut payload: Value,
    prior: &[u8],
    enabled: bool,
    cap: Option<usize>,
    legend: impl FnOnce(&Value) -> Option<Value>,
) -> Vec<u8> {
    let fallback = || prior.to_vec();
    if !enabled || payload["hook_event_name"] != "PostToolUse" || !payload.is_object() {
        return fallback();
    }
    let mut envelope = if prior.iter().all(u8::is_ascii_whitespace) {
        serde_json::json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":""}})
    } else if let Ok(value) = serde_json::from_slice(prior) {
        value
    } else {
        return fallback();
    };
    let Some(prior_text) = context(&envelope) else {
        return fallback();
    };
    // String lengths are UTF-8 bytes. Reserve separator BEFORE requesting.
    let Some(remaining) = cap.and_then(|c| {
        c.min(600)
            .checked_sub(prior_text.len() + usize::from(!prior_text.is_empty()))
    }) else {
        return fallback();
    };
    if remaining == 0 {
        return fallback();
    }
    payload["remaining_context_bytes"] = remaining.into();
    let Some(reply) = legend(&payload) else {
        return fallback();
    };
    let Some(text) = context(&reply) else {
        return fallback();
    };
    if text.is_empty() || text.len() > remaining {
        return fallback();
    }
    let combined = if prior_text.is_empty() {
        text.to_string()
    } else {
        format!("{prior_text}\n{text}")
    };
    envelope["hookSpecificOutput"]["additionalContext"] = combined.into();
    let Ok(mut bytes) = serde_json::to_vec(&envelope) else {
        return fallback();
    };
    bytes.push(b'\n');
    bytes
}

#[cfg(test)]
#[path = "read_pipeline_test.rs"]
mod tests;
