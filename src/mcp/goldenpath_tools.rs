//! Request DTO for `yupana_path_check` (FR-41/FR-42), defined for both arms of
//! the `golden-path` feature — the tool method is registered on every build,
//! exactly as the board tools are for `game-state`.

use serde::Deserialize;

/// Request for `yupana_path_check` — FR-41 (progress) / FR-42 (plan).
#[cfg(feature = "golden-path")]
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PathCheckRequest {
    /// The declared claim being checked.
    #[schemars(description = "The GoldenPath IRI this work declared it follows")]
    pub follows_path: String,
    /// The steps, as v1 signatures.
    #[serde(default)]
    #[schemars(
        description = "The steps to check, each {action_kind, target_class, label?}. A step \
                       without action_kind is unevaluable: reported, never silently skipped."
    )]
    pub steps: Vec<crate::goldenpath::SubmittedStep>,
    /// The projected paths.
    ///
    /// Supplied per call rather than held resident, exactly like the board
    /// guard's policies: they are blessed in Quipu and projected, and a stale
    /// resident copy would enforce yesterday's blessing while looking current.
    #[serde(default)]
    #[schemars(
        description = "Projected golden paths (from Quipu). Each carries its gp-grammar \
                       version, blessing level, pattern, dead ends, exemplars, and projection \
                       time. Empty while follows_path is declared = REFUSED, never clean."
    )]
    pub paths: Vec<crate::goldenpath::ProjectedPath>,
    /// How to read the steps.
    #[serde(default)]
    #[schemars(
        description = "'plan' (default): the steps are the whole intent; deviation is decidable \
                       and named. 'progress': the work so far; reports progress and hazards, \
                       never denies (under gp-grammar/1 an open trajectory cannot hard-deviate)."
    )]
    pub mode: Option<crate::goldenpath::CheckMode>,
    /// The deny opt-in.
    #[serde(default)]
    #[schemars(
        description = "Opt into deny effects. Only a BLESSED path in plan mode can deny, and \
                       only when this is true; advisory paths warn at most."
    )]
    pub deny: Option<bool>,
}

/// Request for `yupana_path_check` on a build without the engine. Fields are
/// accepted and ignored; the handler refuses.
#[cfg(not(feature = "golden-path"))]
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct PathCheckRequest {
    /// The declared path.
    pub follows_path: String,
}
