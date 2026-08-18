//! The work-item maps the capability ladder projects: which paths an item's
//! prior work touched, and which item each one hangs under.
//!
//! Split from [`crate::policy`] under the file-size ratchet (yupana #83). Pure
//! data with no IO, like the rest of that module — the fetches live in
//! `project_scope` behind the `quipu` feature — so the split is by SUBJECT
//! rather than by convenience: `policy` owns what a scope IS and how it
//! decides, this owns what the graph said about which items have one.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::policy::Scope;

/// Which work item each item hangs under — the DERIVED rung's raw material.
///
/// Deliberately a separate map from [`WorkItemScopes`] rather than a field on
/// it: the two are projected by different queries and either can be absent
/// while the other is present, and folding them together would make an absent
/// parent map indistinguishable from an item with no parent. One is UNKNOWN,
/// the other is a fact.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItemParents(BTreeMap<String, String>);

impl WorkItemParents {
    /// Build from projected `(child id, parent id)` rows.
    ///
    /// An item with more than one parent keeps the FIRST seen and ignores the
    /// rest. That is deterministic given a sorted projection and, more to the
    /// point, honest about what a multi-parent item means here: there is no
    /// principled way to pick one ground over another, so the rung declines to
    /// invent a union that no single piece of work ever touched.
    #[must_use]
    pub fn from_rows(rows: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        for (child, parent) in rows {
            map.entry(child).or_insert(parent);
        }
        Self(map)
    }

    /// The parent of `item`, if the graph records one.
    #[must_use]
    pub fn parent_of(&self, item: &str) -> Option<&str> {
        self.0.get(item).map(String::as_str)
    }

    /// How many items carry a parent.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no item carries a parent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The projected work-item scope map: item id → repo-relative paths that work
/// on the item has touched. Pure data (this module's contract); the fetch
/// lives in `project_scope` behind the `quipu` feature.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItemScopes(BTreeMap<String, std::collections::BTreeSet<String>>);

impl WorkItemScopes {
    /// Build from projected `(item id, path)` rows.
    #[must_use]
    pub fn from_rows(rows: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut map: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        for (id, path) in rows {
            map.entry(id).or_default().insert(path);
        }
        Self(map)
    }

    /// The observed [`Scope`] for `item`: its touched paths as literal path
    /// globs. `None` when the item has no observed paths — an UNKNOWN scope,
    /// which advises; it is never an empty scope that denies everything.
    #[must_use]
    pub fn scope_for(&self, item: &str) -> Option<Scope> {
        let paths = self.0.get(item)?;
        Some(Scope {
            allow_paths: paths.iter().cloned().collect(),
            ..Scope::default()
        })
    }

    /// How many work items carry an observed scope.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no item carries an observed scope.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
