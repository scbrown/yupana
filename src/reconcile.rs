//! Reconcile a structural reachable set with a historical co-change set (FR-11).
//!
//! Yupana computes structural coupling (call/dataflow reachability); Bobbin owns
//! historical co-change (FP-Growth over git history). Reconciling the two is the
//! differentiating insight from the vision: a co-change edge backed by a
//! structural path is *real* coupling; a co-change edge with no structural
//! explanation is a *refactoring smell*; a structural edge never seen co-change
//! is *new or unexercised* coupling.
//!
//! Co-change mining stays in Bobbin (the routing rule), so the caller supplies
//! the co-change set; this module is a pure set reconciliation over file paths.

use std::collections::BTreeSet;

/// The three-way partition of structural vs. historical coupling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciliation {
    /// In both sets — corroborated, real coupling.
    pub corroborated: Vec<String>,
    /// Structural but never co-changed — new or unexercised coupling.
    pub structural_only: Vec<String>,
    /// Co-changed but structurally unexplained — a possible refactoring smell.
    pub cochange_only: Vec<String>,
}

/// Partition `structural` and `cochange` file sets into the three buckets.
#[must_use]
pub fn reconcile(structural: &BTreeSet<String>, cochange: &BTreeSet<String>) -> Reconciliation {
    Reconciliation {
        corroborated: structural.intersection(cochange).cloned().collect(),
        structural_only: structural.difference(cochange).cloned().collect(),
        cochange_only: cochange.difference(structural).cloned().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn partitions_three_ways() {
        let structural = set(&["a.rs", "b.rs", "c.rs"]);
        let cochange = set(&["b.rs", "c.rs", "d.rs"]);
        let recon = reconcile(&structural, &cochange);
        assert_eq!(recon.corroborated, vec!["b.rs", "c.rs"]);
        assert_eq!(recon.structural_only, vec!["a.rs"]);
        assert_eq!(recon.cochange_only, vec!["d.rs"]);
    }

    #[test]
    fn disjoint_sets_have_no_corroboration() {
        let recon = reconcile(&set(&["a.rs"]), &set(&["b.rs"]));
        assert!(recon.corroborated.is_empty());
        assert_eq!(recon.structural_only, vec!["a.rs"]);
        assert_eq!(recon.cochange_only, vec!["b.rs"]);
    }
}
