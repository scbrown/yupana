//! `yupana rule-test` — run a governed text rule against sample text through
//! the SAME engine the pre-edit guard uses, with no side effects
//! (aegis-l26g8x).
//!
//! The enforcement-readiness check needs a rule's recorded cases
//! (`aegis:mustMatch` / `aegis:mustNotMatch`) proven by the enforcer itself.
//! A proof by another engine proves nothing about this one: Python's `re` and
//! Rust's `regex` disagree on real syntax (`\b` under Unicode, possessive
//! quantifiers, lookaround), so a case that passes in a Python checker can
//! still fail here, or the other way round.
//!
//! So this verb goes through [`crate::textrules::evaluate_in`], the function
//! the hook's text plane calls, applying the same path and repo exemptions.
//! It reads the projection cache the hook reads and writes nothing: no
//! verdict spool, no metrics line, no cache refresh, no quipu request.
//!
//! Every case gets one of three outcomes, and UNKNOWN never counts as PASS:
//! - `pass`: the rule fired exactly when the case expected it to;
//! - `fail`: it did not;
//! - `unknown`: the case could not be judged. Either the rule is absent from
//!   the projection, or it does not compile. The guard fails open on a rule
//!   that does not compile, so no answer from it proves anything.
//!
//! Exit: 0 all pass · 1 any fail · 2 no fail but something unknown (or no
//! cases at all).

#[cfg(feature = "quipu")]
use std::io::Read;
#[cfg(feature = "quipu")]
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::textrules::{self, TextRule};

/// The path a case is judged at when it names none. It sits at the repo root
/// and no rule's exemption names it, so the rule applies as it would to an
/// ordinary file.
pub const DEFAULT_CASE_PATH: &str = "rule-test-case.txt";

/// Arguments for `yupana rule-test`. Gated with the projection cache it
/// reads: without `quipu` there are no governed rules to test.
#[cfg(feature = "quipu")]
#[derive(Debug, clap::Args)]
pub struct RuleTestArgs {
    /// Cases as a JSON array of `{rule, text, expect, path?, repo?}`, where
    /// `expect` is `match` or `no-match`. `-` reads stdin.
    #[arg(long, default_value = "-")]
    cases: PathBuf,
    /// Projection cache to take the rules from. Defaults to the file the
    /// pre-edit hook serves from.
    #[arg(long)]
    projection: Option<PathBuf>,
}

/// What a case expects the rule to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Expect {
    /// The rule must fire (`aegis:mustMatch`).
    Match,
    /// The rule must stay silent (`aegis:mustNotMatch`).
    NoMatch,
}

/// One recorded case.
#[derive(Debug, Clone, Deserialize)]
pub struct RuleCase {
    /// The rule's name, as projected (the entity IRI tail).
    pub rule: String,
    /// The introduced text to judge.
    pub text: String,
    /// What the rule must do with it.
    pub expect: Expect,
    /// Repo-relative path the text is written to, which decides path
    /// exemptions. Defaults to [`DEFAULT_CASE_PATH`].
    #[serde(default)]
    pub path: Option<String>,
    /// The repo name, which decides `aegis:exemptRepo`. Absent means
    /// unresolved, which exempts nothing, the same as in the hook.
    #[serde(default)]
    pub repo: Option<String>,
}

/// A case's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// The rule did what the case expected.
    Pass,
    /// The rule did not.
    Fail,
    /// The case could not be judged; see `reason`.
    Unknown,
}

/// The verdict on one case.
#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    /// The rule named by the case.
    pub rule: String,
    /// What the case expected.
    pub expect: Expect,
    /// The path the case was judged at.
    pub path: String,
    /// Whether the rule fired. `None` when the case could not be judged.
    pub fired: Option<bool>,
    /// The distinct tokens the rule matched.
    pub matched: Vec<String>,
    /// The case's outcome.
    pub outcome: Outcome,
    /// Why the outcome is `fail` or `unknown`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Judge every case against `rules`. Pure: the whole verb minus its I/O.
#[must_use]
pub fn run_cases(rules: &[TextRule], cases: &[RuleCase]) -> Vec<CaseResult> {
    cases.iter().map(|c| run_case(rules, c)).collect()
}

fn run_case(rules: &[TextRule], case: &RuleCase) -> CaseResult {
    let path = case
        .path
        .clone()
        .unwrap_or_else(|| DEFAULT_CASE_PATH.to_string());
    let mut result = CaseResult {
        rule: case.rule.clone(),
        expect: case.expect,
        path,
        fired: None,
        matched: Vec::new(),
        outcome: Outcome::Unknown,
        reason: None,
    };
    let Some(rule) = rules.iter().find(|r| r.name == case.rule) else {
        result.reason = Some("rule is not in the projection".into());
        return result;
    };
    let one = std::slice::from_ref(rule);
    if let Some((_, why)) = textrules::errors(one).into_iter().next() {
        result.reason = Some(format!("the guard fails open on this rule: {why}"));
        return result;
    }
    let violations = textrules::evaluate_in(one, &case.text, &result.path, case.repo.as_deref());
    let fired = !violations.is_empty();
    result.matched = textrules::distinct_matched(&violations);
    result.fired = Some(fired);
    if fired == (case.expect == Expect::Match) {
        result.outcome = Outcome::Pass;
    } else {
        result.outcome = Outcome::Fail;
        result.reason = Some(if fired {
            "the rule fired on text it must not match".into()
        } else {
            "the rule stayed silent on text it must match".into()
        });
    }
    result
}

/// The exit code for a set of results: 1 on any fail, else 2 if anything is
/// unknown or nothing was judged, else 0.
#[must_use]
pub fn exit_code(results: &[CaseResult]) -> i32 {
    if results.iter().any(|r| r.outcome == Outcome::Fail) {
        1
    } else if results.is_empty() || results.iter().any(|r| r.outcome == Outcome::Unknown) {
        2
    } else {
        0
    }
}

#[cfg(feature = "quipu")]
impl RuleTestArgs {
    /// Run the verb: read, judge, print JSON, exit with [`exit_code`].
    ///
    /// # Errors
    /// When the cases or the projection cannot be read or parsed. That is a
    /// failure to test at all, and reported as an error, never as a pass.
    pub fn run(&self) -> anyhow::Result<()> {
        let body = if self.cases.as_os_str() == "-" {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        } else {
            std::fs::read_to_string(&self.cases)?
        };
        let cases: Vec<RuleCase> = serde_json::from_str(&body)?;
        let path = match &self.projection {
            Some(p) => p.clone(),
            None => crate::projection_cache::cache_path()
                .ok_or_else(|| anyhow::anyhow!("no state directory for the projection cache"))?,
        };
        let cached: crate::projection_cache::CachedProjection =
            serde_json::from_slice(&std::fs::read(&path)?)?;
        let results = run_cases(&cached.text_rules, &cases);
        let count = |o| results.iter().filter(|r| r.outcome == o).count();
        let report = serde_json::json!({
            "projection": {
                "path": path,
                "endpoint": cached.endpoint,
                "written_at": cached.written_at,
                "age_secs": cached.age_secs(crate::projection_cache::now_secs()),
                "text_rules": cached.text_rules.len(),
            },
            "results": results,
            "summary": {
                "pass": count(Outcome::Pass),
                "fail": count(Outcome::Fail),
                "unknown": count(Outcome::Unknown),
            },
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        std::process::exit(exit_code(&results));
    }
}

#[cfg(test)]
#[path = "rule_test_test.rs"]
mod tests;
