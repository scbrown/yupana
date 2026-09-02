//! Small shared helpers of the pre-edit guard — `introduced_text`, the
//! mode-to-outcome mapping and the loud fail-open — lifted out of `pre_edit`
//! for size (the 500-line limit). A child module like `rule_planes`, and
//! re-imported into `pre_edit`, so every plane keeps reaching them through
//! `use super::*`.

use super::*;

/// The text an edit INTRODUCES: the full `Write` content, else the `new_string`s
/// of an `Edit`/`MultiEdit` joined by newlines. `None` when the payload adds no
/// text (e.g. a pure deletion), in which case there is nothing for a rule to see.
pub(super) fn introduced_text(input: &HookInput) -> Option<String> {
    if let Some(content) = &input.tool_input.content {
        return Some(content.clone());
    }
    let mut parts: Vec<&str> = Vec::new();
    if let Some(new) = input.tool_input.new_string.as_deref() {
        parts.push(new);
    }
    for edit in &input.tool_input.edits {
        if let Some(new) = edit.new_string.as_deref() {
            parts.push(new);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Turn a violation into an outcome according to the enforcement mode.
pub(super) fn decide(mode: Mode, message: String) -> Outcome {
    match mode {
        Mode::Enforce => Outcome::Deny(message),
        // Advise: report what would have been denied, but never block.
        Mode::Advise => Outcome::Notify(format!("yupana (advise, not blocking): {message}")),
        Mode::Off => Outcome::Allow,
    }
}

/// Degrade to "allow", loudly. Writes the stderr line the contract requires and,
/// once per session, a user-visible notice — because a hook's stderr is
/// surfaced only on exit `2`, so stderr alone would be silent in practice.
pub(super) fn fail_open(_input: &HookInput, kind: &str, reason: &str) -> Outcome {
    eprintln!("yupana: policy guard failed open: {reason}");
    // The metric that separates "allowed clean" from "allowed because the
    // check could not run" — the two must never share a label (aegis-0nng).
    crate::metrics::emit("fail_open", &[("fail_kind", kind.into())]);
    if matches!(kind, "config" | "globs" | "tripwires") {
        return Outcome::Notify(format!(
            "{} policy guard failed open ({reason}) — edits are UNGUARDED this session.",
            crate::hook::CONFIG_ERROR_PREFIX
        ));
    }
    Outcome::Notify(format!(
        "yupana: policy guard failed open ({reason}) — edits are UNGUARDED this session."
    ))
}

/// Apply the stage-1 denied-edit recurrence advisory (bobbin-fjh) to an
/// already-fixed outcome. The ordering rule is identify-and-inform-before-
/// refusing: the advisory context PRECEDES a refusal's text, so a denial
/// arrives explained; on an allow it surfaces alone as a notice. Similarity
/// never denies — this function can turn an Allow into a Notify and nothing
/// stronger. Returns whether an advisory was attached.
#[cfg_attr(not(feature = "quipu"), allow(dead_code))] // caller is quipu-gated; tests are not
pub(super) fn apply_recurrence(outcome: &mut Outcome, spool: &Path, introduced: &str) -> bool {
    let Some(advisory) = crate::recurrence::advisory(spool, introduced) else {
        return false;
    };
    *outcome = match std::mem::replace(outcome, Outcome::Allow) {
        Outcome::Deny(reason) => Outcome::Deny(format!("{advisory}\n\n{reason}")),
        Outcome::Notify(message) => Outcome::Notify(format!("{advisory}\n\n{message}")),
        Outcome::Allow => Outcome::Notify(advisory),
    };
    true
}

/// The spool the recurrence corpus is read from — the same resolution the
/// verdict writer uses, so the advisory reads exactly what denials wrote.
/// Gated with the writer: without the `quipu` feature no denial is ever
/// spooled, so there is no corpus to read.
#[cfg(feature = "quipu")]
pub(super) fn recurrence_spool() -> Option<PathBuf> {
    crate::verdict_spool::resolve_path(
        std::env::var("YUPANA_VERDICT_PATH").ok().as_deref(),
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}
