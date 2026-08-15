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

/// The projected observed-scope plane, extracted from the governed check's
/// registry so the scope arm never adds a projection round-trip of its own.
pub(super) struct ScopePlane {
    /// Work item id → the paths prior work on it touched.
    pub scopes: crate::policy::WorkItemScopes,
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
// Test names shout the invariant they turn on, the repo's emphasis convention.
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::policy::WorkItemScopes;

    fn config(mode: Mode, rung: Mode) -> YupanaConfig {
        let mut config = YupanaConfig::default();
        config.policy.mode = mode;
        config.policy.work_item_scope = rung;
        config
    }

    fn input(session: &str) -> HookInput {
        HookInput::parse(
            &serde_json::json!({
                "session_id": session,
                "tool_name": "Edit",
                "tool_input": { "file_path": "/r/src/a.rs" },
            })
            .to_string(),
        )
        .unwrap()
    }

    fn plane(rows: &[(&str, &str)]) -> ScopePlane {
        ScopePlane {
            scopes: WorkItemScopes::from_rows(
                rows.iter()
                    .map(|(a, b)| ((*a).to_string(), (*b).to_string())),
            ),
            cache_age: None,
        }
    }

    #[test]
    fn the_rung_is_OFF_by_default_preserving_the_pre_ladder_contract() {
        let d = ladder_fallback(
            &YupanaConfig::default(),
            &input("s-default-off"),
            Some("polecat"),
            "src/other.rs",
            Some("aegis-1"),
            Some(&plane(&[("aegis-1", "src/a.rs")])),
        );
        assert_eq!(d.outcome, Outcome::Allow);
    }

    #[test]
    fn observed_scope_allows_a_touched_path_silently() {
        let d = ladder_fallback(
            &config(Mode::Enforce, Mode::Advise),
            &input("s-obs-in"),
            Some("polecat"),
            "src/a.rs",
            Some("aegis-1"),
            Some(&plane(&[("aegis-1", "src/a.rs")])),
        );
        assert_eq!(d.outcome, Outcome::Allow);
    }

    #[test]
    fn observed_scope_ADVISES_out_of_scope_at_the_advise_rung() {
        let d = ladder_fallback(
            &config(Mode::Enforce, Mode::Advise),
            &input("s-obs-out"),
            Some("polecat"),
            "src/other.rs",
            Some("aegis-1"),
            Some(&plane(&[("aegis-1", "src/a.rs")])),
        );
        let Outcome::Notify(message) = &d.outcome else {
            panic!(
                "observed scope must notify by default, not deny: {:?}",
                d.outcome
            );
        };
        assert!(message.contains("OBSERVED"), "names its rung: {message}");
        assert!(message.contains("aegis-1"), "names the item: {message}");
        assert_eq!(d.constraints[0].id, "scope-observed:allow_paths");
    }

    #[test]
    fn work_item_scope_enforce_DENIES_out_of_scope() {
        let d = ladder_fallback(
            &config(Mode::Enforce, Mode::Enforce),
            &input("s-obs-deny"),
            Some("polecat"),
            "src/other.rs",
            Some("aegis-1"),
            Some(&plane(&[("aegis-1", "src/a.rs")])),
        );
        let Outcome::Deny(message) = &d.outcome else {
            panic!("work_item_scope=enforce must deny: {:?}", d.outcome);
        };
        // The constraint still TEACHES: the deny names the item, its paths,
        // and both legitimate ways forward.
        assert!(message.contains("aegis-1"), "names the item: {message}");
        assert!(
            message.contains("update your tracked item"),
            "guides: {message}"
        );
    }

    #[test]
    fn ambient_advise_stays_a_ceiling_over_enforce() {
        let d = ladder_fallback(
            &config(Mode::Advise, Mode::Enforce),
            &input("s-obs-ceiling"),
            Some("polecat"),
            "src/other.rs",
            Some("aegis-1"),
            Some(&plane(&[("aegis-1", "src/a.rs")])),
        );
        assert!(
            matches!(d.outcome, Outcome::Notify(_)),
            "advise-mode deployments never deny: {:?}",
            d.outcome
        );
    }

    #[test]
    fn unknown_scope_NOTIFIES_an_identified_tenant_once_per_session() {
        let session = format!("scope-arm-test-{}", std::process::id());
        let d = ladder_fallback(
            &config(Mode::Enforce, Mode::Advise),
            &input(&session),
            Some("polecat"),
            "src/a.rs",
            None,
            None,
        );
        let Outcome::Notify(message) = &d.outcome else {
            panic!(
                "unknown scope must notify, not allow silently: {:?}",
                d.outcome
            );
        };
        assert!(message.contains("UNGUARDED"), "says unguarded: {message}");
        // Second edit in the same session: the notice already fired.
        let d2 = ladder_fallback(
            &config(Mode::Enforce, Mode::Advise),
            &input(&session),
            Some("polecat"),
            "src/a.rs",
            None,
            None,
        );
        assert_eq!(d2.outcome, Outcome::Allow);
    }

    #[test]
    fn an_unidentified_caller_stays_silent() {
        let d = ladder_fallback(
            &config(Mode::Enforce, Mode::Advise),
            &input("s-anon"),
            None,
            "src/a.rs",
            None,
            None,
        );
        assert_eq!(d.outcome, Outcome::Allow);
    }

    #[test]
    fn mode_off_is_inert() {
        let d = ladder_fallback(
            &config(Mode::Off, Mode::Enforce),
            &input("s-off"),
            Some("polecat"),
            "src/a.rs",
            Some("aegis-1"),
            Some(&plane(&[("aegis-1", "src/a.rs")])),
        );
        assert_eq!(d.outcome, Outcome::Allow);
    }
}
