//! Structural (tree-sitter-tier) edit policies — the "rule" plane.
//!
//! A capability [`scope`](crate::policy) governs WHERE and HOW FAR an edit may
//! reach. A *rule* governs WHAT the edit's text may look like — the checks a
//! linter finds hard or slow: "a TODO must cite a ticket", "no ticket id in a
//! comment". A rule pairs a tree-sitter query (the **Selector** — which nodes)
//! with a regex predicate (the **Predicate** — what their text must or must not
//! be), so it maps one-to-one onto Quipu's `aegis:Selector` + `aegis:Predicate`
//! (`docs/book/src/design/policy-edit-hooks.md`): a policy projected from Quipu
//! deserializes straight into a [`Rule`], and the field names are chosen to say
//! so.
//!
//! Everything here is tree-sitter-tier ([`Tier::TreeSitter`]): the selector runs
//! against the same best-effort parse the extractor uses. The predicate match is
//! exact given a clean parse, but provenance is still tree-sitter — a misparse is
//! possible — so a verdict is tagged accordingly and never claims more.
//!
//! Pure like [`crate::policy`]: it evaluates a buffer and does no I/O. A rule
//! whose selector/predicate does not compile is surfaced by [`errors`], never
//! silently treated as "nothing matched" (the discipline
//! [`crate::policy::Scope::glob_errors`] applies to path globs).

use serde::{Deserialize, Serialize};

use crate::constraint::{ConstraintClass, VerificationPoint};
use crate::errors::Error;
use crate::extract::query::{run_query, Capture};
use crate::types::Tier;

/// How a rule's predicate is applied to the nodes its selector captures.
///
/// The string forms match Quipu's `aegis:matchType` enum so a projected policy
/// round-trips.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MatchType {
    /// Every captured node's text MUST match the pattern; each that does not is a
    /// violation.
    MustMatch,
    /// No captured node's text may match the pattern; each that does is a
    /// violation.
    MustNotMatch,
    /// At least one captured node's text must match the pattern; a file with none
    /// is a single violation.
    MustExist,
}

impl MatchType {
    /// The default model-facing explanation, when a rule supplies no `message`.
    fn default_explanation(self, pattern: &str) -> String {
        match self {
            MatchType::MustMatch => {
                format!("each selected node's text must match /{pattern}/, but this one does not")
            }
            MatchType::MustNotMatch => {
                format!("no selected node's text may match /{pattern}/, but this one does")
            }
            MatchType::MustExist => {
                format!("at least one selected node's text must match /{pattern}/, but none does")
            }
        }
    }
}

/// One structural rule: a Selector (tree-sitter query) + a Predicate (regex).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Stable identifier, used in verdicts and in the projected-policy IRI.
    pub name: String,
    /// The language whose grammar the selector is written against (`rust`,
    /// `python`, …). A rule only evaluates files of its own language.
    pub language: String,
    /// The **Selector**: a tree-sitter `.scm` capture query, e.g.
    /// `(line_comment) @c`. Mirrors `aegis:Selector.evidenceSource`.
    pub query: String,
    /// An optional pre-filter regex on a capture's text: only captures matching
    /// the gate are tested by the predicate (e.g. only comments containing
    /// `\bTODO\b`).
    #[serde(default)]
    pub gate: Option<String>,
    /// The **Predicate** direction. Mirrors `aegis:matchType`.
    pub match_type: MatchType,
    /// The **Predicate** regex tested against a capture's text. Mirrors
    /// `aegis:Predicate.evidenceSource`.
    pub pattern: String,
    /// Repo-relative path globs this rule applies to. Empty = every path.
    #[serde(default)]
    pub applies_to: Vec<String>,
    /// An optional custom model-facing explanation, overriding the default for
    /// [`MatchType`].
    #[serde(default)]
    pub message: Option<String>,
    /// What KIND of bound this is (SARC §3.1), mirroring quipu's
    /// `aegis:constraintClass`. Distinct from the governed `effect`, which says
    /// what happens when it fires: `deny` is a response, `hard` is a class, and
    /// conflating them is what left "may this ever execute?" unanswerable.
    ///
    /// `None` for a rule that does not declare one — a locally-configured rule
    /// under `[[yupana.policy.rules]]`, or a projected policy from a quipu that
    /// predates the field. Absent means "fall back to the ambient
    /// [`crate::policy::Mode`]", never a guessed class.
    #[serde(default)]
    pub class: Option<ConstraintClass>,
    /// Where this rule is evaluated (SARC §4.1), mirroring
    /// `aegis:verificationPoint`. Carried so a verdict can say which point
    /// produced it; yupana evaluates `PAG` rules at `hook pre-edit` and `PAA`
    /// rules at `hook post-edit`.
    #[serde(default)]
    pub verification_point: Option<VerificationPoint>,
    /// The backoff a `throttle` response applies to SUBSEQUENT actions once this
    /// constraint's window is crossed, mirroring `aegis:backoffFormula`.
    ///
    /// Only meaningful on a soft constraint at the PAA — the only place a
    /// response can act on the next action rather than this one. `None` means
    /// the constraint warns without throttling, which is the honest default: a
    /// backoff nobody declared is a cost nobody agreed to.
    #[serde(default)]
    pub backoff_formula: Option<String>,
}

/// A single rule violation, with the text shown to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleViolation {
    /// The rule that was broken.
    pub rule: String,
    /// Model-facing explanation: what was broken, where, and what to do.
    pub message: String,
}

impl Rule {
    /// Whether this rule governs `rel` (matches one of its `applies_to` globs, or
    /// applies everywhere when the list is empty). A malformed glob never
    /// matches — [`errors`] surfaces it instead of letting it silently widen or
    /// narrow the rule.
    #[must_use]
    pub fn applies(&self, rel: &str) -> bool {
        self.applies_to.is_empty()
            || self
                .applies_to
                .iter()
                .any(|g| glob::Pattern::new(g).is_ok_and(|p| p.matches(rel)))
    }

    /// Evaluate this rule against `source` (already known to be this rule's
    /// `language`), returning any violations. A selector/predicate that does not
    /// compile yields no violations here — the failure is reported by [`errors`]
    /// so the guard can fail open loudly rather than under-enforce silently.
    fn violations(&self, source: &str, rel: &str) -> Vec<RuleViolation> {
        let Ok(captures) = run_query(source, &self.language, &self.query) else {
            return Vec::new();
        };
        let Ok(pattern) = regex::Regex::new(&self.pattern) else {
            return Vec::new();
        };
        let gate = match &self.gate {
            Some(g) => match regex::Regex::new(g) {
                Ok(re) => Some(re),
                Err(_) => return Vec::new(),
            },
            None => None,
        };
        let gated: Vec<&Capture> = captures
            .iter()
            .filter(|c| gate.as_ref().is_none_or(|g| g.is_match(&c.text)))
            .collect();

        match self.match_type {
            MatchType::MustMatch => gated
                .iter()
                .filter(|c| !pattern.is_match(&c.text))
                .map(|c| self.violation(rel, Some(c)))
                .collect(),
            MatchType::MustNotMatch => gated
                .iter()
                .filter(|c| pattern.is_match(&c.text))
                .map(|c| self.violation(rel, Some(c)))
                .collect(),
            MatchType::MustExist => {
                if gated.iter().any(|c| pattern.is_match(&c.text)) {
                    Vec::new()
                } else {
                    vec![self.violation(rel, None)]
                }
            }
        }
    }

    /// Build the model-facing violation for `capture` (or a file-level one when
    /// `None`, as [`MatchType::MustExist`] produces).
    fn violation(&self, rel: &str, capture: Option<&Capture>) -> RuleViolation {
        let explanation = self
            .message
            .clone()
            .unwrap_or_else(|| self.match_type.default_explanation(&self.pattern));
        let (location, offending) = match capture {
            Some(c) => (
                format!("{rel}:{}", c.start_line),
                format!(" Offending text: `{}`.", truncate(&c.text)),
            ),
            None => (rel.to_string(), String::new()),
        };
        RuleViolation {
            rule: self.name.clone(),
            message: format!(
                "yupana: {location}: rule `{}` — {explanation}.{offending} \
                 ({} tier)",
                self.name,
                Tier::TreeSitter.as_str()
            ),
        }
    }
}

/// Every rule whose selector, predicate, gate, or path glob does not compile,
/// as `(rule_name, reason)`. A rule set with such an entry is misconfigured and
/// the guard says so rather than quietly under-enforcing (the `glob_errors`
/// discipline, applied to rules).
///
/// A rule for a language this build cannot parse is NOT an error — it simply
/// never matches a file, so it is skipped, not reported.
#[must_use]
pub fn errors(rules: &[Rule]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for rule in rules {
        // Compiling against an empty source validates the query without needing a
        // real buffer; only a genuine query-compile failure is an error here.
        if let Err(Error::Parse(e)) = run_query("", &rule.language, &rule.query) {
            out.push((rule.name.clone(), format!("selector: {e}")));
        }
        if let Err(e) = regex::Regex::new(&rule.pattern) {
            out.push((rule.name.clone(), format!("predicate regex: {e}")));
        }
        if let Some(gate) = &rule.gate {
            if let Err(e) = regex::Regex::new(gate) {
                out.push((rule.name.clone(), format!("gate regex: {e}")));
            }
        }
        for pattern in &rule.applies_to {
            if let Err(e) = glob::Pattern::new(pattern) {
                out.push((
                    rule.name.clone(),
                    format!("applies_to glob `{pattern}`: {e}"),
                ));
            }
        }
    }
    out
}

/// Evaluate every rule that governs `rel` and is written for `language` against
/// `source`, returning all violations. Rules for other languages or other paths
/// are skipped.
#[must_use]
pub fn evaluate(rules: &[Rule], source: &str, language: &str, rel: &str) -> Vec<RuleViolation> {
    rules
        .iter()
        .filter(|r| r.language == language && r.applies(rel))
        .flat_map(|r| r.violations(source, rel))
        .collect()
}

/// The first line of `text`, capped so a long comment does not flood the model.
fn truncate(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(79).collect::<String>())
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, match_type: MatchType, pattern: &str, gate: Option<&str>) -> Rule {
        Rule {
            name: name.to_string(),
            language: "rust".to_string(),
            query: "(line_comment) @c".to_string(),
            gate: gate.map(str::to_string),
            match_type,
            pattern: pattern.to_string(),
            applies_to: Vec::new(),
            message: None,
            class: None,
            verification_point: None,
            backoff_formula: None,
        }
    }

    const TICKET: &str = r"\b[A-Z]+-[0-9]+\b";

    #[test]
    fn todo_without_a_ticket_is_flagged_and_with_one_passes() {
        // The awkward-for-a-linter rule: a TODO must cite a ticket.
        let rules = vec![rule(
            "todo-needs-ticket",
            MatchType::MustMatch,
            TICKET,
            Some(r"\bTODO\b"),
        )];

        let bad = "// TODO: wire this up\nfn f() {}\n";
        let violations = evaluate(&rules, bad, "rust", "src/a.rs");
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "todo-needs-ticket");
        assert!(violations[0].message.contains("src/a.rs:1"));

        let good = "// TODO(ABC-123): wire this up\nfn f() {}\n";
        assert!(evaluate(&rules, good, "rust", "src/a.rs").is_empty());

        // A non-TODO comment is not gated in, so it is never asked for a ticket.
        let plain = "// just a note\nfn f() {}\n";
        assert!(evaluate(&rules, plain, "rust", "src/a.rs").is_empty());
    }

    #[test]
    fn a_ticket_in_a_comment_is_flagged_and_without_one_passes() {
        // The opposite direction: ticket ids belong in commits, not comments.
        let rules = vec![rule(
            "no-ticket-in-comment",
            MatchType::MustNotMatch,
            TICKET,
            None,
        )];

        let bad = "// see ABC-123 for context\nfn f() {}\n";
        let violations = evaluate(&rules, bad, "rust", "src/a.rs");
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("ABC-123"));

        let good = "// see the linked design doc\nfn f() {}\n";
        assert!(evaluate(&rules, good, "rust", "src/a.rs").is_empty());
    }

    #[test]
    fn must_exist_flags_a_file_missing_the_required_comment() {
        let rules = vec![rule(
            "needs-license-header",
            MatchType::MustExist,
            r"SPDX-License-Identifier",
            None,
        )];
        let missing = "fn f() {}\n";
        let violations = evaluate(&rules, missing, "rust", "src/a.rs");
        assert_eq!(violations.len(), 1);
        // A file-level violation names the file, not a line.
        assert!(violations[0].message.contains("src/a.rs:"));
        assert!(!violations[0].message.contains("src/a.rs:1"));

        let present = "// SPDX-License-Identifier: MIT\nfn f() {}\n";
        assert!(evaluate(&rules, present, "rust", "src/a.rs").is_empty());
    }

    #[test]
    fn a_rule_only_evaluates_its_own_language() {
        let rules = vec![rule(
            "no-ticket-in-comment",
            MatchType::MustNotMatch,
            TICKET,
            None,
        )];
        // Same text, but presented as a different language: the rust rule is skipped.
        assert!(evaluate(&rules, "// ABC-123\n", "python", "a.py").is_empty());
    }

    #[test]
    fn applies_to_scopes_a_rule_to_matching_paths() {
        let mut r = rule(
            "no-ticket-in-comment",
            MatchType::MustNotMatch,
            TICKET,
            None,
        );
        r.applies_to = vec!["src/**".to_string()];
        let rules = vec![r];
        let bad = "// ABC-123\n";
        assert!(!evaluate(&rules, bad, "rust", "src/a.rs").is_empty());
        // Outside the glob, the rule does not apply.
        assert!(evaluate(&rules, bad, "rust", "vendor/a.rs").is_empty());
    }

    #[test]
    fn a_custom_message_overrides_the_default_explanation() {
        let mut r = rule(
            "no-ticket-in-comment",
            MatchType::MustNotMatch,
            TICKET,
            None,
        );
        r.message = Some("keep ticket refs out of source comments".to_string());
        let violations = evaluate(&[r], "// ABC-123\n", "rust", "a.rs");
        assert!(violations[0]
            .message
            .contains("keep ticket refs out of source comments"));
    }

    #[test]
    fn a_malformed_selector_or_predicate_is_reported_not_silently_ignored() {
        let bad_query = Rule {
            query: "(nonexistent_node) @x".to_string(),
            ..rule("bad-selector", MatchType::MustNotMatch, TICKET, None)
        };
        let bad_regex = Rule {
            pattern: "(".to_string(),
            ..rule("bad-predicate", MatchType::MustNotMatch, "(", None)
        };
        let bad_glob = Rule {
            applies_to: vec!["src/[".to_string()],
            ..rule("bad-glob", MatchType::MustNotMatch, TICKET, None)
        };
        let errs = errors(&[bad_query, bad_regex, bad_glob]);
        let names: Vec<&str> = errs.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"bad-selector"));
        assert!(names.contains(&"bad-predicate"));
        assert!(names.contains(&"bad-glob"));

        // ... and a malformed rule contributes no violations (it cannot pass as clean).
        assert!(evaluate(
            &[Rule {
                query: "(nonexistent_node) @x".to_string(),
                ..rule("bad", MatchType::MustNotMatch, TICKET, None)
            }],
            "// ABC-123\n",
            "rust",
            "a.rs"
        )
        .is_empty());
    }

    #[test]
    fn match_type_parses_from_kebab_case_toml() {
        #[derive(Deserialize)]
        struct W {
            m: MatchType,
        }
        assert_eq!(
            toml::from_str::<W>("m = \"must-not-match\"").unwrap().m,
            MatchType::MustNotMatch
        );
        assert!(toml::from_str::<W>("m = \"must_not_match\"").is_err());
    }
}
