//! Partition a promoted Turtle projection by the FILE that owns each fact
//! (aegis-8o7r10).
//!
//! ## Why this exists
//!
//! `code-promote` re-promotes the whole 126,141-triple snapshot on any sha move,
//! because promotion writes one producer key per repo (`code:{repo}`) and
//! quipu's retraction primitive is SOURCE-scoped:
//!
//! ```text
//! // quipu src/store/ops.rs plan_source_retraction
//! WHERE t.source = ?1 AND f.g = ?2 AND f.op = 1 AND f.valid_to IS NULL
//! ```
//!
//! `replace_snapshot` therefore means *retract everything this producer key owns,
//! then assert this turtle*. There is no per-entity retract to call, so a SUBSET
//! promote is achieved by making the producer key finer — per file — not by a new
//! quipu capability. Nothing is required from the quipu side.
//!
//! ## Why the turtle is partitioned rather than re-extracted
//!
//! The obvious implementation — hand only the changed files to the exporter — is
//! WRONG, and silently. [`crate::export`] resolves relationships ACROSS the files
//! it is given: calls, imports and doc mentions are produced by matching a
//! reference in one file against a definition in another. Given only the changed
//! files, a changed file's call into an UNCHANGED file does not resolve and is not
//! emitted — and because `replace_snapshot` retracts by absence, the subset write
//! would then RETRACT call edges that are still true.
//!
//! That is the same class of defect as dropping `--replace-snapshot`: a wrong
//! graph rather than a slow one, arrived at from the other side, and invisible
//! unless someone counts edges.
//!
//! So: extract the FULL tree exactly as today (cross-file resolution intact),
//! then partition the emitted facts by owning file and write only the partitions
//! that changed. The local parse cost is unchanged; the store cost drops to the
//! delta, which is the measurable this work is for.
//!
//! ## Ownership
//!
//! * a module owns the facts whose subject is that module, and it owns a FILE via
//!   `bobbin:filePath`;
//! * a symbol's facts belong to the file of the module it is `bobbin:definedIn`;
//! * an EDGE belongs to the file it is asserted ON (the caller/importer), which is
//!   the file whose snapshot would legitimately retract it if the reference
//!   disappeared.
//!
//! ## Unowned facts are REFUSED, never dropped
//!
//! Some promoted facts belong to no file — commit provenance nodes, for one. They
//! cannot be written under a per-file key, and silently omitting them from a
//! subset write would retract them by absence on the next full resync, or leave
//! them stale forever. [`Partition::unowned`] carries them so the caller can
//! refuse rather than discover it later.

use std::collections::BTreeMap;

use crate::errors::{Error, Result};

const ONTO: &str = "http://aegis.gastown.local/ontology/";

/// The Turtle prefix header every partition needs to stand alone.
const PREFIXES: &str = "@prefix bobbin: <http://aegis.gastown.local/ontology/> .\n\
                        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\n";

/// A promoted projection split by owning file.
#[derive(Debug, Default)]
pub struct Partition {
    /// Repo-relative file path -> the Turtle asserting that file's facts.
    pub by_file: BTreeMap<String, String>,
    /// Triples placed per file. Counted AS PARTITIONED rather than re-derived
    /// from the emitted text: the first version counted lines ending in " ."
    /// and silently included the three `@prefix` declarations in every
    /// partition, reporting 15 triples where there were 9. A count used to
    /// claim "facts written dropped to the delta" must not be able to be wrong
    /// in the flattering direction.
    pub counts: BTreeMap<String, usize>,
    /// Subjects whose facts belong to no file, with a count of their triples.
    ///
    /// NOT an error here and NOT silently dropped: this type reports them and the
    /// CALLER decides. A subset promote cannot express them, so a caller that
    /// finds any must fall back to the full resync rather than write a partial
    /// snapshot whose absences would retract them.
    pub unowned: BTreeMap<String, usize>,
}

impl Partition {
    /// Total triples placed in a file partition.
    #[must_use]
    pub fn owned_triples(&self) -> usize {
        self.counts.values().sum()
    }

    /// Total triples belonging to no file.
    #[must_use]
    pub fn unowned_triples(&self) -> usize {
        self.unowned.values().sum()
    }
}

/// Split `turtle` into one Turtle document per owning file.
///
/// Parses rather than pattern-matching the emitter's text: the projection's
/// formatting is an implementation detail of [`crate::export`], and a partitioner
/// keyed on its whitespace would break silently the next time it is prettified.
pub fn partition_by_file(turtle: &str) -> Result<Partition> {
    let triples = parse(turtle)?;

    // Pass 1: module IRI -> file. `bobbin:filePath` is what makes a module a file.
    let mut file_of_module: BTreeMap<String, String> = BTreeMap::new();
    for (s, p, o) in &triples {
        if p == &format!("<{ONTO}filePath>") {
            if let Some(path) = literal(o) {
                file_of_module.insert(s.clone(), path);
            }
        }
    }

    // Pass 2: child IRI -> its file-owning parent's IRI.
    //
    // TWO predicates, not one. A CodeSymbol belongs to its module via
    // `definedIn`; a Section belongs to its document via `inDocument`. Handling
    // only the first left 5,310 triples across 237 doc sections unowned on the
    // real projection — about 10% of it — which the hand-built fixture could
    // never have shown. Documents themselves are already covered by pass 1,
    // because they carry `filePath` exactly as modules do.
    let parents = [format!("<{ONTO}definedIn>"), format!("<{ONTO}inDocument>")];
    let mut module_of_symbol: BTreeMap<String, String> = BTreeMap::new();
    for (s, p, o) in &triples {
        if parents.contains(p) {
            // Stored AS PARSED, angle brackets included: `file_of_module` is
            // keyed by the SUBJECT term in the same form, and a bare-IRI key
            // here would never match it. Terms are compared in one
            // representation throughout — mixing them is how the first version
            // of this silently partitioned nothing.
            if o.starts_with('<') {
                module_of_symbol.insert(s.clone(), o.clone());
            }
        }
    }

    let owner = |subject: &str| -> Option<String> {
        file_of_module.get(subject).cloned().or_else(|| {
            module_of_symbol
                .get(subject)
                .and_then(|m| file_of_module.get(m))
                .cloned()
        })
    };

    let mut partition = Partition::default();
    for (s, p, o) in &triples {
        match owner(s) {
            Some(file) => {
                let file_key_owned = file.clone();
                let body = partition
                    .by_file
                    .entry(file)
                    .or_insert_with(|| PREFIXES.to_string());
                body.push_str(&format!("{s} {p} {o} .\n", s = term(s), p = term(p)));
                *partition.counts.entry(file_key_owned).or_insert(0) += 1;
            }
            None => *partition.unowned.entry(s.clone()).or_insert(0) += 1,
        }
    }
    Ok(partition)
}

/// An empty document for a file whose facts must be RETRACTED.
///
/// A deleted file is promoted as a snapshot with no statements: absence under
/// that producer key is what authorizes the retraction, exactly as
/// `--replace-snapshot` does repo-wide. Returning the prefixes rather than an
/// empty string keeps it a valid Turtle document, so the write path cannot
/// mistake it for "nothing to send".
#[must_use]
pub fn empty_snapshot() -> String {
    PREFIXES.to_string()
}

/// The producer key for one file's snapshot.
///
/// Finer than the repo-wide `code:{repo}` so that `replace_snapshot` retracts
/// exactly this file's prior facts — which IS the subset mechanism.
#[must_use]
pub fn file_key(repo: &str, file: &str) -> String {
    format!("code:{repo}:{file}")
}

/// The producer key for facts that belong to no single file.
///
/// Commit provenance is the case that exists today: `commit → touched entities`
/// nodes carry no `filePath`, so no file's snapshot can own them. Under the
/// repo-wide key they were replaced wholesale on every promote; this key
/// reproduces that exactly rather than quietly changing their lifecycle.
///
/// `::` cannot collide with a real path: git never yields an empty path segment,
/// so no repo-relative file is ever spelled `:provenance`.
#[must_use]
pub fn provenance_key(repo: &str) -> String {
    format!("code:{repo}::provenance")
}

/// Why a particular key is being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The file changed and has facts: replace its snapshot with them.
    Changed,
    /// The file changed and has NO facts in the projection — deleted, or its
    /// content no longer produces any. Its snapshot is replaced with an EMPTY
    /// document, which is what retracts the facts it used to own.
    ///
    /// Kept DISTINCT from `Changed` because the two are indistinguishable in the
    /// payload (an empty partition is just a short one) and they are the two
    /// outcomes an operator most needs told apart: one asserts, the other
    /// retracts everything under that key.
    Retracted,
    /// Facts belonging to no file, carried under [`provenance_key`].
    Unfiled,
}

/// One snapshot write a subset promote will perform.
#[derive(Debug, Clone)]
pub struct SubsetWrite {
    /// Producer key to replace.
    pub key: String,
    /// Repo-relative file the key stands for, or `None` for the unfiled key.
    pub file: Option<String>,
    /// The Turtle document to write.
    pub turtle: String,
    /// Triples in it — 0 for a retraction.
    pub triples: usize,
    /// Whether this key is being asserted, retracted, or carries unfiled facts.
    pub why: Why,
}

/// What a subset promote would do, before any of it is done.
#[derive(Debug, Default)]
pub struct SubsetPlan {
    /// The writes, in a stable order.
    pub writes: Vec<SubsetWrite>,
    /// Subjects whose facts belong to no file, with a triple count — the
    /// content of the [`provenance_key`] write.
    ///
    /// REPORTED rather than silent: these are the facts a per-file key cannot
    /// express, and an operator reading a subset promote's output should be able
    /// to see that they were carried rather than assume it.
    pub unfiled_subjects: BTreeMap<String, usize>,
    /// Files in the projection that did NOT change and are therefore untouched.
    /// The whole point of the exercise, so it is reported rather than implied.
    pub unchanged_files: usize,
}

impl SubsetPlan {
    /// Triples this plan would write.
    #[must_use]
    pub fn triples(&self) -> usize {
        self.writes.iter().map(|w| w.triples).sum()
    }

    /// Keys whose snapshot becomes empty — i.e. pure retractions.
    #[must_use]
    pub fn retractions(&self) -> usize {
        self.writes
            .iter()
            .filter(|w| w.why == Why::Retracted)
            .count()
    }
}

/// Plan the writes for a subset promote.
///
/// `turtle` is the FULL promoted projection, exactly as the repo-wide path would
/// write it — branch-qualified, provenance appended. Partitioning anything less
/// is the wrong-graph defect this module exists to avoid: [`crate::export`]
/// resolves references ACROSS files, so a projection built from the changed
/// files alone omits their edges into unchanged files, and `replace_snapshot`
/// then retracts edges that are still true.
///
/// `changed_files` is the repo-relative path set from the git diff.
///
/// ## Every fact ends up under exactly one key, and none is ever dropped
///
/// Facts belonging to a file go under that file's key. Facts belonging to NO
/// file — commit provenance is the case that exists today — go under
/// [`provenance_key`], which is written on every subset promote. That preserves
/// the invariant the repo-wide key gave for free: one owner per fact. Omitting
/// them instead would leave them stale forever, or retract them on the next full
/// resync.
///
/// ## Every changed file gets a write, including ones with no facts
///
/// A changed file absent from the partition is planned as an EMPTY snapshot, not
/// skipped. Skipping is what leaves a deleted file's entities in the graph
/// forever, and it is the tempting bug because "nothing to send" reads as
/// "nothing to do".
///
/// A changed file this build has no grammar for gets one too, and that is safe
/// in both directions: if it never had facts, replacing an empty snapshot with
/// an empty snapshot is a no-op; if it DID have facts and no longer parses, the
/// retraction is correct — the graph should not keep asserting symbols from a
/// file this build can no longer read.
pub fn plan(repo: &str, turtle: &str, changed_files: &[String]) -> Result<SubsetPlan> {
    let partition = partition_by_file(turtle)?;
    let mut out = SubsetPlan {
        unfiled_subjects: partition.unowned.clone(),
        ..SubsetPlan::default()
    };

    // Sorted and de-duplicated: the write order must not depend on the order git
    // happened to print the diff, or two runs over the same change produce
    // different transaction sequences and a failure part-way through is not
    // reproducible.
    let mut files: Vec<&String> = changed_files.iter().collect();
    files.sort_unstable();
    files.dedup();

    for file in &files {
        let (body, triples, why) = match partition.by_file.get(*file) {
            Some(t) => (
                t.clone(),
                partition.counts.get(*file).copied().unwrap_or(0),
                Why::Changed,
            ),
            None => (empty_snapshot(), 0, Why::Retracted),
        };
        out.writes.push(SubsetWrite {
            key: file_key(repo, file),
            file: Some((*file).clone()),
            turtle: body,
            triples,
            why,
        });
    }

    out.unchanged_files = partition
        .by_file
        .keys()
        .filter(|f| files.binary_search(f).is_err())
        .count();

    // The unfiled key is written UNCONDITIONALLY, including when it is empty.
    //
    // Writing it empty is not a wasted call: it is the retraction that keeps a
    // previous commit's provenance from outliving the commit. Writing it only
    // when non-empty would make provenance accumulate under a key nothing ever
    // clears — the opposite of what the repo-wide snapshot did, and invisible
    // because the extra facts are all true, just no longer current.
    let mut unfiled = PREFIXES.to_string();
    let mut unfiled_triples = 0usize;
    for (s, p, o) in parse(turtle)? {
        if partition.unowned.contains_key(&s) {
            unfiled.push_str(&format!("{s} {p} {o} .\n", s = term(&s), p = term(&p)));
            unfiled_triples += 1;
        }
    }
    out.writes.push(SubsetWrite {
        key: provenance_key(repo),
        file: None,
        turtle: unfiled,
        triples: unfiled_triples,
        why: Why::Unfiled,
    });

    Ok(out)
}

fn parse(turtle: &str) -> Result<Vec<(String, String, String)>> {
    let mut out = Vec::new();
    for triple in oxttl::TurtleParser::new().for_reader(turtle.as_bytes()) {
        let t = triple.map_err(|e| Error::Promote(format!("unparseable projection: {e}")))?;
        out.push((
            t.subject.to_string(),
            t.predicate.to_string(),
            t.object.to_string(),
        ));
    }
    Ok(out)
}

/// The lexical form of a literal term, or `None` when the term is not one.
fn literal(term: &str) -> Option<String> {
    let inner = term.strip_prefix('"')?;
    // Stop at the closing quote so a datatype or language tag is not folded in.
    let end = inner.rfind('"')?;
    Some(inner[..end].to_string())
}

/// Render a term as it must appear in the output. `oxrdf`'s `Display` already
/// emits N-Triples form (`<iri>` / `"literal"`), which is valid Turtle.
fn term(t: &str) -> &str {
    t
}

#[cfg(test)]
#[path = "promote_subset_test.rs"]
mod promote_subset_test;
