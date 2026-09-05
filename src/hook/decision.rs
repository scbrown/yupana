//! [`Decision`] — a guard outcome plus the constraint evaluations that produced
//! it. Split from `pre_edit` for size; the type is the seam between deciding and
//! recording.

use super::Outcome;

/// A guard outcome plus the identity of what produced it.
///
/// [`Outcome`] is what the harness sees; this is what the AUDIT record sees.
/// They are separate because the model-facing text and the operator-facing
/// record answer different questions: the model needs to know what to do
/// instead, the operator needs to know which rule fired so a false positive can
/// be told from a correct deny (yupana #77).
#[derive(Debug, Clone)]
pub(super) struct Decision {
    pub(super) outcome: Outcome,
    /// The constraints evaluated for this action and what each concluded — SARC
    /// I3's `E_i` (`crate::trace`). Empty for a plain allow, where nothing was
    /// evaluated and there is nothing to record.
    ///
    /// This replaced a `+`-joined string of rule ids. The string answered "what
    /// fired" and could not answer "was each evaluated at a point compatible
    /// with its class", which is pass (ii) of the audit checker: two rules that
    /// fired identically at different points produced byte-identical records.
    /// The old field is still DERIVED from this set for the spool, so live
    /// dashboards grouping on it keep working.
    pub(super) constraints: Vec<crate::trace::ConstraintEvaluation>,
    /// How current the POLICY SET that produced these evaluations was.
    ///
    /// Carried on the decision rather than assumed at the recording seam. A
    /// verdict signed with a hardcoded `fresh` claims currency nobody checked —
    /// the exact defect `verdict_turtle` had — and the recorder is the one place
    /// that cannot know the answer: local config is authoritative and genuinely
    /// fresh, while a projected registry may be serving a stale cache, and only
    /// the plane that evaluated the rules knows which.
    pub(super) freshness: crate::types::Freshness,
    /// Exposure and repository used by the governed plane for THIS decision.
    /// Absent when that plane did not produce it; never resolved by the recorder.
    pub(super) governed_context: Option<(&'static str, String)>,
}

impl From<Outcome> for Decision {
    fn from(outcome: Outcome) -> Self {
        Self {
            outcome,
            constraints: Vec::new(),
            governed_context: None,
            // Nothing was evaluated, so there is no policy set whose currency
            // could be in question.
            freshness: crate::types::Freshness::Fresh,
        }
    }
}

impl Decision {
    /// A decision attributed to one or more evaluated constraints.
    /// A decision attributed to one or more evaluated constraints, declaring
    /// the currency of the policy set behind them.
    ///
    /// `freshness` is a parameter and not a default on purpose: a default would
    /// be `Fresh`, and every caller that forgot it would silently claim currency
    /// nobody checked. That is the defect `verdict_turtle` shipped with, one
    /// layer down, and it is worth not rebuilding here.
    pub(super) fn evaluated(
        outcome: Outcome,
        constraints: Vec<crate::trace::ConstraintEvaluation>,
        freshness: crate::types::Freshness,
    ) -> Self {
        Self {
            outcome,
            constraints,
            freshness,
            governed_context: None,
        }
    }

    /// A decision attributed to a single rule id, with the response inferred
    /// from the outcome it produced.
    ///
    /// The convenience form for the capability-scope checks, which have no
    /// declared class or verification point to carry: they are yupana's own
    /// path/blast-radius rules rather than projected quipu policies. Recording
    /// them with `class: None` is honest — they are constraints yupana enforces
    /// and nobody declared a class for.
    pub(super) fn ruled(outcome: Outcome, rule: impl Into<String>) -> Self {
        let response = crate::trace::Response::of(&outcome);
        Self::evaluated(
            outcome,
            vec![crate::trace::ConstraintEvaluation::new(
                rule,
                crate::trace::Outcome::Unsatisfied,
                response,
            )],
            // yupana's own scope rules: no cache between the rule and the check.
            crate::types::Freshness::Fresh,
        )
    }
}
