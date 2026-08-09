//! The per-tenant copy-on-write overlay (FR-14) — slice 2 of yupana #2.
//!
//! An overlay owns the truth for the files its tenant has TOUCHED, and nothing
//! else: each touched file's re-parsed structure, plus a derived index over
//! exactly those files. Untouched symbols resolve straight from the shared
//! [`super::Base`] with zero overlay cost — the §6.2 shape is
//! `O(touched files + frontier)`, never `O(repo)`.
//!
//! Masking is the copy-on-write rule: a touched file's base symbols are invisible
//! through the composed view ([`super::TenantView`]); the overlay's re-parse
//! is that file's only truth. Touching with content that parses to nothing
//! (or an empty string) therefore masks the file entirely — deletion is just
//! the empty re-parse. [`Overlay::revert`] drops the touch and the base
//! resumes answering.
//!
//! Everything here is OWNED — names, overlay-local `u32` ids, and
//! [`Arc<ParsedFile>`] parses (shared across tenants via the FR-15 intern
//! cache in [`super::TenantRegistry`]) — never a borrowed base `NodeIndex`,
//! so an overlay outlives any particular view without lifetime entanglement.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::Reached;
use crate::extract::{extract_structure, language_for_extension, FileStructure};
use crate::types::Tier;

/// One re-parsed source file, keyed by content hash (FR-15): tenants whose
/// overlays hold identical bytes share ONE of these behind `Arc`.
#[derive(Debug)]
pub struct ParsedFile {
    /// Hex sha256 of the source — the FR-15 intern key.
    pub hash: String,
    /// The language the file parses as (by extension), or `None` for a file
    /// yupana does not extract — which still masks its base symbols when touched.
    pub language: Option<&'static str>,
    /// The extracted structure; empty when `language` is `None` or the parse
    /// found nothing.
    pub structure: FileStructure,
}

impl ParsedFile {
    /// Parse `source` as the language `rel`'s extension names. Never fails:
    /// an unextractable file parses to an empty structure (it still masks).
    #[must_use]
    pub fn parse(rel: &str, source: &str) -> Self {
        let hash = format!("{:x}", Sha256::digest(source.as_bytes()));
        let language = Path::new(rel)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .and_then(language_for_extension);
        let structure = language
            .and_then(|l| extract_structure(source, l).ok())
            .unwrap_or_default();
        Self {
            hash,
            language,
            structure,
        }
    }
}

/// One symbol the overlay defines, addressed by overlay-local id (its index).
#[derive(Debug, Clone)]
pub struct OverlaySymbol {
    /// Symbol name.
    pub name: String,
    /// Symbol kind (lowercase form).
    pub kind: String,
    /// The touched file defining it.
    pub file: String,
    /// 1-based definition line.
    pub start_line: usize,
    /// Provenance tier.
    pub tier: Tier,
    /// Language of the defining file — call edges never cross languages.
    pub language: &'static str,
}

/// A tenant's copy-on-write overlay: touched files + a derived index.
#[derive(Debug, Default)]
pub struct Overlay {
    /// The touched files, rel path → shared parse. The copy-on-write unit.
    files: HashMap<String, Arc<ParsedFile>>,
    /// All overlay symbols; the `u32` id everywhere else indexes this.
    symbols: Vec<OverlaySymbol>,
    /// Definitions by name.
    by_name: HashMap<String, Vec<u32>>,
    /// Callee NAMES per overlay symbol id — resolved against the composed
    /// view at query time, not materialized (names survive base/overlay
    /// changes; indices would not).
    callees_of: Vec<Vec<String>>,
    /// Overlay callers (ids) of a callee NAME — the exact call records of
    /// touched files, so a deleted call never resurrects through remap.
    callers_of: HashMap<String, Vec<u32>>,
}

/// The FR-16 frontier of editing `changed` symbols, over `view`'s composed
/// `base + overlay` graph (yupana #3). Updating an overlay is NOT just the
/// edited file: a signature change perturbs every symbol that references it,
/// often in files the tenant never opened (§5.5). This bounds that blast to
/// the changed symbols' transitive callers AND callees — reusing the ONE
/// `reachable_over` BFS (FR-12), never a second traversal — so the recompute
/// is `O(edited + frontier)`, not `O(repo)`.
///
/// This lives here, next to the overlay, but walks the [`TenantView`] because
/// the frontier genuinely spans base+overlay: an edited symbol's callers can
/// be untouched base files, and a newly introduced name's callers are found
/// through the base's frontier index. The overlay alone cannot see them —
/// which is the whole reason a naive per-file update is wrong.
#[must_use]
pub fn update_frontier(view: &super::TenantView<'_>, changed: &[&str], hops: u32) -> Vec<Reached> {
    view.frontier(changed, hops)
}

/// [`update_frontier`] with the §14.2 high-fan-in guard (FR-18): a seed whose
/// direct fan exceeds `high_fanin_threshold` has its cascade clipped to one hop
/// so a widely-referenced signature edit cannot blow the frontier budget. Any
/// bounding is `warn!`-logged here — the cascade is bounded, never silently
/// truncated (§6.2). Returns only the reached set; callers that need the
/// bounded-seed list can call [`super::TenantView::frontier_bounded`] directly.
#[must_use]
pub fn update_frontier_bounded(
    view: &super::TenantView<'_>,
    changed: &[&str],
    hops: u32,
    high_fanin_threshold: usize,
) -> Vec<Reached> {
    let result = view.frontier_bounded(changed, hops, high_fanin_threshold);
    if !result.bounded_seeds.is_empty() {
        tracing::warn!(
            seeds = ?result.bounded_seeds,
            threshold = high_fanin_threshold,
            "high-fan-in cascade bounded to 1 hop (FR-18 §14.2)"
        );
    }
    result.reached
}

impl Overlay {
    /// Record `parsed` as the truth for `rel`. Replaces any previous touch of
    /// the same file; the index is rebuilt from the touched set —
    /// `O(touched)`, which is the §6.2 budget for an overlay mutation.
    pub fn touch(&mut self, rel: &str, parsed: Arc<ParsedFile>) {
        self.files.insert(rel.to_string(), parsed);
        self.rebuild();
    }

    /// Drop the touch of `rel`, so the base resumes answering for it.
    /// Returns whether the file was touched.
    pub fn revert(&mut self, rel: &str) -> bool {
        let was = self.files.remove(rel).is_some();
        if was {
            self.rebuild();
        }
        was
    }

    /// Whether `rel` is touched — i.e. masked from the base.
    #[must_use]
    pub fn is_touched(&self, rel: &str) -> bool {
        self.files.contains_key(rel)
    }

    /// The touched files, unordered.
    pub fn touched(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Number of touched files.
    #[must_use]
    pub fn touched_count(&self) -> usize {
        self.files.len()
    }

    /// The shared parse for a touched `rel` — how the FR-15 sharing is
    /// observable (two tenants holding identical bytes return the same
    /// allocation).
    #[must_use]
    pub fn parsed(&self, rel: &str) -> Option<&Arc<ParsedFile>> {
        self.files.get(rel)
    }

    /// Number of symbols the overlay defines — `O(touched)` by construction,
    /// which the isolation suite asserts against a much larger base.
    #[must_use]
    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }

    /// Overlay definitions of `name`.
    #[must_use]
    pub fn defs(&self, name: &str) -> &[u32] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }

    /// The symbol behind an overlay id.
    #[must_use]
    pub fn symbol(&self, id: u32) -> &OverlaySymbol {
        &self.symbols[id as usize]
    }

    /// Callee names of an overlay symbol (to resolve against the view).
    #[must_use]
    pub fn callee_names(&self, id: u32) -> &[String] {
        &self.callees_of[id as usize]
    }

    /// Overlay callers of `name` — ids of overlay symbols whose file's call
    /// records invoke that name.
    #[must_use]
    pub fn callers_of(&self, name: &str) -> &[u32] {
        self.callers_of.get(name).map_or(&[], Vec::as_slice)
    }

    /// Rebuild the derived index from the touched set.
    fn rebuild(&mut self) {
        self.symbols.clear();
        self.by_name.clear();
        self.callees_of.clear();
        self.callers_of.clear();

        // Deterministic order: sorted by rel path, then definition order.
        let mut rels: Vec<&String> = self.files.keys().collect();
        rels.sort();
        let mut per_file_ids: HashMap<&str, Vec<u32>> = HashMap::new();
        for rel in &rels {
            let parsed = &self.files[*rel];
            let Some(language) = parsed.language else {
                continue;
            };
            for symbol in &parsed.structure.symbols {
                let id = u32::try_from(self.symbols.len()).expect("overlay id fits u32");
                self.symbols.push(OverlaySymbol {
                    name: symbol.name.clone(),
                    kind: symbol.kind.as_str().to_string(),
                    file: (*rel).clone(),
                    start_line: symbol.start_line,
                    tier: symbol.tier,
                    language,
                });
                self.by_name
                    .entry(symbol.name.clone())
                    .or_default()
                    .push(id);
                per_file_ids.entry(rel.as_str()).or_default().push(id);
                self.callees_of.push(Vec::new());
            }
        }
        // Call records: a call in a touched file belongs to the overlay ids
        // of its CALLER name in that same file (intra-file caller attribution,
        // matching the extractor's own scoping).
        for rel in &rels {
            let parsed = &self.files[*rel];
            let ids = per_file_ids
                .get(rel.as_str())
                .map_or(&[][..], Vec::as_slice);
            for call in &parsed.structure.calls {
                for &id in ids {
                    if self.symbols[id as usize].name == call.caller {
                        self.callees_of[id as usize].push(call.callee.clone());
                        self.callers_of
                            .entry(call.callee.clone())
                            .or_default()
                            .push(id);
                    }
                }
            }
        }
    }
}
