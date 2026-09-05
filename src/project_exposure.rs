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
/// Whether quipu ANSWERED, distinct from what it answered (aegis-q4tt56).
///
/// `RepoExposure::Unknown` conflates two things a CACHE must treat oppositely:
///
/// * **quipu answered "I don't know this repo"** — a real, stable fact about the
///   graph. Safe to cache: asking again in ten seconds gets the same answer.
/// * **we never got an answer** — a timeout, a 502, an unparseable body. Caching
///   this would freeze a transient failure for the whole TTL, converting a blip
///   into hours of degraded enforcement, and it would do so during exactly the
///   quipu trouble that produced it.
///
/// Both still degrade to `Unknown` for the DECISION — a governed rule never
/// blocks on a guess either way. The distinction exists solely so the cache can
/// store what the graph SAID and never store our failure to ask it.
#[derive(Debug, Clone)]
pub enum ExposureAnswer {
    /// quipu responded. Cacheable.
    Answered(RepoExposure),
    /// We never got an answer. NEVER cacheable.
    Unreachable(String),
}

impl ExposureAnswer {
    /// The decision value, which is `Unknown` for both failure shapes.
    #[must_use]
    pub fn exposure(self) -> RepoExposure {
        match self {
            ExposureAnswer::Answered(e) => e,
            ExposureAnswer::Unreachable(why) => RepoExposure::Unknown(why),
        }
    }
}

/// [`fetch_repo_exposure`], keeping whether quipu answered. See [`ExposureAnswer`].
pub fn fetch_exposure_answer(endpoint: &str, repo: &str) -> ExposureAnswer {
    let url = format!("{}/policy/check", endpoint.trim_end_matches('/'));
    let target = format!("http://aegis.gastown.local/ontology/repo_{repo}");
    let body = serde_json::json!({ "policy": EXPOSURE_POLICY_IRI, "target": target }).to_string();
    let resp = match ureq::post(&url)
        .timeout(crate::project::http_timeout())
        .set("Content-Type", "application/json")
        .set("X-Quipu-Client", crate::quipu_label::current())
        .send_string(&body)
    {
        Ok(r) => r,
        Err(e) => return ExposureAnswer::Unreachable(format!("POST {url} failed: {e}")),
    };
    let text = match resp.into_string() {
        Ok(t) => t,
        Err(e) => {
            return ExposureAnswer::Unreachable(format!("unreadable /policy/check reply: {e}"))
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return ExposureAnswer::Unreachable(format!("/policy/check reply is not JSON: {e}"))
        }
    };
    // Every arm below is an ANSWER: quipu replied and we understood the reply,
    // including when the reply is "no such repo". An unrecognised outcome is the
    // one that is NOT an answer — we reached quipu and could not read its verdict,
    // which must not be cached as though it were one.
    match value.get("outcome").and_then(|o| o.as_str()) {
        Some("satisfied") => ExposureAnswer::Answered(RepoExposure::Public),
        Some("unsatisfied") => ExposureAnswer::Answered(RepoExposure::Internal),
        Some("unknown") | None => ExposureAnswer::Answered(RepoExposure::Unknown(format!(
            "repo `{repo}` is not in the graph (no `repo_{repo}` entity with remote facts)"
        ))),
        Some(other) => ExposureAnswer::Unreachable(format!("unrecognised outcome `{other}`")),
    }
}

/// Ask quipu whether `repo` is public. NEVER errors: any failure IS `Unknown`.
#[must_use]
pub fn fetch_repo_exposure(endpoint: &str, repo: &str) -> RepoExposure {
    fetch_exposure_answer(endpoint, repo).exposure()
}
