//! `yupana hook post-bash` — the OUTCOME counterpart to [`super::pre_bash`]
//! (aegis-368cu.10, Phase 1 gap 3).
//!
//! ## Why a second hook exists at all
//!
//! `PreToolUse` fires BEFORE the command runs, so it structurally cannot know
//! whether the action succeeded. The epic's own standard is that a trace's only
//! value is that it is evidence, so an `outcome` field written by a hook that
//! cannot observe the outcome would be fiction. This hook is the observer.
//!
//! ## The join, and why it needed no design
//!
//! The bead scoped a correlation id as work — without one a post-record cannot
//! be joined to its pre-record, and timestamp-plus-agent is the weak correlation
//! that already failed in aegis-0jv06 with ~20 agents running at once.
//!
//! Measured 2026-09-05: **the harness already supplies one.** `tool_use_id` is
//! present on both `PreToolUse` and `PostToolUse`, and a matched pair carries
//! the SAME value. So the two short-lived processes need no shared state and no
//! minted id. It rides the `action` record as `action_id` and the outcome record
//! as `action_id` too — one field name, one join.
//!
//! ## The outcome is READ, never inferred
//!
//! The harness fires two different events: `PostToolUse` on success and
//! `PostToolUseFailure` on failure. `outcome` is taken from `hook_event_name` —
//! what the harness reported. Note what is deliberately NOT done: `tool_response`
//! carries `stdout`/`stderr`/`interrupted` and **no exit code**, and scanning
//! stderr for the word "error" would manufacture a verdict the hook cannot
//! observe. When the event name is absent or unrecognised the outcome is
//! `unknown` and says so, rather than defaulting to success.
//!
//! `interrupted` is carried separately because a cancelled command is neither a
//! success nor a failure of the action, and collapsing it into either would
//! misreport what happened.
//!
//! ## Contract
//!
//! Never denies, never prints, always exits 0 — same as `pre-bash`. This hook is
//! a recorder; a recorder that can change an outcome is no longer measuring one.

use std::io::Read;

/// Record the outcome of a completed Bash tool call.
pub fn run_post_bash() -> anyhow::Result<()> {
    let mut buf = String::new();
    // A read failure is not worth surfacing: the command has already run, and
    // this hook must never be able to affect it.
    std::io::stdin().lock().read_to_string(&mut buf).ok();
    // Scope the plate read to THIS session before building the record. An
    // outcome inherits its item the same way the action does — through
    // `metrics::emit`'s UNSCOPED fallback — so leaving it unclaimed here
    // reproduces aegis-1mp1ls on the outcome row as well as the action row.
    let session = crate::hook::HookInput::parse(&buf).and_then(|i| i.session_id);
    let item =
        crate::plate::current(session.as_deref()).map_or(serde_json::Value::Null, Into::into);
    crate::metrics::emit("action_outcome", &outcome_fields(&buf, item));
    Ok(())
}

/// The fields of an `action_outcome` record.
///
/// Pure, so the contract is testable without a spool or the process
/// environment — the same reason [`super::pre_bash::invocation_fields`] is pure.
/// The work item is PASSED IN for that reason: resolving the plate here would
/// make the one field that must be session-scoped the one field no test can
/// reach.
#[must_use]
pub fn outcome_fields(
    payload: &str,
    item: serde_json::Value,
) -> Vec<(&'static str, serde_json::Value)> {
    let input = crate::hook::HookInput::parse(payload);
    let mut fields: Vec<(&'static str, serde_json::Value)> = Vec::new();

    // TWO-SIDED LIVENESS, as `pre_bash_invoked` established: `parsed` fires on
    // every invocation, so a row with `parsed: false` is a payload shape we did
    // not understand, and the ABSENCE of rows entirely means the hook is not
    // wired into the settings that session loads. Those are different failures
    // and neither may look like a clean run.
    fields.push(("parsed", input.is_some().into()));
    fields.push(("payload_bytes", payload.len().into()));
    // ALWAYS claimed, on EVERY exit from this function including the
    // unparseable one below. A caller that abstains by pushing nothing has its
    // abstention overridden by the unscoped fallback in `metrics::emit`
    // (aegis-368cu.7), so an early return that skips this push is the same bug
    // in a rarer branch — and the unparseable branch is exactly where a
    // fabricated attribution is least defensible, since we could not even read
    // whose session it was.
    fields.push(("item", item));

    let Some(input) = input else {
        return fields;
    };

    // THE JOIN. Omitted rather than blanked when the harness supplied none: a
    // reader must be able to tell "no id on this payload" (join impossible for
    // this row) from "the id was empty" (a bug).
    if let Some(id) = input.tool_use_id {
        fields.push(("action_id", id.into()));
    }
    fields.push((
        "outcome",
        outcome_of(input.hook_event_name.as_deref()).into(),
    ));
    // The HARNESS's measurement, not ours. A hook cannot time a call that
    // already finished before it started.
    if let Some(ms) = input.duration_ms {
        fields.push(("duration_ms", ms.into()));
    }
    // Neither a success nor a failure of the action. Carried separately so it
    // cannot be collapsed into either.
    if let Some(interrupted) = input
        .tool_response
        .get("interrupted")
        .and_then(serde_json::Value::as_bool)
    {
        fields.push(("interrupted", interrupted.into()));
    }
    fields
}

/// Map the harness event to an outcome.
///
/// `unknown` for anything unrecognised, INCLUDING an absent event name. The
/// alternative — treating "no event name" as success — would silently report
/// every payload this hook failed to understand as a working action, which is
/// exactly the shape of a guard believed to be passing while it inspects
/// nothing.
#[must_use]
pub fn outcome_of(hook_event_name: Option<&str>) -> &'static str {
    match hook_event_name {
        Some("PostToolUse") => "ok",
        Some("PostToolUseFailure") => "error",
        _ => "unknown",
    }
}

#[cfg(test)]
#[path = "post_bash_test.rs"]
mod post_bash_test;
