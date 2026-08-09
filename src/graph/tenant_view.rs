//! The composed `base + overlay` VIEW — one tenant's query surface, split out
//! of `tenant` for size (yupana #83).
//!
//! [`TenantView`] is built per query and dropped at its end: it borrows, it
//! never owns, so no overlay ever holds a base `NodeIndex`. Isolation stays
//! structural (§6.3) — a view composes exactly one tenant's overlay by
//! construction, and the registry next door is the only thing that could hand
//! it another, which it does not.
//!
//! Resolution rule (unchanged, restated because it lives here now): a touched
//! file is MASKED, so definitions of a name are the overlay's plus base
//! definitions in untouched files, and a base edge into a touched file is
//! remapped BY NAME to the overlay's.

use super::*;

/// The result of a fan-in-bounded frontier walk (§14.2). `bounded_seeds` names
/// the high-fan-in symbols whose cascade was clipped to one hop — the caller
/// logs them, so bounding is always visible, never a silent truncation.
#[derive(Debug, Clone)]
pub struct BoundedFrontier {
    /// The reached frontier symbols.
    pub reached: Vec<Reached>,
    /// Seed names whose cascade was bounded (fan-in over threshold).
    pub bounded_seeds: Vec<String>,
}

/// One tenant's `base + overlay` composition, built per query, dropped at its
/// end. Implements [`Adjacency`], so the FR-12 BFS walks it unchanged.
pub struct TenantView<'a> {
    // `pub(super)` so the registry next door can still compose a view. It is the
    // ONLY thing that constructs one, which is what keeps §6.3 isolation
    // structural: no caller can pair a base with an overlay of its choosing.
    pub(super) base: &'a Base,
    pub(super) overlay: Option<&'a Overlay>,
}

/// One symbol as the composed view resolves it — the serve-path shape
/// (definitions and per-file listings), with the kind the wire DTOs need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewSymbol {
    /// Symbol name.
    pub name: String,
    /// Symbol kind (lowercase form).
    pub kind: String,
    /// File, relative to the root.
    pub file: String,
    /// 1-based definition line.
    pub start_line: usize,
}

impl TenantView<'_> {
    /// Whether `name` is defined anywhere in the composed view.
    #[must_use]
    pub fn has_symbol(&self, name: &str) -> bool {
        !self.seeds(name).is_empty()
    }

    /// The FR-16 frontier of editing `changed` symbols: everything whose facts
    /// an edit can perturb — the changed symbols' transitive callers (impact)
    /// AND callees (dependencies), within `hops`, over the COMPOSED view. This
    /// is the second caller of the one `reachable_over` BFS (FR-12, "build it
    /// once"); it walks base+overlay, so the frontier spans the tenant's edit
    /// and the shared base — including base callers of a name the overlay just
    /// introduced. Deduplicated by (name, file); the seeds themselves are
    /// excluded (the frontier is what the edit REACHES). Each item keeps its
    /// `distance`/`via`, so a caller can see how far a consequence propagated.
    #[must_use]
    pub fn frontier(&self, changed: &[&str], hops: u32) -> Vec<Reached> {
        self.frontier_bounded(changed, hops, usize::MAX).reached
    }

    /// The FR-16 frontier with the §14.2 high-fan-in guard: a seed whose direct
    /// (1-hop) fan already exceeds `high_fanin_threshold` is a widely-referenced
    /// signature whose transitive cascade could blow the budget, so it is walked
    /// only ONE hop instead of `hops`. The seed names so bounded are returned
    /// (and logged by the caller) — the cascade is bounded, never silently
    /// truncated. `usize::MAX` disables the guard (plain [`frontier`](Self::frontier)).
    #[must_use]
    pub fn frontier_bounded(
        &self,
        changed: &[&str],
        hops: u32,
        high_fanin_threshold: usize,
    ) -> BoundedFrontier {
        let seeds: std::collections::BTreeSet<&str> = changed.iter().copied().collect();
        let mut seen: std::collections::BTreeSet<(String, String)> =
            std::collections::BTreeSet::new();
        let mut out = Vec::new();
        let mut bounded_seeds = Vec::new();
        for &name in changed {
            // Direct fan = 1-hop callers + callees. Cheap, and it is exactly the
            // "widely referenced" measure §14.2 warns about.
            let direct_fan = crate::graph::reachable_over(self, name, Dir::Callers, 1).len()
                + crate::graph::reachable_over(self, name, Dir::Callees, 1).len();
            let effective_hops = if direct_fan > high_fanin_threshold {
                bounded_seeds.push(name.to_string());
                1
            } else {
                hops
            };
            for dir in [Dir::Callers, Dir::Callees] {
                for r in crate::graph::reachable_over(self, name, dir, effective_hops) {
                    if !seeds.contains(r.name.as_str())
                        && seen.insert((r.name.clone(), r.file.clone()))
                    {
                        out.push(r);
                    }
                }
            }
        }
        BoundedFrontier {
            reached: out,
            bounded_seeds,
        }
    }

    /// The composed definition sites of `name` — overlay definitions plus
    /// unmasked base ones, the same resolution the walk seeds from.
    #[must_use]
    pub fn definitions(&self, name: &str) -> Vec<ViewSymbol> {
        self.defs_of_name(name, None)
            .into_iter()
            .map(|r| self.view_symbol(r))
            .collect()
    }

    /// The symbols `rel` contributes to the composed view, in line order:
    /// the overlay's if touched (masking), the base's otherwise.
    #[must_use]
    pub fn file_symbols(&self, rel: &str) -> Vec<ViewSymbol> {
        if let Some(overlay) = self.overlay {
            if overlay.is_touched(rel) {
                let mut out: Vec<ViewSymbol> = (0..overlay.symbol_count() as u32)
                    .filter(|&id| overlay.symbol(id).file == rel)
                    .map(|id| self.view_symbol(SymRef::Overlay(id)))
                    .collect();
                out.sort_by_key(|s| s.start_line);
                return out;
            }
        }
        self.base
            .graph()
            .file_symbols(rel)
            .into_iter()
            .map(|n| ViewSymbol {
                name: n.name.clone(),
                kind: n.kind.clone(),
                file: n.file.clone(),
                start_line: n.start_line,
            })
            .collect()
    }

    fn view_symbol(&self, r: SymRef) -> ViewSymbol {
        match r {
            SymRef::Base(ix) => {
                let n = &self.base.graph().graph[ix];
                ViewSymbol {
                    name: n.name.clone(),
                    kind: n.kind.clone(),
                    file: n.file.clone(),
                    start_line: n.start_line,
                }
            }
            SymRef::Overlay(id) => {
                let s = self
                    .overlay
                    .expect("an overlay id implies an overlay")
                    .symbol(id);
                ViewSymbol {
                    name: s.name.clone(),
                    kind: s.kind.clone(),
                    file: s.file.clone(),
                    start_line: s.start_line,
                }
            }
        }
    }

    fn touched(&self, rel: &str) -> bool {
        self.overlay.is_some_and(|o| o.is_touched(rel))
    }

    /// The language a base node's facts belong to (by file extension — the
    /// same rule the graph build applies).
    fn base_language(&self, ix: NodeIndex) -> Option<&'static str> {
        let file = &self.base.graph().graph[ix].file;
        std::path::Path::new(file)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .and_then(crate::extract::language_for_extension)
    }

    /// Resolve a callee NAME against the view for a caller of `language`:
    /// overlay definitions plus unmasked base definitions, same language only.
    fn defs_of_name(&self, name: &str, language: Option<&'static str>) -> Vec<SymRef> {
        let mut out = Vec::new();
        if let Some(overlay) = self.overlay {
            for &id in overlay.defs(name) {
                if language.is_none_or(|l| overlay.symbol(id).language == l) {
                    out.push(SymRef::Overlay(id));
                }
            }
        }
        for &ix in self
            .base
            .graph()
            .by_name
            .get(name)
            .map_or(&[][..], Vec::as_slice)
        {
            let node = &self.base.graph().graph[ix];
            if !self.touched(&node.file)
                && language.is_none_or(|l| self.base_language(ix) == Some(l))
            {
                out.push(SymRef::Base(ix));
            }
        }
        out
    }

    /// Base callers of `name` from UNTOUCHED files, of `language`, via the
    /// FR-16 frontier index. This answers even when `name` has NO base
    /// definition — the overlay-new symbol a tenant just introduced (yupana #3) —
    /// which walking a definition's incoming edges never could.
    fn base_callers_of_name(&self, name: &str, language: &'static str) -> Vec<SymRef> {
        let mut out = Vec::new();
        for &caller in self.base.graph().callers_of_name(name) {
            let file = self.base.graph().node_file(caller);
            if !self.touched(file) && self.base_language(caller) == Some(language) {
                out.push(SymRef::Base(caller));
            }
        }
        out
    }
}

impl Adjacency for TenantView<'_> {
    type Node = SymRef;

    fn seeds(&self, name: &str) -> Vec<SymRef> {
        self.defs_of_name(name, None)
    }

    fn neighbors(&self, node: SymRef, dir: Dir) -> Vec<SymRef> {
        let mut out = Vec::new();
        match (node, dir) {
            (SymRef::Base(ix), Dir::Callees) => {
                // Materialized base edges; a callee in a touched file is
                // remapped BY NAME to the overlay's definitions (deduped —
                // several masked defs of one name remap to the same targets).
                let language = self.base_language(ix);
                let mut remap: Vec<&str> = Vec::new();
                for callee in self
                    .base
                    .graph()
                    .graph
                    .neighbors_directed(ix, PgDirection::Outgoing)
                {
                    let n = &self.base.graph().graph[callee];
                    if self.touched(&n.file) {
                        if !remap.contains(&n.name.as_str()) {
                            remap.push(&n.name);
                        }
                    } else {
                        out.push(SymRef::Base(callee));
                    }
                }
                if let Some(overlay) = self.overlay {
                    for name in remap {
                        for &id in overlay.defs(name) {
                            if language.is_none_or(|l| overlay.symbol(id).language == l) {
                                out.push(SymRef::Overlay(id));
                            }
                        }
                    }
                }
            }
            (SymRef::Base(ix), Dir::Callers) => {
                // Base callers via materialized edges (untouched files only —
                // a touched caller's calls are the OVERLAY's records now, so
                // a deleted call never resurrects), plus overlay callers of
                // this node's name.
                for caller in self
                    .base
                    .graph()
                    .graph
                    .neighbors_directed(ix, PgDirection::Incoming)
                {
                    let file = &self.base.graph().graph[caller].file;
                    if !self.touched(file) {
                        out.push(SymRef::Base(caller));
                    }
                }
                if let Some(overlay) = self.overlay {
                    let me = &self.base.graph().graph[ix];
                    let language = self.base_language(ix);
                    for &id in overlay.callers_of(&me.name) {
                        if language.is_none_or(|l| overlay.symbol(id).language == l) {
                            out.push(SymRef::Overlay(id));
                        }
                    }
                }
            }
            (SymRef::Overlay(id), Dir::Callees) => {
                let overlay = self.overlay.expect("an overlay id implies an overlay");
                let me = overlay.symbol(id);
                for name in overlay.callee_names(id) {
                    for target in self.defs_of_name(name, Some(me.language)) {
                        if target != node && !out.contains(&target) {
                            out.push(target);
                        }
                    }
                }
            }
            (SymRef::Overlay(id), Dir::Callers) => {
                let overlay = self.overlay.expect("an overlay id implies an overlay");
                let me = overlay.symbol(id);
                for &caller in overlay.callers_of(&me.name) {
                    if overlay.symbol(caller).language == me.language && caller != id {
                        out.push(SymRef::Overlay(caller));
                    }
                }
                // Base callers of my NAME, via the FR-16 frontier index — this
                // now answers even when the overlay INTRODUCED the name (zero
                // base defs), which the old edge-walk could not (yupana #3).
                out.extend(self.base_callers_of_name(&me.name, me.language));
            }
        }
        out
    }

    fn meta(&self, node: SymRef) -> NodeMeta {
        match node {
            SymRef::Base(ix) => {
                let n = &self.base.graph().graph[ix];
                NodeMeta {
                    name: n.name.clone(),
                    file: n.file.clone(),
                    start_line: n.start_line,
                }
            }
            SymRef::Overlay(id) => {
                let s = self
                    .overlay
                    .expect("an overlay id implies an overlay")
                    .symbol(id);
                NodeMeta {
                    name: s.name.clone(),
                    file: s.file.clone(),
                    start_line: s.start_line,
                }
            }
        }
    }
}
