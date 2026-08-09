//! SARC constraint metadata — the fields a governed rule carries beyond its
//! predicate (SARC arXiv:2605.07728 §3.1, §4.1).
//!
//! quipu's `aegis:Policy` declares WHAT KIND of bound a constraint is
//! (`constraintClass`) separately from WHERE it is evaluated
//! (`verificationPoint`) and separately again from what happens when it fires
//! (`effect`). Those were one field until Phase 1; collapsing them is what made
//! "may this action ever execute?" unanswerable from the graph.
//!
//! Lives in its own module rather than in [`crate::rules`] because the
//! post-edit auditor needs the same vocabulary as the pre-edit gate, and
//! neither owns it.

use serde::{Deserialize, Serialize};

/// The SARC constraint class — what kind of bound a rule is, independent of the
/// response it declares. Mirrors quipu's `aegis:constraintClass` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConstraintClass {
    /// Violations are not permitted under any policy. Blocks under
    /// [`crate::policy::Mode::Enforce`].
    Hard,
    /// Violations are admissible at a declared cost. NEVER blocks — a soft
    /// constraint that blocks is a hard one with a misleading name.
    Soft,
    /// Control transfers to a human. Blocks until the escalation router can
    /// grant it, which today means it blocks (there is no router yet); the
    /// message says so rather than implying a ruling was sought.
    Escalation,
}

/// The enforcement point a rule is evaluated at. Mirrors
/// `aegis:verificationPoint`; yupana hosts `Pag` and `Paa`, and carries the
/// remaining values so a projected policy round-trips rather than being dropped
/// for naming a point yupana does not itself host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerificationPoint {
    /// Pre-Action Gate — before dispatch. `yupana hook pre-edit`.
    #[serde(rename = "PAG")]
    Pag,
    /// Action-Time Monitor — mid-flight. Not hosted by yupana today.
    #[serde(rename = "ATM")]
    Atm,
    /// Post-Action Auditor — after the action, before the next. `yupana hook
    /// post-edit`.
    #[serde(rename = "PAA")]
    Paa,
    /// Enforced inside the tool implementation.
    #[serde(rename = "tool_layer")]
    ToolLayer,
    /// Enforced outside the agent process entirely.
    #[serde(rename = "policy_layer")]
    PolicyLayer,
}

impl ConstraintClass {
    /// Parse quipu's lexical form. `None` for an unrecognised value, which the
    /// caller turns into a projection error — never a silent default, because
    /// defaulting an unknown class to `soft` would silently downgrade a hard
    /// constraint and defaulting it to `hard` would block on a typo.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hard" => Some(Self::Hard),
            "soft" => Some(Self::Soft),
            "escalation" => Some(Self::Escalation),
            _ => None,
        }
    }

    /// Whether a violation of this class may block an edit, given the ambient
    /// mode.
    ///
    /// [`crate::policy::Mode::Advise`] is a ceiling, not a class override: an
    /// advise-mode deployment never blocks, whatever the class says. That is
    /// what makes staging a new hard constraint safe.
    #[must_use]
    pub fn blocks(self, mode: crate::policy::Mode) -> bool {
        match mode {
            crate::policy::Mode::Off | crate::policy::Mode::Advise => false,
            crate::policy::Mode::Enforce => matches!(self, Self::Hard | Self::Escalation),
        }
    }
}

impl VerificationPoint {
    /// Parse quipu's lexical form; `None` for an unrecognised value.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "PAG" => Some(Self::Pag),
            "ATM" => Some(Self::Atm),
            "PAA" => Some(Self::Paa),
            "tool_layer" => Some(Self::ToolLayer),
            "policy_layer" => Some(Self::PolicyLayer),
            _ => None,
        }
    }

    /// Whether yupana's pre-edit guard is the host for this point. A rule
    /// declared at the `PAA` must NOT fire at pre-edit — evaluating it there
    /// would block on evidence the rule's author said should be judged after
    /// the fact.
    #[must_use]
    pub fn is_pre_edit(self) -> bool {
        matches!(self, Self::Pag)
    }
}
