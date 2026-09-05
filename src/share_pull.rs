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
use crate::share_client::{align, policy_preview, promote, pull, PolicyPreview, PullVerdict};

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
    /// Which of the publisher's concepts are YOUR concepts?
    ///
    /// Proposes candidate alignments between a pulled graph and one of your
    /// own. A read, and the question that decides whether a share is usable
    /// rather than merely present. Proposes only — deciding and applying an
    /// alignment stays with quipu, where the write lives.
    Align {
        /// The pulled graph — the staging IRI reported by `pull`.
        graph: String,
        /// The local graph to align it against.
        #[arg(long)]
        against: String,
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
            ShareCmd::Align { graph, against, to } => {
                let endpoint = endpoint(to.as_deref(), configured_endpoint)?;
                let body = align(endpoint, graph, against)?;
                self.print_json_or(&body, || {
                    // `candidates` is a COUNT, not a list. Reading it as an
                    // array yields None -> 0 for every response, so the line
                    // would say "0 candidates" however many quipu found — a
                    // display that reads plausibly and is always wrong. Caught
                    // only by inspecting a real response; pinned by a test.
                    let n = body
                        .get("candidates")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    let aside = body
                        .get("set_aside")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    format!(
                        "{n} candidate alignment(s) between {graph} and {against} \
                         ({aside} set aside as ambiguous)"
                    )
                });
                Ok(())
            }
            ShareCmd::Policy { staging_graph, to } => {
                let endpoint = endpoint(to.as_deref(), configured_endpoint)?;
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

/// The endpoint a READ may use: the flag, else the configured one.
///
/// Writes never reach this — `pull` and `promote` take `--to` as a required
/// argument, so the configured endpoint cannot silently authorize one.
fn endpoint<'a>(flag: Option<&'a str>, configured: Option<&'a str>) -> Result<&'a str> {
    flag.or(configured).ok_or_else(|| {
        Error::Share("no quipu endpoint: pass --to, or set `[yupana.quipu] endpoint`".into())
    })
}
