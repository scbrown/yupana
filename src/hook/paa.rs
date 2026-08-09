//! The Post-Action Auditor — constraints evaluated *after* the edit landed.
//!
//! `post_edit` was named an auditor and audited nothing: it computed a
//! blast-radius advisory and evaluated no constraint, emitted no verdict, and
//! could not affect the next action. SARC's soft class had nowhere to live, and
//! `throttle` — the response its evaluation attributes the whole 89.5%
//! soft-overage reduction to — had no implementation.
//!
//! ## What a PAA can and cannot do
//!
//! It cannot prevent the action it just watched. Stating that plainly matters
//! more than it sounds: a PAA presented as prevention is the false-`prevented`
//! claim the enforcement gradient exists to stop, and it is exactly what quipu's
//! placement check refuses to let a *hard* constraint declare here.
//!
//! What it can do is judge completed-action state — the file as it now is,
//! rather than the fragment an edit proposed — and change what happens next. Two
//! consequences follow, and both are real:
//!
//! 1. **Evidence the gate does not have.** The pre-edit gate sees only the text
//!    an edit introduces. A rule like "this file must contain at least one test"
//!    is unanswerable there and trivial here, because `must-exist` over a
//!    fragment is a question about the wrong subject.
//! 2. **A response that acts on the successor.** `throttle` records an expiring
//!    backoff the next edit's advisory surfaces (`crate::throttle`).
//!
//! ## Which constraints run here
//!
//! Only those DECLARING `verificationPoint "PAA"`. A rule with no declared point
//! runs at the gate, which is where it ran before the field existed — the
//! projection's pre-Phase-1 behaviour, unchanged. Nothing is evaluated twice:
//! `runs_at_pre_edit` and this module partition the set rather than overlapping,
//! so a constraint is judged at one point and its verdict says which.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::HookInput;

use crate::config::YupanaConfig;
use crate::constraint::VerificationPoint;
use crate::policy::Mode;
use crate::rules::Rule;
use crate::trace::{ConstraintEvaluation, Outcome, Response};

/// What the auditor concluded about a completed edit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Audit {
    /// The constraints evaluated, for the trace record and the verdict spool.
    pub(super) constraints: Vec<ConstraintEvaluation>,
    /// Model-facing lines, if any.
    pub(super) messages: Vec<String>,
}

impl Audit {
    /// Whether anything was evaluated at all.
    pub(super) fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }
}

/// The locally-configured rules that declare themselves post-action.
fn paa_rules(config: &YupanaConfig) -> Vec<&Rule> {
    config
        .policy
        .rules
        .iter()
        .filter(|r| r.verification_point == Some(VerificationPoint::Paa))
        .collect()
}

/// Audit a completed edit against the PAA-declared constraints.
///
/// `source` is the file as it now stands — the completed-action state, which is
/// the whole reason this point exists. Returns an empty [`Audit`] when the guard
/// is off, nothing declares itself here, or the file has no applicable grammar.
pub(super) fn audit(
    config: &YupanaConfig,
    source: &str,
    rel: &str,
    language: &str,
    scope: &str,
) -> Audit {
    // Mode::Off disarms the whole guard, the auditor included. Note this is the
    // ONLY thing the mode changes here: a soft constraint never blocks, so
    // Advise and Enforce behave identically at this point. That is not an
    // oversight — it is what makes the class meaningful.
    if config.policy.mode == Mode::Off {
        return Audit::default();
    }
    let rules = paa_rules(config);
    if rules.is_empty() {
        return Audit::default();
    }

    let mut audit = Audit::default();
    let now = crate::throttle::now_secs();
    for rule in rules {
        if !rule.applies(rel) || rule.language != language {
            continue;
        }
        let owned = [(*rule).clone()];
        let violations = crate::rules::evaluate(&owned, source, language, rel);
        if violations.is_empty() {
            // A satisfied constraint is RECORDED, not skipped. "The rule ran and
            // held" and "the rule never ran" are different facts, and the audit
            // checker's coverage pass needs to tell them apart — an absent
            // evaluation reads as a constraint nobody applied.
            audit.constraints.push(
                ConstraintEvaluation::new(&rule.name, Outcome::Satisfied, Response::Logged)
                    .placed(rule.class, rule.verification_point)
                    .hosted_at(crate::hosting::YUPANA_HOSTS_AT),
            );
            continue;
        }

        // The response. A PAA constraint that fired gets a throttle when one is
        // declared, and a plain warning otherwise — never a block, whatever the
        // ambient mode says.
        let (response, note) = match rule.backoff_formula.as_deref() {
            Some(formula) => {
                match crate::throttle::backoff_secs(formula, violations.len() as f64) {
                    Some(secs) => {
                        crate::throttle::record(&rule.name, scope, secs, now);
                        (
                            Response::Warned,
                            format!(
                                " (throttling subsequent edits for {secs}s — advisory, \
                             nothing is blocked)"
                            ),
                        )
                    }
                    None => (
                        Response::NoAction,
                        format!(
                            " (declared backoff formula `{formula}` is not one yupana \
                         understands, so NO throttle was applied — the crossing is \
                         recorded and the response was not)"
                        ),
                    ),
                }
            }
            None => (Response::Warned, String::new()),
        };

        for violation in &violations {
            audit.messages.push(format!("{}{note}", violation.message));
        }
        audit.constraints.push(
            ConstraintEvaluation::new(&rule.name, Outcome::Unsatisfied, response)
                .placed(rule.class, rule.verification_point)
                .hosted_at(crate::hosting::YUPANA_HOSTS_AT),
        );
    }
    audit
}

/// The advisory lines for any throttle still in force, for the pre-edit gate to
/// surface on the NEXT edit. Empty when nothing is throttled.
#[must_use]
pub fn active_advisories(scope: &str, now: u64) -> Vec<String> {
    crate::throttle::active_now(scope, now)
        .iter()
        .map(|t| t.advisory(now))
        .collect()
}

/// Fold any live throttle into a gate outcome, returning how many applied.
///
/// This is the throttle response *landing*: recorded at the post-action point,
/// felt at the next pre-action one. Purely additive by construction — it only
/// ever turns `Allow` into `Notify` or appends to an existing `Notify`, and it
/// cannot reach `Deny`. A soft constraint must not become a block by this route,
/// and stating that as code rather than as a convention is what keeps it true.
#[must_use]
pub fn apply_advisories(outcome: &mut super::Outcome, scope: &str, now: u64) -> usize {
    if matches!(outcome, super::Outcome::Deny(_)) {
        return 0;
    }
    let advisories = active_advisories(scope, now);
    if advisories.is_empty() {
        return 0;
    }
    let joined = advisories.join("\n");
    *outcome = match std::mem::replace(outcome, super::Outcome::Allow) {
        super::Outcome::Notify(existing) => super::Outcome::Notify(format!("{existing}\n{joined}")),
        _ => super::Outcome::Notify(joined),
    };
    advisories.len()
}

/// Evaluate the PAA-declared constraints against the completed edit, record the
/// trace line and the verdicts, and return anything the model should be told.
///
/// Returns `None` when there is nothing to audit. Silent on every absence, like
/// the rest of this module.
#[must_use]
pub fn post_action_audit(input_json: &str, default_root: &Path) -> Option<Vec<String>> {
    let input = HookInput::parse(input_json)?;
    let file_path = input.tool_input.file_path.clone()?;
    let file = PathBuf::from(&file_path);
    let root = input.root(default_root);
    let config = crate::config::YupanaConfig::resolve(None, &root).ok()?;

    let rel = super::measure::relative(&file, &root);
    let ext = file.extension().and_then(OsStr::to_str)?;
    let language = crate::extract::language_for_extension(ext)?;
    // The COMPLETED-ACTION state: the file as it now stands, which is the whole
    // reason this enforcement point exists. The gate saw only the fragment.
    let source = std::fs::read_to_string(&file).ok()?;

    let audit = audit(
        &config,
        &source,
        &rel,
        language,
        &root.display().to_string(),
    );
    if audit.is_empty() {
        return None;
    }

    // The trace record, in the same vocabulary the gate emits — including the
    // attribution tuple, so a PAA crossing and the gate decision that follows it
    // are attributable to the same chain. A record that attributed only the gate
    // would leave every soft-constraint crossing unowned.
    let mut fields: Vec<(&str, serde_json::Value)> = vec![
        ("point", "PAA".into()),
        ("constraints", crate::trace::to_json(&audit.constraints)),
        ("ext", ext.into()),
    ];
    fields.extend(crate::attribution::Attribution::capture(input.tool_name.as_deref()).fields());
    crate::metrics::emit("audit", &fields);

    // Verdicts, signed against the completed state that was actually judged.
    #[cfg(feature = "quipu")]
    if config.quipu.enabled {
        if let Some(key) =
            crate::verdict_spool::existing_key(&root.join(&config.quipu.signing_key_path))
        {
            let _ = crate::verdict_spool::record(
                &key,
                &audit.constraints,
                &rel,
                &source,
                crate::types::Freshness::Fresh,
            );
        }
    }

    if audit.messages.is_empty() {
        return None;
    }
    Some(audit.messages)
}

#[cfg(test)]
#[path = "paa_test.rs"]
mod tests;
