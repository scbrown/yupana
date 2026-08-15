//! The `PreToolUse` policy guard — a blocking, capability-scoped edit check.
//!
//! This is where the §5.8 trust boundary becomes concrete: an agent's edit tool
//! call is intercepted *before* it lands, sized against the tenant's capability
//! scope (FR-25), and denied with a readable reason when it exceeds it.
//!
//! Everything here is arranged around one invariant: **fail open**. The harness
//! launches every crew agent through this hook, so a guard that fails closed
//! bricks the fleet the moment Yupana is unavailable. Only a policy decision
//! blocks an edit; every error, timeout, and unknown degrades to "allow". See
//! `docs/book/src/reference/policy-guard.md` for the pinned contract.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[path = "decision.rs"]
mod decision;
#[cfg(feature = "quipu")]
#[path = "verdicts.rs"]
mod verdicts;

use decision::Decision;

use super::measure::{measure_within, relative};
use super::{deny_envelope, first_notice_for_session, system_message, HookInput};
use crate::config::YupanaConfig;
use crate::extract::language_for_extension;
use crate::policy::{BlastRadius, Mode};
use crate::types::Freshness;

/// What the guard decided — the value the CLI turns into stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Allow the edit, silently. Exit 0, empty stdout.
    Allow,
    /// Block the edit with this model-facing reason.
    Deny(String),
    /// Allow the edit but tell the user something (advise mode, or fail-open).
    Notify(String),
}

/// Run the `pre-edit` guard: read the harness payload from stdin, decide, and
/// print at most one JSON object. Always returns `Ok` — the process must exit 0
/// so the harness never treats the guard as a fail-closed block.
pub fn run_pre_edit(tenant: Option<&str>, config_override: Option<&Path>) -> anyhow::Result<()> {
    let mut buf = String::new();
    std::io::stdin().lock().read_to_string(&mut buf).ok();
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    match guard(&buf, &root, tenant, config_override) {
        Outcome::Allow => {}
        Outcome::Deny(reason) => println!("{}", deny_envelope(&reason)),
        Outcome::Notify(message) => println!("{}", system_message(&message)),
    }
    Ok(())
}

/// Decide an edit, and SPOOL the decision (aegis-0nng): one `guard` metrics
/// line per invocation — result, duration, extension, and (yupana #77) the target
/// path and the rule that fired — through the fail-silent spool, after the
/// outcome is already fixed. Measurement rides behind the decision; it can
/// never lean on it.
#[must_use]
pub fn guard(
    input_json: &str,
    default_root: &Path,
    tenant: Option<&str>,
    config_override: Option<&Path>,
) -> Outcome {
    let (outcome, fields) = guard_recorded(input_json, default_root, tenant, config_override);
    crate::metrics::emit("guard", &fields);
    outcome
}

/// The guard, returning its outcome AND the record it would spool.
///
/// Split from [`guard`] so the record is testable. The alternative — driving the
/// real spool from a test — needs `$YUPANA_METRICS_PATH` set at runtime, and this
/// crate denies `unsafe_code`, which `std::env::set_var` now requires. Splitting
/// here leaves exactly one untested line in [`guard`] (the `emit` call itself,
/// which `crate::metrics` covers directly) and puts the whole of the record
/// composition — the part with the fields, the conditionals and the omissions —
/// under test through the real decision path.
#[must_use]
fn guard_recorded(
    input_json: &str,
    default_root: &Path,
    tenant: Option<&str>,
    config_override: Option<&Path>,
) -> (Outcome, Vec<(&'static str, serde_json::Value)>) {
    let started = Instant::now();
    let mut decision = guard_inner(input_json, default_root, tenant, config_override);
    let result = match &decision.outcome {
        Outcome::Allow => "allow",
        Outcome::Deny(_) => "deny",
        Outcome::Notify(_) => "notify",
    };
    let input = HookInput::parse(input_json);
    let file_path = input.as_ref().and_then(|i| i.tool_input.file_path.clone());
    let ext = file_path
        .as_ref()
        .and_then(|f| {
            Path::new(f)
                .extension()
                .and_then(OsStr::to_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    // Resolve the config against the PAYLOAD's root, the way `guard_inner` does
    // — not against the process CWD. The record must describe the decision that
    // was actually made: config resolved from a different root can report a
    // `mode` the deciding code never saw, which is worst precisely where the
    // mode matters (an agent invoking the hook from outside the repo it edits
    // spooled `mode: off` for a decision made under `enforce`). One root, one
    // config, one record.
    let root = input
        .as_ref()
        .map_or_else(|| default_root.to_path_buf(), |i| i.root(default_root));
    let config = YupanaConfig::resolve(config_override, &root).ok();
    // The MODE rides every guard line (soak hygiene): the enforce-flip gate is
    // "zero false positives measured over ambient ADVISE traffic", and the
    // first live window was unusable because operator test bursts under an
    // enforce config were indistinguishable from fleet lines. The mode is the
    // filter that makes the soak evidence clean.
    let mode = config.as_ref().map_or("?", |c| c.policy.mode.as_str());

    let mut fields: Vec<(&str, serde_json::Value)> = vec![
        ("result", result.into()),
        ("mode", mode.into()),
        (
            "duration_ms",
            u64::try_from(started.elapsed().as_millis())
                .unwrap_or(u64::MAX)
                .into(),
        ),
        ("ext", ext.into()),
    ];

    // The SUBJECT of the decision (yupana #77). Recorded for allow as well as
    // deny, deliberately and under the same knob: scope that can only be
    // inferred from the absence of denies cannot be verified at all, and an
    // operator confirming a rule is scoped correctly needs to see what it let
    // through as much as what it stopped.
    //
    // Both fields are omitted rather than blanked when they have nothing to say,
    // so a reader never has to distinguish "recorded as empty" from "not
    // recorded" — the same reason the spool never writes a placeholder tier.
    if let (Some(config), Some(file_path)) = (config.as_ref(), file_path.as_ref()) {
        let file = PathBuf::from(file_path);
        let rel = relative(&file, &root);
        if let Some(recorded) = crate::audit::record_path(config.metrics.record_paths, &rel, &file)
        {
            fields.push(("path", recorded.into()));
        }
    }
    // The Σ-derived constraint set (SARC I3): which constraints were evaluated,
    // where, what each concluded, and what was done about it. One array rather
    // than sibling keys, so a reader never reassembles class-to-id by position.
    if !decision.constraints.is_empty() {
        fields.push(("constraints", crate::trace::to_json(&decision.constraints)));
        // How current the POLICY SET behind those evaluations was. A confidence
        // input in SARC's sense (§5.2) and, more practically, the field that
        // stops a soak window counting verdicts computed against a stale
        // projection as if they were evidence about the current policy.
        fields.push(("policy_freshness", decision.freshness.as_str().into()));
    }
    // The pre-existing `rule` field, DERIVED from the same set. Live dashboards
    // group on it; dropping it would silently empty every panel built on it,
    // and that migration is a separate change from the one adding the structure.
    if let Some(rule) = crate::trace::legacy_rule_field(&decision.constraints) {
        fields.push(("rule", rule.into()));
    }

    // Who is answerable for this action (SARC §9.6's α). Recorded on allow as
    // well as deny: an attribution field that appears only on refusals cannot
    // answer "which chain has been acting here", which is the question it exists
    // for. Every element is omitted when undeclared — see `crate::attribution`.
    let tool = input.as_ref().and_then(|i| i.tool_name.as_deref());
    fields.extend(crate::attribution::Attribution::capture(tool).fields());

    // Any throttle a previous PAA crossing recorded, surfaced on THIS edit — the
    // whole point of a response placed at the post-action point.
    let throttled = super::paa::apply_advisories(
        &mut decision.outcome,
        &root.display().to_string(),
        crate::throttle::now_secs(),
    );
    if throttled > 0 {
        fields.push(("throttled", (throttled as u64).into()));
    }

    // STAGE 1 of the semantic-grounded ordering (bobbin-fjh): the nearest
    // prior DENIAL as advisory context, riding BEFORE a refusal's own text so
    // the refusal arrives explained, or alone as a notice on an allow.
    // Similarity never denies. Gated with the guard: Mode::Off stays silent.
    #[cfg(feature = "quipu")]
    if config.as_ref().is_some_and(|c| c.policy.mode != Mode::Off) {
        if let (Some(spool), Some(introduced)) = (
            pre_edit_util::recurrence_spool(),
            input.as_ref().and_then(introduced_text),
        ) {
            if pre_edit_util::apply_recurrence(&mut decision.outcome, &spool, &introduced) {
                fields.push(("recurrence_advised", true.into()));
            }
        }
    }

    // Sign and spool a verdict per evaluated constraint (SARC I3/I8). AFTER the
    // outcome is fixed, like the metrics emit above and for the same reason:
    // recording rides behind the decision and can never lean on it. Signing is
    // microseconds; the /knot round-trip that would actually promote these is
    // deliberately NOT here — see `crate::verdict_spool`.
    #[cfg(feature = "quipu")]
    verdicts::spool_verdicts(
        &decision,
        config.as_ref(),
        &root,
        file_path.as_deref(),
        input.as_ref(),
    );

    (decision.outcome, fields)
}

/// The decision itself. Pure apart from reading the repo, so it is directly
/// testable without a spool in the way.
#[must_use]
fn guard_inner(
    input_json: &str,
    default_root: &Path,
    tenant: Option<&str>,
    config_override: Option<&Path>,
) -> Decision {
    let started = Instant::now();

    // An unparseable payload is an ALLOW: the guard only speaks up about edits
    // it genuinely understands.
    let Some(input) = HookInput::parse(input_json) else {
        return Outcome::Allow.into();
    };
    let Some(file_path) = input.tool_input.file_path.clone() else {
        return Outcome::Allow.into();
    };
    let root = input.root(default_root);

    // Honour `--config` if the operator scoped the guard at a specific file. A
    // bad override path errors here and lands in `fail_open` — a loud allow,
    // never a silent revert to the ambient config the operator meant to bypass.
    let config = match YupanaConfig::resolve(config_override, &root) {
        Ok(config) => config,
        Err(e) => return fail_open(&input, "config", &format!("unreadable config ({e})")).into(),
    };

    let file = PathBuf::from(&file_path);
    let rel = relative(&file, &root);

    // Structural rules (tree-sitter tier) govern the TEXT an edit introduces and
    // are NOT per-tenant: a "no ticket id in a comment" rule holds for everyone.
    // Evaluate them before the tenant-scope gate so they apply even to an
    // unconstrained tenant. A rule Deny/Notify short-circuits the scope checks —
    // one guard decision per edit.
    if let Some(decision) = rule_check(&config, &input, &rel) {
        return decision;
    }

    // Governed policies projected from quipu (Phase 4), evaluated where the
    // evidence is hot — BOTH planes, one registry refresh, one verdict:
    // language-independent text rules (aegis-mqnl's catalogue: the plane the
    // measured leaks needed, since they were in .md/.yml files no grammar
    // covers) and language-gated structural rules. Opt-in (quipu.enabled +
    // endpoint), and behind the `quipu` feature so a default build carries
    // none of it.
    #[cfg(feature = "quipu")]
    if let Some(decision) = governed_check(&config, &input, &root, &rel) {
        return decision;
    }

    // FR-23 at the blocking seam: verify the proposed buffer against the base
    // graph, opt-in (`policy.verify`) and inside the same deadline. Like the
    // rule planes, not per-tenant — a hallucinated reference is wrong whoever
    // writes it.
    if let Some(decision) = verify_arm::verify_check(&config, &input, &root, &rel, started) {
        return decision;
    }

    // No scope for this tenant — mode is off, or the tenant is unconstrained.
    let (Some(tenant), Some(scope)) = (tenant, config.policy.scope_for(tenant)) else {
        return Outcome::Allow.into();
    };

    // A scope whose globs do not compile is misconfigured; say so rather than
    // quietly under-enforcing it.
    let glob_errors = scope.glob_errors();
    if !glob_errors.is_empty() {
        let detail: Vec<String> = glob_errors
            .iter()
            .map(|(pattern, why)| format!("`{pattern}` ({why})"))
            .collect();
        return fail_open(
            &input,
            "globs",
            &format!(
                "scope for tenant `{tenant}` has malformed path globs: {}",
                detail.join(", ")
            ),
        )
        .into();
    }

    // 1. Path scope — cheap, no graph needed, so it runs even under a blown
    //    deadline. This is the check that must never be skipped.
    if let Some(violation) = scope.check_path(&rel, tenant) {
        return Decision::ruled(
            decide(config.policy.mode, violation.message),
            violation.rule,
        );
    }

    // 2. Blast radius — expensive. Bounded by whatever remains of the budget.
    let budget = Duration::from_millis(config.policy.deadline_ms)
        .checked_sub(started.elapsed())
        .unwrap_or_default();
    if scope.max_impacted_symbols.is_none() && scope.max_impacted_files.is_none() {
        return Outcome::Allow.into(); // Nothing to measure against.
    }

    // Size the edit against the RESIDENT daemon when one is expected and usable,
    // else the transient build (FR-31 thin-client cutover). Both paths yield the
    // same `MeasureReply`; `daemon_absent` is set when a daemon was EXPECTED but
    // could not be used, so we can be LOUD about it below.
    let (reply, daemon_absent) = blast_reply(&config, &root, &file, &rel, &input, budget);

    let radius = if reply.measured {
        BlastRadius {
            symbols: reply.symbols,
            files: reply.files,
        }
    } else {
        // NOT MEASURED. Allowing is still the contract — the guard is fail-open by
        // design and a language we cannot parse must not brick an agent's edits.
        // But allowing SILENTLY is the defect: an unparseable file and a
        // genuinely-clean edit produced identical empty stdout, so a rule that
        // could not be evaluated looked exactly like a rule that passed. Say it
        // instead. Rate-limited to once per session by the same gate the
        // fail-open notice uses, because a per-edit message would be scrolled
        // past and then ignored.
        let reason = reply
            .reason
            .clone()
            .unwrap_or_else(|| "unmeasured".to_string());
        eprintln!("yupana: blast radius UNMEASURED for `{rel}`: {reason}");
        let kind = format!("unmeasured-{}-{rel}", reply.kind);
        if first_notice_for_session(input.session_id.as_deref(), &kind) {
            return Decision::ruled(
                Outcome::Notify(format!(
                    "yupana: blast-radius rules were NOT EVALUATED for `{rel}` — \
                     {reason}. The edit is allowed (the guard fails open), but \
                     tenant `{tenant}`'s ceilings did not apply to it. Treat this \
                     file as UNGUARDED by blast radius, not as within limits."
                )),
                "unmeasured",
            );
        }
        return Outcome::Allow.into();
    };

    let verdict = match scope.check_blast(radius, &rel, tenant) {
        Some(violation) => Decision::ruled(
            decide(config.policy.mode, violation.message),
            violation.rule,
        ),
        None => Outcome::Allow.into(),
    };

    // DAEMON EXPECTED BUT DOWN — the cheapest-bypass scenario the daemon exists to
    // prevent. Always log it; and when the edit is otherwise ALLOWED, surface it to
    // the model once per session, because an allowed edit while the resident guard
    // is down is exactly the silent bypass we must not let pass quietly. A Deny wins
    // (blocking the edit is the priority; the daemon-down stays on stderr); an
    // UNMEASURED Notify already returned above. The fail-open is intact: the edit was
    // still guarded, by the transient rebuild.
    if let Some(reason) = daemon_absent {
        eprintln!("yupana: resident guard daemon EXPECTED but unusable: {reason}");
        if matches!(verdict.outcome, Outcome::Allow)
            && first_notice_for_session(input.session_id.as_deref(), "daemon-absent")
        {
            return Outcome::Notify(format!(
                "yupana: the resident guard daemon is DOWN ({reason}). This edit was guarded by a \
                 transient rebuild and ALLOWED — but the daemon a caller could kill to bypass \
                 the guard on every edit is not running. Restart it."
            ))
            .into();
        }
    }
    verdict
}

/// Size an edit into a [`MeasureReply`], from the resident daemon when one is
/// EXPECTED and usable, else the transient build. Returns `(reply, daemon_absent)`;
/// `daemon_absent` is `Some(reason)` only when a daemon was expected but could not
/// be used — so the caller can be loud about it without ever treating "daemon down"
/// as "allow".
///
/// This is the whole cutover, and its shape is the safety contract:
/// - No daemon expected (`use_daemon = false`, the default and every case today):
///   transient build, `None`. Absence is normal and stays silent.
/// - Daemon expected and it answered: use its reply, `None`.
/// - Daemon expected and it could NOT answer (down, or serving a different repo so
///   `/measure` 400s): FALL BACK to the transient build, and return the reason so
///   the caller warns. The guard still runs — fail-open is preserved.
fn blast_reply(
    config: &YupanaConfig,
    root: &Path,
    file: &Path,
    rel: &str,
    input: &HookInput,
    budget: Duration,
) -> (crate::daemon::MeasureReply, Option<String>) {
    let transient = || {
        crate::daemon::MeasureReply::from_sizing(&measure_within(
            root,
            file,
            rel,
            input,
            config.policy.max_hops,
            budget,
        ))
    };

    if !config.serve.use_daemon {
        return (transient(), None);
    }

    let anchors: Vec<String> = input
        .replaced_texts()
        .into_iter()
        .map(str::to_string)
        .collect();
    match crate::daemon::client::fetch_measure(
        &config.serve.bind_address,
        config.serve.mcp_http_port,
        &file.to_string_lossy(),
        rel,
        &anchors,
        config.policy.max_hops,
        budget,
    ) {
        Ok(reply) => (reply, None),
        Err(reason) => (transient(), Some(reason)),
    }
}

#[cfg(feature = "quipu")]
#[path = "grounded_plane.rs"]
mod grounded_plane;
#[path = "pre_edit_util.rs"]
mod pre_edit_util;
use pre_edit_util::{decide, fail_open, introduced_text};
#[path = "rule_planes.rs"]
mod rule_planes;
#[path = "verify_arm.rs"]
mod verify_arm;
#[cfg(feature = "quipu")]
use rule_planes::governed_check;
use rule_planes::rule_check;
// The text plane's pure core is exercised directly by `pre_edit_test`, which
// sits beside this module rather than beside the plane it tests.
#[cfg(all(test, feature = "quipu"))]
use rule_planes::text_plane;




#[cfg(test)]
#[path = "pre_edit_test.rs"]
mod pre_edit_test;
