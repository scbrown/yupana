//! Entity-grounded predicates — the exact tier of quipu's semantic-grounded
//! edit policies (quipu `docs/design/semantic-grounded-edit-policies.md`,
//! Design A; bobbin-tvn).
//!
//! The shipped ticket rules decide "is this a ticket reference?" with a regex,
//! which is the wrong authority twice over: `ABC-123` in prose about an RFC
//! matches and is denied though no such ticket exists, while a real id the
//! pattern never anticipated sails through. The store already knows what a
//! ticket is — work items are entities with identifiers — so ticket-ness is a
//! **membership question against governed data**, not a lexical shape.
//!
//! Rev 2 of the design: **no regex anywhere.** Candidates are simply the
//! TOKENS of the introduced text; truth is hash-set membership against the
//! projected work-item id set. The id set itself defines what an id looks
//! like: a token grounds iff the graph holds it, and a token that *shapes*
//! like the set's ids (shares an id prefix) but resolves to nothing is a
//! **fabricated reference** — its own violation class, never folded into
//! either neighbor.
//!
//! Evaluation of one rule has three outcomes, not two:
//!
//! | Candidate | Outcome | `must-not-ground` (deny) | `must-ground` (warn) |
//! |---|---|---|---|
//! | resolves to a real work item | grounded | violation | satisfied |
//! | id-shaped, resolves to nothing | unresolvable | fabricated-reference violation | NOT satisfied |
//! | no candidates at all | — | satisfied | violation |
//!
//! A missing grounding projection renders every grounded rule **unevaluated
//! (loud)** — never "empty set, nothing grounds, allow". That is the same
//! typed-non-answer discipline as the rest of the guard.
//!
//! Membership is O(tokens) over a `HashSet`, well inside the 5 ms budget; the
//! projection (fetch + cache) happens in [`crate::project`], never here — this
//! module is pure.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub mod similarity;

/// How a grounded rule reads the three outcomes (`aegis:matchType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroundMatch {
    /// A grounded candidate is a violation (e.g. `no-ticket-in-comment`:
    /// a real ticket named in a comment).
    MustNotGround,
    /// The text must contain at least one grounded candidate (e.g.
    /// `todo-needs-ticket`); no candidate, or only fabricated ones, violates.
    MustGround,
}

/// One entity-grounded rule, projected from quipu's catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundedRule {
    /// Stable identifier (the graph entity's IRI tail), used in verdicts.
    pub name: String,
    /// Human label (`rdfs:label`), shown in the verdict when present.
    #[serde(default)]
    pub label: Option<String>,
    /// How the outcomes map to violations.
    pub match_type: GroundMatch,
    /// Governed severity — same vocabulary as the text plane
    /// (`aegis:enforcementTier`).
    pub tier: crate::textrules::TextTier,
    /// Why the rule exists (`rdfs:comment`), carried into the verdict.
    #[serde(default)]
    pub rationale: Option<String>,
}

/// The projected authoritative work-item id set — the hot-plane half of
/// `aegis:groundingQuery`. Built once per projection; queried per token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundingSet {
    /// Every known work-item identifier, verbatim.
    ids: HashSet<String>,
    /// The id prefixes derived FROM the set (`bobbin-bnq` → `bobbin`): what
    /// makes a non-member token "id-shaped" without any regex. Data-derived,
    /// so the set itself defines what an id looks like.
    prefixes: HashSet<String>,
}

impl GroundingSet {
    /// Build from the projected identifiers.
    #[must_use]
    pub fn new(ids: impl IntoIterator<Item = String>) -> Self {
        let ids: HashSet<String> = ids.into_iter().collect();
        let prefixes = ids
            .iter()
            .filter_map(|id| id.rsplit_once('-').map(|(p, _)| p.to_string()))
            .collect();
        Self { ids, prefixes }
    }

    /// How many identifiers the set holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether the set holds no identifiers. An EMPTY set is still a
    /// projected answer ("the graph has no work items") — distinct from a
    /// MISSING set, which renders rules unevaluated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Classify one token. O(1): two hash lookups.
    fn classify(&self, token: &str) -> Option<Candidate> {
        if self.ids.contains(token) {
            return Some(Candidate::Grounded);
        }
        // Id-shaped: shares a prefix with a real id family but names no
        // member — `bobbin-zzz` when the graph holds `bobbin-*` ids. Case
        // matters: the set is authoritative about its own spelling.
        if let Some((prefix, suffix)) = token.rsplit_once('-') {
            if !suffix.is_empty() && self.prefixes.contains(prefix) {
                return Some(Candidate::Unresolvable);
            }
        }
        None
    }
}

/// What one token turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Candidate {
    Grounded,
    Unresolvable,
}

/// The three-outcome result of grounding one text against the set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grounding {
    /// Tokens that resolved to real work items, deduplicated, in text order.
    pub grounded: Vec<String>,
    /// Id-shaped tokens that resolved to NOTHING — fabricated references,
    /// deduplicated, in text order.
    pub unresolvable: Vec<String>,
}

impl Grounding {
    /// No candidates at all — the third outcome.
    #[must_use]
    pub fn no_candidate(&self) -> bool {
        self.grounded.is_empty() && self.unresolvable.is_empty()
    }
}

/// Tokenize without a regex: maximal runs of `[A-Za-z0-9_-]`, the alphabet of
/// every work-item id family the stack mints. Pure, allocation-light.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .filter(|t| !t.is_empty())
}

/// Ground `text` against `set`: every token classified by O(1) membership.
#[must_use]
pub fn ground(set: &GroundingSet, text: &str) -> Grounding {
    let mut grounded: Vec<String> = Vec::new();
    let mut unresolvable: Vec<String> = Vec::new();
    for token in tokens(text) {
        match set.classify(token) {
            Some(Candidate::Grounded) if !grounded.iter().any(|g| g == token) => {
                grounded.push(token.to_string());
            }
            Some(Candidate::Unresolvable) if !unresolvable.iter().any(|u| u == token) => {
                unresolvable.push(token.to_string());
            }
            _ => {}
        }
    }
    Grounding {
        grounded,
        unresolvable,
    }
}

/// One grounded-rule violation, with the model-facing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundedViolation {
    /// The rule that fired.
    pub rule: String,
    /// The rule's governed severity.
    pub tier: crate::textrules::TextTier,
    /// Model-facing explanation.
    pub message: String,
}

/// Evaluate the grounded rules against `introduced`, three-outcome per rule.
///
/// `set` is `None` when the grounding projection is missing or failed — every
/// rule then reports **unevaluated (loud)** in `unevaluated`, and no
/// violations are produced. Never "nothing grounds, allow".
#[must_use]
pub fn evaluate(
    rules: &[GroundedRule],
    set: Option<&GroundingSet>,
    introduced: &str,
) -> (Vec<GroundedViolation>, Vec<String>) {
    let Some(set) = set else {
        return (Vec::new(), rules.iter().map(|r| r.name.clone()).collect());
    };
    let grounding = ground(set, introduced);
    let mut violations = Vec::new();
    for rule in rules {
        let why = rule
            .rationale
            .as_deref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        match rule.match_type {
            GroundMatch::MustNotGround => {
                if !grounding.grounded.is_empty() {
                    violations.push(GroundedViolation {
                        rule: rule.name.clone(),
                        tier: rule.tier,
                        message: format!(
                            "governed rule `{}`: this edit names real work item(s) {} \
                             where none may be named{why}. [tree-sitter+graph tier: \
                             membership against the projected work-item set]",
                            rule.name,
                            join(&grounding.grounded),
                        ),
                    });
                }
                if !grounding.unresolvable.is_empty() {
                    violations.push(fabricated(rule, &grounding.unresolvable));
                }
            }
            GroundMatch::MustGround => {
                if !grounding.unresolvable.is_empty() {
                    // A hallucinated ticket cannot satisfy the rule — its own
                    // violation class, reported even when a real one is also
                    // present.
                    violations.push(fabricated(rule, &grounding.unresolvable));
                }
                if grounding.no_candidate() {
                    violations.push(GroundedViolation {
                        rule: rule.name.clone(),
                        tier: rule.tier,
                        message: format!(
                            "governed rule `{}`: this edit must reference a tracked \
                             work item, and none was found{why}. \
                             [tree-sitter+graph tier]",
                            rule.name,
                        ),
                    });
                }
            }
        }
    }
    (violations, Vec::new())
}

/// The fabricated-reference violation — the grounding-integrity payoff: an
/// agent that invents an id to satisfy (or trip) a rule is caught by the
/// graph, not by a reviewer. Distinct class, never folded into a neighbor.
fn fabricated(rule: &GroundedRule, unresolvable: &[String]) -> GroundedViolation {
    GroundedViolation {
        rule: format!("{}#fabricated-reference", rule.name),
        tier: rule.tier,
        message: format!(
            "governed rule `{}`: FABRICATED REFERENCE — {} is id-shaped but \
             resolves to no work item in the graph. A reference that resolves \
             to nothing satisfies nothing; cite a real item or drop the \
             citation. [tree-sitter+graph tier]",
            rule.name,
            join(unresolvable),
        ),
    }
}

fn join(items: &[String]) -> String {
    items
        .iter()
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::textrules::TextTier;

    fn set() -> GroundingSet {
        GroundingSet::new(["bobbin-bnq".to_string(), "quipu-3aj".to_string()])
    }

    fn rule(match_type: GroundMatch) -> GroundedRule {
        GroundedRule {
            name: "no-ticket-in-comment".to_string(),
            label: None,
            match_type,
            tier: TextTier::Block,
            rationale: None,
        }
    }

    #[test]
    fn a_real_id_grounds_and_an_invented_one_is_unresolvable_NOT_grounded() {
        let g = ground(&set(), "// fixes bobbin-bnq, see also bobbin-zzz");
        assert_eq!(g.grounded, ["bobbin-bnq"]);
        assert_eq!(g.unresolvable, ["bobbin-zzz"]);
    }

    #[test]
    fn ordinary_prose_produces_no_candidates() {
        // The over-inclusive-regex defect, inverted: hyphenated prose and
        // unknown id families are NOT candidates — the set defines the shape.
        let g = ground(&set(), "// a well-known corner-case, per RFC-9110");
        assert!(g.no_candidate(), "{g:?}");
    }

    #[test]
    fn must_not_ground_fires_on_a_grounded_id_and_names_the_item() {
        let (v, unevaluated) = evaluate(
            &[rule(GroundMatch::MustNotGround)],
            Some(&set()),
            "// see bobbin-bnq",
        );
        assert!(unevaluated.is_empty());
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("bobbin-bnq"), "{}", v[0].message);
    }

    #[test]
    fn a_fabricated_reference_is_its_OWN_violation_class() {
        let (v, _) = evaluate(
            &[rule(GroundMatch::MustNotGround)],
            Some(&set()),
            "// see quipu-999",
        );
        assert_eq!(v.len(), 1);
        assert!(v[0].rule.ends_with("#fabricated-reference"), "{}", v[0].rule);
        assert!(v[0].message.contains("FABRICATED"), "{}", v[0].message);
    }

    #[test]
    fn must_ground_is_satisfied_by_a_real_id_and_violated_by_absence_or_fabrication() {
        let must = [rule(GroundMatch::MustGround)];
        let (v, _) = evaluate(&must, Some(&set()), "TODO: quipu-3aj tracks this");
        assert!(v.is_empty(), "a real citation satisfies: {v:?}");

        let (v, _) = evaluate(&must, Some(&set()), "TODO: fix this later");
        assert_eq!(v.len(), 1, "no candidate violates must-ground");

        // A hallucinated ticket cannot satisfy the rule.
        let (v, _) = evaluate(&must, Some(&set()), "TODO: quipu-999 tracks this");
        assert_eq!(v.len(), 1);
        assert!(v[0].rule.ends_with("#fabricated-reference"));
    }

    #[test]
    fn a_missing_grounding_set_renders_rules_UNEVALUATED_never_satisfied_by_empty() {
        let (v, unevaluated) = evaluate(
            &[rule(GroundMatch::MustNotGround)],
            None,
            "// see bobbin-bnq",
        );
        assert!(v.is_empty(), "no verdict without a set");
        assert_eq!(unevaluated, ["no-ticket-in-comment"], "loud, by name");
    }

    #[test]
    fn an_EMPTY_set_is_a_projected_answer_distinct_from_a_missing_one() {
        let empty = GroundingSet::new([]);
        let (v, unevaluated) = evaluate(
            &[rule(GroundMatch::MustNotGround)],
            Some(&empty),
            "// see bobbin-bnq",
        );
        // The graph genuinely holds no work items: nothing can ground, the
        // rule IS evaluated and satisfied.
        assert!(v.is_empty());
        assert!(unevaluated.is_empty());
    }

    #[test]
    fn membership_is_case_exact_and_dedup_preserves_first_mention() {
        let g = ground(&set(), "bobbin-bnq BOBBIN-BNQ bobbin-bnq quipu-3aj");
        assert_eq!(g.grounded, ["bobbin-bnq", "quipu-3aj"]);
        // BOBBIN-BNQ shares no (case-exact) prefix, so it is not even a
        // candidate — the set is authoritative about its own spelling.
        assert!(g.unresolvable.is_empty());
    }
}
