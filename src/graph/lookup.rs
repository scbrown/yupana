//! Node lookups over a built [`CodeGraph`] — definition sites by name and the
//! symbol list of one file. Child module of `graph` so it can read the private
//! `graph`/`by_name` fields; kept out of `mod.rs` to hold the module at build +
//! traversal (file-size discipline, one responsibility per module).

use super::{CodeGraph, SymbolNode};

impl CodeGraph {
    /// Definition sites of `name`, from the resident node index. Zero results
    /// means the graph holds no symbol by that name — distinct from "the graph
    /// is empty", which [`Self::stats`] reports.
    #[must_use]
    pub fn definitions(&self, name: &str) -> Vec<&SymbolNode> {
        self.by_name.get(name).map_or_else(Vec::new, |ixs| {
            ixs.iter().map(|&ix| &self.graph[ix]).collect()
        })
    }

    /// Symbols defined in `rel` (a root-relative path), sorted by line.
    ///
    /// An empty result means the resident graph holds NO symbols for that path —
    /// which it cannot tell apart from "no such file" or "file not parseable":
    /// files contribute to the graph only through their symbols. Callers that
    /// report to a human must say "no symbols in the resident graph", never
    /// "the file is empty".
    #[must_use]
    pub fn file_symbols(&self, rel: &str) -> Vec<&SymbolNode> {
        let mut symbols: Vec<&SymbolNode> = self
            .graph
            .node_indices()
            .map(|ix| &self.graph[ix])
            .filter(|n| n.file == rel)
            .collect();
        symbols.sort_by_key(|n| n.start_line);
        symbols
    }

    /// The symbol whose definition ENCLOSES `line` in `rel`, innermost first.
    ///
    /// This is the position half of FR-4 (yupana #8) at the tree-sitter tier: name
    /// lookup over-connects on common names — `build`, `new`, `write` — and a
    /// caller looking at one of them in an editor knows the position, not which
    /// of the twelve it is. Pointing at it resolves it.
    ///
    /// "Innermost" is by latest start: a method inside an `impl` inside a module
    /// all enclose the same line, and the one the caller means is the tightest.
    /// Ties (two symbols starting on one line) go to the shorter span.
    ///
    /// LINE granularity, not `(line, col)` — the extractor records lines, so a
    /// column would be accepted and then ignored, which is the shape FR-3
    /// forbids. Two symbols on ONE line are not separable here; that wants the
    /// LSP tier (FR-2), and the caller is told so rather than handed a guess.
    #[must_use]
    pub fn symbol_at(&self, rel: &str, line: usize) -> Option<&SymbolNode> {
        self.graph
            .node_indices()
            .map(|ix| &self.graph[ix])
            .filter(|n| n.file == rel && n.start_line <= line && line <= n.end_line)
            .min_by_key(|n| {
                (
                    std::cmp::Reverse(n.start_line),
                    n.end_line.saturating_sub(n.start_line),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use crate::graph::CodeGraph;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn shared() {}\nfn only_a() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn shared() {}\n").unwrap();
        dir
    }

    #[test]
    fn definitions_returns_every_site_of_a_reused_name() {
        let graph = CodeGraph::build(repo().path()).unwrap();
        let defs = graph.definitions("shared");
        let mut files: Vec<&str> = defs.iter().map(|d| d.file.as_str()).collect();
        files.sort_unstable();
        assert_eq!(files, ["a.rs", "b.rs"], "both definition sites, not one");
        assert!(graph.definitions("absent").is_empty());
    }

    #[test]
    fn symbol_at_picks_the_innermost_enclosing_definition() {
        // The yupana #8 win: `shared` exists in two files, so the NAME is
        // ambiguous by construction. A position is not.
        let dir = repo();
        let graph = CodeGraph::build(dir.path()).unwrap();

        assert_eq!(graph.definitions("shared").len(), 2, "name is ambiguous");
        let at = graph.symbol_at("b.rs", 1).expect("b.rs:1 is `shared`");
        assert_eq!((at.name.as_str(), at.file.as_str()), ("shared", "b.rs"));

        // Same name, other file — the position, not the name, decides.
        let at = graph.symbol_at("a.rs", 1).expect("a.rs:1 is `shared`");
        assert_eq!(at.file, "a.rs");
    }

    #[test]
    fn symbol_at_is_none_off_the_end_and_for_an_unknown_file() {
        let dir = repo();
        let graph = CodeGraph::build(dir.path()).unwrap();
        assert!(
            graph.symbol_at("a.rs", 9_999).is_none(),
            "a line past every definition encloses nothing"
        );
        assert!(graph.symbol_at("missing.rs", 1).is_none());
    }

    #[test]
    fn file_symbols_lists_one_files_symbols_in_line_order() {
        let graph = CodeGraph::build(repo().path()).unwrap();
        let names: Vec<&str> = graph
            .file_symbols("a.rs")
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["shared", "only_a"], "a.rs symbols in line order");
        assert!(graph.file_symbols("missing.rs").is_empty());
    }
}
