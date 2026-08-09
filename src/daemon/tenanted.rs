//! Tenant-scoped engine methods (yupana #2 daemon wiring): the same query
//! surface as the un-tenanted engine, resolved against the requesting
//! tenant's `base + overlay` view, plus the FR-30 feed (`edit`).
//!
//! Every method returns `None` when the tenant layer is ABSENT (the root is
//! not a git repo, so no commit anchors a shared base) — the HTTP layer turns
//! that into an explicit refusal, never an empty answer. A tenant that simply
//! has no overlay composes the bare base: that IS an answer, served normally.

use std::collections::BTreeSet;

use crate::graph::{reachable_over, Dir, TenantRegistry, TenantView};

use super::wire::{graph_tier, AdvisedSymbol, EditReply};
use super::{
    DefItem, Definitions, FileSymbolItem, FileSymbols, Impact, Neighbors, ReachedItem,
    ResidentEngine,
};

impl ResidentEngine {
    /// Run `f` over the registry's read view for `tenant`, or `None` when the
    /// tenant layer is absent. One lock scope per query; views never escape it.
    fn with_view<T>(&self, tenant: &str, f: impl FnOnce(&TenantView<'_>) -> T) -> Option<T> {
        let lock = self.registry()?;
        let reg = lock.read().ok()?;
        Some(f(&reg.view(tenant)))
    }

    /// Tenant-scoped `/callers` / `/callees`.
    #[must_use]
    pub fn neighbors_for(&self, tenant: &str, symbol: &str, dir: Dir) -> Option<Neighbors> {
        self.with_view(tenant, |view| Neighbors {
            symbol: symbol.to_string(),
            found: view.has_symbol(symbol),
            neighbors: reachable_over(view, symbol, dir, 1)
                .iter()
                .map(super::wire::reached_item)
                .collect(),
            tier: graph_tier(),
        })
    }

    /// Tenant-scoped `/impact`.
    #[must_use]
    pub fn impact_for(&self, tenant: &str, symbol: &str, hops: u32) -> Option<Impact> {
        self.with_view(tenant, |view| {
            let reachable: Vec<ReachedItem> = reachable_over(view, symbol, Dir::Callers, hops)
                .iter()
                .map(super::wire::reached_item)
                .collect();
            let files: BTreeSet<String> = reachable.iter().map(|r| r.file.clone()).collect();
            Impact {
                symbol: symbol.to_string(),
                found: view.has_symbol(symbol),
                hops,
                count: reachable.len(),
                reachable,
                files: files.into_iter().collect(),
                tier: graph_tier(),
            }
        })
    }

    /// Tenant-scoped `/references`.
    #[must_use]
    pub fn references_for(&self, tenant: &str, symbol: &str) -> Option<Definitions> {
        self.with_view(tenant, |view| {
            let defs = view.definitions(symbol);
            Definitions {
                symbol: symbol.to_string(),
                found: !defs.is_empty(),
                count: defs.len(),
                definitions: defs
                    .into_iter()
                    .map(|d| DefItem {
                        file: d.file,
                        kind: d.kind,
                        start_line: d.start_line,
                    })
                    .collect(),
                tier: graph_tier(),
            }
        })
    }

    /// Tenant-scoped `/symbols`.
    #[must_use]
    pub fn symbols_for(&self, tenant: &str, rel: &str) -> Option<FileSymbols> {
        self.with_view(tenant, |view| {
            let symbols = view.file_symbols(rel);
            FileSymbols {
                file: rel.to_string(),
                known: !symbols.is_empty(),
                count: symbols.len(),
                symbols: symbols
                    .into_iter()
                    .map(|s| FileSymbolItem {
                        name: s.name,
                        kind: s.kind,
                        start_line: s.start_line,
                    })
                    .collect(),
                tier: graph_tier(),
            }
        })
    }

    /// The FR-30 feed-and-advise cycle: record `source` as `tenant`'s truth
    /// for `rel`, then compute the post-edit advisory — which of the file's
    /// symbols have callers OUTSIDE it — from the FRESH composed view. `None`
    /// when the tenant layer is absent.
    #[must_use]
    pub fn edit(&self, tenant: &str, rel: &str, source: &str) -> Option<EditReply> {
        let lock = self.registry()?;
        {
            let mut reg = lock.write().ok()?;
            reg.touch(tenant, rel, source);
        }
        let reg = lock.read().ok()?;
        let view = reg.view(tenant);
        // Read the edited file's symbols from the COMPOSED view, not the
        // overlay's parse: an edit that matches the baseline (FR-15 base hit)
        // creates no overlay entry, yet its symbols — the base's — still have
        // an advisory. file_symbols resolves overlay-if-touched, base otherwise.
        let names: Vec<String> = view.file_symbols(rel).into_iter().map(|s| s.name).collect();
        let mut advised = Vec::new();
        let mut files: BTreeSet<String> = BTreeSet::new();
        for name in &names {
            let external: Vec<_> = reachable_over(&view, name, Dir::Callers, 1)
                .into_iter()
                .filter(|caller| caller.file != rel)
                .collect();
            if !external.is_empty() {
                advised.push(AdvisedSymbol {
                    symbol: name.clone(),
                    external_callers: external.len(),
                });
                files.extend(external.into_iter().map(|c| c.file));
            }
        }
        advised.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        advised.dedup();
        Some(EditReply {
            tenant: tenant.to_string(),
            file: rel.to_string(),
            symbols: names.len(),
            advised,
            files: files.into_iter().collect(),
            tier: graph_tier(),
        })
    }

    /// The tenant registry, when the root is a repo.
    #[must_use]
    pub(crate) fn registry(&self) -> Option<&std::sync::RwLock<TenantRegistry>> {
        self.inner.registry.as_ref()
    }
}
