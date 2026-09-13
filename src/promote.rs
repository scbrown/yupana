//! Phase-4 promotion: validate a Turtle projection in-process, then write it to
//! Quipu (#14 FR-20, #15 FR-19/21/22). Gated behind the `quipu` feature.
//!
//! THE ORDER IS THE CONTRACT. `export::to_turtle` produces the facts; this module
//! SHACL-validates them against the shipped shapes BEFORE any write, and refuses
//! the whole promotion on a single violation (all-or-nothing per commit, §6.3).
//! Validation is always whole-graph. The WRITE is chunked when the payload would
//! exceed Quipu's request-body limit (axum defaults to 2 MiB and the deployed
//! server sets no override — a 2.28 MB projection of the quipu repo itself came
//! back 413, aegis-hbiw): entity blocks are split across multiple `/knot` posts,
//! each under the limit, prefixes replicated. A chunked write is NOT atomic
//! across chunks — if chunk k fails, chunks 0..k are landed — but every IRI is
//! deterministic and `/knot` supersedes, so a re-run converges to the same graph
//! rather than duplicating. The failure message names exactly what landed.
//!
//! WHY IN-PROCESS VALIDATION, NOT QUIPU'S. Quipu exposes `/validate`, and it works.
//! But validating against the same server you are about to write to proves only
//! that the server agrees with itself. FR-20 wants yupana to hold its own copy of the
//! shapes and check independently, so a shape drift between yupana and Quipu is caught
//! at yupana rather than discovered as bad data already in the graph. rudof_lib is
//! that independent checker; `tests/shape_agreement.rs` is the cross-check that
//! the two engines still agree.
//!
//! WHY `/knot` OVER HTTP, NOT THE `quipu` CRATE. FR-21 names three promotion
//! surfaces — `quipu_knot` (MCP) / `POST /knot` (REST) / `Store::transact`
//! (in-process). The REST surface needs no `quipu` crate dependency (still
//! rev-unpinned, Cargo.toml), and yupana explicitly does NOT stand up its own triple
//! store (§14.4). So promotion is an HTTP POST of validated Turtle. `/knot` is
//! bitemporal: a re-promotion of the same facts supersedes rather than duplicating,
//! which is why re-running is idempotent BY TRIPLE COUNT, not by write count.

use std::io::Write;

use crate::errors::{Error, Result};

use promote_chunk::{chunk_turtle, CHUNK_LIMIT};
use promote_payload::{dump_payload, with_payload};

/// The code-edge SHACL shapes yupana ships and validates against. Compiled in so a
/// promotion can never run against shapes that drift from the binary — the file on
/// disk is for humans and the shape-agreement test; THIS is what actually gates a
/// write.
pub const CODE_EDGE_SHAPES: &str = include_str!("../shapes/code-edges.ttl");

/// The outcome of validating a Turtle projection against the code shapes.
#[derive(Debug, Clone)]
pub struct Validation {
    /// Did the projection satisfy every shape?
    pub conforms: bool,
    /// Human-readable violation messages, empty iff `conforms`.
    pub violations: Vec<String>,
}

/// SHACL-validate `data_ttl` against `shapes_ttl`, in-process, via `rudof_lib`.
///
/// Returns the conformance verdict and, when it does not conform, the specific
/// violations. A parse failure of either input is itself a non-conformance we can
/// name, never a silent pass — an unparseable projection must not reach Quipu.
pub fn validate(data_ttl: &str, shapes_ttl: &str) -> Result<Validation> {
    use rudof_lib::formats::{DataFormat, InputSpec, ResultShaclValidationFormat, ShaclFormat};
    use rudof_lib::{Rudof, RudofConfig};

    let mut rudof = Rudof::new(RudofConfig::default());

    rudof
        .load_data()
        .with_data(&[InputSpec::str(data_ttl)])
        .with_data_format(&DataFormat::Turtle)
        .execute()
        .map_err(|e| Error::Promote(format!("promotion data is not valid Turtle: {e}")))?;

    rudof
        .load_shacl_shapes()
        .with_shacl_schema(&InputSpec::str(shapes_ttl))
        .with_shacl_schema_format(&ShaclFormat::Turtle)
        .execute()
        .map_err(|e| Error::Promote(format!("SHACL shapes did not load: {e}")))?;

    rudof
        .validate_shacl()
        .execute()
        .map_err(|e| Error::Promote(format!("SHACL validation failed to run: {e}")))?;

    // The report lives in rudof's private state; serialize it to Turtle and read
    // `sh:conforms` / `sh:resultMessage` out. This is the only exposed path to the
    // verdict — there is no public `conforms()` accessor on Rudof.
    let mut buf: Vec<u8> = Vec::new();
    rudof
        .serialize_shacl_validation_results(&mut buf)
        .with_result_shacl_validation_format(&ResultShaclValidationFormat::Turtle)
        .execute()
        .map_err(|e| Error::Promote(format!("could not read validation report: {e}")))?;
    let report = String::from_utf8_lossy(&buf);

    Ok(parse_report(&report))
}

/// Strip a Turtle object's trailing punctuation and quoting.
///
/// A property line ends `;` mid-block but `.` (or `] .`) on the LAST property of
/// a block, so trimming only `;` leaked the terminator into the value — real
/// promotions logged `MaxCount(1) not satisfied" .` for exactly this reason.
fn turtle_object(raw: &str) -> String {
    raw.trim()
        .trim_end_matches(['.', ';', ']', ' ', '\t'])
        .trim()
        .trim_matches('"')
        .trim()
        .to_string()
}

/// Read one `sh:`-prefixed property's object out of a report line, wherever it
/// sits on that line.
///
/// Matches anywhere rather than at the line start because the FIRST property of
/// a result shares its line with the subject (`_:2 sh:resultSeverity … ;`). The
/// whitespace check after the name is what keeps `sh:result` from matching
/// inside `sh:resultMessage`.
fn report_field(line: &str, name: &str) -> Option<String> {
    let at = line.find(name)?;
    let rest = &line[at + name.len()..];
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let value = turtle_object(rest.split(';').next().unwrap_or(rest));
    let value = value.trim_matches(['<', '>']).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Read `sh:conforms` and the per-result diagnostics out of a SHACL report in
/// Turtle.
///
/// WHY THIS READS MORE THAN `sh:resultMessage`. A SHACL message names the
/// CONSTRAINT and nothing else — "MaxCount(1) not satisfied" is true of every
/// `maxCount` shape in the file and identifies no node, so a refusal built from
/// it alone cannot be acted on. Measured (aegis-o8rq8): the hourly promotion
/// refused every run for a day on one symbol, and the log named neither the
/// symbol, the file, nor the property; the offender was found only by exporting
/// the payload by hand and diffing cardinalities. The report already carries
/// `sh:focusNode` and `sh:resultPath` — Quipu's own `/validate` returns both —
/// so yupana was discarding the two fields that make a refusal actionable.
///
/// Absent fields degrade to the message alone rather than erroring: a report
/// shape we did not anticipate must still surface its violation.
fn parse_report(report: &str) -> Validation {
    let conforms = report.contains("sh:conforms true") || report.contains("sh:conforms  true");
    let mut violations = Vec::new();
    // Collect per RESULT SCOPE rather than per line, because the order of
    // properties inside a result is NOT stable: rudof serializes `sh:resultPath`
    // AFTER `sh:resultMessage` and moves `a sh:ValidationResult` around between
    // runs, so a scanner that emits when it meets the message drops the path,
    // and one that resets on the type line drops the focus node — each
    // intermittently. A scope ends at a statement terminator (`.`) or at the
    // close of a bracketed blank node (`]`), which covers both the flat
    // subject-per-result form rudof emits and the nested `sh:result [ … ]` form
    // the SHACL spec's examples use.
    let mut focus: Option<String> = None;
    let mut path: Option<String> = None;
    let mut message: Option<String> = None;
    let mut flush =
        |focus: &mut Option<String>, path: &mut Option<String>, msg: &mut Option<String>| {
            if let Some(m) = msg.take() {
                if !m.is_empty() {
                    let mut out = m;
                    if let Some(f) = focus.take() {
                        out.push_str(&format!(" — on {f}"));
                    }
                    if let Some(p) = path.take() {
                        out.push_str(&format!(" (path {p})"));
                    }
                    violations.push(out);
                }
            }
            *focus = None;
            *path = None;
        };
    for line in report.lines() {
        if let Some(v) = report_field(line, "sh:focusNode") {
            focus = Some(v);
        }
        if let Some(v) = report_field(line, "sh:resultPath") {
            path = Some(v);
        }
        if let Some(v) = report_field(line, "sh:resultMessage") {
            message = Some(v);
        }
        let end = line.trim_end();
        if end.ends_with('.') || end.contains(']') {
            flush(&mut focus, &mut path, &mut message);
        }
    }
    flush(&mut focus, &mut path, &mut message);
    // Belt and braces: a report that does not conform but whose messages we failed
    // to parse must still be non-empty, or a caller could read "conforms=false,
    // violations=[]" as "nothing wrong". A refusal must always carry a reason.
    if !conforms && violations.is_empty() {
        violations.push("SHACL validation reported non-conformance (see report)".to_string());
    }
    Validation {
        conforms,
        violations,
    }
}

/// The bearer token for Quipu write endpoints, if the environment carries one.
///
/// Quipu gates writes behind `Authorization: Bearer <token>` once its
/// `[quipu.server] auth_token` is set; reads stay open. `QUIPU_AUTH_TOKEN` is
/// the client-side half: set it and every promotion sends the bearer, leave it
/// unset against an open server and nothing changes. An empty value counts as
/// unset — `Bearer ` (no token) would be sent as a real-but-wrong credential
/// and turn a misconfigured env into a confusing 401.
pub(crate) fn quipu_auth_token() -> Option<String> {
    normalize_token(std::env::var("QUIPU_AUTH_TOKEN").ok()).or_else(token_from_file)
}

/// The token file — the half of distribution that reaches processes launched
/// BEFORE the token existed (an env var is captured at spawn; a file is read
/// per request). Env wins above as the per-invocation override. Path:
/// `QUIPU_AUTH_TOKEN_FILE`, else `~/.config/quipu/token`. Absent/unreadable
/// is `None`: no auth configured, the open-server default.
fn token_from_file() -> Option<String> {
    let path = std::env::var("QUIPU_AUTH_TOKEN_FILE")
        .ok()
        .filter(|p| !p.is_empty())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| format!("{h}/.config/quipu/token"))
        })?;
    normalize_token(
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string()),
    )
}

/// The pure half of [`quipu_auth_token`]: empty-or-absent collapses to `None`.
fn normalize_token(raw: Option<String>) -> Option<String> {
    raw.filter(|t| !t.is_empty())
}

#[path = "promote_wire.rs"]
mod promote_wire;
pub use promote_wire::{write_knot, write_knot_snapshot, KnotResult};

/// The aggregated result of a (possibly chunked) promotion write.
#[derive(Debug, Clone)]
pub struct WriteSummary {
    /// Sum of the per-chunk `count` fields — the idempotence signal (a re-run
    /// returns the same total, not a larger one).
    pub count: u64,
    /// Every transaction id Quipu returned, in write order.
    pub tx_ids: Vec<u64>,
    /// How many `/knot` posts the write took (1 = the classic single-post path).
    pub chunks: usize,
    /// Distinct normalized valid-time query keys confirmed by Quipu.
    pub valid_from: Vec<String>,
}

/// The outcome of the pre-write half of a promotion.
enum Prepared {
    /// Conformed; carries the chunks a write would post, in order.
    Ready(Vec<String>),
    /// Did not conform. Always a [`Promotion::Refused`]; the caller returns it
    /// verbatim so the refusal reads identically whether or not a write followed.
    Refused(Promotion),
}

/// Everything a promotion does BEFORE it touches the network: SHACL-validate the
/// whole graph, retain the payload on any failure, and chunk it for the wire.
///
/// Factored out so [`dry_run`] runs the byte-identical gate rather than a second
/// implementation of it. A dry run whose validation could drift from the real
/// one is worse than no dry run at all — it would report a conformance the write
/// path does not honour, which is the same lie in the other direction as the help
/// text this replaces (aegis-o2h97).
fn prepare(turtle: &str, source: &str) -> Result<Prepared> {
    // Every failure path below retains the payload first: the projection is
    // generated on the fly and dropped after the post, so a failure that does
    // not write it out destroys the only copy of the document that failed.
    let v = match validate(turtle, CODE_EDGE_SHAPES) {
        Ok(v) => v,
        Err(Error::Promote(msg)) => {
            let dump = dump_payload(turtle, source);
            return Err(Error::Promote(with_payload(msg, dump.as_deref())));
        }
        Err(e) => return Err(e),
    };
    if !v.conforms {
        return Ok(Prepared::Refused(Promotion::Refused {
            violations: v.violations,
            payload: dump_payload(turtle, source),
        }));
    }
    Ok(Prepared::Ready(chunk_turtle(turtle, CHUNK_LIMIT)?))
}

/// Validate a projection exactly as [`promote`] does and STOP before the write.
///
/// The capability that was missing entirely (aegis-o2h97): there was no way to
/// ask "would this projection conform?" without writing it, so the only way to
/// find out was to promote — 25k+ triples into a live graph, with no undo
/// (`/episode/retract` is episode-scoped and does not unwind a `promote`).
///
/// `endpoint` is for the REPORT only and is optional: validation is in-process,
/// so a dry run needs no target and works from a checkout with no config at all.
/// Naming the endpoint it *would* have written to is the point — that is the fact
/// the operator was missing.
pub fn dry_run(endpoint: Option<&str>, turtle: &str, source: &str) -> Result<Promotion> {
    match prepare(turtle, source)? {
        Prepared::Ready(chunks) => Ok(Promotion::Conforms {
            chunks: chunks.len(),
            bytes: turtle.len(),
            endpoint: endpoint.map(str::to_string),
        }),
        Prepared::Refused(refusal) => Ok(refusal),
    }
}

/// The full promotion: validate the WHOLE graph, then write iff it conforms —
/// in one `/knot` post when it fits, in idempotent chunks when it would 413.
/// On non-conformance it writes NOTHING and returns the violations.
pub fn promote(endpoint: &str, turtle: &str, source: &str) -> Result<Promotion> {
    promote_at(endpoint, turtle, source, None)
}

/// Promote with independent valid-time; transaction time remains server assigned.
pub fn promote_at(
    endpoint: &str,
    turtle: &str,
    source: &str,
    valid_from: Option<&str>,
) -> Result<Promotion> {
    let chunks = match prepare(turtle, source)? {
        Prepared::Ready(chunks) => chunks,
        Prepared::Refused(refusal) => return Ok(refusal),
    };
    let total = chunks.len();
    let mut summary = WriteSummary {
        count: 0,
        tx_ids: Vec::new(),
        chunks: total,
        valid_from: Vec::new(),
    };
    for (i, chunk) in chunks.iter().enumerate() {
        let knot = promote_wire::write_knot_request(endpoint, chunk, source, None, valid_from).map_err(|e| {
            // A server-side refusal names a focus node in a payload only yupana
            // held, so this failure needs the projection retained too.
            let dump = dump_payload(turtle, source);
            Error::Promote(with_payload(
                format!(
                    "chunk {}/{total} result unconfirmed after {} earlier chunk(s) acknowledged; inspect before retrying: {e}",
                    i + 1,
                    i
                ),
                dump.as_deref(),
            ))
        })?;
        if let Some(time) = knot.valid_from {
            if !summary.valid_from.contains(&time) {
                summary.valid_from.push(time);
            }
        }
        summary.count += knot.count;
        if let Some(t) = knot.tx_id {
            summary.tx_ids.push(t);
        }
    }
    Ok(Promotion::Wrote(summary))
}

/// Validate and atomically replace a complete producer snapshot.
///
/// Snapshot writes deliberately use one request rather than the additive
/// chunk path: Quipu accepts bounded 64 MiB bodies, and replacement must never
/// expose a half-old/half-new graph or retract the first chunk when posting the
/// second. A lost response may follow a committed write; verify before retrying.
pub fn promote_snapshot(
    endpoint: &str,
    turtle: &str,
    source: &str,
    snapshot: &str,
) -> Result<Promotion> {
    promote_snapshot_at(endpoint, turtle, source, snapshot, None)
}

/// Replace a snapshot with independent valid-time; retractions retain transaction time.
pub fn promote_snapshot_at(
    endpoint: &str,
    turtle: &str,
    source: &str,
    snapshot: &str,
    valid_from: Option<&str>,
) -> Result<Promotion> {
    match prepare(turtle, source)? {
        Prepared::Refused(refusal) => Ok(refusal),
        Prepared::Ready(_) => {
            let knot = promote_wire::write_knot_request(
                endpoint,
                turtle,
                source,
                Some(snapshot),
                valid_from,
            )
            .map_err(|e| {
                let dump = dump_payload(turtle, source);
                Error::Promote(with_payload(
                    format!("atomic snapshot result unconfirmed; a write may have landed: {e}"),
                    dump.as_deref(),
                ))
            })?;
            Ok(Promotion::Wrote(WriteSummary {
                count: knot.count,
                tx_ids: knot.tx_id.into_iter().collect(),
                chunks: 1,
                valid_from: knot.valid_from.into_iter().collect(),
            }))
        }
    }
}

/// The result of a full promotion: it either wrote, or refused whole.
#[derive(Debug)]
pub enum Promotion {
    /// Validated and written; carries the aggregated write result.
    Wrote(WriteSummary),
    /// Did not pass SHACL; carries the violations and wrote nothing.
    Refused {
        /// Why it was refused, one entry per SHACL result.
        violations: Vec<String>,
        /// Where the refused projection was retained, if it could be written.
        payload: Option<std::path::PathBuf>,
    },
    /// `--dry-run`: passed SHACL and STOPPED. Nothing was written.
    Conforms {
        /// How many `/knot` posts a real write would take.
        chunks: usize,
        /// Size of the projection that would be posted.
        bytes: usize,
        /// The graph a real write would have gone to, when one was resolvable.
        endpoint: Option<String>,
    },
}

impl Promotion {
    /// Render for a human, and set the process exit intent: a refusal is exit-2
    /// (could-not-promote), never a silent success.
    pub fn report(&self, w: &mut impl Write) -> std::io::Result<bool> {
        match self {
            Promotion::Wrote(k) => {
                let txs = match k.tx_ids.as_slice() {
                    [] => String::new(),
                    [one] => format!(" (tx {one})"),
                    [first, .., last] => format!(" (tx {first}..{last})"),
                };
                let chunked = if k.chunks > 1 {
                    format!(" in {} chunks", k.chunks)
                } else {
                    String::new()
                };
                writeln!(w, "  promoted: {} triples present{txs}{chunked}", k.count)?;
                for time in &k.valid_from {
                    writeln!(w, "  valid-from: {time}")?;
                }
                Ok(true)
            }
            Promotion::Refused {
                violations,
                payload,
            } => {
                writeln!(
                    w,
                    "  REFUSED — promotion did not pass SHACL, wrote nothing:"
                )?;
                for v in violations {
                    writeln!(w, "    - {v}")?;
                }
                // The path is the difference between a refusal a reader can act
                // on and one they can only re-observe.
                match payload {
                    Some(p) => writeln!(w, "    payload retained at: {}", p.display())?,
                    None => writeln!(w, "    payload NOT retained (could not write a dump file)")?,
                }
                Ok(false)
            }
            // A conforming dry run is a SUCCESS (exit 0): the question asked was
            // "would this conform?" and the answer is yes. The word WROTE NOTHING
            // is on the line because the whole defect this closes was an operator
            // believing a command was inert when it was not — so the inert one
            // says so out loud rather than reading like a landed promotion.
            Promotion::Conforms {
                chunks,
                bytes,
                endpoint,
            } => {
                writeln!(w, "  DRY RUN — conforms. WROTE NOTHING.")?;
                writeln!(
                    w,
                    "    would post: {bytes} bytes of Turtle in {chunks} chunk(s)"
                )?;
                match endpoint {
                    Some(e) => writeln!(w, "    would target: {e}/knot")?,
                    None => writeln!(
                        w,
                        "    would target: nothing resolved — a real run needs --to <url>"
                    )?,
                }
                Ok(true)
            }
        }
    }
}

#[path = "promote_chunk.rs"]
mod promote_chunk;
#[path = "promote_payload.rs"]
mod promote_payload;

#[cfg(test)]
#[path = "promote_test.rs"]
mod promote_test;
