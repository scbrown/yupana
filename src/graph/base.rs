//! The shared read-only base graph (FR-13) — slice 1 of yupana #2.
//!
//! One `Base` is built ONCE at a resolved commit and then never mutated: an
//! overlay (FR-14, `overlay.rs`) masks files on top of it, it never writes
//! here. Held behind `Arc` so N tenants share one copy — the §6.2 memory shape
//! is `O(repo)` once for the base plus `O(touched)` per tenant, and the §6.3
//! isolation argument starts from the base being immutable by construction.
//!
//! Beyond the graph itself, the base records per-file facts at build time:
//! the file's **content hash** (the FR-15 structural-sharing key — two tenants
//! whose overlay holds an identical file can share one parse) and its **symbol
//! names** (what an overlay masks when it touches the file). Both come from
//! the same single pass that feeds the graph build — no second read of the
//! tree.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::{BaselineError, CodeGraph};

/// Per-file facts recorded at base build time.
#[derive(Debug, Clone)]
pub struct FileFacts {
    /// Hex sha256 of the file's source at the base commit — the FR-15 key.
    pub hash: String,
    /// Names of the symbols the file defines, in definition order. Empty means
    /// the file parsed to no symbols — distinct from the file being absent
    /// from [`Base::file`] entirely (not in the tree, or not a language yupana
    /// extracts).
    pub symbols: Vec<String>,
}

/// The shared read-only base graph at a resolved commit (FR-13).
///
/// There is no `&mut self` anywhere on this type, and [`Base::build_at`]
/// returns it already inside an `Arc` — read-only is enforced by the API
/// surface, not by convention.
pub struct Base {
    graph: CodeGraph,
    commit: String,
    files: HashMap<String, FileFacts>,
}

impl Base {
    /// Build the base for the tree at `reference`, resolved to a commit.
    ///
    /// Same refusal semantics as [`CodeGraph::build_at_ref_checked`], for the
    /// same reason: a base that failed to build must SAY SO. Every tenant
    /// query resolves against this object — an empty base would not
    /// under-report, it would make every overlay symbol look ADDED and every
    /// impact look clean.
    pub fn build_at(root: &Path, reference: &str) -> Result<Arc<Self>, BaselineError> {
        if !crate::git::is_repo(root) {
            return Err(BaselineError::NoRepo);
        }
        let Some(commit) = crate::git::resolve_commit(root, reference) else {
            return Err(BaselineError::UnresolvedRef(reference.to_string()));
        };

        // One pass over the tree: hash each source as it is read, then hand
        // the same strings to the graph build.
        let sources: Vec<(String, String, &'static str)> = crate::git::list_files_at(root, &commit)
            .into_iter()
            .filter_map(|path| {
                let ext = path.extension().and_then(std::ffi::OsStr::to_str)?;
                let language = crate::extract::language_for_extension(ext)?;
                let source = crate::git::read_blob_at(root, &commit, &path)?;
                Some((path.display().to_string(), source, language))
            })
            .collect();

        let mut files: HashMap<String, FileFacts> = sources
            .iter()
            .map(|(rel, source, _)| {
                let hash = format!("{:x}", Sha256::digest(source.as_bytes()));
                (
                    rel.clone(),
                    FileFacts {
                        hash,
                        symbols: Vec::new(),
                    },
                )
            })
            .collect();

        let graph = CodeGraph::from_sources(sources.into_iter());
        // Symbol names per file, from the nodes the build just made — nodes
        // are added in per-file definition order, so this preserves it.
        for ix in graph.graph.node_indices() {
            let node = &graph.graph[ix];
            if let Some(facts) = files.get_mut(&node.file) {
                facts.symbols.push(node.name.clone());
            }
        }

        Ok(Arc::new(Self {
            graph,
            commit,
            files,
        }))
    }

    /// The base call graph. Read-only: queries only.
    #[must_use]
    pub fn graph(&self) -> &CodeGraph {
        &self.graph
    }

    /// The commit the base was built at — always a resolved id, never the
    /// spelling the caller passed (`HEAD`, a branch name).
    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// The per-file facts for `rel`, or `None` when the file is not in the
    /// base tree (or not a language yupana extracts).
    #[must_use]
    pub fn file(&self, rel: &str) -> Option<&FileFacts> {
        self.files.get(rel)
    }

    /// Number of files the base holds facts for.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("mid.rs"), "fn mid() { leaf(); }\n").unwrap();
        // Identical content to leaf.rs — the FR-15 sharing key must agree.
        std::fs::write(dir.path().join("twin.rs"), "fn leaf() {}\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "base"]);
        dir
    }

    #[test]
    fn base_records_resolved_commit_per_file_hashes_and_symbols() {
        let dir = committed_repo();
        let base = Base::build_at(dir.path(), "main").unwrap();

        // The spelling `main` resolves to a full commit id.
        assert_eq!(base.commit().len(), 40, "resolved id, not the ref spelling");

        // The graph answers like any CodeGraph.
        assert!(base.graph().has_symbol("leaf"));

        // Per-file facts: symbols in definition order, content hash present.
        let mid = base.file("mid.rs").expect("mid.rs is in the base tree");
        assert_eq!(mid.symbols, ["mid"]);
        assert_eq!(mid.hash.len(), 64, "hex sha256");
        assert!(base.file("absent.rs").is_none());
        assert_eq!(base.file_count(), 3);
    }

    #[test]
    fn identical_content_hashes_identically_the_fr15_sharing_key() {
        let dir = committed_repo();
        let base = Base::build_at(dir.path(), "main").unwrap();
        let (leaf, twin) = (base.file("leaf.rs").unwrap(), base.file("twin.rs").unwrap());
        assert_eq!(leaf.hash, twin.hash, "same bytes must share one FR-15 key");
        assert_ne!(base.file("mid.rs").unwrap().hash, leaf.hash);
    }

    #[test]
    fn the_base_is_shared_not_copied() {
        let dir = committed_repo();
        let base = Base::build_at(dir.path(), "main").unwrap();
        let other = Arc::clone(&base);
        assert!(
            std::ptr::eq(Arc::as_ptr(&base), Arc::as_ptr(&other)),
            "tenants share ONE base allocation"
        );
    }

    #[test]
    fn a_base_that_cannot_build_says_so_never_builds_empty() {
        // Outside a repo, and at an unresolved ref: both are ERRORS with the
        // BaselineError distinctions, exactly as build_at_ref_checked refuses.
        let no_repo = tempfile::tempdir().unwrap();
        assert_eq!(
            Base::build_at(no_repo.path(), "HEAD").err(),
            Some(BaselineError::NoRepo)
        );
        let dir = committed_repo();
        assert_eq!(
            Base::build_at(dir.path(), "no-such-ref").err(),
            Some(BaselineError::UnresolvedRef("no-such-ref".to_string()))
        );
    }

    #[test]
    fn the_base_ignores_the_working_tree() {
        let dir = committed_repo();
        // Uncommitted change: `late` exists only in the working tree.
        std::fs::write(dir.path().join("late.rs"), "fn late() {}\n").unwrap();
        let base = Base::build_at(dir.path(), "main").unwrap();
        assert!(
            !base.graph().has_symbol("late"),
            "the base reads the committed tree, never the working copy"
        );
        assert!(base.file("late.rs").is_none());
    }
}
