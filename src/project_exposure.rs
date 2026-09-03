//! Repo-exposure resolution — split out of [`crate::project`] purely for file
//! size (the 500-line limit), the same move `project_queries` made. The types
//! stay re-exported from `project`, so every existing path
//! (`project::RepoExposure`) still resolves: a move, not an API change.

use crate::project_queries::EXPOSURE_POLICY_IRI;

/// How exposed is the repo an edit lands in? Three-valued BY DESIGN (the
/// mqnl seam): collapsing "not in the graph" into either answer is the bug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoExposure {
    /// The graph says this repo has a public remote: block-tier rules block.
    Public,
    /// The graph knows this repo and it has no public remote: block-tier
    /// rules DOWNGRADE to warnings — the token is not leaking anywhere, but
    /// saying so keeps the habit honest.
    Internal,
    /// The graph does not know this repo (or could not be asked). Warn AND SAY
    /// SO — never block on a guess, never stay silent on ignorance. Carries
    /// the reason so the verdict can explain itself.
    Unknown(String),
}

/// Ask quipu whether `repo` (by label) is public, via the governed policy's
/// own `/policy/check` — the same signed-verdict seam every other consumer of
/// rule #1 uses, so yupana and the pre-push gate can never disagree about what
/// "public" means. NEVER errors: any failure IS the `Unknown` answer, with the
/// reason carried.
///
/// `outcome` mapping (quipu's three-valued contract):
///   satisfied   -> the repo has a public remote        -> Public
///   unsatisfied -> known repo, no public remote        -> Internal
///   unknown     -> the evidence probe found no repo    -> Unknown
pub fn fetch_repo_exposure(endpoint: &str, repo: &str) -> RepoExposure {
    let url = format!("{}/policy/check", endpoint.trim_end_matches('/'));
    let target = format!("http://aegis.gastown.local/ontology/repo_{repo}");
    let body = serde_json::json!({ "policy": EXPOSURE_POLICY_IRI, "target": target }).to_string();
    let resp = match ureq::post(&url)
        .timeout(crate::project::http_timeout())
        .set("Content-Type", "application/json")
        .send_string(&body)
    {
        Ok(r) => r,
        Err(e) => return RepoExposure::Unknown(format!("POST {url} failed: {e}")),
    };
    let text = match resp.into_string() {
        Ok(t) => t,
        Err(e) => return RepoExposure::Unknown(format!("unreadable /policy/check reply: {e}")),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return RepoExposure::Unknown(format!("/policy/check reply is not JSON: {e}")),
    };
    match value.get("outcome").and_then(|o| o.as_str()) {
        Some("satisfied") => RepoExposure::Public,
        Some("unsatisfied") => RepoExposure::Internal,
        Some("unknown") | None => RepoExposure::Unknown(format!(
            "repo `{repo}` is not in the graph (no `repo_{repo}` entity with remote facts)"
        )),
        Some(other) => RepoExposure::Unknown(format!("unrecognised outcome `{other}`")),
    }
}
