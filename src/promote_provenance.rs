//! §9.7's `commit → touched entities` provenance edge, produced INSIDE yupana at
//! promotion time (GH #5).
//!
//! ── WHY THIS IS IN YUPANA AND NOT IN A CRON ─────────────────────────────────
//! The edge already existed in the graph, written hourly by an out-of-tree
//! ingest job, so the OUTCOME was never the gap — §9.7's *placement* was. At
//! promotion time yupana already holds both halves (the commit it is promoting
//! and the entities it just projected), so the edge is a free by-product here
//! and a whole second git walk somewhere else. More to the point, an out-of-tree
//! producer can only guess yupana's IRI scheme, and if it guesses differently
//! the edge points at entities that do not exist. That is not hypothetical — see
//! the divergence note below.
//!
//! ── THE CONTRACT (pinned on GH #5 by the quipu side) ────────────────────────
//! `Bead <--aegis:implements-- GitCommit --aegis:modifies--> CodeModule` is what
//! quipu's `/cooccurrence` joins on. `aegis:` and `bobbin:` are the SAME base
//! IRI (`http://aegis.gastown.local/ontology/`); the prefixes are a convention
//! separating provenance from code structure, so the existing `bobbin:` header
//! the exporter writes serves both.
//!
//! Yupana emits the `GitCommit` node and its `modifies` edges. It does **NOT**
//! emit `implements`, and that is a decision rather than an omission: the
//! commit→work-item link needs a declared project-prefix vocabulary yupana does
//! not hold. The tracker-aware lane (camayoc's `ingest_git_provenance.py`)
//! already owns it, with an abstention rule that was tuned against measured
//! false matches — its first pattern read `work-item`, `pre-push` and
//! `force-push` as work-item ids and wrote paths into the ground of five items
//! that do not exist. Re-deriving that heuristic here is exactly how two lanes
//! drift. The chain still closes, because both predicates join on the COMMIT
//! IRI, not on each other.
//!
//! Commit and module IRIs share the exporter's base. The tracker-aware ingest
//! lane uses that base too; escaping unusual paths can still differ, so this
//! does not claim universal identity across producers.
//!
//! Touched entities have module granularity: a file diff does not prove which
//! symbols changed. Author identity/date and committer identity are separate
//! facts. The CLI also sends the authored `%aI` as `/knot` valid_from, leaving
//! transaction time server assigned and actor as the writing process.

use std::path::Path;

use crate::export::{esc, module_iri, ONTO};

/// The `GitCommit` node and its `modifies` edges for `commit`, or `None` when
/// there is nothing to say.
///
/// `projection` is the Turtle the promotion is about to write, and it is the
/// FILTER: a `modifies` edge is emitted only for a changed path that appears in
/// it as a declared `bobbin:CodeModule`. So the edge can never point at an
/// entity this promotion did not also assert — no dangling reference to a
/// deleted file, no edge to a `.md` or a lockfile that has no `CodeModule`, and
/// nothing that would fail a future `sh:class` tightening.
///
/// Returns `None` when the commit touched no promoted module. A bare commit node
/// with no edges is not the fact §9.7 asks for, and the ingest lane abstains in
/// the same case for the same reason.
pub fn commit_turtle(root: &Path, repo: &str, commit: &str, projection: &str) -> Option<String> {
    let sha = crate::git::resolve_commit(root, commit)?;
    let (author, date) = crate::git::commit_identity(root, &sha)?;
    let committer = crate::git::commit_committer(root, &sha)?;
    let touched: Vec<String> = crate::git::commit_touched_paths(root, &sha)
        .iter()
        .map(|p| module_iri(repo, &p.display().to_string()))
        .filter(|iri| declares_module(projection, iri))
        .collect();
    if touched.is_empty() {
        return None;
    }
    let iri = commit_iri(repo, &sha);
    let short: String = sha.chars().take(12).collect();
    let mut out = String::new();
    out.push_str(&format!(
        "\n<{iri}> a bobbin:GitCommit ;\n    rdfs:label \"{}@{short}\" ;\n    \
         bobbin:hash \"{}\" ;\n    bobbin:repo \"{}\" ;\n    \
         bobbin:author \"{}\" ;\n    bobbin:date \"{}\"^^xsd:dateTime .\n",
        esc(repo),
        esc(&sha),
        esc(repo),
        esc(&author),
        esc(&date),
    ));
    out.push_str(&format!(
        "<{iri}> bobbin:committer \"{}\" .\n",
        esc(&committer)
    ));
    // One STATEMENT per edge rather than one `;`-joined block. The chunker can
    // only split at statement boundaries, so a single block would be
    // unsplittable — and an import-shaped commit touching every file in a large
    // repo would then blow the chunk limit and refuse the whole promotion.
    out.push('\n');
    for module in &touched {
        out.push_str(&format!("<{iri}> bobbin:modifies <{module}> .\n"));
    }
    Some(out)
}

/// Mint the `GitCommit` IRI: `{ONTO}code/{repo}/commit/{sha}`.
///
/// Under the SAME base as `module_iri`, deliberately — see the divergence note
/// in the module docs. Deterministic, so a re-promotion of the same commit
/// supersedes rather than forking, exactly like every other promoted IRI.
fn commit_iri(repo: &str, sha: &str) -> String {
    format!("{ONTO}code/{repo}/commit/{sha}")
}

/// Does `projection` declare `iri` as a `CodeModule`?
///
/// Matched against the exact statement `export::render` emits, so a substring of
/// a longer IRI (a symbol IRI extends its module's with `::…`) cannot satisfy
/// it: the ` a bobbin:CodeModule` suffix pins the whole subject.
fn declares_module(projection: &str, iri: &str) -> bool {
    projection.contains(&format!("<{iri}> a bobbin:CodeModule"))
}

#[cfg(test)]
#[path = "promote_provenance_test.rs"]
mod promote_provenance_test;
