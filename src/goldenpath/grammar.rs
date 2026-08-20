//! gp-grammar/1 — Yupana's implementation of the shared conformance grammar.
//!
//! ONE contract, two implementations that must never disagree: Quipu's
//! backtest justifies a promotion under this grammar, and this guard enforces
//! the promoted path under the same one. The definition lives in
//! `quipu docs/design/conformance-grammar.md`; any change to matching
//! semantics is a new major version, because a verdict under new rules is not
//! comparable to a backtest under old ones. That is why every
//! [`ProjectedPath`] carries its version and an unknown one is refused rather
//! than approximated — the `selectorLang "sparql"` rule, one plane over.

use serde::{Deserialize, Serialize};

/// The grammar version this module implements.
pub const GRAMMAR_VERSION: &str = "gp-grammar/1";

/// A step's v1 signature: `(actionKind, targetClass)`, compared exactly.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepSig {
    /// The step's `actionKind`, case-sensitive.
    pub action_kind: String,
    /// The class of the step's target: an IRI target's deterministically
    /// chosen `rdf:type` (`untyped` when it has none), `literal` for a
    /// literal target, `none` when the step has no target.
    pub target_class: String,
}

/// A dead-end hazard on a path: a signature the exemplars tried that did not
/// help. Matching one is a warning note, never a deviation by itself.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadEnd {
    /// The hazard's signature.
    #[serde(flatten)]
    pub sig: StepSig,
    /// What the exemplars learned, verbatim from the path.
    #[serde(default)]
    pub note: Option<String>,
}

/// The blessing level of a projected path. Levels below advisory are never
/// projected (there is nothing to enforce), and constraint-backing is out of
/// scope until verdict signing exists — refusing to parse it here is what
/// keeps an unsigned L5 from enforcing as if it were signed.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathLevel {
    /// L3 — warn-tier guidance; conformance recorded, nothing blocks.
    Advisory,
    /// L4 — warn by default; deny is a per-call opt-in.
    Blessed,
}

/// A golden path as projected from Quipu — the serialization pinned in
/// `conformance-grammar.md`, plus the blessing metadata FR-40 requires it to
/// carry.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedPath {
    /// The grammar version this path's promotion was backtested under. An
    /// unknown version makes the path UNEVALUABLE here, never approximated.
    pub grammar: String,
    /// The `GoldenPath` IRI.
    pub path: String,
    /// The blessing level the effect ceiling comes from.
    pub level: PathLevel,
    /// The pattern: kept-step signatures, in order.
    pub pattern: Vec<StepSig>,
    /// Dead-end hazards.
    #[serde(default)]
    pub dead_ends: Vec<DeadEnd>,
    /// Exemplar trajectory IRIs — what a warn or deny cites: "because this
    /// concrete work succeeded this way".
    #[serde(default)]
    pub exemplars: Vec<String>,
    /// When this projection was taken. Echoed on every verdict as its
    /// projection freshness; omitted rather than faked when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_at: Option<String>,
}

/// One step of the work being checked, as submitted by the caller. A step
/// with no `action_kind` has no v1 signature and is unevaluable: it never
/// matches and never deviates — missing data is not misconduct — and it is
/// reported in the verdict's unevaluated list rather than silently skipped.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedStep {
    /// The step's `actionKind`; absent = unevaluable.
    #[serde(default)]
    pub action_kind: Option<String>,
    /// The step's target class; absent = `none`.
    #[serde(default)]
    pub target_class: Option<String>,
    /// Optional human-readable label, echoed in reports.
    #[serde(default)]
    pub label: Option<String>,
}

impl SubmittedStep {
    /// This step's v1 signature, if it has one.
    #[must_use]
    pub fn sig(&self) -> Option<StepSig> {
        self.action_kind.as_ref().map(|kind| StepSig {
            action_kind: kind.clone(),
            target_class: self
                .target_class
                .clone()
                .unwrap_or_else(|| "none".to_string()),
        })
    }
}

/// Where a complete plan landed against a pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanMatch {
    /// Pattern elements matched, in order.
    pub matched: usize,
    /// The first pattern element no remaining step could match, with the
    /// index of the last plan step that DID match (the deviation anchor).
    /// `None` = the whole pattern was matched.
    pub first_deviation: Option<Deviation>,
}

/// The first point a plan leaves the path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Deviation {
    /// Index into the path's pattern of the unmatched element.
    pub pattern_index: usize,
    /// Index into the submitted steps of the last matched step, if any
    /// matched at all.
    pub after_step: Option<usize>,
}

/// Match `pattern` as an in-order subsequence of the submitted steps (gaps
/// allowed), per gp-grammar/1. Unevaluable steps neither match nor deviate.
#[must_use]
pub fn match_plan(pattern: &[StepSig], steps: &[SubmittedStep]) -> PlanMatch {
    let mut next = 0usize;
    let mut last_matched: Option<usize> = None;
    for (i, step) in steps.iter().enumerate() {
        if next == pattern.len() {
            break;
        }
        if step.sig().is_some_and(|sig| sig == pattern[next]) {
            next += 1;
            last_matched = Some(i);
        }
    }
    PlanMatch {
        matched: next,
        first_deviation: (next < pattern.len()).then_some(Deviation {
            pattern_index: next,
            after_step: last_matched,
        }),
    }
}

/// Which submitted steps match a dead-end signature: `(step index, note)`.
#[must_use]
pub fn hazards(dead_ends: &[DeadEnd], steps: &[SubmittedStep]) -> Vec<(usize, Option<String>)> {
    let mut out = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let Some(sig) = step.sig() else { continue };
        if let Some(hit) = dead_ends.iter().find(|d| d.sig == sig) {
            out.push((i, hit.note.clone()));
        }
    }
    out
}

#[cfg(test)]
#[path = "grammar_test.rs"]
mod grammar_test;
