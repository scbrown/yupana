//! The DERIVED rung of the capability ladder — an item with no ground of its
//! own, scoped to its parent's.
//!
//! Child module of [`super`], split out under the file-size ratchet
//! (yupana #83). The seam is the natural one: `scope_arm` owns the LADDER —
//! which rung answers, in what order, and what happens when none does — and
//! this owns the one rung whose answer is an inference rather than a record.

use super::{Decision, Freshness, Outcome, ScopePlane};
use crate::policy::ScopeProvenance;

/// The DERIVED rung: an item with no observed ground of its own is scoped to
/// its parent's.
///
/// Returns `None` — falling through to the unknown-scope notice — ONLY when it
/// cannot answer at all: no parent map projected, no parent recorded for this
/// item, or a parent that has no observed ground either. Each of those is
/// UNKNOWN, and an inference that cannot be drawn must not be reported as a
/// scope.
///
/// When it CAN answer and the edit is inside the parent's ground it returns
/// `Allow`, not `None`. The distinction is the whole contract: `None` means
/// "no rung applies", which the caller renders as "treat this as UNGUARDED by
/// scope". Saying that about an edit a rung just cleared would be false, and
/// falsely alarming in the direction that makes advisories get ignored.
pub(super) fn derived_rung(
    item: &str,
    rel: &str,
    tenant: Option<&str>,
    plane: &ScopePlane,
) -> Option<Decision> {
    let parent = plane.parents.as_ref()?.parent_of(item)?;
    let scope = plane.scopes.scope_for(parent)?;
    let label = tenant.unwrap_or("unidentified");
    // The rung APPLIES from here on, so every path below returns `Some`.
    let Some(violation) = scope.check_path(rel, label) else {
        return Some(Outcome::Allow.into());
    };

    crate::metrics::emit(
        "scope",
        &[
            ("provenance", ScopeProvenance::Derived.as_str().into()),
            ("item", item.into()),
            ("parent", parent.into()),
            ("rule", violation.rule.clone().into()),
            // Always "advise": this rung has no deny to record, so a `result`
            // field that could read "deny" would misdescribe the rung in the
            // very records a later replay derives rules from.
            ("result", "advise".into()),
        ],
    );

    let staleness = plane.cache_age.map_or_else(String::new, |age| {
        format!(", served from a projection cached {age}s ago")
    });
    let outcome = Outcome::Notify(format!(
        "{} [scope provenance: DERIVED — `{item}` has no observed paths of its \
         own yet, so this is its parent `{parent}`'s ground{staleness}. That is \
         an INFERENCE about where this work belongs, not a record of it, so it \
         advises and never denies: if this edit is right, carry on and your \
         own ground starts with the commit that lands it.]",
        violation.message
    ));
    let response = crate::trace::Response::of(&outcome);
    Some(Decision::evaluated(
        outcome,
        vec![crate::trace::ConstraintEvaluation::new(
            format!("scope-derived:{}", violation.rule),
            crate::trace::Outcome::Unsatisfied,
            response,
        )],
        if plane.cache_age.is_some() {
            Freshness::Stale
        } else {
            Freshness::Fresh
        },
    ))
}
