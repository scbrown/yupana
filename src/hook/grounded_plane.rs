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
/// The projection source is retained through rendering because both cache arms
/// carry an age but mean opposite things: [`ProjectionSource::FreshCache`]
/// deliberately avoids a network request and is fresh, while
/// [`ProjectionSource::Cache`] means a live refresh failed and is stale.
pub(super) fn rule_verdict_message_from(
    body: &str,
    freshness: Freshness,
    source: &crate::project::ProjectionSource,
    cache_ttl_secs: u64,
) -> String {
    let note = match source {
        crate::project::ProjectionSource::FreshCache { age_secs } => format!(
            "verdict freshness: fresh (governed policy served from a valid cache, \
             {age_secs}s old, within the {cache_ttl_secs}s TTL — no live policy \
             projection was attempted)"
        ),
        crate::project::ProjectionSource::Cache { age_secs, .. } => format!(
            "verdict freshness: STALE — quipu could not be projected, so this \
             verdict was computed against the last-known governed policy, \
             cached {age_secs}s ago. The rules were ENFORCED; what is unconfirmed is \
             whether they are still the current ones"
        ),
        crate::project::ProjectionSource::Live => match freshness {
            Freshness::Fresh => {
                "verdict freshness: fresh (governed policy projected from quipu)".to_string()
            }
            Freshness::Stale => "verdict freshness: STALE — the projected policy registry could \
             not be refreshed from quipu, so this verdict may not reflect the latest governed policy"
                .to_string(),
            Freshness::Recomputing => {
                "verdict freshness: recomputing — the policy registry is mid-refresh".to_string()
            }
        },
    };
    format!("{body}\n({note})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectionSource;

    #[test]
    fn a_valid_cache_is_fresh_and_does_not_claim_projection_failed() {
        let message = rule_verdict_message_from(
            "rule fired",
            Freshness::Fresh,
            &ProjectionSource::FreshCache { age_secs: 588 },
            3600,
        );

        assert!(message.contains("verdict freshness: fresh"), "{message}");
        assert!(message.contains("588s old"), "{message}");
        assert!(message.contains("within the 3600s TTL"), "{message}");
        assert!(
            message.contains("no live policy projection was attempted"),
            "{message}"
        );
        assert!(!message.contains("could not be projected"), "{message}");
    }

    #[test]
    fn a_failed_refresh_cache_remains_stale() {
        let message = rule_verdict_message_from(
            "rule fired",
            Freshness::Stale,
            &ProjectionSource::Cache {
                age_secs: 588,
                error: "timeout".to_string(),
            },
            3600,
        );

        assert!(message.contains("verdict freshness: STALE"), "{message}");
        assert!(
            message.contains("quipu could not be projected"),
            "{message}"
        );
        assert!(message.contains("cached 588s ago"), "{message}");
    }
}
