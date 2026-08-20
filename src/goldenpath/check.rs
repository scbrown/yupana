//! FR-41/FR-42 — the conformance verdict over supplied projected paths.
//!
//! The honesty rules here are hard requirements, inherited from the board
//! guard one plane over:
//!
//! - `followsPath` declared but the path is not among the supplied
//!   projections → REFUSED, never zero findings. Zero findings over a
//!   registry that was never loaded is a green light over a dead backend.
//! - a path under a grammar version this build does not implement →
//!   REFUSED with the versions named; unevaluated is never silently skipped.
//! - unevaluable steps are listed on the verdict, never dropped.
//! - the verdict names its projection freshness when the projection carries
//!   one, and omits the field rather than faking it when it does not.

use serde::{Deserialize, Serialize};

use super::grammar::{
    hazards, match_plan, Deviation, PathLevel, ProjectedPath, SubmittedStep, GRAMMAR_VERSION,
};

/// How the submitted steps are to be read.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CheckMode {
    /// FR-42: the steps are the WHOLE intended plan; deviation is decidable
    /// and named.
    #[default]
    Plan,
    /// FR-41: the steps are the work so far. Under gp-grammar/1 gaps are
    /// allowed, so an open trajectory never hard-deviates — this mode reports
    /// progress and hazards, and never denies.
    Progress,
}

/// A hazard note: a submitted step matching a dead-end signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HazardNote {
    /// Index into the submitted steps.
    pub step: usize,
    /// The path's note for this dead end, when it carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The conformance verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathCheckReport {
    /// The grammar version this verdict applied — equal to the path's, by
    /// construction (a mismatch is a refusal, not a verdict).
    pub grammar: String,
    /// The path checked against.
    pub path: String,
    /// Its blessing level — the effect ceiling.
    pub level: PathLevel,
    /// The mode the steps were read under.
    pub mode: CheckMode,
    /// Pattern elements matched, in order.
    pub matched: usize,
    /// Total pattern length.
    pub pattern_len: usize,
    /// Plan mode only: the first point the plan leaves the path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_deviation: Option<Deviation>,
    /// Steps matching dead-end signatures — served even when nothing blocks.
    pub hazards: Vec<HazardNote>,
    /// Indices of submitted steps with no v1 signature. Reported, never
    /// silently skipped.
    pub unevaluated_steps: Vec<usize>,
    /// The verdict's effect: `none`, `warn`, or `deny`.
    pub effect: String,
    /// The exemplars the path cites — why this path has standing at all.
    pub exemplars: Vec<String>,
    /// Projection freshness, echoed from the projected path; omitted rather
    /// than faked when the projection carries none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_at: Option<String>,
}

/// A check either evaluates or refuses — a refusal is never a verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum CheckOutcome {
    /// The check could not be answered. Not an approval, not a clean report.
    Refused {
        /// Why, naming what was missing or mismatched.
        reason: String,
    },
    /// The verdict.
    Evaluated(Box<PathCheckReport>),
}

/// Evaluate the submitted steps against the declared path.
#[must_use]
pub fn check(
    paths: &[ProjectedPath],
    follows_path: &str,
    steps: &[SubmittedStep],
    mode: CheckMode,
    deny_opt_in: bool,
) -> CheckOutcome {
    if paths.is_empty() {
        return CheckOutcome::Refused {
            reason: format!(
                "followsPath {follows_path} is declared but no projected paths were supplied — \
                 an empty path registry is refused, not reported clean: zero findings over a \
                 registry that was never loaded is a green light over a dead backend"
            ),
        };
    }
    let Some(path) = paths.iter().find(|p| p.path == follows_path) else {
        let held: Vec<&str> = paths.iter().map(|p| p.path.as_str()).collect();
        return CheckOutcome::Refused {
            reason: format!(
                "followsPath {follows_path} is not among the supplied projected paths \
                 ({held:?}) — refusing rather than checking against nothing"
            ),
        };
    };
    if path.grammar != GRAMMAR_VERSION {
        return CheckOutcome::Refused {
            reason: format!(
                "path {} was projected under grammar {} and this build implements {} — the \
                 path is UNEVALUATED, not approved; a cross-version verdict would silently \
                 disagree with the backtest that justified the promotion",
                path.path, path.grammar, GRAMMAR_VERSION
            ),
        };
    }

    let plan = match_plan(&path.pattern, steps);
    let hazard_notes: Vec<HazardNote> = hazards(&path.dead_ends, steps)
        .into_iter()
        .map(|(step, note)| HazardNote { step, note })
        .collect();
    let unevaluated: Vec<usize> = steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.sig().is_none())
        .map(|(i, _)| i)
        .collect();

    let first_deviation = match mode {
        CheckMode::Plan => plan.first_deviation,
        CheckMode::Progress => None,
    };
    let effect = match (&first_deviation, hazard_notes.is_empty()) {
        (None, true) => "none",
        (Some(_), _) if path.level == PathLevel::Blessed && deny_opt_in => "deny",
        _ => "warn",
    };

    CheckOutcome::Evaluated(Box::new(PathCheckReport {
        grammar: path.grammar.clone(),
        path: path.path.clone(),
        level: path.level,
        mode,
        matched: plan.matched,
        pattern_len: path.pattern.len(),
        first_deviation,
        hazards: hazard_notes,
        unevaluated_steps: unevaluated,
        effect: effect.to_string(),
        exemplars: path.exemplars.clone(),
        projected_at: path.projected_at.clone(),
    }))
}

#[cfg(test)]
#[path = "check_test.rs"]
mod check_test;
