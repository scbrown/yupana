//! `H-SARC-I6` — checking a constraint's claimed hosting layer against reality.
//!
//! SARC (arXiv:2605.07728) invariant I6: a constraint must be hosted at a layer
//! the agent cannot rewrite. `aegis:hostedAtLayer` declares that layer, quipu's
//! placement check rejects the values that are wrong on their face
//! (`"prompt"` is not in the vocabulary at all), and until this module nothing
//! compared the declaration to **where the constraint is actually evaluated**.
//!
//! That gap has a specific shape. A policy may declare `"tool"` — the layer an
//! agent cannot route around — while being enforced solely by yupana's pre-edit
//! hook, which an agent bypasses by writing the file some other way. I6 was
//! checked for well-formedness and not for truth, and a false `tool` claim is
//! worse than an honest `orchestration` one, because it stops people looking.
//!
//! ## One-directional, and why
//!
//! Declaring a *weaker* layer than the truth is harmless: a tool-enforced
//! constraint described as `orchestration` understates its own robustness, and
//! nobody is misled in a direction that costs them. Declaring a *stronger* one
//! is the failure. So this flags only `declared > actual` and stays silent
//! otherwise — including when nothing is declared, because an absent claim
//! overclaims nothing.
//!
//! ## A loud fail-open, never a block
//!
//! A mismatch is a **documentation defect**, and yupana refusing to project the
//! policy would disable a rule that does still work — trading a metadata error
//! for an enforcement gap, which is the worse of the two by a wide margin.
//! [`overclaims`] reports; the caller projects the rule anyway and evaluates it
//! at the layer that is real. The layer actually used is then recorded on the
//! trace, so the audit checker can verify the claim against the record instead
//! of taking the claim.

use serde::{Deserialize, Serialize};

/// Where a constraint is enforced, ordered by how hard it is for the agent to
/// route around. Mirrors quipu's `aegis:hostedAtLayer`.
///
/// There is no `Prompt` variant, and that is the vocabulary's decision rather
/// than an omission here: a constraint that lives in the prompt is one the model
/// can be talked out of, which is exactly what I6 forbids. Adding the variant to
/// "round-trip faithfully" would make the unrepresentable representable for the
/// sake of tidiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostingLayer {
    /// The agent's own loop — a hook, a wrapper, a harness matcher. Real, and
    /// bypassable by an agent that takes a different route to the same effect.
    Orchestration,
    /// Inside the tool implementation. The agent cannot reach past it without
    /// not using the tool.
    Tool,
    /// Outside the agent process entirely — a write gate, a server, a reference
    /// monitor. The agent cannot reach it at all.
    Policy,
}

/// The layer yupana's own hooks enforce at.
///
/// A constant, not a configurable. A hook in the agent's loop **is** the
/// orchestration layer; making this settable would let a deployment declare
/// itself into a robustness it does not have, which is the exact claim this
/// module exists to check.
pub const YUPANA_HOSTS_AT: HostingLayer = HostingLayer::Orchestration;

impl HostingLayer {
    /// Parse quipu's lexical form. `None` for anything else — including
    /// `"prompt"`, which the vocabulary does not define.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "orchestration" => Some(Self::Orchestration),
            "tool" => Some(Self::Tool),
            "policy" => Some(Self::Policy),
            _ => None,
        }
    }

    /// The lexical form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Orchestration => "orchestration",
            Self::Tool => "tool",
            Self::Policy => "policy",
        }
    }
}

/// The notice for a policy claiming a more robust layer than the one that will
/// evaluate it. `None` when the claim is honest, understated, or absent.
///
/// Names the policy, the claim and the truth, because a notice saying only
/// "layer mismatch" leaves an author unable to tell which direction to fix.
#[must_use]
pub fn overclaims(
    name: &str,
    declared: Option<HostingLayer>,
    actual: HostingLayer,
) -> Option<String> {
    let declared = declared?;
    if declared <= actual {
        return None;
    }
    Some(format!(
        "policy '{name}' declares aegis:hostedAtLayer \"{declared}\" but is \
         evaluated at the {actual} layer, which an agent can route around by \
         taking a different path to the same effect. The rule still runs — this \
         is a metadata defect, not an enforcement one — but the claim is \
         stronger than the placement. Either move the enforcement to \
         \"{declared}\", or declare \"{actual}\" and be right.",
        declared = declared.as_str(),
        actual = actual.as_str(),
    ))
}

/// Every overclaim across a projected catalog, in catalog order.
///
/// Returned rather than logged so the caller decides where it surfaces: this
/// runs at the projection seam, and the seam is inside the pre-edit hook, where
/// printing is the guard's decision to make and not a library's.
#[must_use]
pub fn audit_projection(
    policies: &[(String, Option<HostingLayer>)],
    actual: HostingLayer,
) -> Vec<String> {
    policies
        .iter()
        .filter_map(|(name, declared)| overclaims(name, *declared, actual))
        .collect()
}

#[cfg(test)]
#[path = "hosting_test.rs"]
mod tests;
