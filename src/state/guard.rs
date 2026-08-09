//! `yupana_guard` — the `(game_state + proposed_orders)` analog of `yupana_verify`
//! (FR-37).
//!
//! Apply the proposed orders to a copy-on-write overlay, run each policy's
//! selector over the POST-order board, evaluate the gated predicate on the same
//! board, and route `deny` to violations and `warn` to advisories.
//!
//! ## Three things this reports that a bare violation list cannot
//!
//! 1. **[`GuardOutcome::Refused`] for an empty board.** A tenant with no board
//!    gets a REFUSAL, never an empty violation list. This is the whole reason
//!    the type is an enum: ingest and guard are separate calls, possibly to
//!    separate processes, and "nothing was ingested here" and "everything is
//!    fine" are otherwise the same JSON. A guard that answers `ok` over a board
//!    it does not have is a green light over a dead backend.
//! 2. **[`GuardReport::unevaluated`].** A policy that could not be compiled or
//!    is not yupana's to evaluate (`sparql`) is listed, not dropped. A dropped
//!    policy is indistinguishable from a satisfied one.
//! 3. **[`GuardReport::vacuous`].** A policy whose SELECTOR matched nothing
//!    neither passed nor failed — it was never asked. A selector that has rotted
//!    away from the adapter's vocabulary matches zero nodes and produces zero
//!    violations, which reads exactly like a clean board.
//!
//! ## `pre_existing`
//!
//! Every finding is also evaluated against the PRE-order board. A violation that
//! was already true before the orders is marked [`Finding::pre_existing`] and
//! blames no order ids: denying a move for a condition it did not cause is a
//! false deny, and a false deny removes a legal, possibly correct move — the
//! conservatism FR-37 asks for, made mechanical.

use std::collections::BTreeSet;

use serde::Serialize;

use super::graph::StateGraph;
use super::orders::{speculate, Applied, Order};
use super::overlay::{StateOverlay, StateView};
use super::pattern::{Binding, Pattern};
use super::policy::{Effect, MatchType, StatePolicy};
use crate::types::Tier;

/// One policy finding.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    /// The policy's label.
    pub policy: String,
    /// Always `engine-state` (FR-3): this is an adapter-stated board, not a
    /// derived code fact.
    pub tier: String,
    /// The policy's human-readable claim.
    pub claim: String,
    /// `deny` or `warn`.
    pub effect: String,
    /// The orders blamed for it — empty when [`Self::pre_existing`].
    pub offending_order_ids: Vec<String>,
    /// Which board entities the finding is about, and what they held.
    pub detail: String,
    /// Whether the condition already held BEFORE the proposed orders.
    pub pre_existing: bool,
}

/// A policy that was not evaluated, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PolicyNote {
    /// The policy's label.
    pub policy: String,
    /// Why it was not evaluated, or what it did not reach.
    pub reason: String,
}

/// What a guard run found.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GuardReport {
    /// `deny` findings.
    pub violations: Vec<Finding>,
    /// `warn` findings.
    pub advisories: Vec<Finding>,
    /// Policies that could not be evaluated at all.
    pub unevaluated: Vec<PolicyNote>,
    /// Policies whose selector matched nothing — never asked, not passed.
    pub vacuous: Vec<PolicyNote>,
    /// Declared effects that referenced something absent from the board.
    pub unapplied: Vec<String>,
    /// Comparisons with no defined answer, from the pattern engine.
    pub warnings: Vec<String>,
    /// How many orders were applied.
    pub orders: usize,
    /// Post-order board size, as `(nodes, edges)`.
    pub board: (usize, usize),
    /// The tier of everything above.
    pub tier: String,
}

impl GuardReport {
    /// Whether the order set is free of `deny` findings.
    ///
    /// Deliberately NOT a field on the wire, and deliberately not the whole
    /// answer: `unevaluated` and `vacuous` can both be non-empty while this is
    /// true, and a caller that reads only this learns nothing about the policies
    /// that were never asked.
    #[must_use]
    pub fn allowed(&self) -> bool {
        self.violations.is_empty()
    }
}

/// The result of a guard call.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GuardOutcome {
    /// The board could not be guarded. See the module docs — this is never
    /// collapsed into an empty report.
    Refused {
        /// Why.
        reason: String,
    },
    /// A completed evaluation.
    Evaluated(GuardReport),
}

/// Run the FR-37 guard.
///
/// `base` + `overlay` are the tenant's board; the orders are applied to a clone
/// of the overlay, so nothing here commits.
#[must_use]
pub fn guard(
    base: &StateGraph,
    overlay: &StateOverlay,
    policies: &[StatePolicy],
    orders: &[Order],
) -> GuardOutcome {
    let pre = StateView::new(base, Some(overlay));
    if pre.is_empty() {
        return GuardOutcome::Refused {
            reason: "no board is loaded for this tenant — ingest game state before guarding. \
                     Refusing rather than reporting zero violations over an empty board, which \
                     would read as `these orders are fine`."
                .to_string(),
        };
    }
    if policies.is_empty() {
        return GuardOutcome::Refused {
            reason: "no policies were supplied — a guard with nothing to check cannot clear an \
                     order set. Supply `policies`, or do not gate on this call."
                .to_string(),
        };
    }

    let (speculative, applied) = speculate(base, overlay, orders);
    let post = StateView::new(base, Some(&speculative));

    let mut report = GuardReport {
        violations: Vec::new(),
        advisories: Vec::new(),
        unevaluated: Vec::new(),
        vacuous: Vec::new(),
        unapplied: applied.unapplied.clone(),
        warnings: Vec::new(),
        orders: orders.len(),
        board: post.stats(),
        tier: Tier::EngineState.as_str().to_string(),
    };

    for policy in policies {
        evaluate_policy(policy, &pre, &post, orders, &applied, &mut report);
    }
    report.warnings.sort();
    report.warnings.dedup();
    GuardOutcome::Evaluated(report)
}

/// Evaluate one policy, filing its results into `report`.
fn evaluate_policy(
    policy: &StatePolicy,
    pre: &StateView<'_>,
    post: &StateView<'_>,
    orders: &[Order],
    applied: &Applied,
    report: &mut GuardReport,
) {
    let (selector, predicate) = match policy.compile() {
        Ok(pair) => pair,
        Err(reason) => {
            report.unevaluated.push(PolicyNote {
                policy: policy.label.clone(),
                reason,
            });
            return;
        }
    };

    let post_hits = breaches(&selector, &predicate, post, policy.predicate.match_type);
    report.warnings.extend(post_hits.warnings);
    if post_hits.selected == 0 {
        report.vacuous.push(PolicyNote {
            policy: policy.label.clone(),
            reason: format!(
                "selector `{}` matched no node on the post-order board — the policy was never \
                 asked, which is not the same as satisfied",
                policy.selector.evidence_source
            ),
        });
        return;
    }
    if post_hits.breaches.is_empty() {
        return;
    }

    let pre_hits = breaches(&selector, &predicate, pre, policy.predicate.match_type);
    for breach in post_hits.breaches {
        let pre_existing = pre_hits
            .breaches
            .iter()
            .any(|b| b.binding == breach.binding);
        let offending = if pre_existing {
            Vec::new()
        } else {
            blame(&breach.binding, orders, applied)
        };
        let finding = Finding {
            policy: policy.label.clone(),
            tier: Tier::EngineState.as_str().to_string(),
            claim: policy.claim.clone(),
            effect: policy.effect.as_str().to_string(),
            offending_order_ids: offending,
            detail: breach.detail,
            pre_existing,
        };
        match policy.effect {
            Effect::Deny => report.violations.push(finding),
            Effect::Warn => report.advisories.push(finding),
        }
    }
}

/// One breach of a policy's predicate.
struct Breach {
    binding: Binding,
    detail: String,
}

/// Selector/predicate evaluation over one board.
struct Breaches {
    selected: usize,
    breaches: Vec<Breach>,
    warnings: Vec<String>,
}

/// Run `selector`, then the gated `predicate` per selection, applying the
/// [`MatchType`] direction — the same three directions as the code plane.
fn breaches(
    selector: &Pattern,
    predicate: &Pattern,
    view: &StateView<'_>,
    match_type: MatchType,
) -> Breaches {
    let selected = selector.eval(view);
    let mut warnings = selected.warnings.clone();
    let mut breaches = Vec::new();
    let mut any_satisfied = false;

    for binding in &selected.bindings {
        let mut hit = predicate.eval_seeded(view, binding);
        warnings.append(&mut hit.warnings);
        let satisfied = match match_type {
            MatchType::MustMatch | MatchType::MustExist => !hit.is_empty(),
            MatchType::MustNotMatch => hit.is_empty(),
        };
        if satisfied {
            any_satisfied = true;
        }
        // `MustExist` is a file-level (here: board-level) question in the code
        // plane too — one violation for the whole set, not one per node — so it
        // is decided after the loop.
        if !satisfied && match_type != MatchType::MustExist {
            breaches.push(Breach {
                detail: render(binding),
                binding: binding.clone(),
            });
        }
    }

    if match_type == MatchType::MustExist && !any_satisfied && !selected.bindings.is_empty() {
        breaches.push(Breach {
            detail: format!(
                "no selected entity satisfies the predicate ({} selected)",
                selected.bindings.len()
            ),
            binding: Binding::new(),
        });
    }

    Breaches {
        selected: selected.bindings.len(),
        breaches,
        warnings,
    }
}

/// Which orders touched any node in this binding.
///
/// Attribution runs off DECLARED effects ([`super::orders`]), so it names the
/// orders that actually stated a change to an involved entity rather than
/// inferring blame from proximity. An order set that touched nothing in the
/// binding yields an empty list, and an empty list on a non-`pre_existing`
/// finding is itself informative: the condition changed, but no order claims to
/// have changed it.
fn blame(binding: &Binding, orders: &[Order], applied: &Applied) -> Vec<String> {
    let involved: BTreeSet<&str> = binding
        .values()
        .filter_map(super::pattern::Bound::node_id)
        .collect();
    let mut out: Vec<String> = orders
        .iter()
        .filter(|o| {
            o.touched()
                .iter()
                .any(|id| involved.contains(id.as_str()) && applied.touched.contains(id))
        })
        .map(|o| o.id.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A binding rendered for a human: `?b=base_alpha, ?n=0`.
fn render(binding: &Binding) -> String {
    binding
        .iter()
        .map(|(var, bound)| format!("?{var}={}", bound.render()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "guard_test.rs"]
mod guard_test;
