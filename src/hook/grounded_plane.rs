//! The guard's GROUNDED rule plane (bobbin-tvn) — entity membership against
//! the projected work-item id set, three outcomes per rule, no regex anywhere.
//!
//! A child module of `pre_edit` like `rule_planes`, and quipu-gated with it:
//! the plane evaluates rules and a set that only the projection can supply.
//! The evaluation core is pure and lives in [`crate::grounding`]; this file is
//! the seam that turns its typed outcomes into guard messages — including the
//! LOUD unevaluated notice when the grounding set is missing, which must never
//! read as "nothing grounds, allow".

use super::*;

/// Evaluate the grounded plane. Returns `(messages, any_blocking, rule_names)`
/// for the caller to merge into the one-verdict-per-edit flow.
pub(super) fn grounded_check(
    registry: &crate::project::ProjectionRegistry,
    introduced: &str,
) -> (Vec<String>, bool, Vec<String>) {
    let rules = registry.grounded_rules();
    if rules.is_empty() {
        return (Vec::new(), false, Vec::new());
    }
    let (violations, unevaluated) =
        crate::grounding::evaluate(rules, registry.grounding(), introduced);
    let mut messages: Vec<String> = Vec::new();
    let mut any_blocking = false;
    let mut names: Vec<String> = Vec::new();
    for v in &violations {
        if v.tier == crate::textrules::TextTier::Block {
            any_blocking = true;
        }
        messages.push(v.message.clone());
        names.push(v.rule.clone());
    }
    if !unevaluated.is_empty() {
        // The typed-non-answer discipline: a rule whose grounding set is not
        // projected is UNEVALUATED, by name, and the edit is allowed — but the
        // gap is visible, never a silent empty-set pass (bobbin-tvn).
        messages.push(format!(
            "[grounding: rule(s) {} NOT EVALUATED — the work-item grounding set \
             is not projected into the hot plane, so membership could not be \
             checked. The edit is allowed, but these rules did not apply to it; \
             treat it as unguarded by them, not as within them.]",
            unevaluated
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    (messages, any_blocking, names)
}

/// Attach the FR-3 freshness declaration to an already-joined verdict body.
///
/// `cache_age_secs` is `Some` when the policies came from the durable cache
/// rather than a live projection, and the AGE is stated rather than merely
/// implied by "STALE". A reader who is told a verdict is stale can only guess
/// whether that means seconds or a week, and those are opposite decisions:
/// seconds-old is a slow quipu and the verdict stands, week-old means a retired
/// rule may still be firing. Naming the number is what makes this cache
/// falsifiable from the outside (aegis-0upyu).
pub(super) fn rule_verdict_message_from(
    body: &str,
    freshness: Freshness,
    cache_age_secs: Option<u64>,
) -> String {
    let note = match (freshness, cache_age_secs) {
        (_, Some(age)) => format!(
            "verdict freshness: STALE — quipu could not be projected, so this \
             verdict was computed against the last-known governed policy, \
             cached {age}s ago. The rules were ENFORCED; what is unconfirmed is \
             whether they are still the current ones"
        ),
        (Freshness::Fresh, None) => {
            "verdict freshness: fresh (governed policy projected from quipu)".to_string()
        }
        (Freshness::Stale, None) => "verdict freshness: STALE — the projected policy registry could \
             not be refreshed from quipu, so this verdict may not reflect the latest governed policy"
            .to_string(),
        (Freshness::Recomputing, None) => {
            "verdict freshness: recomputing — the policy registry is mid-refresh".to_string()
        }
    };
    format!("{body}\n({note})")
}
