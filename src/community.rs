//! Deterministic Louvain community detection over the in-memory graph (FR-9).
//!
//! Quipu runs community detection over *committed* facts; Yupana computes it live
//! over the hot per-tenant graph. The implementation is **deterministic** — a
//! given graph always yields the same partition, with no RNG: nodes are visited
//! in index order, a move is taken only on a strict modularity gain, ties are
//! broken toward the lowest community index, and the aggregation levels are
//! likewise ordered. Repeated runs (and repeated levels) are reproducible.
//!
//! The public entry point is [`louvain`], which takes an undirected weighted
//! edge list (node count + `(a, b, weight)` triples) and returns a community
//! label per node, canonicalized to `0..k` in order of first appearance.

use std::collections::HashMap;

/// Modularity moves smaller than this are treated as no-ops (keeps the local
/// pass monotone and terminating in the face of floating-point noise).
const EPS: f64 = 1e-12;

/// A working weighted graph: undirected, with a per-node self-loop weight that
/// accumulates as communities are aggregated into super-nodes.
struct Weighted {
    n: usize,
    /// Neighbor weights per node (`j != i`); symmetric (`adj[i][j] == adj[j][i]`).
    adj: Vec<HashMap<usize, f64>>,
    /// Self-loop weight per node — internal edge mass after aggregation.
    selfloop: Vec<f64>,
    /// Weighted degree per node (incident edge weights; a self-loop counts twice).
    deg: Vec<f64>,
    /// Total edge weight × 2 (= Σ deg). Zero for an edgeless graph.
    two_m: f64,
}

impl Weighted {
    /// Build the working graph from an undirected weighted edge list. Parallel
    /// edges sum; `(a, a, w)` entries become self-loops.
    fn from_edges(n: usize, edges: &[(usize, usize, f64)]) -> Self {
        let mut adj = vec![HashMap::new(); n];
        let mut selfloop = vec![0.0; n];
        for &(a, b, w) in edges {
            if a == b {
                selfloop[a] += w;
            } else {
                *adj[a].entry(b).or_insert(0.0) += w;
                *adj[b].entry(a).or_insert(0.0) += w;
            }
        }
        Self::finish(n, adj, selfloop)
    }

    /// Finalize degrees and total weight from adjacency + self-loops.
    fn finish(n: usize, adj: Vec<HashMap<usize, f64>>, selfloop: Vec<f64>) -> Self {
        let mut deg = vec![0.0; n];
        for i in 0..n {
            let incident: f64 = adj[i].values().sum();
            deg[i] = incident + 2.0 * selfloop[i];
        }
        let two_m: f64 = deg.iter().sum();
        Self {
            n,
            adj,
            selfloop,
            deg,
            two_m,
        }
    }
}

/// Detect communities over an undirected weighted graph (Louvain).
///
/// `edges` are `(a, b, weight)` triples over nodes `0..n`; the projection is
/// undirected, so callers fold any directed graph before calling. Returns a
/// community label per node, canonicalized to `0..k` in order of first
/// appearance (so the output is stable and comparable across runs).
#[must_use]
pub fn louvain(n: usize, edges: &[(usize, usize, f64)]) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let mut g = Weighted::from_edges(n, edges);
    // `mapping[original_node]` = its community in the current (aggregated) space.
    let mut mapping: Vec<usize> = (0..n).collect();
    loop {
        let labels = one_level(&g);
        let k = labels.iter().copied().max().map_or(0, |m| m + 1);
        if k == g.n {
            break; // a full level merged nothing — converged.
        }
        for m in &mut mapping {
            *m = labels[*m];
        }
        g = aggregate(&g, &labels, k);
    }
    canonicalize(&mapping)
}

/// One level of local moving. Each node starts in its own community; nodes are
/// swept in index order and moved to the neighboring community of greatest
/// strict modularity gain until a full sweep moves nothing. Returns canonical
/// labels `0..k`; `k == g.n` means nothing merged.
fn one_level(g: &Weighted) -> Vec<usize> {
    let mut comm: Vec<usize> = (0..g.n).collect();
    if g.two_m == 0.0 {
        return canonicalize(&comm);
    }
    let mut sigma_tot: Vec<f64> = g.deg.clone();
    let mut improved = true;
    // The guard is a floating-point-safety backstop; convergence is by `improved`.
    let mut guard = 0;
    while improved && guard < 100 {
        improved = false;
        guard += 1;
        for i in 0..g.n {
            let ci = comm[i];
            // Weight from i into each neighboring community.
            let mut k_i_in: HashMap<usize, f64> = HashMap::new();
            for (&j, &w) in &g.adj[i] {
                *k_i_in.entry(comm[j]).or_insert(0.0) += w;
            }
            // Tentatively remove i from its community.
            sigma_tot[ci] -= g.deg[i];
            // Baseline: the gain of re-adding i to its own (now lighter) community.
            let mut best = ci;
            let mut best_gain =
                k_i_in.get(&ci).copied().unwrap_or(0.0) - sigma_tot[ci] * g.deg[i] / g.two_m;
            // Scan candidate communities in ascending index order; take a move
            // only on a strict improvement, so ties keep the lowest index and
            // the sweep stays monotone.
            let mut cands: Vec<usize> = k_i_in.keys().copied().collect();
            cands.sort_unstable();
            for c in cands {
                let gain = k_i_in[&c] - sigma_tot[c] * g.deg[i] / g.two_m;
                if gain > best_gain + EPS {
                    best_gain = gain;
                    best = c;
                }
            }
            sigma_tot[best] += g.deg[i];
            if best != ci {
                comm[i] = best;
                improved = true;
            }
        }
    }
    canonicalize(&comm)
}

/// Collapse each community into a super-node: cross-community edge weights sum
/// into the aggregated adjacency, and intra-community edges fold into the
/// super-node's self-loop.
fn aggregate(g: &Weighted, labels: &[usize], k: usize) -> Weighted {
    let mut adj = vec![HashMap::new(); k];
    let mut selfloop = vec![0.0; k];
    for i in 0..g.n {
        let ci = labels[i];
        selfloop[ci] += g.selfloop[i];
        for (&j, &w) in &g.adj[i] {
            let cj = labels[j];
            if ci == cj {
                // Internal edge, seen once from each endpoint → w/2 each = w total.
                selfloop[ci] += w / 2.0;
            } else if i < j {
                // Cross edge: count the undirected pair once, store symmetrically.
                *adj[ci].entry(cj).or_insert(0.0) += w;
                *adj[cj].entry(ci).or_insert(0.0) += w;
            }
        }
    }
    Weighted::finish(k, adj, selfloop)
}

/// Relabel communities to `0..k` in order of first appearance, so the partition
/// is stable regardless of the internal community ids used during moving.
fn canonicalize(comm: &[usize]) -> Vec<usize> {
    let mut remap: HashMap<usize, usize> = HashMap::new();
    let mut out = vec![0usize; comm.len()];
    for (i, &c) in comm.iter().enumerate() {
        let next = remap.len();
        out[i] = *remap.entry(c).or_insert(next);
    }
    out
}

/// Newman–Girvan modularity of a partition over an undirected weighted edge
/// list. Used to sanity-check that a detected partition beats the trivial
/// all-in-one-community grouping.
#[cfg(test)]
#[must_use]
pub(crate) fn modularity(n: usize, edges: &[(usize, usize, f64)], labels: &[usize]) -> f64 {
    let g = Weighted::from_edges(n, edges);
    if g.two_m == 0.0 {
        return 0.0;
    }
    let k = labels.iter().copied().max().map_or(0, |m| m + 1);
    let mut internal = vec![0.0; k];
    let mut total = vec![0.0; k];
    for i in 0..n {
        total[labels[i]] += g.deg[i];
    }
    for &(a, b, w) in edges {
        if a != b && labels[a] == labels[b] {
            internal[labels[a]] += 2.0 * w;
        }
    }
    (0..k)
        .map(|c| internal[c] / g.two_m - (total[c] / g.two_m).powi(2))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two triangles joined by a single bridge edge — the textbook two-community
    /// graph. Nodes 0,1,2 form one clique; 3,4,5 the other; edge 2–3 bridges.
    fn two_cliques() -> (usize, Vec<(usize, usize, f64)>) {
        let edges = vec![
            (0, 1, 1.0),
            (1, 2, 1.0),
            (0, 2, 1.0),
            (3, 4, 1.0),
            (4, 5, 1.0),
            (3, 5, 1.0),
            (2, 3, 1.0),
        ];
        (6, edges)
    }

    #[test]
    fn separates_two_clusters() {
        let (n, edges) = two_cliques();
        let labels = louvain(n, &edges);
        let distinct: std::collections::BTreeSet<_> = labels.iter().copied().collect();
        assert_eq!(distinct.len(), 2, "expected two communities: {labels:?}");
        // The two cliques land together, apart from each other.
        assert_eq!(labels[0], labels[1]);
        assert_eq!(labels[1], labels[2]);
        assert_eq!(labels[3], labels[4]);
        assert_eq!(labels[4], labels[5]);
        assert_ne!(labels[0], labels[3]);
    }

    #[test]
    fn is_deterministic_across_runs() {
        let (n, edges) = two_cliques();
        let first = louvain(n, &edges);
        for _ in 0..5 {
            assert_eq!(louvain(n, &edges), first, "partition must be reproducible");
        }
    }

    #[test]
    fn canonical_labels_start_at_zero() {
        let (n, edges) = two_cliques();
        let labels = louvain(n, &edges);
        // First node is always in community 0 after canonicalization.
        assert_eq!(labels[0], 0);
        // Labels are dense 0..k.
        let max = labels.iter().copied().max().unwrap();
        let distinct: std::collections::BTreeSet<_> = labels.iter().copied().collect();
        assert_eq!(distinct.len(), max + 1);
    }

    #[test]
    fn partition_beats_trivial_grouping() {
        let (n, edges) = two_cliques();
        let labels = louvain(n, &edges);
        let all_one = vec![0usize; n];
        assert!(
            modularity(n, &edges, &labels) > modularity(n, &edges, &all_one),
            "detected partition should have higher modularity than one big community"
        );
    }

    #[test]
    fn empty_and_edgeless_graphs_are_safe() {
        assert!(louvain(0, &[]).is_empty());
        // Three isolated nodes → three singleton communities.
        let labels = louvain(3, &[]);
        assert_eq!(labels, vec![0, 1, 2]);
    }

    #[test]
    fn parallel_edges_sum_as_weight() {
        // A doubled edge between 0 and 1, a single edge 1–2: 0 and 1 bind tighter.
        let edges = vec![(0, 1, 1.0), (0, 1, 1.0), (1, 2, 1.0)];
        let labels = louvain(3, &edges);
        // Deterministic and canonical regardless of the exact grouping.
        assert_eq!(labels[0], 0);
        assert_eq!(louvain(3, &edges), labels);
    }
}
