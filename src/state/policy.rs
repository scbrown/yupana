//! The game-state policy model (FR-36) — the same governance shape as
//! [`crate::rules::Rule`], widened from code to board.
//!
//! ## What is reused, and why saying so matters
//!
//! `matchType`, `gate`, `effect`, `claim`, `targets` and `label` are the SAME
//! fields, with the same meanings, as the code-policy plane. [`MatchType`] is
//! literally [`crate::rules::MatchType`] — imported, not redeclared — so a
//! `must-match` policy cannot come to mean one thing over an AST and another
//! over a board. Naming the reuse is not bookkeeping: the alternative is two
//! governance models that agree today and diverge quietly, and the second one
//! would be invisible to everyone who audits the first.
//!
//! ## What is new
//!
//! - [`SelectorLang`] — a discriminator, so a policy declares which engine can
//!   evaluate it instead of the engine guessing from context.
//! - [`Boundary::Order`] — a new seam beside the pre-edit `action` one,
//!   evaluated at pre-apply of proposed orders.
//! - [`Tier::EngineState`](crate::types::Tier::EngineState) on everything this
//!   plane produces.
//!
//! ## `sparql` is reserved, and refused rather than approximated
//!
//! Yupana is not an RDF store. A policy declaring `selectorLang "sparql"` is one
//! **Quipu** evaluates; yupana REFUSES it, loudly, via [`errors`] and
//! [`StatePolicy::evaluable`]. It is not skipped and it is not best-effort
//! matched by the [`super::pattern`] engine — a policy silently not evaluated is
//! a policy that reports a clean board it never looked at, which is precisely
//! the disarm-that-reads-as-healthy shape this repo keeps finding.

use serde::{Deserialize, Serialize};

use super::pattern::Pattern;
pub use crate::rules::MatchType;

/// Which engine can evaluate a selector or predicate.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectorLang {
    /// A tree-sitter `.scm` query over an AST — the code plane
    /// ([`crate::rules`]). Not evaluable against a board.
    TreeSitter,
    /// The compact ASK-style pattern over the generic fact graph
    /// ([`super::pattern`]). The only value yupana evaluates on this plane.
    GraphPattern,
    /// Reserved for Quipu. Yupana refuses it; see the module docs.
    Sparql,
}

impl SelectorLang {
    /// The wire/ontology spelling (`aegis:selectorLang`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SelectorLang::TreeSitter => "tree-sitter",
            SelectorLang::GraphPattern => "graph-pattern",
            SelectorLang::Sparql => "sparql",
        }
    }
}

/// Where a policy is evaluated.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Boundary {
    /// Pre-edit, against a proposed buffer — the existing code seam.
    Action,
    /// Pre-apply, against a proposed order set — the FR-36 seam.
    Order,
}

impl Boundary {
    /// The wire spelling (`aegis:boundary`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Boundary::Action => "action",
            Boundary::Order => "order",
        }
    }
}

/// What happens when a policy fires. Reused unchanged from the code plane.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    /// Report it as an advisory; the order set may still be applied.
    Warn,
    /// Report it as a violation.
    Deny,
}

impl Effect {
    /// The wire spelling (`aegis:effect`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Warn => "warn",
            Effect::Deny => "deny",
        }
    }
}

/// A selector: which nodes the policy is about.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Selector {
    /// Which engine evaluates it (`aegis:selectorLang`).
    pub selector_lang: SelectorLang,
    /// The pattern source (`aegis:evidenceSource`).
    pub evidence_source: String,
}

/// A predicate: what must (or must not) hold of the nodes the selector found.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Predicate {
    /// Which engine evaluates it (`aegis:selectorLang`).
    pub selector_lang: SelectorLang,
    /// The predicate direction (`aegis:matchType`) — reused from the code plane.
    pub match_type: MatchType,
    /// The pattern source (`aegis:evidenceSource`).
    pub evidence_source: String,
}

/// One game-state policy.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatePolicy {
    /// Stable identifier (`rdfs:label`).
    pub label: String,
    /// The node kind the policy is about (`aegis:targets`). Descriptive: the
    /// selector is what actually scopes evaluation. Carried because a finding
    /// that names only a pattern is unreadable to the human who has to act on it.
    #[serde(default)]
    pub targets: Option<String>,
    /// The human-readable claim the policy asserts (`aegis:claim`).
    pub claim: String,
    /// Which seam it is evaluated at (`aegis:boundary`).
    pub boundary: Boundary,
    /// What happens when it fires (`aegis:effect`).
    pub effect: Effect,
    /// Which nodes it is about.
    pub selector: Selector,
    /// What must hold of them.
    pub predicate: Predicate,
}

impl StatePolicy {
    /// Whether yupana can evaluate this policy at the ORDER boundary, or the
    /// reason it cannot.
    ///
    /// Returning the reason — rather than a bare `bool` — is what lets the guard
    /// report the policy as UNEVALUATED instead of dropping it. A dropped policy
    /// and a satisfied policy are indistinguishable in a report that only lists
    /// what fired.
    pub fn evaluable(&self) -> Result<(), String> {
        if self.boundary != Boundary::Order {
            return Err(format!(
                "boundary is `{}`; the state guard evaluates `order` policies only",
                self.boundary.as_str()
            ));
        }
        for (part, lang) in [
            ("selector", self.selector.selector_lang),
            ("predicate", self.predicate.selector_lang),
        ] {
            match lang {
                SelectorLang::GraphPattern => {}
                SelectorLang::Sparql => {
                    return Err(format!(
                        "{part} declares selectorLang `sparql`, which is RESERVED for Quipu — \
                         yupana is not an RDF store and refuses rather than approximating it. \
                         Project the datalinks it needs into the board and restate the {part} \
                         as `graph-pattern`, or evaluate this policy in Quipu."
                    ))
                }
                SelectorLang::TreeSitter => {
                    return Err(format!(
                        "{part} declares selectorLang `tree-sitter`, which is the CODE plane \
                         (a .scm query over an AST). There is no AST here."
                    ))
                }
            }
        }
        Ok(())
    }

    /// Parse this policy's selector and predicate patterns.
    pub fn compile(&self) -> Result<(Pattern, Pattern), String> {
        self.evaluable()?;
        let selector =
            Pattern::parse(&self.selector.evidence_source).map_err(|e| format!("selector: {e}"))?;
        let predicate = Pattern::parse(&self.predicate.evidence_source)
            .map_err(|e| format!("predicate: {e}"))?;
        Ok((selector, predicate))
    }
}

/// Every policy that cannot be evaluated, as `(label, reason)`.
///
/// The [`crate::rules::errors`] discipline, applied to this plane: a policy set
/// with an entry here is misconfigured, and the guard says so rather than
/// quietly under-enforcing. Note what is NOT filtered out — a `sparql` policy
/// appears here rather than being skipped as "not ours".
#[must_use]
pub fn errors(policies: &[StatePolicy]) -> Vec<(String, String)> {
    policies
        .iter()
        .filter_map(|p| p.compile().err().map(|e| (p.label.clone(), e)))
        .collect()
}

#[cfg(test)]
#[path = "policy_test.rs"]
mod policy_test;
