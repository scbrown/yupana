//! Governed session-trajectory advice. The graph owns the CLI vocabulary,
//! frequency and explanation; this module only observes and orders events.
//! Observation happens before Bash executes, so it proves an invocation was
//! attempted, not that a work item was successfully created.

#[cfg(feature = "quipu")]
use super::HookInput;
#[cfg(feature = "quipu")]
use crate::project_trajectory::{InvocationTrigger, OncePer, TrajectoryPolicy};

/// Observe an attempted command using the local projection; never query on a
/// hook's hot path. Refresh is the existing scheduled/explicit command's job.
#[cfg(feature = "quipu")]
pub(super) fn note_command(payload: &str, command: &str) {
    let Some(input) = HookInput::parse(payload) else {
        return;
    };
    let Some(session) = input.session_id.as_deref() else {
        return;
    };
    match policies(&input) {
        Ok(policies) => record_command(Some(session), command, &policies),
        Err(error) => {
            // The next post-edit also reports the gap to the model. Stderr
            // keeps the command-side observation failure independently visible.
            eprintln!("yupana trajectory NOT EVALUATED: {error}");
        }
    }
}

#[cfg(not(feature = "quipu"))]
pub(super) fn note_command(_payload: &str, _command: &str) {}

#[cfg(feature = "quipu")]
pub(super) fn advisory(payload: &str) -> Option<String> {
    let input = HookInput::parse(payload)?;
    let session = input.session_id.as_deref()?;
    match policies(&input) {
        Ok(policies) => advice(Some(session), &policies),
        Err(error) if super::first_notice_for_session(Some(session), "trajectory-unavailable") => {
            Some(format!("yupana trajectory NOT EVALUATED: {error}. Run yupana refresh-projection; no trajectory decision was made."))
        }
        Err(_) => None,
    }
}

#[cfg(not(feature = "quipu"))]
pub(super) fn advisory(_payload: &str) -> Option<String> {
    None
}

#[cfg(feature = "quipu")]
fn policies(input: &HookInput) -> Result<Vec<TrajectoryPolicy>, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let config = crate::config::YupanaConfig::resolve(None, &input.root(&cwd))
        .map_err(|e| format!("unreadable configuration ({e})"))?;
    if config.policy.mode == crate::policy::Mode::Off || !config.quipu.enabled {
        return Ok(Vec::new());
    }
    let path = crate::projection_cache::cache_path().ok_or("no projection cache path")?;
    let cached = crate::projection_cache::load_servable(
        &path,
        &config.quipu.endpoint,
        config.quipu.projection_cache_ttl_secs,
        crate::projection_cache::now_secs(),
    )
    .map_err(|e| e.to_string())?;
    let policies = cached
        .trajectory_policies
        .ok_or("cached projection predates the trajectory channel")?;
    for policy in &policies {
        policy.validate().map_err(|e| e.to_string())?;
    }
    Ok(policies)
}

#[cfg(feature = "quipu")]
fn event_key(policy: &TrajectoryPolicy, kind: &str) -> String {
    use sha2::{Digest, Sha256};
    // A changed trigger cannot inherit evidence observed under an older one.
    // Fixed-size digest also avoids marker-name truncation collisions.
    let evidence = serde_json::to_vec(&(&policy.id, &policy.trigger, &policy.ordering)).unwrap();
    format!("trajectory-{kind}-{:x}", Sha256::digest(evidence))
}

#[cfg(feature = "quipu")]
fn record_command(session: Option<&str>, command: &str, policies: &[TrajectoryPolicy]) {
    for policy in policies {
        if command_matches(command, &policy.trigger) {
            super::record_session_event(session, &event_key(policy, "seen"));
        }
    }
}

#[cfg(feature = "quipu")]
fn advice(session: Option<&str>, policies: &[TrajectoryPolicy]) -> Option<String> {
    session?;
    let mut messages = Vec::new();
    for policy in policies {
        if !super::session_event_recorded(session, &event_key(policy, "seen")) {
            continue;
        }
        if policy.once_per == OncePer::Session
            && !super::first_notice_for_session(session, &event_key(policy, "said"))
        {
            continue;
        }
        messages.push(format!(
            "yupana trajectory `{}` ({}): {}",
            policy.label, policy.tier, policy.rationale
        ));
        crate::metrics::emit(
            "trajectory_advised",
            &[
                ("policy", serde_json::json!(policy.id)),
                ("tier", serde_json::json!(policy.tier)),
                ("once_per", serde_json::json!(policy.once_per)),
                ("evidence", serde_json::json!("command-attempt-before-edit")),
            ],
        );
    }
    (!messages.is_empty()).then(|| messages.join("\n\n"))
}

/// Deliberately retains the existing narrow shell parser. A mention is not an
/// invocation. Flags consume their values; a value-less long flag before the
/// verb can therefore miss the invocation, rather than inventing one.
#[cfg(feature = "quipu")]
fn command_matches(command: &str, trigger: &InvocationTrigger) -> bool {
    command
        .split(['\n', ';'])
        .flat_map(|line| line.split("&&"))
        .flat_map(|line| line.split('|'))
        .any(|segment| {
            let mut tokens = segment.split_whitespace();
            let Some(program) = tokens.next() else {
                return false;
            };
            let program = program.rsplit('/').next().unwrap_or(program);
            if !trigger.programs.iter().any(|p| p == program) {
                return false;
            }
            let mut expect_value = false;
            for token in tokens {
                if expect_value {
                    expect_value = false;
                    continue;
                }
                if let Some(flag) = token.strip_prefix('-') {
                    expect_value = !flag.contains('=');
                    continue;
                }
                return trigger.verbs.iter().any(|verb| verb == token);
            }
            false
        })
}

#[cfg(all(test, feature = "quipu"))]
#[path = "delegate_line_test.rs"]
mod tests;
