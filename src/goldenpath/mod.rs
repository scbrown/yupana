//! The golden-path conformance guard (FR-40..FR-42) — enforcement of blessed
//! trajectories, per [docs/golden-path-guard.md](../../docs/golden-path-guard.md).
//!
//! Gated behind the `golden-path` Cargo feature, which joins the CI matrix in
//! the same change (the "don't let a feature ship dark" rule).
//!
//! A golden path is a pruned, human-promoted record of how verified-successful
//! work went, governed in Quipu. This plane evaluates work against it: given a
//! declared `followsPath` and a sequence of steps, answer where the work stands
//! — matched, deviating, hazard-adjacent — under the versioned conformance
//! grammar shared with Quipu's backtest (`gp-grammar/1`,
//! `quipu docs/design/conformance-grammar.md`).
//!
//! ## As built, three choices worth stating
//!
//! - **Paths are supplied per call, like `StatePolicy`.** They are authored
//!   and blessed in Quipu and projected; a stale resident copy would enforce
//!   yesterday's blessing while looking current. "Empty registry" is then a
//!   per-call property: a request declaring `followsPath` whose path is not in
//!   the supplied set is REFUSED, never reported clean.
//! - **Deviation exists only against a complete intent.** Under gp-grammar/1,
//!   gaps are allowed, so an OPEN trajectory never hard-deviates — a future
//!   step could always match next. So `mode: "plan"` treats the submitted
//!   sequence as the whole intent and names the first deviation point;
//!   `mode: "progress"` reports how far along the path the work is and which
//!   hazards it has brushed, and never denies.
//! - **Effects are capped by blessing level.** Advisory paths warn. Blessed
//!   paths warn by default and deny only when the caller opted in AND the mode
//!   is `plan` — the only mode in which deviation is decidable.
//!   Constraint-backing (L5) is out of scope until verdict signing exists.

pub mod check;
pub mod grammar;

pub use check::{check, CheckMode, CheckOutcome, PathCheckReport};
pub use grammar::{
    hazards, match_plan, DeadEnd, PathLevel, PlanMatch, ProjectedPath, StepSig, SubmittedStep,
    GRAMMAR_VERSION,
};
