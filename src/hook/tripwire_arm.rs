//! The tripwire arm of the pre-edit guard — local Binding/Gate enforcement.
//!
//! Evaluates `[[yupana.policy.tripwires]]` (see [`crate::tripwire`]) against the
//! edit and turns any trips into one guard decision. Runs BEFORE the plain rule
//! plane: a wire is the more specific binding, and its declared effect must not
//! be preempted by the ambient-mode outcome the rule plane would produce for
//! the same violation.
//!
//! Effects land here as follows, with the ambient [`Mode`] a ceiling throughout:
//!
//! - `deny` → [`decide`]: a Deny under `Enforce`, a Notify under `Advise`.
//! - `warn` → Notify, whatever the mode (above `Off`).
//! - `throttle` → the backoff is RECORDED via [`crate::throttle`] and the edit
//!   is allowed with a Notify. Recording happens even when a sibling `deny`
//!   wire blocks the same edit — the attempt crossed the boundary, and the
//!   crossing is recorded either way.
//!
//! A misconfigured wire set (bad glob, unknown rule reference, a throttle with
//! no declared backoff) is a LOUD fail-open, the [`crate::rules::errors`]
//! discipline: an inert wire that reads as armed is the defect class the
//! tripwire concept exists to close.

// A child module of `pre_edit`, like `rule_planes` and `verify_arm`: it reads
// that module's private `Decision`, `decide` and `introduced_text` through
// `use super::*`.
use super::*;

use crate::constraint::VerificationPoint;
use crate::tripwire::{Trip, TripEffect};

/// Evaluate the tripwires. `None` means no wire tripped (or none is configured)
/// and the guard's remaining checks proceed; `Some` is the one decision this
/// edit gets from the wires.
pub(super) fn tripwire_check(
    config: &YupanaConfig,
    input: &HookInput,
    root: &Path,
    rel: &str,
) -> Option<Decision> {
    // Mode::Off disarms the whole guard, wires included.
    if config.policy.mode == Mode::Off {
        return None;
    }
    let wires = &config.policy.tripwires;
    if wires.is_empty() {
        return None;
    }

    // A wire set that cannot do its job fails open LOUDLY rather than quietly
    // under-enforcing (the malformed-glob discipline, for wires).
    let errors = crate::tripwire::errors(wires, &config.policy.rules);
    if !errors.is_empty() {
        let detail: Vec<String> = errors
            .iter()
            .map(|(name, why)| format!("`{name}` ({why})"))
            .collect();
        return Some(
            fail_open(
                input,
                "tripwires",
                &format!("policy tripwires are misconfigured: {}", detail.join(", ")),
            )
            .into(),
        );
    }

    // Rule-conditioned wires need the introduced text and a grammar; path-only
    // wires need neither. Both facts are resolved once, optionally.
    let introduced = introduced_text(input);
    let language = Path::new(rel)
        .extension()
        .and_then(OsStr::to_str)
        .and_then(language_for_extension);

    let trips = crate::tripwire::evaluate(
        wires,
        &config.policy.rules,
        rel,
        introduced.as_deref(),
        language,
    );
    if trips.is_empty() {
        return None;
    }

    // Throttles are recorded FIRST, unconditionally: whatever the combined
    // outcome, the boundary was crossed and subsequent edits surface it.
    let now = crate::throttle::now_secs();
    let scope = root.display().to_string();
    for trip in trips.iter().filter(|t| t.effect == TripEffect::Throttle) {
        if let Some(secs) = trip.backoff_secs {
            crate::throttle::record(&trip.name, &scope, secs, now);
        }
    }

    let combined = trips
        .iter()
        .map(|t| t.message.clone())
        .collect::<Vec<_>>()
        .join("\n");
    // One decision per edit: a deny-effect trip decides it (mode-ceilinged);
    // warn/throttle trips ride along in the same message either way.
    let outcome = if trips.iter().any(|t| t.effect == TripEffect::Deny) {
        decide(config.policy.mode, combined)
    } else {
        Outcome::Notify(combined)
    };
    let response = crate::trace::Response::of(&outcome);
    Some(Decision::evaluated(
        outcome,
        evaluations(&trips, response),
        // Local config is authoritative, and the evidence is the exact proposed
        // edit — genuinely fresh, the rule plane's own reasoning.
        Freshness::Fresh,
    ))
}

/// The GOVERNED tripwire plane — quipu's path-boundary policies, evaluated
/// inside `governed_check`'s flow against the registry it already refreshed
/// (one refresh per edit; the ladder's own rule). Returns the violations for
/// the governed accumulator; the throttle orders the pure evaluation produced
/// are RECORDED here, because this arm is the seam that owns that I/O — same
/// split as the local wires above.
#[cfg(feature = "quipu")]
pub(super) fn governed_plane(
    registry: &crate::project::ProjectionRegistry,
    config: &YupanaConfig,
    root: &Path,
    rel: &str,
) -> Vec<crate::project::ProjectedViolation> {
    let (violations, throttles) =
        crate::project_tripwire::gate_violations(&registry.tripwires, rel, config.policy.mode);
    let now = crate::throttle::now_secs();
    let scope = root.display().to_string();
    for order in &throttles {
        crate::throttle::record(&order.name, &scope, order.secs, now);
    }
    violations
}

/// One [`ConstraintEvaluation`](crate::trace::ConstraintEvaluation) per tripped
/// wire, in stable order — the `evaluations_for` shape, with the wire's fixed
/// placement: a tripwire is a local binding with no declared class, evaluated
/// at the gate.
fn evaluations(
    trips: &[Trip],
    response: crate::trace::Response,
) -> Vec<crate::trace::ConstraintEvaluation> {
    let mut names: Vec<&str> = trips.iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            crate::trace::ConstraintEvaluation::new(
                name,
                crate::trace::Outcome::Unsatisfied,
                response,
            )
            .placed(None, Some(VerificationPoint::Pag))
            .hosted_at(crate::hosting::YUPANA_HOSTS_AT)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tripwire::Tripwire;

    fn config(mode: Mode, wires: Vec<Tripwire>) -> YupanaConfig {
        let mut config = YupanaConfig::default();
        config.policy.mode = mode;
        config.policy.tripwires = wires;
        config
    }

    fn edit(path: &str, new_text: &str) -> HookInput {
        HookInput::parse(
            &serde_json::json!({
                "session_id": "trip-test",
                "tool_name": "Edit",
                "tool_input": { "file_path": path, "old_string": "x", "new_string": new_text },
            })
            .to_string(),
        )
        .unwrap()
    }

    fn deny_wire() -> Tripwire {
        Tripwire {
            name: "auth-boundary".to_string(),
            paths: vec!["src/auth/**".to_string()],
            rule: None,
            effect: TripEffect::Deny,
            backoff_secs: None,
            message: None,
        }
    }

    #[test]
    fn a_deny_wire_blocks_under_enforce_and_advises_under_advise() {
        let input = edit("/repo/src/auth/login.rs", "fn f() {}");
        let root = Path::new("/repo");

        let enforced = tripwire_check(
            &config(Mode::Enforce, vec![deny_wire()]),
            &input,
            root,
            "src/auth/login.rs",
        )
        .unwrap();
        assert!(matches!(enforced.outcome, Outcome::Deny(_)));

        let advised = tripwire_check(
            &config(Mode::Advise, vec![deny_wire()]),
            &input,
            root,
            "src/auth/login.rs",
        )
        .unwrap();
        match advised.outcome {
            Outcome::Notify(msg) => assert!(msg.contains("auth-boundary")),
            other => panic!("expected Notify, got {other:?}"),
        }
    }

    #[test]
    fn off_mode_and_a_clean_path_both_produce_no_opinion() {
        let input = edit("/repo/src/auth/login.rs", "fn f() {}");
        let root = Path::new("/repo");
        assert!(tripwire_check(
            &config(Mode::Off, vec![deny_wire()]),
            &input,
            root,
            "src/auth/login.rs"
        )
        .is_none());
        assert!(tripwire_check(
            &config(Mode::Enforce, vec![deny_wire()]),
            &input,
            root,
            "src/lib.rs"
        )
        .is_none());
    }

    #[test]
    fn a_misconfigured_wire_set_fails_open_loudly_not_silently() {
        let broken = Tripwire {
            name: "broken".to_string(),
            paths: Vec::new(),
            rule: None, // no boundary at all
            effect: TripEffect::Warn,
            backoff_secs: None,
            message: None,
        };
        let input = edit("/repo/src/lib.rs", "fn f() {}");
        let decision = tripwire_check(
            &config(Mode::Enforce, vec![broken]),
            &input,
            Path::new("/repo"),
            "src/lib.rs",
        )
        .unwrap();
        // Fail-open: never a Deny, and configuration errors stay loud.
        match decision.outcome {
            Outcome::Notify(msg) => assert!(msg.contains("failed open")),
            Outcome::Allow => panic!("a configuration error must not be deduplicated"),
            Outcome::Deny(msg) => panic!("misconfiguration must not deny: {msg}"),
        }
    }

    #[test]
    fn a_warn_wire_notifies_and_the_decision_records_the_wire() {
        let warn = Tripwire {
            name: "vendor-touch".to_string(),
            paths: vec!["vendor/**".to_string()],
            rule: None,
            effect: TripEffect::Warn,
            backoff_secs: None,
            message: None,
        };
        let input = edit("/repo/vendor/lib.c", "int x;");
        let decision = tripwire_check(
            &config(Mode::Enforce, vec![warn]),
            &input,
            Path::new("/repo"),
            "vendor/lib.c",
        )
        .unwrap();
        assert!(matches!(decision.outcome, Outcome::Notify(_)));
        assert_eq!(decision.constraints.len(), 1);
        assert_eq!(decision.constraints[0].id, "vendor-touch");
        assert_eq!(
            decision.constraints[0].verification_point,
            Some(VerificationPoint::Pag)
        );
    }

    #[test]
    fn a_deny_trip_decides_the_edit_even_alongside_a_warn_trip() {
        let warn = Tripwire {
            name: "wide-warn".to_string(),
            paths: vec!["src/**".to_string()],
            rule: None,
            effect: TripEffect::Warn,
            backoff_secs: None,
            message: None,
        };
        let input = edit("/repo/src/auth/login.rs", "fn f() {}");
        let decision = tripwire_check(
            &config(Mode::Enforce, vec![warn, deny_wire()]),
            &input,
            Path::new("/repo"),
            "src/auth/login.rs",
        )
        .unwrap();
        // Both wires are in the record; the deny decides the outcome, and its
        // message carries both trips.
        assert_eq!(decision.constraints.len(), 2);
        match decision.outcome {
            Outcome::Deny(msg) => {
                assert!(msg.contains("wide-warn"));
                assert!(msg.contains("auth-boundary"));
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }
}
