//! The briefing's read-only view of the graph — per-file symbol inventory and
//! caller locality, for the work-item briefing (`crate::brief`). Split from
//! `mod.rs` for file size; nothing here mutates or caches.

use super::{CodeGraph, SymbolNode};

impl CodeGraph {
    /// The symbols defined in `file` (relative path), in definition order —
    /// the work-item briefing's per-path inventory.
    #[must_use]
    pub fn symbols_in(&self, file: &str) -> Vec<&SymbolNode> {
        let mut symbols: Vec<&SymbolNode> = self
            .graph
            .node_weights()
            .filter(|node| node.file == file)
            .collect();
        symbols.sort_by_key(|node| node.start_line);
        symbols
    }

    /// The distinct files containing callers of `name`, sorted — so a briefing
    /// can say where an edit to `name` will be felt from.
    #[must_use]
    pub fn caller_files_of(&self, name: &str) -> Vec<String> {
        let mut files: Vec<String> = self
            .callers_of_name(name)
            .iter()
            .map(|ix| self.graph[*ix].file.clone())
            .collect();
        files.sort();
        files.dedup();
        files
    }
}
