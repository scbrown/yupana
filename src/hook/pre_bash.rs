//! `pre_bash` feeds harness Bash payloads to the action resolver and guard chain.
//!
//! TWO-SIDED LIVENESS (aegis-tv9ri). Every invocation emits `pre_bash_invoked`
//! BEFORE the payload is inspected, so the trace can tell apart the two ways
//! this hook goes quiet — which need opposite fixes and used to look identical:
//!
//! ```text
//! {"kind":"pre_bash_invoked","parsed":true,"payload_bytes":123,…}
//! {"kind":"pre_bash_invoked","parsed":false,"payload_bytes":123,
//!  "payload_keys":"session_id,tool_name,…"}      <- ran, shape unrecognised
//! ```
//! No `pre_bash_invoked` at all across known Bash traffic means the hook is not
//! wired into the settings THAT SESSION loads — which is a settings-scope
//! question, not a yupana one.
//! ACTION SCOPE IS RECORD-ONLY BY DEFAULT. Governed command policies project
//! from Quipu, while the deployment's `[yupana.policy] mode` remains their ceiling.
//! When armed, only the DECLARED scope's `allow_targets` / `deny_targets` apply;
//! there is no observed rung from which to infer target scope.
//! ABSTENTIONS ARE NEVER VIOLATIONS. An `Unknown` target means no check performed.
//!
//! ALWAYS EXIT 0. Harness denial is expressed through the hook JSON envelope,
//! never through process status. Projection and signal failures are loud
//! fail-open; only a valid governed policy under `enforce` can emit denial.
//! `verb` and `target` are OMITTED on abstention; empty values would replay as facts.
//! But `target_class` is ALWAYS written, including
//! `"unknown"`, because the replay in the next phase needs to divide resolved
//! actions by TOTAL actions. Without a row for the abstentions the denominator
//! is invisible and a resolver covering 5% of traffic looks identical to one
//! covering 95%.

use super::pre_bash_grounding::action_fields;
use crate::action;
use std::io::Read;
/// Handle a Claude Code `PreToolUse` payload for the Bash tool.
///
/// Reads the payload on stdin, resolves the command to (verb, target,
/// `target_class`), and records it. Errors are swallowed: an unparseable or
/// unexpected payload records nothing and still succeeds.
pub fn run_pre_bash() -> anyhow::Result<()> {
    let mut buf = String::new();
    // A read failure is not an error worth surfacing: the command the operator
    // asked for must run either way.
    std::io::stdin().lock().read_to_string(&mut buf).ok();
    let cmd = command_of(&buf);
    record_invocation(&buf, cmd.is_some());

    if let Some(cmd) = cmd {
        let input = crate::hook::HookInput::parse(&buf);
        // Record that this session produced a delegable artifact, so the
        // post-edit DELEGATE advisory can tell "still investigating" from
        // "already had something to hand off" (aegis-2o9eo). Record-only: this
        // hook never refuses a command on that basis.
        super::delegate_line::note_command(
            input.as_ref().and_then(|i| i.session_id.as_deref()),
            &cmd,
        );
        let grounding = input
            .as_ref()
            .and_then(|i| i.grounding.as_ref())
            .map(|reference| {
                let state = crate::turn_grounding::assess(
                    Some(reference),
                    crate::turn_grounding::cache_dir().as_deref(),
                    crate::turn_grounding::now_secs(),
                    crate::turn_grounding::max_age_secs(),
                );
                (reference, state)
            });
        let resolved = action::resolve(&cmd);
        record(&resolved, grounding);
        #[cfg(feature = "quipu")]
        let ci = crate::ci_shift::hook_advisory(&buf, &cmd);
        #[cfg(not(feature = "quipu"))]
        let ci = super::pre_edit::Outcome::Allow;
        #[cfg(feature = "quipu")]
        let disk = super::disk_guard::observe_and_check(&buf, &cmd);
        #[cfg(not(feature = "quipu"))]
        let disk = super::pre_edit::Outcome::Allow;
        let memory = super::memory_guard::check(&buf, &cmd);
        // The governed LANDING policy — who may put code on a repository's
        // protected ref. Same chain, so the mode stays the single ceiling.
        let landing = super::landing_guard::check(&buf, &cmd);
        // The scope check rides AFTER the record, deliberately: the trace is
        // the product and must not depend on a policy decision succeeding.
        if let Some(text) = scope_refusal(&buf, &resolved) {
            println!("{}", super::deny_envelope(&text));
        } else {
            // Folded rather than nested: a plane is added by extending this
            // list, not by wrapping the expression one layer deeper.
            let outcome = [memory, landing, ci, disk]
                .into_iter()
                .fold(super::pre_edit::Outcome::Allow, combine_advisories);
            match outcome {
                super::pre_edit::Outcome::Allow => {}
                super::pre_edit::Outcome::Deny(reason) => {
                    println!("{}", super::deny_envelope(&reason));
                }
                super::pre_edit::Outcome::Notify(message) => {
                    if let Some(message) = super::advisory_for_session(&buf, message) {
                        println!("{}", super::system_message(&message));
                    }
                }
            }
        }
    }
    Ok(())
}

fn combine_advisories(
    first: super::pre_edit::Outcome,
    second: super::pre_edit::Outcome,
) -> super::pre_edit::Outcome {
    use super::pre_edit::Outcome;
    match (first, second) {
        (Outcome::Deny(a), Outcome::Deny(b)) => Outcome::Deny(format!("{a}\n{b}")),
        (Outcome::Deny(a), _) | (_, Outcome::Deny(a)) => Outcome::Deny(a),
        (Outcome::Notify(a), Outcome::Notify(b)) => Outcome::Notify(format!("{a}\n{b}")),
        (Outcome::Notify(a), _) | (_, Outcome::Notify(a)) => Outcome::Notify(a),
        (Outcome::Allow, Outcome::Allow) => Outcome::Allow,
    }
}

/// The action-scope verdict for a resolved command, or `None` for silence.
///
/// Returns the DENY text only at `enforce`. At `advise` it emits the record and
/// a stderr line and returns `None` — the same staging the path rung uses, and
/// for the same reason: a deployment arms the boundary once it has seen what it
/// would have refused.
///
/// Every failure path here is silence. No config, no tenant, an unresolvable
/// target, or a scope with no target globs all mean "no check performed", which
/// is not the same as "permitted" and is recorded as neither.
fn scope_refusal(payload: &str, resolved: &action::Action) -> Option<String> {
    // An abstention is not a target. See the module note: `Unknown` must never
    // reach a glob, or every pipeline on the host becomes a violation.
    if resolved.target_class == action::TargetClass::Unknown {
        return None;
    }
    let target = resolved.target.as_deref()?;

    let input = super::HookInput::parse(payload)?;
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let root = input.root(&root);
    let config = crate::config::YupanaConfig::resolve(None, &root).ok()?;

    let rung = if config.policy.action_scope.is_lower_than(config.policy.mode) {
        config.policy.action_scope
    } else {
        config.policy.mode
    };
    if rung == crate::policy::Mode::Off {
        return None;
    }

    // Tenant resolution matches the edit guard's: the explicit identity the
    // launcher exports. No tenant means no declared scope to check against.
    let tenant = std::env::var("BOBBIN_ROLE").ok()?;
    let scope = config.policy.scopes.get(&tenant)?;
    let violation = scope.check_target(resolved.target_class.as_str(), target, &tenant)?;

    let denying = rung == crate::policy::Mode::Enforce;
    crate::metrics::emit(
        "action_scope",
        &[
            ("class", resolved.target_class.as_str().into()),
            ("target", target.to_string().into()),
            ("rule", violation.rule.clone().into()),
            ("result", if denying { "deny" } else { "advise" }.into()),
        ],
    );
    if !denying {
        eprintln!(
            "yupana: {} (advisory: action_scope is not \"enforce\")",
            violation.message
        );
        return None;
    }
    Some(violation.message)
}

/// Emit ONE record per invocation, unconditionally, before anything can
/// suppress it. This is what makes the trace two-sided (aegis-tv9ri).
///
/// The `action` record only appears when the payload parsed, so its ABSENCE was
/// ambiguous in exactly the way that matters: "the hook never ran" and "the hook
/// ran and did not recognise the payload" produced identical evidence — an
/// empty trace — and they need opposite fixes. Measured 2026-08-04: 22 action
/// records total, 21 of them from one 62-second verification burst on 08-02,
/// while ordinary agent Bash traffic produced none for two days. Nothing in the
/// trace could say which failure that was.
///
/// So: `pre_bash_invoked` present with no `action` means the payload shape is
/// wrong and this hook is running; `pre_bash_invoked` absent across known Bash
/// traffic means the hook is not wired into the settings that session loads.
///
/// `payload_keys` is recorded ONLY on the failure path, and only the top-level
/// KEY NAMES — never values. It is the field that names an unexpected payload
/// shape without putting command text into a record whose whole purpose is to
/// be readable by someone debugging a hook. `payload_bytes` separates "stdin was
/// empty" from "stdin was a payload we did not understand"; both are silent
/// no-ops today and they are not the same bug.
fn record_invocation(payload: &str, parsed: bool) {
    crate::metrics::emit("pre_bash_invoked", &invocation_fields(payload, parsed));
}

/// The fields of a `pre_bash_invoked` record. Pure, so the contract is testable
/// without touching the process environment or the real spool — the same reason
/// [`crate::metrics::resolve_path`] is pure (parallel tests race on env vars).
#[must_use]
pub fn invocation_fields(payload: &str, parsed: bool) -> Vec<(&'static str, serde_json::Value)> {
    let mut fields: Vec<(&'static str, serde_json::Value)> = vec![
        ("parsed", parsed.into()),
        ("payload_bytes", payload.len().into()),
    ];
    if !parsed && !payload.is_empty() {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(payload) {
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            fields.push(("payload_keys", keys.join(",").into()));
        }
    }
    fields
}

/// Pull `tool_input.command` out of a harness payload.
///
/// Returns `None` for anything that is not a Bash-shaped payload, which
/// includes the case where this hook is wired to a matcher that also delivers
/// other tools: recording a non-command as a command would put fiction in the
/// trace, and the trace's only value is that it is evidence.
#[must_use]
pub fn command_of(payload: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let cmd = v.get("tool_input")?.get("command")?.as_str()?;
    if cmd.trim().is_empty() {
        return None;
    }
    Some(cmd.to_string())
}

fn record(
    a: &action::Action,
    grounding: Option<(
        &crate::turn_grounding::GroundingRef,
        crate::turn_grounding::GroundingState,
    )>,
) {
    crate::metrics::emit("action", &action_fields(a, grounding));
}

#[cfg(test)]
#[path = "pre_bash_test.rs"]
mod pre_bash_test;
