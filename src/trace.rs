//! The Σ-derived trace record — what an action tells an auditor afterwards.
//!
//! SARC (arXiv:2605.07728) invariant I3 requires that every execution emit a
//! structured trace **whose schema is derived from the specification**, holding
//! the pre-state, the action, the post-state, *the constraints evaluated and
//! their outcomes*, and an attribution tuple. I8 then makes the audit decidable:
//! given Σ and a trace T, check `T ⊨ Σ` mechanically. The check needs four
//! things per action — which constraints applied, where each was evaluated, what
//! outcome it produced, and what response was taken — and none of them can be
//! recovered from a record that says only which rules fired.
//!
//! That was the shape of the spool before this module: `rule` was a `+`-joined
//! string of names. It answers "what fired" and cannot answer "was this
//! evaluated at a point compatible with its class", which is exactly pass (ii)
//! of the audit checker. Two rules that fired identically but at different
//! points produced byte-identical records.
//!
//! ## What is honestly recordable today
//!
//! [`ConstraintEvaluation`] is the full `E_i` for one constraint. The
//! attribution tuple `α` — SARC §9.6's `⟨P, planner, executor, tool, auth,
//! C_eval⟩` — lives next door in [`crate::attribution`], which composes the
//! other five elements around the `C_eval` this module emits. Only `auth` is
//! deliberately not yupana's to record: the effective authority is the
//! intersection of every link's grant, the grants live in quipu, and a
//! locally-guessed value would put a number in the field the grant store never
//! agreed to. Recording `P` is what lets the checker derive it.

use serde::{Deserialize, Serialize};

use crate::constraint::{ConstraintClass, VerificationPoint};

/// What a constraint evaluation concluded. Mirrors quipu's `aegis:outcome`
/// enum, and keeps SARC's distinction between the two ways a constraint fails
/// to be satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The constraint holds.
    Satisfied,
    /// The evidence says it does not hold.
    Unsatisfied,
    /// There was no evidence to decide on. NOT a soft `unsatisfied`: a
    /// constraint that could not be evaluated has told you nothing, and
    /// collapsing the two is how an unevaluated check reads as a passing one.
    Unknown,
}

/// What the guard actually did about a fired constraint.
///
/// Recorded separately from [`Outcome`] because they answer different audit
/// questions — the outcome is what the predicate concluded, the response is what
/// the runtime did with it — and because pass (iii) of the audit checker is
/// precisely "does the recorded response match the one the policy declared".
/// Collapsing them would make that check unwritable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Response {
    /// The action was refused.
    Blocked,
    /// The violation was reported to the model without blocking (advise mode,
    /// or a soft constraint).
    Warned,
    /// Recorded only; the model was told nothing.
    Logged,
    /// Routed to a human. Not reachable until the escalation router lands;
    /// present so the vocabulary does not have to change when it does.
    Escalated,
    /// The constraint fired and the runtime did nothing about it. Distinct from
    /// `Logged`: this is the state to grep for, not one to design toward.
    NoAction,
}

impl Response {
    /// The response implied by a guard outcome.
    ///
    /// Lives here rather than in the hook so the mapping is stated once: the
    /// audit checker compares the recorded response against the one the policy
    /// declared, and a second, drifting copy of this mapping would make the two
    /// disagree for reasons no reader could locate.
    #[must_use]
    pub fn of(outcome: &crate::hook::Outcome) -> Self {
        match outcome {
            crate::hook::Outcome::Deny(_) => Self::Blocked,
            crate::hook::Outcome::Notify(_) => Self::Warned,
            // An allow after a constraint was evaluated means it did not fire;
            // `NoAction` is the honest label, and callers that know the
            // constraint was satisfied record that as the outcome.
            crate::hook::Outcome::Allow => Self::NoAction,
        }
    }
}

/// One constraint's evaluation, for one action — SARC's `E_i` element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstraintEvaluation {
    /// The constraint's stable id — the same name its verdict cites.
    pub id: String,
    /// The declared class, when the constraint declared one. `None` for a
    /// locally-configured rule or a policy projected from a pre-Phase-1 catalog;
    /// absent rather than guessed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<ConstraintClass>,
    /// Where it was evaluated. Absent when undeclared, for the same reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_point: Option<VerificationPoint>,
    /// The layer that ACTUALLY evaluated it — never the layer the policy
    /// claimed. SARC I6 is checkable only if the record says what happened
    /// rather than repeating the declaration; a field that echoed the claim
    /// would let a `tool` overclaim survive the audit by being asserted twice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hosted_at: Option<crate::hosting::HostingLayer>,
    /// What the predicate concluded.
    pub outcome: Outcome,
    /// What the runtime did about it.
    pub response: Response,
    /// Content-addressed evidence used for this evaluation, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grounding_id: Option<String>,
    /// Fog boundary bound into the grounding evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faction_id: Option<String>,
    /// Exact world-view content hash bound into the grounding evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worldview_sha256: Option<String>,
    /// Turn-grounding resolver state (`used`, `empty`, `missing`, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grounding_outcome: Option<String>,
}

impl ConstraintEvaluation {
    /// An evaluation of `id` that concluded `outcome` and drew `response`.
    pub fn new(id: impl Into<String>, outcome: Outcome, response: Response) -> Self {
        Self {
            id: id.into(),
            class: None,
            verification_point: None,
            hosted_at: None,
            outcome,
            response,
            grounding_id: None,
            faction_id: None,
            worldview_sha256: None,
            grounding_outcome: None,
        }
    }

    /// Attach the declared class and point, when the constraint carried them.
    #[must_use]
    pub fn placed(
        mut self,
        class: Option<ConstraintClass>,
        verification_point: Option<VerificationPoint>,
    ) -> Self {
        self.class = class;
        self.verification_point = verification_point;
        self
    }

    /// Record the layer that actually evaluated this constraint.
    ///
    /// Takes the layer as an argument rather than defaulting it, so no caller
    /// can claim a layer by forgetting to say one — the same reason
    /// `Decision::evaluated` takes freshness as a parameter.
    #[must_use]
    pub fn hosted_at(mut self, layer: crate::hosting::HostingLayer) -> Self {
        self.hosted_at = Some(layer);
        self
    }

    /// Bind a turn-grounding evaluation to its content-addressed evidence.
    #[must_use]
    pub fn grounded(
        mut self,
        reference: &crate::turn_grounding::GroundingRef,
        outcome: &crate::turn_grounding::GroundingState,
    ) -> Self {
        self.grounding_id = reference.grounding_id.clone();
        self.faction_id = reference.faction_id.clone();
        self.worldview_sha256 = reference.worldview_sha256.clone();
        self.grounding_outcome = Some(outcome.as_str().to_string());
        self
    }
}

/// Render a set of evaluations as the spool's `constraints` field.
///
/// Serialised as one JSON array rather than flattened into sibling keys so a
/// reader never has to reassemble which class went with which id — the
/// reassembly-by-position bug that a `+`-joined `rule` string plus a separate
/// count would invite.
#[must_use]
pub fn to_json(evaluations: &[ConstraintEvaluation]) -> serde_json::Value {
    serde_json::to_value(evaluations).unwrap_or(serde_json::Value::Null)
}

/// The `+`-joined rule id string the spool carried before this module.
///
/// Retained because a live spool reader and the existing soak dashboards group
/// on it: dropping the field would silently empty every panel built on it, and
/// the migration is a separate change from the one that adds the structure.
/// Sorted and deduplicated, so the same set of rules always renders the same
/// string.
#[must_use]
pub fn legacy_rule_field(evaluations: &[ConstraintEvaluation]) -> Option<String> {
    let mut ids: Vec<&str> = evaluations
        .iter()
        .filter(|e| e.outcome == Outcome::Unsatisfied)
        .map(|e| e.id.as_str())
        .collect();
    if ids.is_empty() {
        return None;
    }
    ids.sort_unstable();
    ids.dedup();
    Some(ids.join("+"))
}

#[cfg(test)]
#[path = "trace_test.rs"]
mod tests;
