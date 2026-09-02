//! The verify arm of the pre-edit guard (FR-23 at the blocking seam).
//!
//! `crate::verify::verify_buffer` rejects hallucinated references — unknown
//! identifiers, wrong arity, unresolved `mod` imports — but until this arm it
//! had no caller in the hook path: the guard checked capability, blast radius,
//! and text rules, and a hallucinated reference sailed through unless something
//! explicitly called `yupana verify`. This wires verification into the
//! `PreToolUse` seam as an OPT-IN arm (`[yupana.policy] verify = true`, off by
//! default) that rides inside the guard's existing latency budget.
//!
//! The fail-open contract holds throughout: a buffer that cannot be
//! reconstructed, a parse error, or a blown deadline degrades to "no opinion",
//! never to a block. Only a *verdict* — this edit introduces a reference that
//! resolves to nothing — produces a Deny (enforce) or Notify (advise).

// A child module of `pre_edit`, like `rule_planes`: it reads that module's
// private `Decision`, `decide` and `introduced_text` through `use super::*`.
use super::*;

/// Evaluate the verify arm. `None` means "no opinion" — the guard's remaining
/// checks proceed. `Some` is a decision (deny/notify per mode) attributed to
/// the `verify` rule.
pub(super) fn verify_check(
    config: &YupanaConfig,
    input: &HookInput,
    root: &Path,
    rel: &str,
    started: Instant,
) -> Option<Decision> {
    // Opt-in, and Mode::Off disarms the whole guard, this arm included.
    if !config.policy.verify || config.policy.mode == Mode::Off {
        return None;
    }
    // The verifier is a tree-sitter-rust pass; other languages have nothing to
    // verify at this tier (not a gap to report — same rule as text rules).
    if Path::new(rel).extension().and_then(OsStr::to_str) != Some("rs") {
        return None;
    }
    // A pure deletion introduces no text, so there is no new reference to
    // hallucinate; the verifier would trivially pass. Skip the parse.
    introduced_text(input)?;

    // The latency budget is the guard's own deadline. Verification is a full
    // parse of the proposed buffer (plus the baseline), so when the earlier
    // checks have already spent the budget, skip rather than blow it — silence
    // here is honest because the arm is advisory-by-configuration, and the
    // deadline expiring mid-guard is already surfaced by the blast-radius path.
    let remaining = Duration::from_millis(config.policy.deadline_ms)
        .checked_sub(started.elapsed())
        .unwrap_or_default();
    if remaining.is_zero() {
        eprintln!("yupana: verify arm skipped for `{rel}`: guard deadline exhausted");
        return None;
    }

    let file = PathBuf::from(input.tool_input.file_path.as_deref()?);
    // Baseline: the file as it stands. `None` for a new file — verify treats
    // every call in the buffer as introduced, which is exactly right.
    let baseline = std::fs::read_to_string(&file).ok();
    let proposed = proposed_buffer(input, baseline.as_deref())?;

    match crate::verify::verify_buffer(root, &file, &proposed, baseline.as_deref()) {
        Ok(verdict) if verdict.ok => None,
        Ok(verdict) => {
            let mut lines: Vec<String> = verdict
                .violations
                .iter()
                .map(|v| format!("line {}: {}", v.line, v.message))
                .collect();
            lines.sort();
            Some(Decision::ruled(
                decide(
                    config.policy.mode,
                    format!(
                        "this edit introduces references that do not resolve \
                         [verify, {} tier]:\n{}",
                        verdict.tier.as_str(),
                        lines.join("\n")
                    ),
                ),
                "verify",
            ))
        }
        // Unparseable buffer or extractor failure: the arm has no verdict.
        // Fail open quietly — an agent mid-refactor writes syntactically broken
        // intermediate states all the time, and a per-edit warning would train
        // everyone to ignore it.
        Err(e) => {
            eprintln!("yupana: verify arm failed open for `{rel}`: {e}");
            None
        }
    }
}

/// Reconstruct the buffer the edit proposes. `Write` carries it whole; an
/// `Edit`/`MultiEdit` is applied to the baseline one replacement at a time.
/// `None` when the buffer cannot be reconstructed — an anchor that does not
/// match the file means the harness will reject the tool call itself, so there
/// is nothing real to verify.
pub(crate) fn proposed_buffer(input: &HookInput, baseline: Option<&str>) -> Option<String> {
    if let Some(content) = &input.tool_input.content {
        return Some(content.clone());
    }
    let mut text = baseline?.to_string();
    let pairs: Vec<(&str, &str)> = std::iter::once((
        input.tool_input.old_string.as_deref(),
        input.tool_input.new_string.as_deref(),
    ))
    .chain(
        input
            .tool_input
            .edits
            .iter()
            .map(|e| (e.old_string.as_deref(), e.new_string.as_deref())),
    )
    .filter_map(|(old, new)| Some((old?, new.unwrap_or_default())))
    .collect();
    if pairs.is_empty() {
        return None;
    }
    for (old, new) in pairs {
        if !text.contains(old) {
            return None;
        }
        text = text.replacen(old, new, 1);
    }
    Some(text)
}
