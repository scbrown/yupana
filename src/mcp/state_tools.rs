//! Request DTOs for the board tools — `yupana_ingest`, `yupana_guard`,
//! `yupana_whatif` (FR-35/37/38).
//!
//! Each type is defined TWICE, once per `game-state` arm. The tool methods
//! themselves are registered unconditionally — `#[tool_router]` references every
//! `#[tool]` method in the impl block — so their parameter types must exist on
//! every build, exactly as `PromoteRequest` does for the `quipu` arm.
//!
//! With the engine compiled in, the DTOs ARE the [`crate::state`] types: one
//! definition of the wire shape, so the MCP schema a client reads and the shape
//! the guard evaluates cannot drift. Without it, they degrade to a minimal stub
//! and the handler returns an honest refusal — the tool says it is not built
//! rather than accepting a board and quietly doing nothing with it.

use serde::Deserialize;

/// Request for `yupana_ingest`.
#[cfg(feature = "game-state")]
pub type StateIngestRequest = crate::state::IngestRequest;

/// Request for `yupana_ingest` on a build without the `game-state` engine. Fields
/// are accepted and ignored; the handler refuses.
#[cfg(not(feature = "game-state"))]
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct StateIngestRequest {
    /// The game these facts belong to.
    pub game_id: String,
    /// The faction whose view is ingesting.
    pub faction_id: String,
}

/// Request for `yupana_guard` — FR-37.
#[cfg(feature = "game-state")]
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StateGuardRequest {
    /// The game.
    #[schemars(description = "Game id — the shared, common-knowledge board to guard against")]
    pub game_id: String,
    /// The faction whose private overlay composes with it.
    #[schemars(description = "Faction id — its private intel composes over the shared board")]
    pub faction_id: String,
    /// The policies to evaluate.
    ///
    /// Passed per call rather than held resident: they are authored in Quipu and
    /// projected, and a stale resident copy would enforce yesterday's governance
    /// while looking current.
    #[serde(default)]
    #[schemars(
        description = "Order-boundary policies to evaluate. selector_lang must be 'graph-pattern'; \
                       'sparql' is reserved for Quipu and is REFUSED here, not approximated."
    )]
    pub policies: Vec<crate::state::StatePolicy>,
    /// The proposed orders, each carrying its declared board effects.
    #[serde(default)]
    #[schemars(
        description = "Proposed orders with their DECLARED effects. Yupana applies exactly these — \
                       it does not infer what an order kind implies."
    )]
    pub orders: Vec<crate::state::Order>,
}

/// Request for `yupana_guard` on a build without the engine.
#[cfg(not(feature = "game-state"))]
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct StateGuardRequest {
    /// The game.
    pub game_id: String,
    /// The faction.
    pub faction_id: String,
}

/// Request for `yupana_whatif` — FR-38.
#[cfg(feature = "game-state")]
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StateWhatIfRequest {
    /// The game.
    #[schemars(description = "Game id")]
    pub game_id: String,
    /// The faction.
    #[schemars(description = "Faction id")]
    pub faction_id: String,
    /// The order set to speculate.
    #[serde(default)]
    #[schemars(description = "Orders to apply speculatively. Nothing is committed.")]
    pub orders: Vec<crate::state::Order>,
    /// Hops to follow; defaults to 3.
    #[serde(default)]
    #[schemars(
        description = "Hops to follow (default 3 — lower than the code plane's 5, because a \
                       board is far denser than a call graph and this is the this-turn path)"
    )]
    pub hops: Option<u32>,
}

/// Request for `yupana_whatif` on a build without the engine.
#[cfg(not(feature = "game-state"))]
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct StateWhatIfRequest {
    /// The game.
    pub game_id: String,
    /// The faction.
    pub faction_id: String,
}
