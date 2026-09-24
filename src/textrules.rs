//! Text-tier edit policies — language-INDEPENDENT rules over the raw text an
//! edit introduces (aegis-m9ln, consuming aegis-mqnl's rule catalogue).
//!
//! The structural rule plane ([`crate::rules`]) pairs a tree-sitter Selector
//! with a regex Predicate, which makes it precise — and language-GATED: a file
//! whose extension has no grammar gets no rule evaluation at all. That gate is
//! correct for structural rules and disqualifying for the first governed rule
//! this stack shipped: "internal identifiers must not enter public-remote
//! repos" (quipu `aegis:InternalIdentifierPattern`, aegis-mqnl). The identifiers
//! that actually leaked were in `.md`, `.yml` and workflow files — exactly the
//! extensions the structural plane skips. A text rule has no Selector: its
//! evidence is the raw text the edit ADDS, in any file.
//!
//! Same disciplines as [`crate::rules`], deliberately:
//! - INTRODUCED TEXT ONLY. An agent answers for what it writes, never for
//!   pre-existing debt in the file (a dirty file must not brick every edit to
//!   it — mqnl's own design constraint, 129+ pre-existing hits measured).
//! - Pure, no I/O. The projection ([`crate::project`]) fetches; this evaluates.
//! - A rule that does not compile is surfaced by [`errors`], never silently
//!   "nothing matched" — under-enforcement must be loud.
//!
//! Per-rule tier (`aegis:enforcementTier`): `block` or `warn`. The tier is the
//! RULE's severity, from the graph; the local `[yupana.policy] mode` stays the
//! host's enforcement ceiling (hac0: one tiering vocabulary — Mode — and the
//! tier is data that maps into it, never a second local knob).

use serde::{Deserialize, Serialize};

/// A rule's governed severity, from `aegis:enforcementTier`. The string forms
/// match the graph so a projected rule round-trips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextTier {
    /// A match must stop the edit (under an enforcing mode).
    Block,
    /// A match must be surfaced to the model, never blocked.
    Warn,
}

/// One text rule: a Predicate regex over introduced text, any language, any
/// file — minus the rule's own exemptions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRule {
    /// Stable identifier (the graph entity's IRI tail), used in verdicts.
    pub name: String,
    /// Human label (`rdfs:label`), shown in the verdict when present.
    #[serde(default)]
    pub label: Option<String>,
    /// The Predicate regex (`aegis:regex`). RE2-compatible by the catalogue's
    /// own contract, so every consumer runs the identical string.
    pub pattern: String,
    /// Governed severity (`aegis:enforcementTier`).
    pub tier: TextTier,
    /// What the pattern identifies (`aegis:identifierClass`, e.g. `hostname`).
    #[serde(default)]
    pub class: Option<String>,
    /// Paths where this rule deliberately does not apply
    /// (`aegis:exemptPathRegex`) — e.g. the ratchet tests that must name the
    /// forbidden tokens to forbid them. Tested against the repo-relative path.
    #[serde(default)]
    pub exempt_path_regex: Option<String>,
    /// Repos where this rule deliberately does not apply (`aegis:exemptRepo`,
    /// aegis-40j2pq): e.g. "infra calls outside goldblum" exempts goldblum.
    /// Matched against the edited file's `origin` repo name, so every worktree
    /// of that repo is exempt. An unresolvable repo is NOT exempt: failing
    /// toward the warning, like a malformed `exempt_path_regex`.
    #[serde(default)]
    pub exempt_repos: Vec<String>,
    /// A literal marker that exempts a match on the same introduced line
    /// (`aegis:exemptLineMarker`), for text that names a pattern on purpose,
    /// such as an incident runbook. Literal, not a regex, so it stays explicit
    /// and greppable (RE2 has no lookaround to fold it into the pattern).
    #[serde(default)]
    pub exempt_line_marker: Option<String>,
    /// Why the rule exists (`rdfs:comment`); carried into the verdict so a
    /// refusal explains itself instead of citing an opaque rule id.
    #[serde(default)]
    pub rationale: Option<String>,
}

/// A single text-rule violation, with the model-facing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextViolation {
    /// The rule that fired.
    pub rule: String,
    /// The rule's governed severity.
    pub tier: TextTier,
    /// Model-facing explanation: what matched, why it is forbidden, what to do.
    pub message: String,
    /// The exact token that matched.
    ///
    /// Kept as a FIELD and not only inside `message`, because the spool needs it
    /// as data: adjudicating whether a block was right means asking "was THIS
    /// string really an internal identifier in THAT repo", and a reviewer who has
    /// to regex it back out of prose is reading the wrong artifact (aegis-mqnl —
    /// wu adjudicated 16 blocks from the repos because the record did not carry
    /// it).
    pub matched: String,
}

/// The distinct matched tokens across `violations`, first-seen order.
///
/// One fact per token, not one per occurrence — the same de-duplication
/// `violations()` already applies within a single rule, extended across rules so
/// the spool row reads as a set.
///
/// Lives here rather than at the call site because it is a property of
/// `TextViolation`, and because `rule_planes.rs` is under a size ratchet that
/// only lets it shrink.
#[must_use]
pub fn distinct_matched(violations: &[TextViolation]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for v in violations {
        if !seen.contains(&v.matched) {
            seen.push(v.matched.clone());
        }
    }
    seen
}

impl TextRule {
    /// Whether this rule governs `rel`. A rule applies everywhere EXCEPT paths
    /// its own `exempt_path_regex` names. A malformed exemption regex exempts
    /// NOTHING (the rule still applies — failing toward enforcement) and is
    /// surfaced by [`errors`] so the misconfiguration is loud, not laundered
    /// into silent over- or under-enforcement.
    #[must_use]
    pub fn applies(&self, rel: &str) -> bool {
        match &self.exempt_path_regex {
            Some(exempt) => match regex::Regex::new(exempt) {
                Ok(re) => !re.is_match(rel),
                Err(_) => true,
            },
            None => true,
        }
    }

    /// Evaluate this rule against the text an edit introduces. Every distinct
    /// matched token is one violation, so the verdict can name exactly what
    /// tripped it — "something matched" is not actionable, "`db.lan` at
    /// offset 14" is. A pattern that does not compile yields no violations
    /// here; [`errors`] reports it and the guard fails open loudly.
    #[must_use]
    pub fn violations(&self, introduced: &str, rel: &str) -> Vec<TextViolation> {
        self.violations_in(introduced, rel, None)
    }

    /// Whether this rule governs an edit to `rel` in the repo named `repo`
    /// (`None` = the repo could not be resolved, which exempts nothing).
    #[must_use]
    pub fn applies_in(&self, rel: &str, repo: Option<&str>) -> bool {
        let repo_exempt =
            repo.is_some_and(|r| self.exempt_repos.iter().any(|e| e.eq_ignore_ascii_case(r)));
        !repo_exempt && self.applies(rel)
    }

    /// [`TextRule::violations`], knowing which repo the edited file is in.
    #[must_use]
    pub fn violations_in(
        &self,
        introduced: &str,
        rel: &str,
        repo: Option<&str>,
    ) -> Vec<TextViolation> {
        if !self.applies_in(rel, repo) {
            return Vec::new();
        }
        let Ok(re) = regex::Regex::new(&self.pattern) else {
            return Vec::new();
        };
        // Distinct matches, first-seen order: `db.lan` appearing nine times
        // in one edit is one fact to tell the model, not nine lines of it.
        let mut seen: Vec<&str> = Vec::new();
        for m in re.find_iter(introduced) {
            if self.marked_line(introduced, m.start()) {
                continue;
            }
            if !seen.contains(&m.as_str()) {
                seen.push(m.as_str());
            }
        }
        seen.into_iter()
            .map(|token| TextViolation {
                rule: self.name.clone(),
                tier: self.tier,
                message: self.message_for(token),
                matched: token.to_string(),
            })
            .collect()
    }

    /// Whether the introduced line holding byte `at` carries this rule's
    /// exemption marker.
    fn marked_line(&self, introduced: &str, at: usize) -> bool {
        let Some(marker) = self.exempt_line_marker.as_deref() else {
            return false;
        };
        let start = introduced[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = introduced[at..]
            .find('\n')
            .map_or(introduced.len(), |i| at + i);
        introduced[start..end].contains(marker)
    }

    /// The model-facing message for one matched token: names the token, the
    /// rule, its class, and its rationale — a refusal that explains itself.
    fn message_for(&self, token: &str) -> String {
        let what = self.label.as_deref().unwrap_or(&self.name);
        let class = self
            .class
            .as_deref()
            .map(|c| format!(" ({c})"))
            .unwrap_or_default();
        let why = self
            .rationale
            .as_deref()
            .map(|r| format!(" {r}"))
            .unwrap_or_default();
        format!(
            "governed text rule `{}`: the edit introduces `{token}`{class} — {what}.{why}",
            self.name
        )
    }
}

/// Compile problems across a text-rule set: `(rule name, why)` per broken
/// pattern or exemption. Same contract as [`crate::rules::errors`] — a rule
/// set that does not compile must be a loud fail-open, never a silent
/// under-enforcement.
#[must_use]
pub fn errors(rules: &[TextRule]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for rule in rules {
        if let Err(e) = regex::Regex::new(&rule.pattern) {
            out.push((rule.name.clone(), format!("pattern does not compile: {e}")));
        }
        if let Some(exempt) = &rule.exempt_path_regex {
            if let Err(e) = regex::Regex::new(exempt) {
                out.push((
                    rule.name.clone(),
                    format!("exemptPathRegex does not compile (rule applies EVERYWHERE): {e}"),
                ));
            }
        }
    }
    out
}

/// Evaluate every rule against the introduced text. Language-independent by
/// construction: there is no grammar and no extension gate anywhere on this
/// path — a `.yml` edit is judged exactly like a `.rs` one.
#[must_use]
pub fn evaluate(rules: &[TextRule], introduced: &str, rel: &str) -> Vec<TextViolation> {
    evaluate_in(rules, introduced, rel, None)
}

/// [`evaluate`], knowing the edited file's repo so `aegis:exemptRepo` can apply.
#[must_use]
pub fn evaluate_in(
    rules: &[TextRule],
    introduced: &str,
    rel: &str,
    repo: Option<&str>,
) -> Vec<TextViolation> {
    rules
        .iter()
        .flat_map(|r| r.violations_in(introduced, rel, repo))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lan_rule() -> TextRule {
        TextRule {
            name: "pattern_internal-lan-host".into(),
            label: Some("internal .lan hostname".into()),
            pattern: r"\b[a-z0-9][a-z0-9-]*\.lan\b".into(),
            tier: TextTier::Block,
            class: Some("hostname".into()),
            exempt_path_regex: Some(r"(^|/)no_internal_identifiers\.rs$".into()),
            exempt_repos: Vec::new(),
            exempt_line_marker: None,
            rationale: Some("Maps the private estate.".into()),
        }
    }

    fn bead_rule() -> TextRule {
        TextRule {
            name: "pattern_bead-reference".into(),
            label: Some("bead reference".into()),
            pattern: r"\b(?:aegis|hq)-[a-z0-9]{3,6}\b".into(),
            tier: TextTier::Warn,
            class: None,
            exempt_path_regex: None,
            exempt_repos: Vec::new(),
            exempt_line_marker: None,
            rationale: None,
        }
    }

    #[test]
    fn a_forbidden_token_in_any_file_type_is_a_violation() {
        // The defining property: the structural plane skips .md/.yml (no
        // grammar); the text plane must not. These are the extensions the
        // measured leaks actually used.
        for rel in ["README.md", "deploy.yml", "notes.txt", "src/a.rs"] {
            let v = lan_rule().violations("host: db.lan\n", rel);
            assert_eq!(v.len(), 1, "must fire in {rel}");
            assert!(v[0].message.contains("`db.lan`"));
            assert!(v[0].message.contains("internal .lan hostname"));
        }
    }

    #[test]
    fn clean_text_is_silent() {
        assert!(lan_rule()
            .violations("host: db.example.invalid\n", "a.md")
            .is_empty());
    }

    #[test]
    fn an_exempt_path_is_not_judged() {
        // The ratchet test must be able to NAME the tokens it forbids.
        let v = lan_rule().violations("assert db.lan", "src/no_internal_identifiers.rs");
        assert!(v.is_empty());
        // ...but the same content anywhere else still fires.
        assert_eq!(
            lan_rule().violations("assert db.lan", "src/lib.rs").len(),
            1
        );
    }

    #[test]
    fn a_malformed_exemption_exempts_nothing_and_is_reported() {
        let mut rule = lan_rule();
        rule.exempt_path_regex = Some("([unclosed".into());
        // Failing toward enforcement: the rule still applies everywhere...
        assert_eq!(
            rule.violations("db.lan", "src/no_internal_identifiers.rs")
                .len(),
            1
        );
        // ...and the misconfiguration is loud.
        let errs = errors(&[rule]);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].1.contains("exemptPathRegex"));
    }

    #[test]
    fn a_malformed_pattern_is_an_error_not_a_silent_pass() {
        let mut rule = lan_rule();
        rule.pattern = "([unclosed".into();
        assert!(rule.violations("db.lan", "a.md").is_empty()); // engine yields nothing...
        let errs = errors(&[rule]);
        assert_eq!(
            errs.len(),
            1,
            "...and errors() surfaces it for the loud fail-open"
        );
    }

    #[test]
    fn repeated_tokens_are_one_violation_each_distinct_token() {
        let v = lan_rule().violations("db.lan db.lan scm.lan", "a.md");
        assert_eq!(v.len(), 2, "dedup by token, not by occurrence");
    }

    #[test]
    fn tiers_ride_the_violation() {
        let rules = [lan_rule(), bead_rule()];
        let v = evaluate(&rules, "see aegis-x1y2 on db.lan", "a.md");
        assert_eq!(v.len(), 2);
        assert!(v.iter().any(|x| x.tier == TextTier::Block));
        assert!(v.iter().any(|x| x.tier == TextTier::Warn));
    }

    #[test]
    fn word_boundaries_hold_the_false_positive_line() {
        // The measured trap: an unanchored short node name matched inside a
        // longer word ("acti" sits inside "activation"). Synthetic names,
        // chosen to KEEP that property — a name that is not a substring of
        // anything would leave this test asserting nothing (aegis-wvuhj).
        let node = TextRule {
            name: "pattern_internal-node-name".into(),
            label: None,
            pattern: r"\b(?:alpha|acti)\b".into(),
            tier: TextTier::Block,
            class: None,
            exempt_path_regex: None,
            exempt_repos: Vec::new(),
            exempt_line_marker: None,
            rationale: None,
        };
        assert!(node
            .violations("activation of a private key", "a.md")
            .is_empty());
        assert_eq!(node.violations("deploy to acti", "a.md").len(), 1);
    }

    /// The matched token is carried as DATA, not only inside the prose message.
    ///
    /// Adjudicating a block means asking "was THIS string really an internal
    /// identifier in THAT repo". A reviewer who has to regex the token back out
    /// of a model-facing sentence is reading the wrong artifact — measured on
    /// aegis-mqnl, where 16 blocks were adjudicated by going back to the repos
    /// because the record could not say what tripped.
    #[test]
    fn violations_carry_the_matched_token_deduplicated() {
        let rule = TextRule {
            name: "pattern_probe".into(),
            pattern: r"\bTOKEN-\w+\b".into(),
            tier: TextTier::Block,
            label: Some("probe rule".into()),
            class: None,
            exempt_path_regex: None,
            exempt_repos: Vec::new(),
            exempt_line_marker: None,
            rationale: None,
        };
        let v = rule.violations("call TOKEN-aaa then TOKEN-aaa then TOKEN-bbb", "a.md");
        let tokens: Vec<&str> = v.iter().map(|x| x.matched.as_str()).collect();
        assert_eq!(
            tokens,
            vec!["TOKEN-aaa", "TOKEN-bbb"],
            "one violation per DISTINCT token, first-seen order"
        );
        // and the message still names it, so neither representation regressed
        assert!(v[0].message.contains("TOKEN-aaa"));
    }

    fn infra_rule() -> TextRule {
        TextRule {
            exempt_repos: vec!["goldblum".into()],
            exempt_line_marker: Some("goldblum-iac: incident-runbook".into()),
            ..bead_rule()
        }
    }

    /// aegis-40j2pq arms: fires in a crew repo, silent in goldblum (any
    /// worktree shares the origin name), and an UNRESOLVED repo still fires.
    #[test]
    fn exempt_repo_silences_only_the_named_repo() {
        let r = infra_rule();
        let text = "see aegis-abc1 for context";
        assert_eq!(r.violations_in(text, "x.sh", Some("aegis")).len(), 1);
        assert!(r.violations_in(text, "x.sh", Some("goldblum")).is_empty());
        assert!(r.violations_in(text, "x.sh", Some("Goldblum")).is_empty());
        assert_eq!(
            r.violations_in(text, "x.sh", None).len(),
            1,
            "unknown repo is not exempt"
        );
        assert_eq!(evaluate_in(&[r], text, "x.sh", Some("aegis")).len(), 1);
    }

    #[test]
    fn the_line_marker_exempts_its_own_line_only() {
        let r = infra_rule();
        let marked = "aegis-abc1  # goldblum-iac: incident-runbook\nnothing here";
        assert!(r.violations_in(marked, "x.md", Some("aegis")).is_empty());
        let other_line = "# goldblum-iac: incident-runbook\nthen aegis-abc1 unmarked";
        assert_eq!(r.violations_in(other_line, "x.md", Some("aegis")).len(), 1);
    }
}
