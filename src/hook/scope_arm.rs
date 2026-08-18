//! The capability-scope ladder (docs/work-scoped-governance.md) — what the
//! guard does when the DECLARED scope table has no entry for the tenant.
//!
//! Rungs, in trust order: `declared` (the operator's static
//! `[yupana.policy.scopes.*]` TOML — evaluated in `pre_edit`, may hard-deny),
//! then `observed` (the paths prior work on the agent's tracked item actually
//! touched, projected from quipu's commit-provenance chain). The observed
//! rung is staged by `[yupana.policy] work_item_scope`: `advise` first —
//! an observed scope grows with use, and an incomplete one hard-denying
//! legitimate work is an outage — then `enforce`, where the boundary denies
//! and the deny still names the right move. When no rung answers, the scope
//! is UNKNOWN: the edit is allowed and the guard says so once per session —
//! never a silent allow that reads as "within scope".
//!
//! A child module of `pre_edit` like `grounded_plane`, but NOT quipu-gated:
//! the unknown-scope advisory is honest (and due) even in a build that can
//! project nothing.

use super::*;

use crate::policy::{Mode, ScopeProvenance};

#[path = "scope_derived.rs"]
mod scope_derived;
use scope_derived::derived_rung;

/// The projected observed-scope plane, extracted from the governed check's
/// registry so the scope arm never adds a projection round-trip of its own.
pub(super) struct ScopePlane {
    /// Work item id → the paths prior work on it touched.
    pub scopes: crate::policy::WorkItemScopes,
    /// Work item id → the item it hangs under, for the DERIVED rung. `None`
    /// when the parent map did not project — the rung then simply does not
    /// fire, which is the pre-rung behaviour and not a new failure mode.
    pub parents: Option<crate::policy::WorkItemParents>,
    /// `Some(age)` when the projection was served from the durable cache —
    /// carried into the advisory so a stale scope names its staleness.
    #[cfg_attr(not(feature = "quipu"), allow(dead_code))]
    pub cache_age: Option<u64>,
}

/// Resolve and apply the ladder below `declared`. Called only when the static
/// scope table has no entry for this tenant (or there is no tenant).
pub(super) fn ladder_fallback(
    config: &YupanaConfig,
    input: &HookInput,
    tenant: Option<&str>,
    rel: &str,
    item: Option<&str>,
    plane: Option<&ScopePlane>,
) -> Decision {
    // The rung's effective level: `work_item_scope`, ceilinged by the ambient
    // mode. Off (the default) keeps the pre-ladder contract — an identified
    // tenant with no declared scope stays silently unconstrained — so a
    // deployment arms both the observed rung AND the unknown-scope advisory
    // deliberately.
    let rung = if config
        .policy
        .work_item_scope
        .is_lower_than(config.policy.mode)
    {
        config.policy.work_item_scope
    } else {
        config.policy.mode
    };
    if rung == Mode::Off {
        return Outcome::Allow.into();
    }

    // OBSERVED rung: the item's touched paths, from the graph. The rung does
    // two jobs at once — it TELLS the agent the right move (the message names
    // the item, its paths, and how to proceed) and, at
    // `[yupana.policy] work_item_scope = "enforce"`, it CONSTRAINS: the
    // out-of-scope edit is denied, not just advised. Stage advise first: an
    // observed scope is what work HAS touched, not all it MAY touch, so a
    // deployment opts into the hard boundary deliberately.
    if let (Some(item), Some(plane)) = (item, plane) {
        // DERIVED, evaluated ONLY when the item has no observed ground of its
        // own. Trust order is declared > derived > observed, but availability
        // runs the other way: observed is a RECORD of what this item touched
        // and is therefore the better answer whenever it exists. Derived is an
        // INFERENCE — "your parent epic's work has all landed here, so this
        // probably belongs here too" — and an inference must not override a
        // record.
        //
        // IT NEVER HARD-DENIES, at any rung setting. `crate::policy::ScopeProvenance`
        // states the rule this honours: a declared scope may hard-deny;
        // derived and observed advise. Observed already departs from that at
        // `enforce`, deliberately, because it is evidence about THIS item.
        // Derived has no such standing: denying an edit because a SIBLING's
        // work happened to land elsewhere would strand an agent on the strength
        // of somebody else's history, and a guard that strands an operator is
        // worse than the thing it prevents.
        if plane.scopes.scope_for(item).is_none() {
            if let Some(decision) = derived_rung(item, rel, tenant, plane) {
                return decision;
            }
        }
        if let Some(scope) = plane.scopes.scope_for(item) {
            let label = tenant.unwrap_or("unidentified");
            let Some(violation) = scope.check_path(rel, label) else {
                return Outcome::Allow.into();
            };
            let denying = rung == Mode::Enforce;
            crate::metrics::emit(
                "scope",
                &[
                    ("provenance", ScopeProvenance::Observed.as_str().into()),
                    ("item", item.into()),
                    ("rule", violation.rule.clone().into()),
                    ("result", if denying { "deny" } else { "advise" }.into()),
                ],
            );
            let staleness = plane.cache_age.map_or_else(String::new, |age| {
                format!(", served from a projection cached {age}s ago")
            });
            let guidance = format!(
                "{} [scope provenance: OBSERVED — the paths prior work on \
                 `{item}` touched{staleness}. If this edit belongs to \
                 `{item}`, keep it within those paths (an operator can widen \
                 the declared scope); if it belongs to different work, \
                 update your tracked item first.]",
                violation.message
            );
            let outcome = if denying {
                Outcome::Deny(guidance)
            } else {
                Outcome::Notify(format!(
                    "{guidance} (advisory: work_item_scope is not \"enforce\")"
                ))
            };
            let response = crate::trace::Response::of(&outcome);
            return Decision::evaluated(
                outcome,
                vec![crate::trace::ConstraintEvaluation::new(
                    format!("scope-observed:{}", violation.rule),
                    crate::trace::Outcome::Unsatisfied,
                    response,
                )],
                // The verdict is only as current as the projection that
                // supplied the scope — a cached plane is stale, and the
                // record must not claim currency nobody checked.
                if plane.cache_age.is_some() {
                    Freshness::Stale
                } else {
                    Freshness::Fresh
                },
            );
        }
    }

    // UNKNOWN scope. An unidentified caller (no tenant) is the ordinary
    // single-operator case and stays silent; an identified tenant with no
    // scope on any rung is told so once per session.
    let Some(tenant) = tenant else {
        return Outcome::Allow.into();
    };
    let why = match (item, plane) {
        (None, _) => "no tracked work item resolved (no plate published, or it is stale)".into(),
        (Some(item), None) => format!(
            "work item `{item}` is tracked, but the observed-scope map is not \
             projected from quipu"
        ),
        (Some(item), Some(_)) => {
            format!("work item `{item}` has no observed paths in the graph yet")
        }
    };
    if first_notice_for_session(input.session_id.as_deref(), "unknown-scope") {
        crate::metrics::emit(
            "scope",
            &[
                ("provenance", "unknown".into()),
                ("result", "notify".into()),
            ],
        );
        let outcome = Outcome::Notify(format!(
            "yupana: no capability scope applies to tenant `{tenant}` — {why}. \
             The edit is allowed (unknown scope advises, never blocks), but \
             treat it as UNGUARDED by scope, not as within scope."
        ));
        let response = crate::trace::Response::of(&outcome);
        // Outcome::Unknown, NOT Unsatisfied: no scope was evaluated, so there
        // is nothing a signed verdict could honestly claim — and an
        // Unsatisfied here lands in the spool as a "denied edit", which the
        // recurrence advisory then mines and re-surfaces against unrelated
        // edits (caught by e2e s12b). The spool skips Unknown by design.
        return Decision::evaluated(
            outcome,
            vec![crate::trace::ConstraintEvaluation::new(
                "unknown-scope",
                crate::trace::Outcome::Unknown,
                response,
            )],
            Freshness::Fresh,
        );
    }
    Outcome::Allow.into()
}

#[cfg(test)]
#[path = "scope_arm_test.rs"]
mod tests;
