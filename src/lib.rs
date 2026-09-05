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
//! (`mcp` feature), LSP definition/reference precision (`lsp`), and Quipu
//! promotion (`quipu`) are feature-gated; the CPG/dataflow tier remains planned.

pub mod action;
#[cfg(feature = "quipu")]
pub mod action_certification;
pub mod attribution;
pub mod audit;
/// The work-item briefing — the CONTEXT consumer of the scope ladder. Gated
/// with the projection it reads.
#[cfg(feature = "quipu")]
pub mod brief;
/// What the graph knows about a path an agent has just stepped OUTSIDE its
/// work item's ground onto — the deviation-seeded counterpart to
/// `brief_sources`. Gated with the projection it reads.
#[cfg(feature = "quipu")]
pub mod brief_deviation;
/// The L0 half of that briefing: the small push, plus a census of what it held
/// back. Gated with `brief`, whose `Brief` it renders.
#[cfg(feature = "quipu")]
pub mod brief_l0;
/// The briefing's sources — each a reused surface of the suite.
#[cfg(feature = "quipu")]
pub mod brief_sources;
pub mod change;
pub mod ci_shift;
pub mod cli;
mod cli_cmds;
pub mod community;
pub mod config;
pub mod constraint;
pub mod daemon;
pub mod dataflow;
pub mod docref;
pub mod errors;
pub mod exemplar;
pub mod export;
pub mod extract;
pub mod git;
/// The golden-path conformance guard (FR-40..FR-42): blessed-trajectory
/// projections and the plan/progress conformance verdict under gp-grammar/1.
/// Gated so a build that cannot check a path does not advertise the tier.
#[cfg(feature = "golden-path")]
pub mod goldenpath;
pub mod graph;
pub mod grounding;
pub mod hook;
pub mod hosting;
/// The action selector for LANDINGS — `git push` / `gh pr merge` — which the
/// governed single-writer policy is written against.
pub mod landing;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod metrics;
pub mod plate;
pub mod policy;
/// The projected work-item maps behind the capability ladder's lower rungs.
pub mod policy_items;
/// Phase-4 projection: a hot, one-directional cache of quipu's structural policies.
#[cfg(feature = "quipu")]
pub mod project;
/// The SPARQL `project` sends. Gated with it: queries for a plane this build
/// does not have are dead weight, and an ungated half of a gated feature is how
/// a `--features` matrix starts lying about what a binary can do.
#[cfg(feature = "quipu")]
pub mod project_decode;
/// Command disk-impact history and the advise-only pre-execution guard.
#[cfg(feature = "quipu")]
pub mod project_disk;
/// Repo-exposure resolution, split from `project` for size and gated with it.
#[cfg(feature = "quipu")]
pub mod project_exposure;
/// Grounding projection (bobbin-tvn), split from `project` for size and gated
/// with it.
#[cfg(feature = "quipu")]
pub mod project_grounding;
/// Governed LANDING policy — who may put code on a repository's protected ref.
/// Holds the owner-of predicate, whose evidence is a graph fact rather than
/// anything readable from the command line. Gated with the projection it reads.
#[cfg(feature = "quipu")]
pub mod project_landing;
/// Governed host-memory command policies, projected from Quipu.
#[cfg(feature = "quipu")]
pub mod project_memory;
pub mod project_queries;
/// Observed work-item scope projection (work-scoped-governance ladder), split
/// from `project` for size and gated with it.
#[cfg(feature = "quipu")]
pub mod project_scope;
/// Governed session trajectory advice.
#[cfg(feature = "quipu")]
pub mod project_trajectory;
/// Governed tripwires — quipu's path-boundary policies, projected. Gated with
/// the projection that serves them.
#[cfg(feature = "quipu")]
pub mod project_tripwire;
/// The DURABLE half of the projection cache — what lets a projection failure
/// degrade to stale-but-enforcing instead of to unguarded (aegis-0upyu).
#[cfg(feature = "quipu")]
pub mod projection_cache;
/// Phase-4 Quipu promotion: SHACL-validate a Turtle projection, then write it.
#[cfg(feature = "quipu")]
pub mod promote;
/// §9.4 branch modeling for promoted facts: the `bobbin:onBranch` qualifier
/// fallback, and a loud refusal for the named-graph design blocked on quipu#36.
pub mod promote_branch;
/// §9.7's `commit → touched entities` provenance edge, produced inside yupana at
/// promotion time (feeds quipu#37's work-item co-occurrence).
pub mod promote_provenance;
/// When promotion runs: `[yupana.quipu] promote_on` × the declared trigger.
/// Ungated on purpose — the policy is meaningful (and testable) in a build
/// without `quipu`, where it decides whether the feature-off refusal is even
/// reached.
pub mod promote_trigger;
pub mod reconcile;
pub mod recurrence;
mod render;
pub mod rules;
/// Reading and verifying a Quipu share bundle before anything is sent anywhere.
#[cfg(feature = "quipu")]
mod share_bundle;
/// The wire half of `yupana share`: quipu calls and the emitted next step.
#[cfg(feature = "quipu")]
mod share_client;
/// `yupana share` — pull a Quipu share, preview its policy, admit it.
#[cfg(feature = "quipu")]
pub mod share_pull;
/// The game-state harness (FR-35..FR-39): a generic in-memory fact graph, a
/// `graph-pattern` policy plane over it, and `(game, faction)` tenancy. Gated so
/// a build that cannot ingest a board does not advertise the tier.
#[cfg(feature = "game-state")]
pub mod state;
pub mod textrules;
pub mod throttle;
pub mod trace;
pub mod tripwire;
pub mod turn_grounding;
pub mod types;
/// Phase-4 verdict signing + promotion (H-PROMOTE-VERDICT).
#[cfg(feature = "quipu")]
pub mod verdict;
#[cfg(feature = "quipu")]
pub mod verdict_spool;
pub mod verify;
pub mod watch;

pub use errors::{Error, Result};
