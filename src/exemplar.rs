//! Exemplar extraction — from an observed denial to a drafted policy's raw
//! material (bobbin-9k3; quipu `docs/design/policy-by-example.md`, sequencing
//! step 2).
//!
//! A human points at a concrete instance — a verdict-spool entry, or a
//! path + offending text — and wants "never do this again" turned into a
//! governed rule without hand-authoring Turtle. Yupana's half is mechanical
//! extraction: it already knows the structural context the offending text
//! lived in (`line_comment`, string literal, identifier, …), so it emits
//!
//! - the **Selector draft** — the enclosing node kind as a tree-sitter query
//!   (`(line_comment) @c`), with its language;
//! - **predicate candidates at each viable tier**, weakest authority to
//!   strongest claim:
//!   1. *exact* — the specific offending token(s), for membership checking
//!      (`tree-sitter+graph`, the only hard-capable tier);
//!   2. *lexical* — a generated narrowing pattern, OFFERED FOR HUMAN
//!      APPROVAL and never self-asserted as authority;
//!   3. *embedding* — the exemplar's embedding as a similarity anchor
//!      (pinned model, suggested threshold; the real threshold comes from
//!      quipu's backtest, and the candidate says so).
//!
//! The output feeds quipu's drafting scaffold; quipu's definition-time
//! placement check remains the refusal authority — nothing emitted here is a
//! policy, only the filled-in form a human edits.

use serde::Serialize;

/// The Selector draft: where the offending text structurally lived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectorDraft {
    /// The grammar the query targets.
    pub language: String,
    /// The enclosing node kind (`line_comment`, `string_literal`, …).
    pub node_kind: String,
    /// The tree-sitter query the drafting scaffold starts from.
    pub query: String,
}

/// One predicate candidate, tier-tagged (FR-3 discipline: a lexical guess and
/// an exact membership check must be impossible to mistake for one another).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PredicateCandidate {
    /// `tree-sitter+graph` (exact membership) / `lexical` / `embedding`.
    pub tier: String,
    /// What this candidate may claim: `membership` (hard-capable),
    /// `human-approval-required`, or `similarity-anchor` (advisory/escalate
    /// placement only).
    pub authority: String,
    /// The exact token(s), for the membership tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Vec<String>>,
    /// The generated narrowing pattern, for the lexical tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// The pinned embedding-model id, for the similarity tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    /// The exemplar's embedding — the similarity anchor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// A starting threshold for the similarity tier. A SUGGESTION only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_threshold: Option<f32>,
    /// What the human must know before accepting this candidate.
    pub note: String,
}

/// Everything extracted from one exemplar.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExemplarDraft {
    /// The offending text, verbatim.
    pub offending: String,
    /// Structural context, when the language is parseable and the text was
    /// found in the source. `None` is honest absence (e.g. a `.md` file with
    /// no grammar) — the text-plane candidates below still stand.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<SelectorDraft>,
    /// Predicate candidates, one per viable tier.
    pub candidates: Vec<PredicateCandidate>,
    /// Provenance tier of the extraction itself (FR-3).
    pub tier: String,
}

/// Extract a draft from an exemplar: the `offending` text as it appeared, and
/// (when available) the `source` of the file it appeared in with its
/// `language`, so the Selector can name the enclosing node.
#[must_use]
pub fn extract(offending: &str, source: Option<&str>, language: Option<&str>) -> ExemplarDraft {
    let selector = match (source, language) {
        (Some(source), Some(language)) => {
            crate::extract::query::enclosing_node_kind(source, language, offending)
                .ok()
                .flatten()
                .map(|node_kind| SelectorDraft {
                    language: language.to_string(),
                    query: format!("({node_kind}) @c"),
                    node_kind,
                })
        }
        _ => None,
    };

    let tokens: Vec<String> = offending
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();

    let candidates = vec![
        PredicateCandidate {
            tier: "tree-sitter+graph".to_string(),
            authority: "membership".to_string(),
            tokens: Some(tokens.clone()),
            pattern: None,
            embedding_model: None,
            embedding: None,
            suggested_threshold: None,
            note: "exact membership against a governed set — the only \
                   hard-capable tier"
                .to_string(),
        },
        PredicateCandidate {
            tier: "lexical".to_string(),
            authority: "human-approval-required".to_string(),
            tokens: None,
            pattern: Some(narrowing_pattern(&tokens)),
            embedding_model: None,
            embedding: None,
            suggested_threshold: None,
            note: "generated narrowing pattern — a scaffold for the human to \
                   edit, never self-asserted as authority"
                .to_string(),
        },
        PredicateCandidate {
            tier: "embedding".to_string(),
            authority: "similarity-anchor".to_string(),
            tokens: None,
            pattern: None,
            embedding_model: Some(crate::recurrence::MODEL.to_string()),
            embedding: Some(crate::recurrence::embed(offending)),
            suggested_threshold: Some(crate::recurrence::THRESHOLD),
            note: "the exemplar's embedding as a similarity anchor; the \
                   threshold is a starting suggestion — quipu's backtest over \
                   recorded history sets the real one. Advisory or escalate \
                   placement only; similarity never hard-denies"
                .to_string(),
        },
    ];

    ExemplarDraft {
        offending: offending.to_string(),
        selector,
        candidates,
        tier: crate::types::Tier::TreeSitter.as_str().to_string(),
    }
}

/// Generalize the exemplar's tokens into a narrowing pattern: runs of digits
/// become `[0-9]+`, runs of letters `[A-Za-z]+`, casing and separators kept.
/// A SCAFFOLD for the human, deliberately conservative: it narrows to the
/// exemplar's shape rather than guessing a family.
fn narrowing_pattern(tokens: &[String]) -> String {
    let shapes: Vec<String> = tokens
        .iter()
        .map(|token| {
            let mut out = String::from(r"\b");
            let mut last: Option<char> = None;
            for c in token.chars() {
                let class = match c {
                    '0'..='9' => 'd',
                    'a'..='z' | 'A'..='Z' => 'a',
                    other => other,
                };
                if last == Some(class) {
                    continue; // the previous `+` already covers the run
                }
                match class {
                    'd' => out.push_str("[0-9]+"),
                    'a' => out.push_str("[A-Za-z]+"),
                    '-' => out.push('-'),
                    '_' => out.push('_'),
                    other => out.extend(regex_escape(other)),
                }
                last = Some(class);
            }
            out.push_str(r"\b");
            out
        })
        .collect();
    shapes.join("|")
}

fn regex_escape(c: char) -> impl Iterator<Item = char> {
    let needs = matches!(
        c,
        '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|'
    );
    needs.then_some('\\').into_iter().chain(std::iter::once(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selector_names_the_enclosing_node_kind() {
        let source = "fn f() {\n    // see ABC-123 for context\n}\n";
        let draft = extract("ABC-123", Some(source), Some("rust"));
        let selector = draft.selector.expect("a rust comment is parseable");
        assert_eq!(selector.node_kind, "line_comment");
        assert_eq!(selector.query, "(line_comment) @c");
        assert_eq!(selector.language, "rust");
    }

    #[test]
    fn all_three_tiers_are_offered_with_honest_authority() {
        let draft = extract("ABC-123", None, None);
        let tiers: Vec<&str> = draft.candidates.iter().map(|c| c.tier.as_str()).collect();
        assert_eq!(tiers, ["tree-sitter+graph", "lexical", "embedding"]);

        let exact = &draft.candidates[0];
        assert_eq!(exact.authority, "membership");
        assert_eq!(exact.tokens.as_deref(), Some(&["ABC-123".to_string()][..]));

        let lexical = &draft.candidates[1];
        assert_eq!(lexical.authority, "human-approval-required");
        assert_eq!(lexical.pattern.as_deref(), Some(r"\b[A-Za-z]+-[0-9]+\b"));

        let embedding = &draft.candidates[2];
        assert_eq!(embedding.authority, "similarity-anchor");
        assert_eq!(
            embedding.embedding_model.as_deref(),
            Some(crate::recurrence::MODEL)
        );
        assert!(embedding.embedding.is_some());
        assert!(embedding.note.contains("backtest"), "{}", embedding.note);
    }

    #[test]
    fn an_unparseable_context_still_drafts_the_text_tiers() {
        // A .md file has no grammar: the Selector is honestly absent, and the
        // exact/lexical/embedding candidates still stand.
        let draft = extract("db1.gastown.local", None, None);
        assert!(draft.selector.is_none());
        assert_eq!(draft.candidates.len(), 3);
        // Dots split tokens; the membership tier carries the pieces.
        assert_eq!(
            draft.candidates[0].tokens.as_ref().unwrap().len(),
            3,
            "{:?}",
            draft.candidates[0].tokens
        );
    }

    #[test]
    fn extraction_is_tier_tagged_like_every_served_fact() {
        assert_eq!(extract("x", None, None).tier, "treesitter");
    }

    #[test]
    fn the_narrowing_pattern_keeps_structure_and_escapes_metacharacters() {
        let draft = extract("a.b+c", None, None);
        // Tokenization splits on '.' and '+', so the pattern is alternation of
        // letter runs — no raw metacharacters survive into the scaffold.
        assert_eq!(
            draft.candidates[1].pattern.as_deref(),
            Some(r"\b[A-Za-z]+\b|\b[A-Za-z]+\b|\b[A-Za-z]+\b")
        );
    }
}
