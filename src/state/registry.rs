//! Per-game / per-faction tenancy — a fog-of-war **security** boundary (FR-39).
//!
//! Yupana's code tenancy is one shared base plus a copy-on-write overlay per
//! developer. Mapped onto games: the tenant is `(game_id, faction_id)`, the
//! shared base is the game's COMMON KNOWLEDGE (map, public treaties, observed
//! sightings), and each faction's overlay is its private intel (own units and
//! bases, unexplored fog, plans).
//!
//! ## Why this is not merely organisation
//!
//! When several factions in one game are LLM-driven, a leak between overlays is
//! not a tidiness bug — it is one player reading another's private intel, and it
//! would be **invisible in results**: the run just looks unusually well-informed.
//! So the isolation is asserted three ways, because one of them alone is not
//! enough:
//!
//! 1. **By construction.** [`StateView`] holds exactly one overlay reference,
//!    chosen here from the tenant key. No API takes two keys; no method reaches
//!    a sibling. A cross-tenant read is unrepresentable rather than checked.
//! 2. **By routing.** A `shared` ingest carrying a faction is refused
//!    ([`super::ingest`]) — the one way private intel could reach the layer
//!    everybody reads.
//! 3. **By count.** [`StateRegistry::isolation_report`] scans the shared bases
//!    for faction-stamped facts anyway. (1) and (2) are arguments; this is a
//!    measurement, and it is the only one of the three that can catch a leak
//!    arriving by a path nobody anticipated.
//!
//! Bases are **per game**, not one per process: two games sharing a base would
//! make every fact in one common knowledge in the other, which is the same leak
//! one level up.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::graph::StateGraph;
use super::guard::{guard, GuardOutcome};
use super::ingest::{apply, IngestReport, IngestRequest};
use super::orders::Order;
use super::overlay::{StateOverlay, StateView};
use super::policy::StatePolicy;
use super::whatif::{whatif, WhatIfOutcome};

/// How many `(game, faction)` tenants one registry will hold before refusing.
///
/// A cap that REFUSES rather than evicting: an evicted faction's overlay is its
/// private intel, and dropping it mid-game would silently return that faction to
/// a fog-free view of the shared base — a correctness change disguised as a
/// resource policy. The code plane evicts because an evicted developer overlay
/// costs a re-touch; this one cannot make that trade.
const DEFAULT_MAX_TENANTS: usize = 64;

/// A tenant: one faction's seat in one game.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TenantKey {
    /// The game.
    pub game_id: String,
    /// The faction within it.
    pub faction_id: String,
}

impl TenantKey {
    /// A key from its two halves.
    #[must_use]
    pub fn new(game_id: impl Into<String>, faction_id: impl Into<String>) -> Self {
        Self {
            game_id: game_id.into(),
            faction_id: faction_id.into(),
        }
    }
}

impl std::fmt::Display for TenantKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.game_id, self.faction_id)
    }
}

/// One faction's overlay, in a status snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FactionStatus {
    /// The faction.
    pub faction_id: String,
    /// Entities its overlay states — `O(delta)`, never board-sized.
    pub overlay_nodes: usize,
    /// Relationships its overlay states or masks.
    pub overlay_edges: usize,
}

/// One game's shared base and its factions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameStatus {
    /// The game.
    pub game_id: String,
    /// Entities in the shared base.
    pub shared_nodes: usize,
    /// Relationships in the shared base.
    pub shared_edges: usize,
    /// The factions holding an overlay, sorted.
    pub factions: Vec<FactionStatus>,
}

/// What `yupana status` reports about the board layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateStatus {
    /// The games resident, sorted.
    pub games: Vec<GameStatus>,
    /// Shared writes refused for carrying a faction, over the registry's life.
    pub fog_leaks_blocked: usize,
}

/// Faction-stamped facts found in a shared base — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IsolationReport {
    /// Entity ids in a shared base whose provenance names a faction, by game.
    pub faction_tagged_shared_entities: BTreeMap<String, BTreeSet<String>>,
    /// Whether anything was found. `false` is the healthy state; it is stated
    /// rather than inferred from an empty map so a reader is not left deciding
    /// whether the scan ran.
    pub leaked: bool,
}

/// The board registry: one shared base per game, one overlay per tenant.
#[derive(Debug)]
pub struct StateRegistry {
    games: BTreeMap<String, StateGraph>,
    overlays: BTreeMap<TenantKey, StateOverlay>,
    empty_base: StateGraph,
    empty_overlay: StateOverlay,
    max_tenants: usize,
    fog_leaks_blocked: usize,
}

impl Default for StateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl StateRegistry {
    /// An empty registry with the default tenant cap.
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_tenants(DEFAULT_MAX_TENANTS)
    }

    /// An empty registry holding at most `max_tenants` tenants.
    #[must_use]
    pub fn with_max_tenants(max_tenants: usize) -> Self {
        Self {
            games: BTreeMap::new(),
            overlays: BTreeMap::new(),
            empty_base: StateGraph::new(),
            empty_overlay: StateOverlay::new(),
            max_tenants,
            fog_leaks_blocked: 0,
        }
    }

    /// Ingest a request on behalf of its `(game_id, faction_id)`.
    ///
    /// The overlay handed to [`apply`] is looked up from the REQUEST's own key,
    /// which is what makes "a tenant writes only its own private layer" true of
    /// the code rather than of the caller's discipline.
    pub fn ingest(&mut self, request: &IngestRequest) -> Result<IngestReport, String> {
        let key = TenantKey::new(&request.game_id, &request.faction_id);
        if !self.overlays.contains_key(&key) && self.overlays.len() >= self.max_tenants {
            return Err(format!(
                "tenant cap reached ({} of {}) — refusing to admit `{key}`. Refusing rather \
                 than evicting: an evicted overlay is a faction's private intel, and dropping \
                 it would silently widen that faction's view to the shared base.",
                self.overlays.len(),
                self.max_tenants
            ));
        }
        let base = self.games.entry(request.game_id.clone()).or_default();
        let overlay = self.overlays.entry(key).or_default();
        // `replace` makes this ingest the WHOLE of the tenant's private layer, rather than a
        // patch on top of whatever it held. An adapter whose world view IS the board needs
        // this: without it a node that has since left the board — a base razed twenty turns
        // ago — survives every later ingest that simply does not mention it, and goes on
        // matching policy selectors forever. That is a second, stale source of board state
        // sitting behind a caller who believes it just stated the current one.
        //
        // Private layer only, deliberately. The shared base is common knowledge and is not
        // this tenant's to clear.
        if request.replace {
            *overlay = StateOverlay::default();
        }
        let report = apply(request, base, overlay);
        self.fog_leaks_blocked += report.fog_leaks_blocked;
        Ok(report)
    }

    /// The composed view for one tenant. A tenant with no overlay — including
    /// one never seen — views its game's bare base; a game never seen views an
    /// empty board, which every consumer refuses rather than treats as clean.
    #[must_use]
    pub fn view(&self, key: &TenantKey) -> StateView<'_> {
        StateView::new(
            self.games.get(&key.game_id).unwrap_or(&self.empty_base),
            Some(self.overlays.get(key).unwrap_or(&self.empty_overlay)),
        )
    }

    /// Run the FR-37 guard for one tenant.
    #[must_use]
    pub fn guard(
        &self,
        key: &TenantKey,
        policies: &[StatePolicy],
        orders: &[Order],
    ) -> GuardOutcome {
        guard(self.base_of(key), self.overlay_of(key), policies, orders)
    }

    /// Run the FR-38 what-if for one tenant.
    #[must_use]
    pub fn whatif(&self, key: &TenantKey, orders: &[Order], hops: u32) -> WhatIfOutcome {
        whatif(self.base_of(key), self.overlay_of(key), orders, hops)
    }

    /// Drop a tenant's overlay — end of game, or a faction leaving. The shared
    /// base is untouched.
    pub fn close_tenant(&mut self, key: &TenantKey) {
        self.overlays.remove(key);
    }

    /// Drop a whole game: its shared base and every faction overlay in it.
    pub fn close_game(&mut self, game_id: &str) {
        self.games.remove(game_id);
        self.overlays.retain(|k, _| k.game_id != game_id);
    }

    /// The board layer as `yupana status` reports it.
    #[must_use]
    pub fn status(&self) -> StateStatus {
        let games = self
            .games
            .iter()
            .map(|(game_id, base)| {
                let (shared_nodes, shared_edges) = base.stats();
                GameStatus {
                    game_id: game_id.clone(),
                    shared_nodes,
                    shared_edges,
                    factions: self
                        .overlays
                        .iter()
                        .filter(|(k, _)| &k.game_id == game_id)
                        .map(|(k, overlay)| {
                            let (overlay_nodes, overlay_edges) = overlay.stats();
                            FactionStatus {
                                faction_id: k.faction_id.clone(),
                                overlay_nodes,
                                overlay_edges,
                            }
                        })
                        .collect(),
                }
            })
            .collect();
        StateStatus {
            games,
            fog_leaks_blocked: self.fog_leaks_blocked,
        }
    }

    /// Scan every shared base for faction-stamped facts — the measurement half
    /// of the isolation argument (see the module docs).
    #[must_use]
    pub fn isolation_report(&self) -> IsolationReport {
        let faction_tagged_shared_entities: BTreeMap<String, BTreeSet<String>> = self
            .games
            .iter()
            .map(|(game_id, base)| (game_id.clone(), base.faction_tagged_ids()))
            .filter(|(_, ids)| !ids.is_empty())
            .collect();
        IsolationReport {
            leaked: !faction_tagged_shared_entities.is_empty(),
            faction_tagged_shared_entities,
        }
    }

    fn base_of(&self, key: &TenantKey) -> &StateGraph {
        self.games.get(&key.game_id).unwrap_or(&self.empty_base)
    }

    fn overlay_of(&self, key: &TenantKey) -> &StateOverlay {
        self.overlays.get(key).unwrap_or(&self.empty_overlay)
    }
}

#[cfg(test)]
#[path = "registry_test.rs"]
mod registry_test;
