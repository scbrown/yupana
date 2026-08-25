//! Branch modeling for promoted facts (§9.4, GH #4) — the **qualifier**
//! fallback, and a loud refusal for the named-graph design that is not ours to
//! build yet.
//!
//! ── THE TWO DESIGNS, AND WHY ONLY ONE IS HERE ───────────────────────────────
//! §9.4 specifies named-graph-per-branch as the preferred model and
//! branch-as-qualifier as the zero-Quipu-change fallback. The preferred one is
//! blocked on [quipu#36](https://github.com/scbrown/quipu/issues/36) (quad
//! support) — an external dependency on another repository's core — so
//! `branch_model = "named_graph"` **refuses**, naming that blocker, rather than
//! quietly falling back to the qualifier. A config that looks live and is not is
//! precisely what `config_test.rs`'s guard exists to prevent, and silently
//! serving design B under the name of design A is the same defect with better
//! manners: the operator would believe their branches were partitioned when
//! nothing partitions them.
//!
//! The DEFAULT therefore moved from `"named_graph"` to `"qualifier"` in the same
//! change. The spec's default named the design it wanted, which was right while
//! neither existed; now that exactly one is implementable, defaulting to the
//! other would refuse every promotion out of the box.
//!
//! ── WHAT THE QUALIFIER HONESTLY BUYS, AND WHAT IT DOES NOT ──────────────────
//! Each entity the projection declares gains `bobbin:onBranch "<branch>"`, so
//! `?m bobbin:onBranch "main"` is answerable with no Quipu change — which is the
//! §9.4 fallback's whole promise.
//!
//! It is NOT per-branch structure, and saying so here is the point. Promoted
//! IRIs are deterministic and branch-independent by design (`code/{repo}/{path}`
//! — that is what makes a re-promotion supersede instead of forking), so two
//! branches promoting the same module write the SAME subject and simply
//! accumulate both branch values on it. That answers "which branches is this
//! module on"; it cannot answer "what did the call graph look like on `feature`
//! as opposed to `main`", because the two branches' `calls` edges land on one
//! set of subjects with nothing to tell them apart. Distinguishing them needs
//! either per-branch IRIs (which forks the graph — the failure aegis-o2h97
//! catalogued) or named graphs (quipu#36). This is exactly why §9.4 calls
//! named-graph the preferred design and this the fallback, and why the migration
//! note in `docs/book/src/concepts/promotion.md` is worth reading before
//! depending on the qualifier for more than membership.
//!
//! ── ABSENT, NEVER FAKED ─────────────────────────────────────────────────────
//! A promotion whose branch cannot be determined (a detached HEAD at a bare SHA
//! that is not a branch tip) emits **no** qualifier and says so, rather than
//! guessing `"main"` or writing `"unknown"`. Same rule as FR-3 freshness: a
//! field that is absent is recoverable, a field that is wrong is not.

use crate::errors::{Error, Result};

/// The branch model, as `[yupana.quipu] branch_model` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchModel {
    /// `bobbin:onBranch "<branch>"` on every promoted entity. No Quipu change.
    Qualifier,
    /// `GRAPH bobbin:branch/<b> { … }`. Preferred (§9.4), blocked on quipu#36.
    NamedGraph,
}

/// The predicate the qualifier fallback attaches.
pub const ON_BRANCH: &str = "bobbin:onBranch";

/// The `branch_model` values that mean anything.
const VALUES: &[&str] = &["qualifier", "named_graph"];

/// Parse `[yupana.quipu] branch_model`. An unrecognised value is an ERROR, not a
/// fallback to the default — the same rule `promote_trigger::decide` applies to
/// `promote_on`, and for the same reason: a typo that behaved as the default
/// would be indistinguishable from the key working.
pub fn parse(value: &str) -> Result<BranchModel> {
    match value.trim() {
        "qualifier" => Ok(BranchModel::Qualifier),
        "named_graph" => Ok(BranchModel::NamedGraph),
        other => Err(Error::Config(format!(
            "`[yupana.quipu] branch_model = {other:?}` is not a recognised value. \
             Use one of: {}. Refusing rather than falling back to the default, which \
             would make a typo indistinguishable from the key working.",
            VALUES.join(" | ")
        ))),
    }
}

/// Refuse a branch model nothing implements, naming the blocker.
///
/// Separate from [`qualify`] so a promotion can refuse BEFORE it extracts a
/// tree: a misconfigured `branch_model` should cost a message, not a full
/// projection. [`qualify`] calls it too, so the refusal cannot be skipped by
/// reaching the write through some other path.
pub fn ensure_implemented(model: BranchModel) -> Result<()> {
    if model == BranchModel::NamedGraph {
        return Err(Error::Config(
            "`[yupana.quipu] branch_model = \"named_graph\"` is NOT implemented, and this \
             promotion is refused rather than silently written under the qualifier model — \
             which would leave you believing your branches were partitioned when nothing \
             partitions them.\n  \
             Named-graph-per-branch (spec §9.4) needs quad support in Quipu, tracked at \
             scbrown/quipu#36: a change to Quipu's core (graph column, graph-aware SPARQL, \
             the graph × valid-time × tx-time interaction), not something yupana can supply.\n  \
             Set `branch_model = \"qualifier\"` for the zero-Quipu-change fallback \
             (`bobbin:onBranch` per promoted entity), which is the default."
                .to_string(),
        ));
    }
    Ok(())
}

/// Apply the configured branch model to a Turtle projection.
///
/// `branch` is the branch the promoted commit belongs to, or `None` when it
/// could not be determined — in which case the projection is returned unchanged
/// and the caller says so. Absent beats invented.
///
/// [`BranchModel::NamedGraph`] refuses (via [`ensure_implemented`]) rather than
/// degrading to the qualifier, with the blocker named so the reader is not left
/// to discover quipu#36 for themselves.
pub fn qualify(model: BranchModel, turtle: &str, branch: Option<&str>) -> Result<String> {
    ensure_implemented(model)?;
    let Some(branch) = branch.map(str::trim).filter(|b| !b.is_empty()) else {
        return Ok(turtle.to_string());
    };
    let subjects = typed_subjects(turtle);
    if subjects.is_empty() {
        return Ok(turtle.to_string());
    }
    let mut out = String::with_capacity(turtle.len() + subjects.len() * 96);
    out.push_str(turtle);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    // Its own blank-line-separated block, so `promote_chunk::chunk_turtle` sees
    // an ordinary splittable block rather than one giant statement.
    out.push('\n');
    let branch = escape(branch);
    for iri in subjects {
        out.push_str(&format!("<{iri}> {ON_BRANCH} \"{branch}\" .\n"));
    }
    Ok(out)
}

/// Every subject the projection TYPES, in emission order, deduplicated.
///
/// Keyed off `<iri> a bobbin:` at the start of a line, which is exactly the form
/// `export::render` emits for each of `CodeModule`, `CodeSymbol`, `Document` and
/// `Section` — and for the `GitCommit` node the provenance writer appends. Any
/// entity the projection declares is on the branch that declared it, so this
/// deliberately does not enumerate class names: a class added to the exporter
/// gets qualified without this file needing to hear about it.
fn typed_subjects(turtle: &str) -> Vec<&str> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in turtle.lines() {
        let Some(rest) = line.strip_prefix('<') else {
            continue;
        };
        let Some(gt) = rest.find('>') else { continue };
        let after = rest[gt + 1..].trim_start();
        let types = after
            .strip_prefix('a')
            .is_some_and(|r| r.trim_start().starts_with("bobbin:"));
        if types && seen.insert(&rest[..gt]) {
            out.push(&rest[..gt]);
        }
    }
    out
}

/// Escape a Turtle string literal. A branch name may contain almost anything git
/// allows, `"` and `\` included, and an unescaped one would break the document
/// — not just its own triple.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
#[path = "promote_branch_test.rs"]
mod promote_branch_test;
