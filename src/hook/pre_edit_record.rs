//! Composing the `guard` spool record — the audit half of the pre-edit path,
//! split out of `pre_edit` so the decision and the record it produces are read
//! separately. Nothing here can change an outcome: [`guard_recorded`] takes the
//! decision already made and describes it.
//!
//! Extracted under the file-size ratchet (yupana #83) when the provenance
//! fields landed. The seam is deliberate rather than incidental — record
//! composition is where the omit-never-blank rules live, and they are easier to
//! hold to when they are not interleaved with the evaluation order.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::super::HookInput;
use super::guard_inner;
#[cfg(feature = "quipu")]
use super::pre_edit_util;
#[cfg(feature = "quipu")]
use super::pre_edit_util::introduced_text;
#[cfg(feature = "quipu")]
use super::verdicts;
use super::Outcome;
use crate::config::YupanaConfig;
use crate::hook::measure::relative;
#[cfg(feature = "quipu")]
use crate::policy::Mode;

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
pub(super) fn guard_recorded(
    input_json: &str,
    default_root: &Path,
    tenant: Option<&str>,
    config_override: Option<&Path>,
) -> (Outcome, Vec<(&'static str, serde_json::Value)>) {
    let started = Instant::now();
    let mut decision = guard_inner(input_json, default_root, tenant, config_override);
    let input = HookInput::parse(input_json);
    if let Some(reference) = input.as_ref().and_then(|i| i.grounding.as_ref()) {
        let state = crate::turn_grounding::assess(
            Some(reference),
            crate::turn_grounding::cache_dir().as_deref(),
            crate::turn_grounding::now_secs(),
            crate::turn_grounding::max_age_secs(),
        );
        super::apply_turn_grounding(&mut decision, reference, &state);
    }
    let result = match &decision.outcome {
        Outcome::Allow => "allow",
        Outcome::Deny(_) => "deny",
        Outcome::Notify(_) => "notify",
    };
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
        // TWO-SIDED LIVENESS. `guard` is emitted on every invocation, so its
        // ABSENCE already means the hook did not run. Its PRESENCE did not
        // distinguish the two ways it can run and say nothing — a clean allow,
        // and a payload we could not parse — because both produce
        // `result: allow` with an empty `ext`. Those need opposite fixes: one is
        // the system working, the other is a harness whose payload shape moved
        // under us, and reading the second as the first is how a guard is
        // believed to be passing while it inspects nothing.
        //
        // This is the same discriminator `pre_bash_invoked` carries
        // (`crate::hook::pre_bash::invocation_fields`), for the same reason and
        // under the same name. It rides the existing record rather than a second
        // one: emitting a separate `pre_edit_invoked` would duplicate a line
        // that already fires unconditionally, and two records per edit is a
        // measurable cost on the highest-volume path in the spool.
        ("parsed", input.is_some().into()),
        (
            "duration_ms",
            u64::try_from(started.elapsed().as_millis())
                .unwrap_or(u64::MAX)
                .into(),
        ),
        ("ext", ext.into()),
    ];
    // WHICH PIECE OF WORK CAUSED THIS, and WITHIN WHICH SESSION. The spool
    // already answers "who" (agent) and "to what" (path); without these two it
    // cannot answer the question `crate::plate` exists for, and it cannot be
    // replayed — a windowed rule ("this session has already done N of these")
    // has nothing to group by. `item` rode only the `action` records emitted by
    // `pre_bash`, so the highest-volume kind in the spool — every edit — carried
    // no work item at all.
    //
    // OMITTED, NEVER BLANKED, on the same rule as `path` and `ext` above: an
    // unresolvable plate is UNKNOWN, and these records get replayed to DERIVE
    // enforcement rules, so a fabricated value does not merely mislabel one row
    // — it manufactures evidence for a rule that then applies to everyone.
    if let Some(session) = input.as_ref().and_then(|i| i.session_id.as_deref()) {
        fields.push(("session", session.into()));
    }
    // Scope the plate read to THIS session. A plate stamped by a session that has
    // since died no longer answers for the one making this edit, and attributing
    // an action to a dead session's work item is the confidently-wrong case the
    // plate docs call out. A dispatcher-written plate carries no session and is
    // still read — see `plate::parse`.
    if let Some(item) = crate::plate::current(input.as_ref().and_then(|i| i.session_id.as_deref()))
    {
        fields.push(("item", item.into()));
    }

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
    let throttled = crate::hook::paa::apply_advisories(
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
