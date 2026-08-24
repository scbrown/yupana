//! Tripwires — local Binding/Gate declarations for the pre-edit guard.
//!
//! The governance plane's `Binding / Gate` primitive
//! (`docs/book/src/design/governance-plane.md`) attaches a policy to a
//! *boundary* with an *effect*. This module is the local-config slice of that
//! primitive: a **tripwire** names a boundary (path globs, a rule, or both) and
//! declares what crossing it triggers — `warn`, `deny`, or `throttle`. The
//! edit itself is the crossing; nothing has to remember to check the wire.
//!
//! A tripwire differs from a plain `[[yupana.policy.rules]]` entry in what it
//! binds: a rule says what text may look like everywhere it applies, while a
//! tripwire says what happens *at this boundary* — the same rule can advise
//! repo-wide and trip a `deny` inside `src/auth/**`, and a boundary can trip on
//! being touched at all, with no rule involved.
//!
//! Pure like [`crate::rules`]: evaluation takes the edit's facts and does no
//! I/O. The `throttle` effect's *recording* is the hook arm's job
//! (`src/hook/tripwire_arm.rs`); here it is only a declared effect.

use serde::{Deserialize, Serialize};

use crate::rules::Rule;

/// What a tripped wire triggers.
///
/// A typed enum rather than a string for the same reason [`crate::policy::Mode`]
/// is: a typo in `effect` must be a loud config error, never a wire that looks
/// armed and is inert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TripEffect {
    /// Tell the model, block nothing.
    Warn,
    /// Block the edit under [`crate::policy::Mode::Enforce`]; advise below it.
    Deny,
    /// Record an expiring backoff ([`crate::throttle`]) that subsequent edits
    /// surface. Never blocks — a throttle that blocks is a deny with a
    /// misleading name.
    Throttle,
}

impl TripEffect {
    /// The lowercase name, matching the config value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Deny => "deny",
            Self::Throttle => "throttle",
        }
    }
}

/// One tripwire: a boundary and the effect crossing it triggers
/// (`[[yupana.policy.tripwires]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tripwire {
    /// Stable identifier, cited by verdicts and advisories.
    pub name: String,
    /// Repo-relative path globs the wire spans. An edit whose path matches is a
    /// crossing (when `rule` is also set, a *candidate* crossing). Empty = the
    /// wire spans every path, which then requires `rule` to give it a condition.
    #[serde(default)]
    pub paths: Vec<String>,
    /// The name of a `[[yupana.policy.rules]]` entry that must fire on the
    /// introduced text for the wire to trip. `None` = touching the paths is
    /// enough.
    #[serde(default)]
    pub rule: Option<String>,
    /// What tripping triggers.
    pub effect: TripEffect,
    /// The backoff a [`TripEffect::Throttle`] records, in seconds. Required for
    /// that effect — a backoff nobody declared is a cost nobody agreed to.
    #[serde(default)]
    pub backoff_secs: Option<u64>,
    /// Optional custom model-facing explanation, overriding the default.
    #[serde(default)]
    pub message: Option<String>,
}

impl Tripwire {
    /// Whether `rel` is inside this wire's path boundary. Empty spans everywhere;
    /// a malformed glob never matches — [`errors`] surfaces it instead.
    fn spans(&self, rel: &str) -> bool {
        self.paths.is_empty()
            || self
                .paths
                .iter()
                .any(|g| glob::Pattern::new(g).is_ok_and(|p| p.matches(rel)))
    }
}

/// A wire that tripped on this edit, with the text shown to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trip {
    /// The tripped wire's name.
    pub name: String,
    /// Its declared effect.
    pub effect: TripEffect,
    /// The declared backoff, for [`TripEffect::Throttle`].
    pub backoff_secs: Option<u64>,
    /// Model-facing explanation: which boundary, why it tripped, what triggers.
    pub message: String,
}

/// Every tripwire that cannot do its job, as `(name, reason)` — the
/// [`crate::rules::errors`] discipline. A wire in this list is misconfigured
/// and the guard fails open LOUDLY rather than treating an inert wire as a
/// quiet one; a control that cannot fire, believed armed, is the defect class
/// this crate keeps rediscovering.
#[must_use]
pub fn errors(tripwires: &[Tripwire], rules: &[Rule]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for wire in tripwires {
        if wire.paths.is_empty() && wire.rule.is_none() {
            out.push((
                wire.name.clone(),
                "no paths and no rule — a wire with no boundary can never trip".to_string(),
            ));
        }
        for pattern in &wire.paths {
            if let Err(e) = glob::Pattern::new(pattern) {
                out.push((wire.name.clone(), format!("paths glob `{pattern}`: {e}")));
            }
        }
        if let Some(rule) = &wire.rule {
            if !rules.iter().any(|r| &r.name == rule) {
                out.push((
                    wire.name.clone(),
                    format!(
                        "references rule `{rule}`, which no [[yupana.policy.rules]] entry defines"
                    ),
                ));
            }
        }
        if wire.effect == TripEffect::Throttle && wire.backoff_secs.is_none() {
            out.push((
                wire.name.clone(),
                "effect = \"throttle\" declares no backoff_secs".to_string(),
            ));
        }
    }
    out
}

/// Evaluate every tripwire against one edit, returning the wires that tripped.
///
/// `introduced` and `language` describe the text the edit adds, when there is
/// any and this build has a grammar for it — a rule-conditioned wire without
/// both has no evidence and does not trip (the rule plane's own discipline: an
/// unevaluable rule is not a fired one). A path-only wire needs neither: the
/// crossing is the edit's target, not its text, so a pure deletion inside the
/// boundary still trips it.
#[must_use]
pub fn evaluate(
    tripwires: &[Tripwire],
    rules: &[Rule],
    rel: &str,
    introduced: Option<&str>,
    language: Option<&str>,
) -> Vec<Trip> {
    let mut out = Vec::new();
    for wire in tripwires {
        if !wire.spans(rel) {
            continue;
        }
        let detail = match &wire.rule {
            None => format!("edit touches `{rel}`, inside the wire's boundary"),
            Some(rule_name) => {
                let (Some(text), Some(language)) = (introduced, language) else {
                    continue;
                };
                let Some(rule) = rules.iter().find(|r| &r.name == rule_name) else {
                    continue; // Surfaced by `errors`, never silently skipped here alone.
                };
                let violations =
                    crate::rules::evaluate(std::slice::from_ref(rule), text, language, rel);
                if violations.is_empty() {
                    continue;
                }
                let fired: Vec<String> = violations.into_iter().map(|v| v.message).collect();
                format!(
                    "rule `{rule_name}` fired inside the boundary:\n{}",
                    fired.join("\n")
                )
            }
        };
        let effect_note = match wire.effect {
            TripEffect::Warn => "effect: warn".to_string(),
            TripEffect::Deny => "effect: deny".to_string(),
            TripEffect::Throttle => format!(
                "effect: throttle ({}s backoff on subsequent edits — advisory, nothing is blocked)",
                wire.backoff_secs.unwrap_or_default()
            ),
        };
        let message = match &wire.message {
            Some(custom) => format!(
                "yupana: tripwire `{}` tripped — {custom} ({effect_note})",
                wire.name
            ),
            None => format!(
                "yupana: tripwire `{}` tripped — {detail} ({effect_note})",
                wire.name
            ),
        };
        out.push(Trip {
            name: wire.name.clone(),
            effect: wire.effect,
            backoff_secs: wire.backoff_secs,
            message,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::MatchType;

    fn wire(name: &str, paths: &[&str], rule: Option<&str>, effect: TripEffect) -> Tripwire {
        Tripwire {
            name: name.to_string(),
            paths: paths.iter().map(|s| (*s).to_string()).collect(),
            rule: rule.map(str::to_string),
            effect,
            backoff_secs: None,
            message: None,
        }
    }

    fn ticket_rule() -> Rule {
        Rule {
            name: "no-ticket-in-comment".to_string(),
            language: "rust".to_string(),
            query: "(line_comment) @c".to_string(),
            gate: None,
            match_type: MatchType::MustNotMatch,
            pattern: r"\b[A-Z]+-[0-9]+\b".to_string(),
            applies_to: Vec::new(),
            message: None,
            class: None,
            verification_point: None,
            backoff_formula: None,
        }
    }

    #[test]
    fn a_path_only_wire_trips_on_any_edit_inside_its_boundary() {
        let wires = [wire(
            "auth-boundary",
            &["src/auth/**"],
            None,
            TripEffect::Deny,
        )];
        let trips = evaluate(&wires, &[], "src/auth/login.rs", None, None);
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].name, "auth-boundary");
        assert_eq!(trips[0].effect, TripEffect::Deny);
        assert!(trips[0].message.contains("src/auth/login.rs"));
        // Outside the boundary, nothing trips.
        assert!(evaluate(&wires, &[], "src/lib.rs", None, None).is_empty());
    }

    #[test]
    fn a_rule_conditioned_wire_trips_only_when_the_rule_fires_inside_it() {
        let wires = [wire(
            "no-tickets-in-auth",
            &["src/auth/**"],
            Some("no-ticket-in-comment"),
            TripEffect::Deny,
        )];
        let rules = [ticket_rule()];
        let bad = "// see ABC-123\nfn f() {}\n";
        let good = "// clean\nfn f() {}\n";

        let trips = evaluate(&wires, &rules, "src/auth/login.rs", Some(bad), Some("rust"));
        assert_eq!(trips.len(), 1);
        assert!(trips[0].message.contains("no-ticket-in-comment"));

        // The rule holding means no trip; the same violation OUTSIDE the
        // boundary means no trip either — the wire is the boundary, not the rule.
        assert!(evaluate(
            &wires,
            &rules,
            "src/auth/login.rs",
            Some(good),
            Some("rust")
        )
        .is_empty());
        assert!(evaluate(&wires, &rules, "src/other.rs", Some(bad), Some("rust")).is_empty());
    }

    #[test]
    fn a_rule_conditioned_wire_without_evidence_does_not_trip() {
        let wires = [wire(
            "no-tickets-in-auth",
            &["src/auth/**"],
            Some("no-ticket-in-comment"),
            TripEffect::Warn,
        )];
        let rules = [ticket_rule()];
        // No introduced text (pure deletion) and no grammar: no evidence, no trip.
        assert!(evaluate(&wires, &rules, "src/auth/a.rs", None, Some("rust")).is_empty());
        assert!(evaluate(&wires, &rules, "src/auth/a.rs", Some("// ABC-123"), None).is_empty());
    }

    #[test]
    fn a_path_only_wire_trips_even_on_a_pure_deletion() {
        // The crossing is the edit's TARGET: no introduced text required.
        let wires = [wire("frozen", &["vendor/**"], None, TripEffect::Warn)];
        assert_eq!(evaluate(&wires, &[], "vendor/lib.c", None, None).len(), 1);
    }

    #[test]
    fn a_throttle_trip_carries_its_declared_backoff_and_says_it_blocks_nothing() {
        let mut w = wire("hot-file", &["src/core.rs"], None, TripEffect::Throttle);
        w.backoff_secs = Some(300);
        let trips = evaluate(&[w], &[], "src/core.rs", None, None);
        assert_eq!(trips[0].backoff_secs, Some(300));
        assert!(trips[0].message.contains("300s"));
        assert!(trips[0].message.contains("nothing is blocked"));
    }

    #[test]
    fn a_custom_message_overrides_the_default_detail() {
        let mut w = wire("auth-boundary", &["src/auth/**"], None, TripEffect::Deny);
        w.message = Some("auth changes need the security workflow".to_string());
        let trips = evaluate(&[w], &[], "src/auth/a.rs", None, None);
        assert!(trips[0].message.contains("security workflow"));
        assert!(trips[0].message.contains("tripwire `auth-boundary`"));
    }

    #[test]
    fn misconfigured_wires_are_reported_not_silently_inert() {
        let no_condition = wire("no-condition", &[], None, TripEffect::Warn);
        let bad_glob = wire("bad-glob", &["src/["], None, TripEffect::Warn);
        let unknown_rule = wire("unknown-rule", &[], Some("nonexistent"), TripEffect::Warn);
        let bare_throttle = wire("bare-throttle", &["src/**"], None, TripEffect::Throttle);
        let errs = errors(
            &[no_condition, bad_glob, unknown_rule, bare_throttle],
            &[ticket_rule()],
        );
        let names: Vec<&str> = errs.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"no-condition"));
        assert!(names.contains(&"bad-glob"));
        assert!(names.contains(&"unknown-rule"));
        assert!(names.contains(&"bare-throttle"));

        // A well-formed wire reports nothing.
        let mut ok = wire(
            "ok",
            &["src/**"],
            Some("no-ticket-in-comment"),
            TripEffect::Throttle,
        );
        ok.backoff_secs = Some(60);
        assert!(errors(&[ok], &[ticket_rule()]).is_empty());
    }

    #[test]
    fn effect_parses_from_lowercase_toml_and_a_typo_is_a_loud_error() {
        #[derive(Deserialize)]
        struct W {
            e: TripEffect,
        }
        assert_eq!(
            toml::from_str::<W>("e = \"throttle\"").unwrap().e,
            TripEffect::Throttle
        );
        assert!(toml::from_str::<W>("e = \"block\"").is_err());
    }
}
