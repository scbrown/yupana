//! `graph-pattern` — the compact ASK-style selector language over the generic
//! fact graph (FR-36).
//!
//! ## This is deliberately not SPARQL, and the boundary is the point
//!
//! Yupana is not an RDF store. `selectorLang "sparql"` is RESERVED for policies
//! **Quipu** evaluates, and this module refuses to evaluate one rather than
//! approximating it (see [`super::policy`]). What is implemented here is a
//! fixed, small subset with no inference, no property paths, no `OPTIONAL`, no
//! `UNION`, no aggregation and no negation. If a predicate starts wanting those,
//! that is the signal the policy belongs in Quipu — not the signal to grow this.
//!
//! ## Grammar
//!
//! ```text
//! pattern := clause { '.' clause } [ '|' filter { ',' filter } ]
//! clause  := ?var pair { ';' pair }
//! pair    := ('a' | name) term
//! term    := ?var | name | "string" | number | true | false
//! filter  := ?var ('=' | '!=' | '<' | '<=' | '>' | '>=') literal
//! ```
//!
//! `?b a smac:BaseState ; smac:isBorderBase true` reads as: bind `?b` to nodes
//! whose kind is `smac:BaseState` and whose `isBorderBase`… — **no**. It reads
//! as: whose `smac:isBorderBase` attribute is `true`. **Yupana performs no prefix
//! expansion.** A name is matched against what the adapter ingested, byte for
//! byte, so `smac:garrisonCount` matches an attribute literally called
//! `smac:garrisonCount`. Inventing a prefix map here would create a second,
//! drifting copy of Quipu's, and a silently-unexpanded prefix matches nothing
//! while looking exactly like a pattern that found nothing.
//!
//! ## Attribute or edge
//!
//! A `name` predicate resolves against the subject's ATTRIBUTES first, and only
//! against its outgoing EDGES if it has no such attribute. Outgoing only:
//! direction is information, and matching both ways would bind `?a adjacent_to
//! ?b` to each pair twice, in mirror. An adapter that means a symmetric relation
//! ingests both directions.

use std::collections::BTreeMap;

use super::graph::AttrValue;
use super::overlay::StateView;

/// What a pattern variable is bound to.
#[derive(Debug, Clone, PartialEq)]
pub enum Bound {
    /// A node, by id.
    Node(String),
    /// A scalar attribute value.
    Value(AttrValue),
}

impl Bound {
    /// A short rendering for a finding's detail text.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Bound::Node(id) => id.clone(),
            Bound::Value(v) => v.render(),
        }
    }

    /// The node id, when this is a node binding.
    #[must_use]
    pub fn node_id(&self) -> Option<&str> {
        match self {
            Bound::Node(id) => Some(id.as_str()),
            Bound::Value(_) => None,
        }
    }
}

/// One solution: variable name → what it matched.
pub type Binding = BTreeMap<String, Bound>;

/// A parsed pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    clauses: Vec<Clause>,
    filters: Vec<Filter>,
}

#[derive(Debug, Clone, PartialEq)]
struct Clause {
    subject: String,
    pairs: Vec<(Pred, Term)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Pred {
    /// `a` — the node's kind.
    Kind,
    /// A named attribute or outgoing relation.
    Named(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Term {
    Var(String),
    Lit(AttrValue),
    Name(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
struct Filter {
    var: String,
    op: CmpOp,
    value: AttrValue,
}

/// The result of matching a pattern: its solutions, plus anything that could not
/// be evaluated.
///
/// `warnings` is not decoration. A comparison between a number and a string has
/// no answer, and a guard that folded that into "did not match" would report a
/// `must-match` policy as SATISFIED because its predicate was unevaluable —
/// a silent pass produced by a modelling error. The warning is what lets
/// [`super::guard`] surface it instead.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Matches {
    /// Every solution, in discovery order.
    pub bindings: Vec<Binding>,
    /// Comparisons that had no defined answer, described for a human.
    pub warnings: Vec<String>,
}

impl Matches {
    /// Whether anything matched.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

impl Pattern {
    /// Parse a pattern, or say precisely what is wrong with it.
    ///
    /// A malformed pattern is an ERROR, never an empty match set — the
    /// discipline [`crate::rules::errors`] applies to tree-sitter selectors. A
    /// selector that silently matched nothing would disarm its policy while
    /// reporting a clean board.
    pub fn parse(source: &str) -> Result<Self, String> {
        let tokens = tokenize(source)?;
        parse_tokens(&tokens)
    }

    /// Every variable the pattern binds, in first-appearance order. Used to
    /// report a filter over a variable no clause binds.
    #[must_use]
    pub fn variables(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for clause in &self.clauses {
            for name in std::iter::once(clause.subject.as_str()).chain(
                clause.pairs.iter().filter_map(|(_, t)| match t {
                    Term::Var(v) => Some(v.as_str()),
                    _ => None,
                }),
            ) {
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        out
    }

    /// Filters over variables no clause binds. A filter on an unbound variable
    /// can never hold, so a policy carrying one is inert — reported, not
    /// silently ignored.
    #[must_use]
    pub fn unbound_filters(&self) -> Vec<String> {
        let bound = self.variables();
        self.filters
            .iter()
            .filter(|f| !bound.contains(&f.var.as_str()))
            .map(|f| f.var.clone())
            .collect()
    }

    /// Match this pattern against a composed view.
    #[must_use]
    pub fn eval(&self, view: &StateView<'_>) -> Matches {
        self.eval_seeded(view, &Binding::new())
    }

    /// Match this pattern with `seed`'s variables already bound.
    ///
    /// This is how a predicate is evaluated *for* one selector solution
    /// ([`super::guard`]): the shared variable — `?b` in the addendum's garrison
    /// example — is pinned to the base the selector found, so the predicate
    /// answers "does THIS base keep a garrison", not "does SOME base". Without
    /// seeding, a single compliant node anywhere on the board would satisfy a
    /// `must-match` policy for every node on it.
    #[must_use]
    pub fn eval_seeded(&self, view: &StateView<'_>, seed: &Binding) -> Matches {
        let mut warnings: Vec<String> = Vec::new();
        let mut solutions: Vec<Binding> = vec![seed.clone()];

        for clause in &self.clauses {
            let mut next: Vec<Binding> = Vec::new();
            for binding in &solutions {
                let candidates: Vec<String> = match binding.get(&clause.subject) {
                    Some(Bound::Node(id)) => vec![id.clone()],
                    // Already bound to a scalar: a clause subject must be a
                    // node, so this solution cannot extend. Not a warning — it
                    // is an ordinary non-match of an over-constrained pattern.
                    Some(Bound::Value(_)) => Vec::new(),
                    None => view.nodes().iter().map(|n| n.id.clone()).collect(),
                };
                for id in candidates {
                    let mut candidate = binding.clone();
                    candidate.insert(clause.subject.clone(), Bound::Node(id.clone()));
                    if match_pairs(view, &id, &clause.pairs, &mut candidate) {
                        next.push(candidate);
                    }
                }
            }
            solutions = next;
        }

        for var in self.unbound_filters() {
            warnings.push(format!(
                "filter on `?{var}`, which no clause binds — it can never hold"
            ));
        }

        solutions.retain(|binding| {
            self.filters
                .iter()
                .all(|f| apply_filter(f, binding, &mut warnings))
        });
        // Two solutions differing only in a variable the caller never reads are
        // still two solutions; dedup keeps a finding from being reported twice
        // for one board fact.
        let mut seen: Vec<Binding> = Vec::new();
        for binding in solutions {
            if !seen.contains(&binding) {
                seen.push(binding);
            }
        }
        warnings.sort();
        warnings.dedup();
        Matches {
            bindings: seen,
            warnings,
        }
    }
}

/// Fold every `pred term` pair of one clause against node `id`, extending
/// `binding`. Returns whether the whole clause matched.
fn match_pairs(
    view: &StateView<'_>,
    id: &str,
    pairs: &[(Pred, Term)],
    binding: &mut Binding,
) -> bool {
    let Some(node) = view.node(id) else {
        return false;
    };
    for (pred, term) in pairs {
        let matched = match pred {
            Pred::Kind => unify(
                term,
                &Bound::Value(AttrValue::Str(node.kind.clone())),
                binding,
            ),
            Pred::Named(name) => match node.attrs.get(name) {
                Some(value) => unify(term, &Bound::Value(value.clone()), binding),
                None => view
                    .edges()
                    .into_iter()
                    .filter(|e| e.source == id && &e.relation == name)
                    .any(|e| unify(term, &Bound::Node(e.target.clone()), binding)),
            },
        };
        if !matched {
            return false;
        }
    }
    true
}

/// Unify one term with one observed value, binding a free variable or comparing
/// a fixed term. A bare `name` compares against a node id or a string value —
/// which of the two is decided by what was observed, not by the term.
fn unify(term: &Term, observed: &Bound, binding: &mut Binding) -> bool {
    match term {
        Term::Var(var) => match binding.get(var) {
            Some(existing) => existing == observed,
            None => {
                binding.insert(var.clone(), observed.clone());
                true
            }
        },
        Term::Lit(literal) => matches!(observed, Bound::Value(v) if v == literal),
        Term::Name(name) => match observed {
            Bound::Node(id) => id == name,
            Bound::Value(AttrValue::Str(s)) => s == name,
            Bound::Value(_) => false,
        },
    }
}

/// Apply one filter to one solution, recording a warning when the comparison has
/// no defined answer.
fn apply_filter(filter: &Filter, binding: &Binding, warnings: &mut Vec<String>) -> bool {
    let Some(bound) = binding.get(&filter.var) else {
        return false;
    };
    let Bound::Value(value) = bound else {
        warnings.push(format!(
            "`?{}` is bound to a node, not a value — it cannot be compared with {}",
            filter.var,
            filter.value.render()
        ));
        return false;
    };
    match filter.op {
        CmpOp::Eq => value == &filter.value,
        CmpOp::Ne => value != &filter.value,
        _ => {
            let (Some(left), Some(right)) = (value.as_num(), filter.value.as_num()) else {
                warnings.push(format!(
                    "`?{}` = {} compared with {} by an ordering operator, but ordering is \
                     defined only over numbers — this comparison has no answer and did NOT hold",
                    filter.var,
                    value.render(),
                    filter.value.render()
                ));
                return false;
            };
            match filter.op {
                CmpOp::Lt => left < right,
                CmpOp::Le => left <= right,
                CmpOp::Gt => left > right,
                CmpOp::Ge => left >= right,
                CmpOp::Eq | CmpOp::Ne => unreachable!("handled above"),
            }
        }
    }
}

// The tokenizer and the recursive-descent parser live beside this file so the
// evaluator above stays readable at a glance.
#[path = "pattern_parse.rs"]
mod pattern_parse;
use pattern_parse::{parse_tokens, tokenize};

#[cfg(test)]
#[path = "pattern_test.rs"]
mod pattern_test;
