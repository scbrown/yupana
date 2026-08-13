//! Yupana — an in-memory, multi-tenant code-analysis engine.
//!
//! Yupana extracts precise structure from a codebase (AST, symbols, call graph,
//! and — in later phases — control/data dependence and LSP facts), keeps it hot
//! in memory, and serves it per tenant so a whole team can edit concurrently
//! without corrupting each other's view. It is the third peer in the
//! Bobbin × Yupana × Quipu stack; see `docs/yupana-spec.md` for the full design.
//!
//! This crate is an early Phase-1 skeleton: tree-sitter structural extraction,
//! a config model, a typed fact model, and a CLI. The MCP/HTTP serving layer
//! (`mcp` feature), CPG/dataflow (`cpg`), LSP precision (`lsp`), and Quipu
//! promotion (`quipu`) land in subsequent phases.

pub mod action;
pub mod attribution;
pub mod audit;
pub mod change;
/// The clap CLI. Native-only: the serve command drives a tokio runtime, and a
/// browser has no argv — wasm consumers call the library surface directly.
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
#[cfg(not(target_arch = "wasm32"))]
mod cli_cmds;
pub mod community;
pub mod config;
pub mod constraint;
pub mod daemon;
pub mod dataflow;
pub mod docref;
pub mod errors;
pub mod export;
pub mod extract;
pub mod git;
pub mod graph;
pub mod hook;
pub mod hosting;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod metrics;
pub mod plate;
pub mod policy;
/// Phase-4 projection: a hot, one-directional cache of quipu's structural policies.
#[cfg(feature = "quipu")]
pub mod project;
/// The SPARQL `project` sends. Gated with it: queries for a plane this build
/// does not have are dead weight, and an ungated half of a gated feature is how
/// a `--features` matrix starts lying about what a binary can do.
#[cfg(feature = "quipu")]
pub mod project_decode;
pub mod project_queries;
/// The DURABLE half of the projection cache — what lets a projection failure
/// degrade to stale-but-enforcing instead of to unguarded (aegis-0upyu).
#[cfg(feature = "quipu")]
pub mod projection_cache;
/// Phase-4 Quipu promotion: SHACL-validate a Turtle projection, then write it.
#[cfg(feature = "quipu")]
pub mod promote;
pub mod reconcile;
// Terminal rendering for the CLI — gated with it (its only consumer).
#[cfg(not(target_arch = "wasm32"))]
mod render;
pub mod rules;
/// The game-state harness (FR-35..FR-39): a generic in-memory fact graph, a
/// `graph-pattern` policy plane over it, and `(game, faction)` tenancy. Gated so
/// a build that cannot ingest a board does not advertise the tier.
#[cfg(feature = "game-state")]
pub mod state;
pub mod textrules;
pub mod throttle;
pub mod trace;
pub mod types;
/// Phase-4 verdict signing + promotion (H-PROMOTE-VERDICT).
#[cfg(feature = "quipu")]
pub mod verdict;
#[cfg(feature = "quipu")]
pub mod verdict_spool;
pub mod verify;
/// C-runtime shims for `wasm32-unknown-unknown` (tree-sitter's libc surface).
#[cfg(target_arch = "wasm32")]
mod wasm_shim;
/// File-watch (FR-17). Native-only: `notify` is an inotify/FSEvents adapter and
/// has no wasm backend; in the browser, edits arrive as explicit overlay
/// touches, not filesystem events.
#[cfg(not(target_arch = "wasm32"))]
pub mod watch;

pub use errors::{Error, Result};
