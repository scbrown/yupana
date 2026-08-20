//! `POST /path/check` — the FR-41/FR-42 HTTP surface.
//!
//! A sibling of [`super::state_http`] for the same reason that file is a
//! sibling of [`super::http`]: this route is gated on `golden-path` and the
//! code routes are not, and interleaving feature gates through one router
//! function is how a route ends up mounted on a build that cannot serve it.
//!
//! ## Status codes
//!
//! A check that cannot be answered — no projected paths, an undeclared path,
//! a grammar version this build does not implement — answers **409 Conflict**,
//! never 200-with-no-findings. Same property as the board guard: a caller that
//! gates on HTTP status alone cannot mistake "nothing was loaded" for "this
//! plan conforms".

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use super::ResidentEngine;
use crate::goldenpath::{check, CheckMode, CheckOutcome, ProjectedPath, SubmittedStep};

/// The golden-path endpoint, merged into the daemon router.
pub fn routes() -> Router<ResidentEngine> {
    Router::new().route("/path/check", post(path_check))
}

/// The refusal status: the request is well-formed, but what was supplied makes
/// it unanswerable.
const CANNOT_CHECK: StatusCode = StatusCode::CONFLICT;

/// `POST /path/check` body — the MCP request's wire twin.
#[derive(Debug, Deserialize)]
struct CheckBody {
    /// The `GoldenPath` IRI this work declared it follows.
    follows_path: String,
    /// The steps, as v1 signatures.
    #[serde(default)]
    steps: Vec<SubmittedStep>,
    /// The projected paths, supplied per call (a stale resident copy would
    /// enforce yesterday's blessing while looking current).
    #[serde(default)]
    paths: Vec<ProjectedPath>,
    /// `plan` (default) or `progress`.
    #[serde(default)]
    mode: Option<CheckMode>,
    /// Deny opt-in — only a blessed path in plan mode can deny.
    #[serde(default)]
    deny: Option<bool>,
}

/// `POST /path/check` — FR-41 (progress) / FR-42 (plan).
async fn path_check(
    State(_engine): State<ResidentEngine>,
    Json(req): Json<CheckBody>,
) -> Result<Json<CheckOutcome>, (StatusCode, String)> {
    match check(
        &req.paths,
        &req.follows_path,
        &req.steps,
        req.mode.unwrap_or_default(),
        req.deny.unwrap_or(false),
    ) {
        // The refusal is a STATUS, not a field a caller has to remember to read.
        CheckOutcome::Refused { reason } => Err((CANNOT_CHECK, reason)),
        evaluated @ CheckOutcome::Evaluated(_) => Ok(Json(evaluated)),
    }
}

#[cfg(test)]
#[path = "path_http_test.rs"]
mod path_http_test;
