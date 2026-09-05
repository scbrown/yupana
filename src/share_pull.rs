//! `yupana share` — pull a Quipu share into the graph yupana reads, see what
//! policy it would contribute, and admit it deliberately.
//!
//! # Why this is REST and not a shell-out to the quipu CLI
//!
//! Camayoc's equivalent wraps the `quipu` binary, and that is right for camayoc:
//! it owns a local store file and the CLI is the surface over it. Yupana owns no
//! store. Every quipu interaction it has — `promote`, the policy projection, the
//! rule catalogue the pre-edit guard reads — is HTTP to an endpoint whose
//! database sits on another host, where a local `--db` path means nothing. So
//! this verb posts to `/import`, exactly as `promote` posts to `/knot`.
//!
//! # The property that makes a share safe to pull, MEASURED
//!
//! quipu stages an import into a named graph — `urn:quipu:import:staging:<hash>`
//! or `…:quarantine:<hash>` when its vocabulary or SHACL checks do not pass —
//! and yupana's policy projection ([`crate::project_queries::POLICY_QUERY`],
//! [`crate::project_queries::TEXT_POLICY_QUERY`]) sends SPARQL with **no `GRAPH`
//! clause**. Whether those two facts compose safely is the whole security
//! question of this verb: if a no-`GRAPH` query saw named-graph triples, then
//! pulling a stranger's share would inject rules into the pre-edit guard of
//! every agent on the host, with no promotion and no operator decision.
//!
//! It does not. Measured 2026-09-05 against the live store, on a quarantine
//! graph that was already there:
//!
//! ```text
//! SELECT ?s ?p ?o WHERE { GRAPH <urn:quipu:import:quarantine:bb6c…> { ?s ?p ?o } }
//!     -> 2 rows   (the staged triples exist)
//! SELECT ?p ?o WHERE { <https://example.org/people/alice> ?p ?o }
//!     -> 0 rows   (the projection's shape cannot see them)
//! CONTROL: SELECT ?s WHERE { ?s a aegis:Directive } LIMIT 2
//!     -> 2 rows   (the instrument is not simply blind)
//! ```
//!
//! The control is the load-bearing line. Two zeroes prove nothing without it —
//! an endpoint that answers nothing at all would produce the same reassuring
//! result, and this is precisely the shape of absence-measured-with-an-unproven-
//! instrument that gets mistaken for a safety property.
//!
//! So a pulled share is INERT until promoted, and this module keeps it that way:
//! **nothing here promotes.** `pull` always stages. Admission is [`ShareCmd::Promote`],
//! a separate act with a separate invocation, because admitting a publisher's
//! facts into the graph your guard enforces from is a governance decision and
//! not a side effect of fetching bytes.
//!
//! # A quarantine is a SUCCESS
//!
//! `pull` exits 0 for `quarantined` as well as `staged`. If staging exited
//! nonzero, every wrapper downstream would treat the correct outcome as a
//! failure, and the eventual "fix" for that is auto-promotion — which is the
//! silent vocabulary widening the quarantine exists to prevent. Nonzero is
//! reserved for a failed verification or an endpoint that would not serve.
//! Pulling from a publisher whose vocabulary you do not govern is the DEFAULT
//! case, not the exceptional one.

use clap::{Args, Subcommand};

use crate::errors::{Error, Result};
use crate::share_bundle::{self, Bundle};

/// `yupana share <pull|promote|policy>`.
#[derive(Debug, Args)]
pub struct ShareArgs {
    #[command(subcommand)]
    cmd: ShareCmd,
    /// Emit the verdict as JSON.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum ShareCmd {
    /// Pull a share bundle into quipu's staging area and report where you stand.
    /// Always stages; never promotes.
    Pull {
        /// Share bundle directory, or a base URL serving `manifest.json`.
        source: String,
        /// Quipu base URL to import into. REQUIRED, and it is the only thing
        /// that authorizes the write: a discovered `[yupana.quipu] endpoint` is
        /// deliberately not enough, on the same reasoning as `yupana promote`
        /// — that key is set host-wide so the pre-edit guard can READ the rule
        /// catalogue, and a read credential must not silently become a write one.
        #[arg(long)]
        to: String,
        /// Actor to attribute the import to in quipu's provenance.
        #[arg(long)]
        actor: Option<String>,
    },
    /// Admit an already-staged share into the graph the projection reads. This
    /// is the governance decision, kept separate from fetching the bytes.
    Promote {
        /// The share id reported by `pull`.
        share_id: String,
        /// Quipu base URL. Required for the same reason as `pull --to`.
        #[arg(long)]
        to: String,
        /// Actor to attribute the promotion to.
        #[arg(long)]
        actor: Option<String>,
    },
    /// What policy would this share contribute if you promoted it?
    ///
    /// Runs yupana's OWN projection queries, scoped into the share's staging
    /// graph, so the answer comes from the same SPARQL and the same decoders the
    /// guard uses — not from a second description of what a policy is. A read,
    /// so it accepts the configured endpoint.
    Policy {
        /// The staging graph IRI reported by `pull`.
        staging_graph: String,
        /// Quipu base URL. Defaults to the configured projection endpoint.
        #[arg(long)]
        to: Option<String>,
    },
}

impl ShareArgs {
    /// Run the subcommand, printing a verdict and returning its exit meaning.
    pub fn run(&self, configured_endpoint: Option<&str>) -> Result<()> {
        match &self.cmd {
            ShareCmd::Pull { source, to, actor } => {
                let verdict = pull(source, to, actor.as_deref())?;
                self.print_pull(&verdict);
                Ok(())
            }
            ShareCmd::Promote {
                share_id,
                to,
                actor,
            } => {
                let body = promote(share_id, to, actor.as_deref())?;
                self.print_json_or(&body, || {
                    format!(
                        "promoted {}: {} triples admitted at tx {}",
                        share_id,
                        body.get("triples")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        body.get("tx_id")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                    )
                });
                Ok(())
            }
            ShareCmd::Policy { staging_graph, to } => {
                let endpoint = to.as_deref().or(configured_endpoint).ok_or_else(|| {
                    Error::Share(
                        "no quipu endpoint: pass --to, or set `[yupana.quipu] endpoint`".into(),
                    )
                })?;
                let preview = policy_preview(endpoint, staging_graph)?;
                self.print_policy(staging_graph, &preview);
                Ok(())
            }
        }
    }

    fn print_json_or(&self, value: &serde_json::Value, human: impl FnOnce() -> String) {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            );
        } else {
            println!("{}", human());
        }
    }

    fn print_pull(&self, v: &PullVerdict) {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&v.as_json()).unwrap_or_default()
            );
            return;
        }
        println!("{}: {} triples from {}", v.outcome, v.triples, v.source);
        println!("  share:   {}", v.share_id);
        println!("  staged:  {}", v.staging_graph);
        if !v.blockers.is_empty() {
            println!("  blocked: {}", v.blockers.join(", "));
        }
        if !v.unmatched.is_empty() {
            println!(
                "  unresolved terms: {} (first: {})",
                v.unmatched.len(),
                v.unmatched[0]
            );
        }
        match (&v.next, &v.next_reason) {
            (Some(next), _) => println!("  next:    {next}"),
            (None, Some(reason)) => println!("  next:    none — {reason}"),
            (None, None) => {}
        }
    }

    fn print_policy(&self, graph: &str, p: &PolicyPreview) {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&p.as_json(graph)).unwrap_or_default()
            );
            return;
        }
        println!("policy this share would contribute to {graph}:");
        println!("  structural policies: {}", p.structural.len());
        for name in &p.structural {
            println!("    - {name}");
        }
        println!("  text rules:          {}", p.text.len());
        for name in &p.text {
            println!("    - {name}");
        }
        if p.structural.is_empty() && p.text.is_empty() {
            println!(
                "  This share carries no policy yupana would enforce. It may still carry \
                 facts — promotion admits those too."
            );
        }
    }
}

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
    fn as_json(&self) -> serde_json::Value {
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
    fn as_json(&self, graph: &str) -> serde_json::Value {
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
#[path = "share_pull_test.rs"]
mod share_pull_test;
