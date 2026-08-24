//! Governed tripwires — quipu's path-boundary policies, projected.
//!
//! A governed **tripwire** is an `aegis:Policy` at `boundary:"action"` carrying
//! `aegis:appliesTo` globs and *no* Selector or Predicate: touching the path is
//! the crossing (quipu `shapes/policies/tripwire.ttl`). It is the governed twin
//! of the local `[[yupana.policy.tripwires]]` ([`crate::tripwire`]) — quipu
//! canonical, yupana holding only a projected cache, exactly the rule/govered
//! split the rule planes already follow.
//!
//! Decode discipline is [`crate::project_decode`]'s, restated because it is
//! load-bearing: an unrecognised effect is an ERROR, never a skipped wire — a
//! dropped tripwire is a boundary that reads as guarded and is not. Rows whose
//! policy binds a Selector or Predicate are not errors: they are rule policies,
//! enforced by [`POLICY_QUERY`](crate::project_queries::POLICY_QUERY)'s plane,
//! and this decode leaves them to it.
//!
//! Placement follows the same partition the rule planes use: a wire declaring
//! `verificationPoint "PAA"` is SKIPPED at the gate (its author said it judges
//! completed-action state; quipu's own placement law puts soft throttle wires
//! there), and the PAA-side projection is the seam's sequencing step 2 — the
//! quipu catalog says so in the same words.

use serde::{Deserialize, Serialize};

use crate::constraint::{ConstraintClass, VerificationPoint};
use crate::errors::{Error, Result};
use crate::project::ProjectedViolation;
use crate::tripwire::{TripEffect, Tripwire};

/// A tripwire projected from quipu, with the SARC placement it declared.
///
/// Serializable so it can ride the durable projection cache
/// ([`crate::projection_cache`]) — a projection failure must degrade to
/// last-known wires, stale and saying so, never to wires silently vanishing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedTripwire {
    /// The wire itself, in the same shape local config uses — the seam is a
    /// decode, not a redesign.
    pub wire: Tripwire,
    /// The declared class, when quipu declared one.
    pub class: Option<ConstraintClass>,
    /// Where its author said it is judged. `PAA` wires are skipped at the gate.
    pub verification_point: Option<VerificationPoint>,
    /// The declared `aegis:backoffFormula` a throttle wire's backoff is
    /// compiled from ([`crate::throttle::backoff_secs`]).
    pub backoff_formula: Option<String>,
}

/// A throttle a tripped governed wire orders for subsequent edits. Pure data —
/// the hook records it; this module never touches the state file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThrottleOrder {
    /// The wire whose crossing is being priced.
    pub name: String,
    /// Backoff seconds, compiled from the declared formula.
    pub secs: u64,
}

/// Fetch and decode quipu's tripwire policies over HTTP — same transport as
/// every other projection ([`crate::project::query`]).
pub fn fetch_tripwires(endpoint: &str) -> Result<Vec<ProjectedTripwire>> {
    decode_tripwires(&crate::project::query(
        endpoint,
        crate::project_queries::TRIPWIRE_QUERY,
    )?)
}

/// Decode the [`TRIPWIRE_QUERY`](crate::project_queries::TRIPWIRE_QUERY) result.
///
/// `appliesTo` is the genuinely multi-valued binding: N globs arrive as N rows
/// and are ACCUMULATED. Any other field disagreeing across one policy's rows is
/// a conflicting definition and refuses the projection — never a coin flip on
/// row order.
pub fn decode_tripwires(sparql_json: &str) -> Result<Vec<ProjectedTripwire>> {
    let value: serde_json::Value = serde_json::from_str(sparql_json)
        .map_err(|e| Error::Projection(format!("results are not JSON: {e}")))?;
    let rows = crate::project_decode::rows_of(&value)?;

    // policy IRI -> accumulated fields, insertion-ordered via the names vec.
    let mut order: Vec<String> = Vec::new();
    let mut paths: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut scalars: std::collections::HashMap<String, ScalarFields> =
        std::collections::HashMap::new();

    for (i, row) in rows.iter().enumerate() {
        let get = |key: &str| crate::project_decode::binding_value(row, key);
        // A row binding a Selector or Predicate is a RULE policy — the other
        // plane's business, not a tripwire and not an error here.
        if get("selector").is_some() || get("predicate").is_some() {
            continue;
        }
        let iri = get("policy")
            .ok_or_else(|| Error::Projection(format!("tripwire row {i}: missing `policy`")))?;
        let glob = get("appliesTo")
            .ok_or_else(|| Error::Projection(format!("tripwire row {i}: missing `appliesTo`")))?;
        if !order.contains(&iri) {
            order.push(iri.clone());
        }
        let globs = paths.entry(iri.clone()).or_default();
        if !globs.contains(&glob) {
            globs.push(glob);
        }
        let fields = scalars.entry(iri.clone()).or_default();
        for (slot, key) in [
            (&mut fields.name, "name"),
            (&mut fields.effect, "effect"),
            (&mut fields.class, "constraintClass"),
            (&mut fields.point, "verificationPoint"),
            (&mut fields.formula, "backoffFormula"),
        ] {
            if let Some(v) = get(key) {
                match slot {
                    Some(existing) if *existing != v => {
                        return Err(Error::Projection(format!(
                            "tripwire `{iri}`: conflicting `{key}` values \
                             (`{existing}` vs `{v}`) — refusing rather than \
                             picking a row"
                        )));
                    }
                    Some(_) => {}
                    None => *slot = Some(v),
                }
            }
        }
    }

    let mut out = Vec::with_capacity(order.len());
    for iri in order {
        let fields = scalars.remove(&iri).unwrap_or_default();
        let name = fields
            .name
            .unwrap_or_else(|| iri.rsplit('/').next().unwrap_or(&iri).to_string());
        // No effect, or one this seam cannot enforce, is an ERROR: a wire that
        // decodes and then does nothing is the inert-armed control this whole
        // concept exists to prevent. (`require-approval` etc. are real effects
        // with no channel at this seam — refusing names that plainly.)
        let effect = match fields.effect.as_deref() {
            Some("deny") => TripEffect::Deny,
            Some("warn") => TripEffect::Warn,
            Some("throttle") => TripEffect::Throttle,
            Some(other) => {
                return Err(Error::Projection(format!(
                    "tripwire `{name}`: effect `{other}` is not enforceable at \
                     the pre-edit tripwire seam (warn / deny / throttle are)"
                )));
            }
            None => {
                return Err(Error::Projection(format!(
                    "tripwire `{name}` declares no effect"
                )));
            }
        };
        let class = match fields.class.as_deref() {
            Some(s) => Some(ConstraintClass::parse(s).ok_or_else(|| {
                Error::Projection(format!("tripwire `{name}`: unknown constraintClass `{s}`"))
            })?),
            None => None,
        };
        let point = match fields.point.as_deref() {
            Some(s) => Some(VerificationPoint::parse(s).ok_or_else(|| {
                Error::Projection(format!(
                    "tripwire `{name}`: unknown verificationPoint `{s}`"
                ))
            })?),
            None => None,
        };
        out.push(ProjectedTripwire {
            wire: Tripwire {
                name,
                paths: paths.remove(&iri).unwrap_or_default(),
                rule: None,
                effect,
                backoff_secs: None,
                message: None,
            },
            class,
            verification_point: point,
            backoff_formula: fields.formula,
        });
    }
    Ok(out)
}

#[derive(Default)]
struct ScalarFields {
    name: Option<String>,
    effect: Option<String>,
    class: Option<String>,
    point: Option<String>,
    formula: Option<String>,
}

/// Evaluate the gate-eligible governed wires against one edit's path.
///
/// Pure: returns the violations for the governed plane to merge, plus the
/// throttle orders for the hook to record — this module never does the I/O.
///
/// A `PAA`-declared wire is skipped (its author said post-action); an
/// undeclared point runs here, the pre-field behaviour, same as
/// [`runs_at_pre_edit`](crate::project::runs_at_pre_edit). Blocking is
/// class-first, mirroring [`policy_blocks`](crate::project::policy_blocks):
/// the declared class decides when quipu supplied one (a soft wire never
/// blocks, whatever its effect), else a `deny` effect blocks under `Enforce`.
#[must_use]
pub fn gate_violations(
    wires: &[ProjectedTripwire],
    rel: &str,
    mode: crate::policy::Mode,
) -> (Vec<ProjectedViolation>, Vec<ThrottleOrder>) {
    let mut violations = Vec::new();
    let mut throttles = Vec::new();
    for projected in wires {
        if projected.verification_point == Some(VerificationPoint::Paa) {
            continue;
        }
        // Compile the declared formula into the wire before evaluation so the
        // model-facing message names the real backoff. A crossing's overage is
        // 1.0 — the wire tripped once — matching the PAA's violations-count
        // reading of the same formula.
        let mut wire = projected.wire.clone();
        if wire.effect == TripEffect::Throttle && wire.backoff_secs.is_none() {
            wire.backoff_secs = projected
                .backoff_formula
                .as_deref()
                .and_then(|f| crate::throttle::backoff_secs(f, 1.0));
        }
        let trips = crate::tripwire::evaluate(std::slice::from_ref(&wire), &[], rel, None, None);
        for trip in trips {
            let blocking = match projected.class {
                Some(class) => class.blocks(mode),
                None => mode == crate::policy::Mode::Enforce && trip.effect == TripEffect::Deny,
            };
            // A declared formula this build cannot compile applies NO backoff
            // (`backoff_secs` stayed `None`) — the PAA's own discipline for
            // the same field: the crossing records, the response does not.
            if let (TripEffect::Throttle, Some(secs)) = (trip.effect, trip.backoff_secs) {
                throttles.push(ThrottleOrder {
                    name: trip.name.clone(),
                    secs,
                });
            }
            violations.push(ProjectedViolation {
                message: format!("{} [governed]", trip.message),
                blocking,
                id: trip.name,
                class: projected.class,
                verification_point: projected.verification_point,
            });
        }
    }
    (violations, throttles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Mode;

    fn row(pairs: &[(&str, &str)]) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for (k, v) in pairs {
            m.insert(
                (*k).to_string(),
                serde_json::json!({"type": "literal", "value": v}),
            );
        }
        serde_json::Value::Object(m)
    }

    fn results(rows: &[serde_json::Value]) -> String {
        serde_json::json!({"results": {"bindings": rows}}).to_string()
    }

    #[test]
    fn a_multi_glob_wire_accumulates_its_rows() {
        let json = results(&[
            row(&[
                ("policy", "http://x/p1"),
                ("name", "auth-boundary"),
                ("appliesTo", "src/auth/**"),
                ("effect", "deny"),
                ("constraintClass", "hard"),
                ("verificationPoint", "PAG"),
            ]),
            row(&[
                ("policy", "http://x/p1"),
                ("name", "auth-boundary"),
                ("appliesTo", "src/session/**"),
                ("effect", "deny"),
                ("constraintClass", "hard"),
                ("verificationPoint", "PAG"),
            ]),
        ]);
        let wires = decode_tripwires(&json).unwrap();
        assert_eq!(wires.len(), 1);
        assert_eq!(
            wires[0].wire.paths,
            vec!["src/auth/**".to_string(), "src/session/**".to_string()]
        );
        assert_eq!(wires[0].wire.effect, TripEffect::Deny);
        assert_eq!(
            wires[0].class,
            Some(crate::constraint::ConstraintClass::Hard)
        );
    }

    #[test]
    fn a_rule_policy_row_is_left_to_the_other_plane_not_an_error() {
        let json = results(&[row(&[
            ("policy", "http://x/rule1"),
            ("appliesTo", "src/**"),
            ("effect", "deny"),
            ("selector", "http://x/sel1"),
        ])]);
        assert!(decode_tripwires(&json).unwrap().is_empty());
    }

    #[test]
    fn an_unenforceable_or_missing_effect_is_an_error_not_a_dropped_wire() {
        let approval = results(&[row(&[
            ("policy", "http://x/p1"),
            ("appliesTo", "src/**"),
            ("effect", "require-approval"),
        ])]);
        assert!(decode_tripwires(&approval).is_err());
        let none = results(&[row(&[("policy", "http://x/p1"), ("appliesTo", "src/**")])]);
        assert!(decode_tripwires(&none).is_err());
    }

    #[test]
    fn conflicting_scalar_rows_refuse_the_projection() {
        let json = results(&[
            row(&[
                ("policy", "http://x/p1"),
                ("appliesTo", "a/**"),
                ("effect", "deny"),
            ]),
            row(&[
                ("policy", "http://x/p1"),
                ("appliesTo", "a/**"),
                ("effect", "warn"),
            ]),
        ]);
        assert!(decode_tripwires(&json).is_err());
    }

    fn wire(effect: TripEffect, point: Option<VerificationPoint>) -> ProjectedTripwire {
        ProjectedTripwire {
            wire: Tripwire {
                name: "w".to_string(),
                paths: vec!["src/auth/**".to_string()],
                rule: None,
                effect,
                backoff_secs: None,
                message: None,
            },
            class: None,
            verification_point: point,
            backoff_formula: None,
        }
    }

    #[test]
    fn a_paa_wire_is_skipped_at_the_gate() {
        let wires = [wire(TripEffect::Throttle, Some(VerificationPoint::Paa))];
        let (violations, throttles) = gate_violations(&wires, "src/auth/a.rs", Mode::Enforce);
        assert!(violations.is_empty());
        assert!(throttles.is_empty());
    }

    #[test]
    fn class_decides_blocking_before_effect_does() {
        // Soft class: never blocks, even declared `deny` — policy_blocks' rule.
        let mut soft = wire(TripEffect::Deny, None);
        soft.class = Some(crate::constraint::ConstraintClass::Soft);
        let (violations, _) = gate_violations(&[soft], "src/auth/a.rs", Mode::Enforce);
        assert!(!violations[0].blocking);
        // No class: deny blocks under enforce, not under advise.
        let bare = wire(TripEffect::Deny, None);
        let (violations, _) =
            gate_violations(std::slice::from_ref(&bare), "src/auth/a.rs", Mode::Enforce);
        assert!(violations[0].blocking);
        let (violations, _) = gate_violations(&[bare], "src/auth/a.rs", Mode::Advise);
        assert!(!violations[0].blocking);
    }

    #[test]
    fn a_throttle_wire_compiles_its_declared_formula_into_an_order() {
        let mut throttle = wire(TripEffect::Throttle, None);
        throttle.backoff_formula = Some("exp(min(overage / 1.0, 8.0))".to_string());
        let (violations, throttles) = gate_violations(&[throttle], "src/auth/a.rs", Mode::Enforce);
        assert_eq!(throttles.len(), 1);
        assert_eq!(throttles[0].secs, 2); // exp(1.0) truncated
        assert!(!violations[0].blocking);
        // An uncompilable formula orders NO throttle; the crossing still records.
        let mut bad = wire(TripEffect::Throttle, None);
        bad.backoff_formula = Some("vibes".to_string());
        let (violations, throttles) = gate_violations(&[bad], "src/auth/a.rs", Mode::Enforce);
        assert!(throttles.is_empty());
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn outside_the_boundary_nothing_fires() {
        let wires = [wire(TripEffect::Deny, None)];
        let (violations, throttles) = gate_violations(&wires, "src/lib.rs", Mode::Enforce);
        assert!(violations.is_empty());
        assert!(throttles.is_empty());
    }
}
