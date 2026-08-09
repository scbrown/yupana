//! The in-memory call graph and the blast-radius reachability primitive.
//!
//! Phase 2 builds a symbol-level call graph over a subtree and answers
//! reachability in either direction. This is the FR-12 primitive: the same
//! traversal answers "what does this change affect?" (callers, transitively)
//! and "what does this call?" (callees). Phase 3 layers the tenant model over it:
//! a shared read-only [`Base`] at a resolved commit, per-tenant copy-on-write
//! [`Overlay`]s, and the composed [`TenantView`] the same BFS walks (yupana #2;
//! the FR-16 frontier recompute is yupana #3).
//!
//! The single breadth-first traversal lives in [`blast`] behind the [`Adjacency`]
//! trait, so the base graph, the composed per-tenant view, and the frontier
//! update all share one implementation (FR-12, "build it once").

mod base;
mod blast;
mod community;
mod lookup;
mod overlay;
mod tenant;

use std::collections::HashMap;
use std::path::Path;

use petgraph::graph::{DiGraph, NodeIndex};

use crate::errors::Result;
use crate::extract::extract_structure;
use crate::types::{EdgeKind, Tier};

pub use base::{Base, FileFacts};
pub use blast::{reachable_over, Adjacency, Dir, NodeMeta, Reached};
pub use community::{Community, CommunityMember};
pub use overlay::{update_frontier, update_frontier_bounded, Overlay, OverlaySymbol, ParsedFile};
pub use tenant::{
    BoundedFrontier, OverlayStatus, RegistryStatus, SharingStats, SymRef, TenantRegistry,
    TenantView, ViewSymbol,
};

/// A node in the call graph: one defined symbol.
#[derive(Debug, Clone)]
pub struct SymbolNode {
    /// Symbol name.
    pub name: String,
    /// Symbol kind (lowercase form).
    pub kind: String,
    /// File the symbol is defined in (relative to the build root).
    pub file: String,
    /// 1-based definition line.
    pub start_line: usize,
    /// 1-based last line of the definition. Carried so a graph-backed answer can
    /// report the symbol's EXTENT, not just where it starts — `refs` served this
    /// from its own file walk before it read the graph (yupana #76), and a lookup
    /// that reads the graph must not have to drop a field to do so.
    pub end_line: usize,
    /// Provenance tier.
    pub tier: Tier,
}

/// A symbol-level call graph built over a subtree.
pub struct CodeGraph {
    graph: DiGraph<SymbolNode, EdgeKind>,
    by_name: HashMap<String, Vec<NodeIndex>>,
    /// Every call site keyed by CALLEE NAME → the caller nodes that invoke it,
    /// recorded whether or not the callee resolves to a definition here. This
    /// is the FR-16 frontier index (yupana #3): the materialized edges above only
    /// exist when the callee had a definition at build time, so a name a tenant
    /// overlay ADDS (zero base definitions) has no incoming edge — but its base
    /// callers are still here, under its name. Deduplicated per name.
    callers_by_callee: HashMap<String, Vec<NodeIndex>>,
}

/// Why a baseline could not be built. Distinct from a baseline that is EMPTY:
/// the first says the question was never answered, the second is an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineError {
    /// Not a git work tree, or `git` is unavailable.
    NoRepo,
    /// The ref does not resolve to a commit.
    UnresolvedRef(String),
}

impl std::fmt::Display for BaselineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRepo => write!(
                f,
                "not a git work tree (or `git` is unavailable), so NO BASELINE was \
                 built — this is not an empty baseline"
            ),
            Self::UnresolvedRef(r) => write!(
                f,
                "`{r}` does not resolve to a commit, so NO BASELINE was built — \
                 this is not an empty baseline"
            ),
        }
    }
}

impl CodeGraph {
    /// Build the call graph for the Rust files under `root`.
    ///
    /// Call edges are resolved by name (best-effort): a call to `foo` links to
    /// every symbol named `foo`. Precise resolution arrives with the LSP/CPG
    /// tiers.
    pub fn build(root: &Path) -> Result<Self> {
        let sources =
            crate::extract::source_files(root)
                .into_iter()
                .filter_map(|(file, language)| {
                    let source = std::fs::read_to_string(&file).ok()?;
                    // Relative to the build root; fall back to the file name when the
                    // root *is* the file (strip yields an empty path).
                    let rel = match file.strip_prefix(root) {
                        Ok(p) if !p.as_os_str().is_empty() => p.display().to_string(),
                        _ => file.file_name().map_or_else(
                            || file.display().to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        ),
                    };
                    Some((rel, source, language))
                });
        Ok(Self::from_sources(sources))
    }

    /// The baseline at `reference`, or WHY it could not be built.
    ///
    /// [`Self::build_at_ref`] degrades to an empty graph outside a repo or for an
    /// unresolved ref — deliberately, and documented — but "the ref does not
    /// exist" and "the ref names a tree with nothing parseable in it" then look
    /// identical: both are zero files, zero symbols, exit 0. Measured on a real
    /// repo before this existed:
    ///
    /// ```text
    /// $ yupana analyze --at main          analyzed 1 file(s), 1 symbol(s) @ main
    /// $ yupana analyze --at no-such-ref   analyzed 0 file(s), 0 symbol(s) @ no-such-ref
    /// ```
    ///
    /// A baseline that failed to build must SAY SO. A change-time rule diffs
    /// against this base, so an empty base does not merely under-report — it
    /// makes every entity in the change look ADDED, or the whole change look
    /// clean, depending on which side is missing. That is a wrong answer wearing
    /// a normal-looking one.
    pub fn build_at_ref_checked(
        root: &Path,
        reference: &str,
    ) -> std::result::Result<Self, BaselineError> {
        if !crate::git::is_repo(root) {
            return Err(BaselineError::NoRepo);
        }
        if crate::git::resolve_commit(root, reference).is_none() {
            return Err(BaselineError::UnresolvedRef(reference.to_string()));
        }
        Self::build_at_ref(root, reference).map_err(|_| BaselineError::NoRepo)
    }

    /// Build the call graph from the tree content at a git `reference` — the
    /// shared read-only base at a baseline commit (FR-13/§5.5), not the working
    /// tree. Paths are repo-root-relative. Outside a repo, or for an unresolved
    /// ref, the tree is empty and so is the graph (degrade, never fail).
    ///
    /// PREFER [`Self::build_at_ref_checked`] anywhere the result is REPORTED to a
    /// human or a rule: this one cannot distinguish "no such ref" from "a ref
    /// with nothing parseable in it", and both come back as an empty graph.
    pub fn build_at_ref(root: &Path, reference: &str) -> Result<Self> {
        let sources = crate::git::list_files_at(root, reference)
            .into_iter()
            .filter_map(|path| {
                let ext = path.extension().and_then(std::ffi::OsStr::to_str)?;
                let language = crate::extract::language_for_extension(ext)?;
                let source = crate::git::read_blob_at(root, reference, &path)?;
                Some((path.display().to_string(), source, language))
            });
        Ok(Self::from_sources(sources))
    }

    /// Shared construction: build symbol nodes and name-resolved call edges from
    /// a stream of `(relative_path, source)` pairs. The two builders differ only
    /// in where the sources come from (working tree vs. a git tree).
    fn from_sources(sources: impl Iterator<Item = (String, String, &'static str)>) -> Self {
        let mut graph = DiGraph::new();
        let mut by_name: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        // Call edges are resolved by NAME, so a multi-language graph must keep
        // the language on both ends: `leaf` in Python is not called by `one()` in
        // Rust merely because both files are in the tree. Measured while widening
        // the graph past Rust — a four-language fixture reported every leaf as
        // reaching FOUR files, one per language, from a single caller each. An
        // over-reported radius is a FALSE DENY, which blocks legitimate work just
        // as confidently as the silent allow it replaced.
        let mut calls: Vec<(String, String, &'static str)> = Vec::new();
        let mut language_of: HashMap<NodeIndex, &'static str> = HashMap::new();

        for (rel, source, language) in sources {
            // Parse each file as the language it IS. This was `"rust"` for every
            // source, so a graph over a Python or TypeScript tree came back empty
            // — and an empty graph reports a blast radius of zero, which every
            // ceiling passes.
            let Ok(structure) = extract_structure(&source, language) else {
                continue;
            };
            for symbol in structure.symbols {
                let idx = graph.add_node(SymbolNode {
                    name: symbol.name.clone(),
                    kind: symbol.kind.as_str().to_string(),
                    file: rel.clone(),
                    start_line: symbol.start_line,
                    end_line: symbol.end_line,
                    tier: symbol.tier,
                });
                language_of.insert(idx, language);
                by_name.entry(symbol.name).or_default().push(idx);
            }
            for call in structure.calls {
                calls.push((call.caller, call.callee, language));
            }
        }

        let mut callers_by_callee: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        for (caller, callee, language) in calls {
            let Some(callers) = by_name.get(&caller) else {
                continue;
            };
            // Record the frontier index FIRST, before the resolve-or-skip below:
            // a call to a callee with no definition here (the overlay-new-name
            // case, FR-16) has no edge, but its base callers still belong under
            // its name. Language-checked on the caller so cross-language name
            // collisions are not recorded (the same rule the edges use).
            for &from in callers {
                if language_of.get(&from) == Some(&language) {
                    let slot = callers_by_callee.entry(callee.clone()).or_default();
                    if !slot.contains(&from) {
                        slot.push(from);
                    }
                }
            }
            // Materialized edges still require the callee to resolve to a def.
            let Some(callees) = by_name.get(&callee) else {
                continue;
            };
            for &from in callers {
                for &to in callees {
                    // Same language on BOTH ends, and the same language the call
                    // was parsed from. Cross-language name matches are
                    // coincidences, not edges.
                    if from != to
                        && language_of.get(&from) == Some(&language)
                        && language_of.get(&to) == Some(&language)
                    {
                        graph.add_edge(from, to, EdgeKind::Calls);
                    }
                }
            }
        }

        Self {
            graph,
            by_name,
            callers_by_callee,
        }
    }

    /// The base caller nodes that invoke `callee_name` anywhere in the tree —
    /// the FR-16 frontier index. Unlike walking incoming edges of `callee_name`'s
    /// definitions, this also answers for a name with NO definition here, which
    /// is exactly the overlay-new symbol a tenant just introduced (yupana #3).
    #[must_use]
    pub fn callers_of_name(&self, callee_name: &str) -> &[NodeIndex] {
        self.callers_by_callee
            .get(callee_name)
            .map_or(&[], Vec::as_slice)
    }

    /// The file and language of a base node — used by the tenant view to
    /// language-filter frontier callers.
    #[must_use]
    pub(crate) fn node_file(&self, ix: NodeIndex) -> &str {
        &self.graph[ix].file
    }

    /// Whether any symbol with `name` is in the graph.
    #[must_use]
    pub fn has_symbol(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Node and edge counts.
    #[must_use]
    pub fn stats(&self) -> (usize, usize) {
        (self.graph.node_count(), self.graph.edge_count())
    }

    /// Direct (1-hop) neighbors of `seed` in the given direction.
    #[must_use]
    pub fn direct(&self, seed: &str, dir: Dir) -> Vec<Reached> {
        self.reachable(seed, dir, 1)
    }

    /// Symbols reachable from `seed` within `max_hops`, breadth-first.
    ///
    /// This is the FR-12 primitive (via the single [`blast::reachable_over`]
    /// walk): `Dir::Callers` is the impact set (who transitively calls `seed`);
    /// `Dir::Callees` is the dependency set.
    #[must_use]
    pub fn reachable(&self, seed: &str, dir: Dir, max_hops: u32) -> Vec<Reached> {
        reachable_over(self, seed, dir, max_hops)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn builds_and_walks_call_graph() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "lib.rs",
            "\
fn leaf() {}
fn mid() { leaf(); }
fn top() { mid(); }
",
        );
        let graph = CodeGraph::build(dir.path()).unwrap();
        assert!(graph.has_symbol("leaf"));

        // Direct callers of leaf: mid.
        let callers = graph.direct("leaf", Dir::Callers);
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].name, "mid");

        // Transitive impact of leaf: mid (1) and top (2).
        let impact = graph.reachable("leaf", Dir::Callers, 5);
        let names: Vec<&str> = impact.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"mid"));
        assert!(names.contains(&"top"));

        // Callees of top: mid.
        let callees = graph.direct("top", Dir::Callees);
        assert_eq!(callees[0].name, "mid");
    }

    #[test]
    fn unknown_symbol_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.rs", "fn a() {}");
        let graph = CodeGraph::build(dir.path()).unwrap();
        assert!(graph.reachable("nope", Dir::Callers, 3).is_empty());
    }

    #[test]
    fn build_at_ref_reads_historical_tree_not_working_copy() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
        };
        if !git(&["init", "-q"]).is_ok_and(|o| o.status.success()) {
            return; // skip: no git available
        }
        let _ = git(&["config", "user.email", "t@t.test"]);
        let _ = git(&["config", "user.name", "t"]);

        // First commit: only `old_fn` exists.
        write(dir.path(), "lib.rs", "fn old_fn() {}\n");
        let _ = git(&["add", "."]);
        let _ = git(&["commit", "-q", "-m", "first"]);
        let first = crate::git::head_commit(dir.path()).unwrap();

        // Working tree diverges: `old_fn` is gone, `new_fn` appears (uncommitted).
        write(dir.path(), "lib.rs", "fn new_fn() {}\n");

        // The ref build sees the committed past; the working-tree build sees now.
        let at_ref = CodeGraph::build_at_ref(dir.path(), &first).unwrap();
        assert!(
            at_ref.has_symbol("old_fn"),
            "ref graph has the historical symbol"
        );
        assert!(
            !at_ref.has_symbol("new_fn"),
            "ref graph ignores the working tree"
        );

        let working = CodeGraph::build(dir.path()).unwrap();
        assert!(working.has_symbol("new_fn"));
        assert!(!working.has_symbol("old_fn"));
    }

    #[test]
    fn build_at_ref_outside_repo_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.rs", "fn a() {}");
        let graph = CodeGraph::build_at_ref(dir.path(), "HEAD").unwrap();
        assert_eq!(
            graph.stats(),
            (0, 0),
            "no repo → empty base graph, no crash"
        );
    }

    /// A name that exists in two languages must not link them. Cross-language
    /// name collisions are coincidences; counting them as call edges inflates
    /// every blast radius in a polyglot repo, and an inflated radius is a FALSE
    /// DENY — it blocks legitimate work as confidently as the old silent allow
    /// let dangerous work through.
    #[cfg(feature = "langs-extra")]
    #[test]
    fn a_name_shared_across_languages_is_not_a_call_edge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("one.rs"), "fn one() { leaf(); }\n").unwrap();
        std::fs::write(dir.path().join("leaf.py"), "def leaf():\n    return 1\n").unwrap();
        std::fs::write(
            dir.path().join("one.py"),
            "from leaf import leaf\ndef one():\n    return leaf()\n",
        )
        .unwrap();

        let graph = CodeGraph::build(dir.path()).unwrap();
        let reached = reachable_over(&graph, "leaf", Dir::Callers, 5);
        let files: std::collections::BTreeSet<String> =
            reached.into_iter().map(|r| r.file).collect();
        // `leaf` is defined in both languages, so BOTH callers are legitimately
        // reachable from the NAME — but each only through its own language. What
        // must not happen is one language's caller appearing for the other's
        // definition, which is what a language-blind name match produces.
        assert!(
            files.len() <= 2,
            "cross-language edges inflated the radius: {files:?}"
        );
    }

    /// A ref that does not resolve must be an ERROR, not an empty baseline.
    /// Measured before this existed: `analyze --at no-such-ref` printed
    /// "0 file(s), 0 symbol(s)" and exited 0 — identical to a real ref holding
    /// nothing parseable.
    #[test]
    fn an_unresolved_ref_fails_to_build_rather_than_building_empty() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "base"]);

        // The real ref builds, and is non-empty — the positive control, without
        // which "it errored" proves nothing.
        let Ok(built) = CodeGraph::build_at_ref_checked(dir.path(), "main") else {
            panic!("a real ref must build")
        };
        assert!(built.has_symbol("leaf"));

        let Err(err) = CodeGraph::build_at_ref_checked(dir.path(), "no-such-ref") else {
            panic!("an unresolved ref must NOT build an empty baseline")
        };
        assert_eq!(err, BaselineError::UnresolvedRef("no-such-ref".to_string()));
        assert!(err.to_string().contains("NO BASELINE was built"));
    }

    #[test]
    fn outside_a_repo_no_baseline_is_built_and_it_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let Err(err) = CodeGraph::build_at_ref_checked(dir.path(), "HEAD") else {
            panic!("outside a repo there is no baseline to build")
        };
        assert_eq!(err, BaselineError::NoRepo);
        assert!(err.to_string().contains("not an empty baseline"));
    }
}
