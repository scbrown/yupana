//! The wire half of `yupana share`: talking to quipu, and deciding what to tell
//! the operator to do next.
//!
//! Split from [`crate::share_pull`], which is the clap surface and the printing.
//! The reasoning for the whole verb — why REST rather than a shell-out, and the
//! measured property that a staged share is inert until promoted — lives there.

use crate::errors::{Error, Result};
use crate::share_bundle::{self, Bundle};

/// What a pull found, in the terms the operator has to decide on.
#[derive(Debug, Clone)]
pub(crate) struct PullVerdict {
    pub(crate) outcome: String,
    pub(crate) source: String,
    pub(crate) share_id: String,
    pub(crate) staging_graph: String,
    pub(crate) triples: u64,
    pub(crate) blockers: Vec<String>,
    pub(crate) unmatched: Vec<String>,
    pub(crate) next: Option<String>,
    pub(crate) next_reason: Option<String>,
}

impl PullVerdict {
    pub(crate) fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "outcome": self.outcome,
            "source": self.source,
            "verified": true,
            "share_id": self.share_id,
            "staging_graph": self.staging_graph,
            "triples": self.triples,
            "blockers": self.blockers,
            "unmatched": self.unmatched,
            "next": self.next,
            "next_reason": self.next_reason,
        })
    }
}

/// Read, verify, and stage a share. Never promotes.
pub(crate) fn pull(source: &str, to: &str, actor: Option<&str>) -> Result<PullVerdict> {
    let bundle = share_bundle::read(source)?;
    // Local verification BEFORE the endpoint is touched: a corrupt bundle is a
    // local story and should not become a store write's problem.
    share_bundle::verify(&bundle)?;
    let body = post(
        to,
        "/import",
        &import_request(&bundle, actor),
        "import a share",
    )?;
    Ok(verdict_from(&bundle, source, to, &body))
}

fn import_request(bundle: &Bundle, actor: Option<&str>) -> serde_json::Value {
    let mut req = serde_json::json!({
        "manifest": bundle.manifest,
        "export_ntriples": bundle.export_nt,
        "shapes_turtle": bundle.shapes_ttl,
        "source": bundle.source,
    });
    if let Some(actor) = actor {
        req["actor"] = serde_json::Value::String(actor.to_string());
    }
    req
}

fn verdict_from(bundle: &Bundle, source: &str, to: &str, body: &serde_json::Value) -> PullVerdict {
    let str_at = |k: &str| {
        body.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let blockers: Vec<String> = body
        .get("promotion")
        .and_then(|p| p.get("blockers"))
        .and_then(|b| b.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let unmatched: Vec<String> = body
        .get("resolution")
        .and_then(|r| r.get("unmatched"))
        .and_then(|u| u.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let triples = body
        .get("triples")
        .and_then(|t| {
            t.get("accepted")
                .and_then(serde_json::Value::as_u64)
                .zip(t.get("quarantined").and_then(serde_json::Value::as_u64))
        })
        .map_or(0, |(a, q)| a + q);
    let share_id = {
        let from_body = str_at("share_id");
        if from_body.is_empty() {
            bundle.share_id().unwrap_or_default().to_string()
        } else {
            from_body
        }
    };
    let staging_graph = str_at("staging_graph");
    let (next, next_reason) = next_step(
        to,
        &share_id,
        &staging_graph,
        &blockers,
        bundle.has_shapes(),
    );
    PullVerdict {
        outcome: str_at("outcome"),
        source: source.to_string(),
        share_id,
        staging_graph,
        triples,
        blockers,
        unmatched,
        next,
        next_reason,
    }
}

/// The literal command to run next, or an honest absence.
///
/// Every branch that emits a command emits one that RUNS: the current
/// executable's own path (not a bare name, which resolves only from whichever
/// directory you happened to be in) and the endpoint that just worked (not a
/// literal `quipu`, which on a host with an old CLI is the exact command this
/// verb's existence is an argument against).
fn next_step(
    to: &str,
    share_id: &str,
    staging_graph: &str,
    blockers: &[String],
    has_shapes: bool,
) -> (Option<String>, Option<String>) {
    let me = self_command();
    if share_id.is_empty() {
        return (
            None,
            Some("quipu reported no share id, so there is nothing to name in a follow-up".into()),
        );
    }
    if blockers.is_empty() {
        return (
            Some(format!("{me} share promote {share_id} --to {to}")),
            None,
        );
    }
    if blockers.iter().any(|b| b == "off_vocabulary") {
        if !has_shapes {
            return (
                None,
                Some(format!(
                    "this share is blocked on {blockers:?} and ships an EMPTY shapes.ttl, so \
                     there is no bundled vocabulary to adopt. Govern these types in this quipu \
                     first, or ask the publisher to share WITH shapes."
                )),
            );
        }
        // The share carries the vocabulary it needs. Adopting it is a governance
        // change to this store and stays an explicit, separate act — so what is
        // offered is the INSPECTION, not the adoption.
        return (
            Some(format!("{me} share policy {staging_graph} --to {to}")),
            Some(
                "blocked on vocabulary this quipu does not govern. The bundle ships its own \
                 shapes.ttl; adopting a publisher's vocabulary is a governance change, so see \
                 what it would contribute first."
                    .into(),
            ),
        );
    }
    (
        None,
        Some(format!(
            "blocked on {blockers:?}, which this verb has no automatic remedy for"
        )),
    )
}

/// This executable, as something a reader can paste.
fn self_command() -> String {
    std::env::current_exe().map_or_else(|_| "yupana".into(), |p| p.display().to_string())
}

/// Admit a staged share into the graph the projection reads.
pub(crate) fn promote(share_id: &str, to: &str, actor: Option<&str>) -> Result<serde_json::Value> {
    let mut req = serde_json::json!({ "share_id": share_id });
    if let Some(actor) = actor {
        req["actor"] = serde_json::Value::String(actor.to_string());
    }
    post(to, "/import/promote", &req, "promote a staged share")
}

/// The policy a staged share would contribute, read through yupana's own queries.
#[derive(Debug, Default, Clone)]
pub(crate) struct PolicyPreview {
    pub(crate) structural: Vec<String>,
    pub(crate) text: Vec<String>,
}

impl PolicyPreview {
    pub(crate) fn as_json(&self, graph: &str) -> serde_json::Value {
        serde_json::json!({
            "staging_graph": graph,
            "structural_policies": self.structural,
            "text_rules": self.text,
            "enforced_now": false,
            "note": "A staged graph is INERT: yupana's projection queries carry no GRAPH \
                     clause and cannot see it. These take effect only after `share promote`.",
        })
    }
}

pub(crate) fn policy_preview(endpoint: &str, staging_graph: &str) -> Result<PolicyPreview> {
    let structural = crate::project_decode::decode_policies(&crate::project::query(
        endpoint,
        &scope_to_graph(crate::project_queries::POLICY_QUERY, staging_graph)?,
    )?)
    .map_err(|e| Error::Share(format!("decoding staged structural policy: {e}")))?;
    let text = crate::project_decode::decode_text_rules(&crate::project::query(
        endpoint,
        &scope_to_graph(crate::project_queries::TEXT_POLICY_QUERY, staging_graph)?,
    )?)
    .map_err(|e| Error::Share(format!("decoding staged text rules: {e}")))?;
    Ok(PolicyPreview {
        structural: structural.iter().map(policy_name).collect(),
        text: text.iter().map(|r| r.name.clone()).collect(),
    })
}

fn policy_name(p: &crate::project::ProjectedPolicy) -> String {
    format!("{} [{}]", p.rule.name, p.effect)
}

/// Wrap a projection query's WHERE body in `GRAPH <iri> { … }`.
///
/// A TRANSFORM of the real constant rather than a second query written to
/// resemble it. The catalogue's definition of "a policy" is intricate — atoms,
/// tiers, a fistful of OPTIONALs — and a hand-written preview query would be a
/// second source of truth for it that drifts silently, showing an operator a
/// different set of rules from the one the guard will actually enforce. Reusing
/// the constant means the preview is wrong only if the guard is wrong too.
fn scope_to_graph(query: &str, graph: &str) -> Result<String> {
    if graph.is_empty() || graph.contains('>') {
        return Err(Error::Share(format!(
            "{graph:?} is not usable as a graph IRI"
        )));
    }
    const MARK: &str = "WHERE {";
    let open = query
        .find(MARK)
        .map(|i| i + MARK.len())
        .ok_or_else(|| Error::Share("projection query has no WHERE clause to scope".into()))?;
    let close = query
        .rfind('}')
        .filter(|c| *c > open)
        .ok_or_else(|| Error::Share("projection query has no closing brace".into()))?;
    Ok(format!(
        "{}\n  GRAPH <{graph}> {{{}}}\n{}",
        &query[..open],
        &query[open..close],
        &query[close..]
    ))
}

/// Candidate alignments between a pulled graph and one of your own.
///
/// This is the third thing a consumer needs and the one a bare import cannot
/// give them: a share arrives full of the publisher's IRIs, and the question
/// that decides whether it is USABLE is which of their concepts are your
/// concepts. quipu's `/align/propose` answers it, and it is a READ that needs
/// no bearer — so a consumer can ask before committing to anything.
///
/// **A zero here means "no candidates", never "I could not see the graph".**
/// quipu REFUSES an unknown graph IRI rather than proposing across it, because
/// a mistyped IRI otherwise returns 0 candidates, which is indistinguishable
/// from two graphs that genuinely share nothing (quipu #159 / aegis-19o403).
/// Yupana relays that refusal rather than flattening it into an empty result.
pub(crate) fn align(endpoint: &str, graph_a: &str, graph_b: &str) -> Result<serde_json::Value> {
    post(
        endpoint,
        "/align/propose",
        &serde_json::json!({ "graph_a": graph_a, "graph_b": graph_b }),
        "propose alignments",
    )
}

/// Publish a graph back out as a share bundle, written to `out`.
///
/// The parent is settled by the CALLER, not here, so the decision is visible at
/// the CLI surface where an operator can see it — see [`crate::share_reshare`].
pub(crate) fn reshare(
    endpoint: &str,
    graph: &str,
    parent: Option<&str>,
    shapes: &[String],
    no_shapes: bool,
    out: &std::path::Path,
) -> Result<(serde_json::Value, Vec<String>)> {
    let body = crate::share_reshare::request(graph, parent, shapes, no_shapes);
    let payload = post(endpoint, "/share", &body, "author a share")?;
    let files = crate::share_reshare::write_bundle(&payload, out)?;
    Ok((payload, files))
}

fn post(
    base: &str,
    route: &str,
    body: &serde_json::Value,
    what: &str,
) -> Result<serde_json::Value> {
    let url = format!("{}{route}", base.trim_end_matches('/'));
    let mut req = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(300))
        .set("Content-Type", "application/json")
        // quipu attributes request time per caller and falls back to
        // User-Agent, which collapses every tool into one bucket.
        .set("X-Quipu-Client", "yupana-share");
    if let Some(token) = crate::promote::quipu_auth_token() {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    match req.send_string(&body.to_string()) {
        Ok(resp) => {
            let text = resp
                .into_string()
                .map_err(|e| Error::Share(format!("reading {route} response: {e}")))?;
            serde_json::from_str(&text)
                .map_err(|e| Error::Share(format!("{route} returned non-JSON ({e}): {text}")))
        }
        // A 404 here is not "the import failed" — it is an endpoint too old to
        // have the route, and saying so is the difference between a reader
        // upgrading their quipu and a reader concluding sharing is broken.
        Err(ureq::Error::Status(404, _)) => Err(Error::Share(format!(
            "{url} has no {route} route, so this quipu cannot {what}. The share import \
             surface landed in quipu 0.3.30; ask this deployment's owner for its version \
             (GET {}/version).",
            base.trim_end_matches('/')
        ))),
        Err(ureq::Error::Status(401 | 403, _)) => Err(Error::Share(format!(
            "{url} refused the credential. {route} is a WRITE endpoint and needs quipu's \
             bearer: set QUIPU_AUTH_TOKEN, or QUIPU_AUTH_TOKEN_FILE / ~/.config/quipu/token. \
             Reads on this endpoint stay open, so a working `yupana impact` proves nothing \
             about this."
        ))),
        Err(ureq::Error::Status(code, resp)) => {
            let detail = resp.into_string().unwrap_or_default();
            Err(Error::Share(format!(
                "{url} refused with {code}: {}",
                detail.trim()
            )))
        }
        Err(e) => Err(Error::Share(format!("POST {url} failed: {e}"))),
    }
}

#[cfg(test)]
#[path = "share_client_test.rs"]
mod share_client_test;
