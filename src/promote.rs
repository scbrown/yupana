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
//! that independent checker; `scripts/check-code-shapes.sh` is the cross-check that
//! the two engines still agree.
//!
//! WHY `/knot` OVER HTTP, NOT THE `quipu` CRATE. FR-21 names three promotion
//! surfaces — `quipu_knot` (MCP) / `POST /knot` (REST) / `Store::transact`
//! (in-process). The REST surface needs no `quipu` crate dependency (still
//! rev-unpinned, Cargo.toml), and yupana explicitly does NOT stand up its own triple
//! store (§14.4). So promotion is an HTTP POST of validated Turtle. `/knot` is
//! bitemporal: a re-promotion of the same facts supersedes rather than duplicating,
//! which is why re-running is idempotent BY TRIPLE COUNT, not by write count.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;

use crate::errors::{Error, Result};

/// The code-edge SHACL shapes yupana ships and validates against. Compiled in so a
/// promotion can never run against shapes that drift from the binary — the file on
/// disk is for humans and `check-code-shapes.sh`; THIS is what actually gates a
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
fn quipu_auth_token() -> Option<String> {
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

/// Post validated Turtle to Quipu's `/knot`. Returns the number of triples the
/// transaction reports as present for these facts — the count that makes
/// idempotence checkable (a re-promotion returns the same count, not a larger one).
///
/// `endpoint` is the Quipu base URL (e.g. from `--to` / config); this appends
/// `/knot`. NEVER defaulted to a hardcoded host — a promotion that silently picks a
/// graph is how facts land in the wrong one.
pub fn write_knot(endpoint: &str, turtle: &str, source: &str) -> Result<KnotResult> {
    write_knot_request(endpoint, turtle, source, None)
}

/// Atomically replace one stable producer snapshot through `/knot`.
pub fn write_knot_snapshot(
    endpoint: &str,
    turtle: &str,
    source: &str,
    snapshot: &str,
) -> Result<KnotResult> {
    write_knot_request(endpoint, turtle, source, Some(snapshot))
}

fn write_knot_request(
    endpoint: &str,
    turtle: &str,
    source: &str,
    snapshot: Option<&str>,
) -> Result<KnotResult> {
    let url = format!("{}/knot", endpoint.trim_end_matches('/'));
    let auth = quipu_auth_token();
    // Provenance on every write (promotion tail item 4): quipu records actor +
    // source per transaction; an anonymous writer is unauditable, and yupana was
    // the only anonymous one left.
    let mut body = serde_json::json!({
        "turtle": turtle,
        "actor": "yupana",
        "source": source
    });
    if let Some(key) = snapshot {
        body["replace_snapshot"] = serde_json::Value::Bool(true);
        body["snapshot"] = serde_json::Value::String(key.to_string());
    }
    let body = body.to_string();

    // Quipu is known to flap (transient 503 "no available server", recovering in
    // seconds). Ride through TRANSIENT failures — 5xx and transport errors — with
    // a short backoff; a 4xx is a real answer and fails immediately. The
    // all-or-nothing guarantee is unaffected: every attempt is the same full
    // idempotent write, and exhausting retries still fails loud, never partial.
    const ATTEMPTS: u32 = 3;
    let mut resp = None;
    let mut last_err = String::new();
    for attempt in 1..=ATTEMPTS {
        let mut req = ureq::post(&url).set("Content-Type", "application/json");
        if let Some(token) = &auth {
            req = req.set("Authorization", &format!("Bearer {token}"));
        }
        match req.send_string(&body) {
            Ok(r) => {
                resp = Some(r);
                break;
            }
            Err(ureq::Error::Status(code, _)) if code < 500 => {
                return Err(Error::Promote(format!("POST {url} failed: status {code}")));
            }
            Err(e) => {
                last_err = e.to_string();
                if attempt < ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_secs(2 * u64::from(attempt)));
                }
            }
        }
    }
    let resp = resp.ok_or_else(|| {
        Error::Promote(format!(
            "POST {url} failed after {ATTEMPTS} attempts (transient errors retried): {last_err}"
        ))
    })?;

    let text = resp
        .into_string()
        .map_err(|e| Error::Promote(format!("could not read /knot response: {e}")))?;
    // Quipu can REFUSE the write server-side: its persistent shape registry,
    // when loaded, validates independently of yupana's in-process gate, and a
    // shape the server holds that yupana's copy lacks surfaces HERE as HTTP 200
    // with conforms:false (seen live: a stored symbolKind maxCount(1) refused
    // a projection yupana's shapes accepted). That is a real refusal and must
    // read as one — not as a JSON parse error on a missing `count` field.
    if let Ok(refusal) = serde_json::from_str::<KnotRefusal>(&text) {
        if !refusal.conforms {
            let issues = refusal
                .issues
                .iter()
                .map(|i| format!("{} {} on {}", i.component, i.message, i.focus_node))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(Error::Promote(format!(
                "quipu refused the write (server-side SHACL, {} violation(s)): {issues}. \
                 yupana's own shapes ACCEPTED this projection — the two shape sets have \
                 drifted; reconcile shapes/code-edges.ttl with quipu's stored registry.",
                refusal.violations
            )));
        }
    }
    let parsed: KnotResult = serde_json::from_str(&text)
        .map_err(|e| Error::Promote(format!("unexpected /knot response {text:?}: {e}")))?;
    Ok(parsed)
}

/// Quipu's `/knot` refusal shape (HTTP 200, `conforms:false`).
#[derive(Debug, serde::Deserialize)]
struct KnotRefusal {
    conforms: bool,
    #[serde(default)]
    violations: u64,
    #[serde(default)]
    issues: Vec<KnotIssue>,
}

#[derive(Debug, serde::Deserialize)]
struct KnotIssue {
    #[serde(default)]
    component: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    focus_node: String,
}

/// Quipu `/knot` response. `conforms` here is Quipu's OWN field and is NOT the
/// validation gate — Quipu's persistent shape registry may be empty, in which case
/// it reports `conforms:true` for anything. yupana's gate is [`validate`] above,
/// which ran before this. `count` is the load-bearing field for idempotence.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct KnotResult {
    /// Triples present for these facts after the write — the idempotence signal.
    pub count: u64,
    /// Quipu's monotonic transaction id, when returned.
    #[serde(default)]
    pub tx_id: Option<u64>,
}

/// Write the projection that failed to disk, and return where it landed.
///
/// WHY A FAILED PROMOTION MUST LEAVE ITS PAYLOAD BEHIND. The Turtle is generated,
/// posted and dropped; when it does not parse or does not conform, the document
/// that failed no longer exists anywhere. The parse errors are positional —
/// "line 8656 between columns 1 and 97" — so without the payload the one fact the
/// error gives you is unusable, and the line MOVES between runs because the
/// content is regenerated. Measured (aegis-o8rq8): a scheduled promotion failed
/// every hour for a day and the payload's absence was most of why diagnosing it
/// was hard.
///
/// Best-effort BY DESIGN: a promotion already failing must not fail differently
/// because a dump could not be written, so every error here collapses to `None`
/// and the caller reports the original failure without a path.
///
/// The filename is derived from `source`, which for a CLI promotion carries the
/// repo AND the resolved commit. So the hourly case — the same commit refused
/// over and over because the marker did not advance — overwrites ONE file rather
/// than growing without bound; a diagnostic that fills a disk is its own outage.
/// Across DIFFERENT failing commits it is one dump each, deliberately: the SHA is
/// what tells you which projection you are holding, and reusing one name would
/// overwrite the payload you were still reading. That bound is
/// distinct-failing-commits, not runs. `YUPANA_PROMOTE_DUMP_DIR` overrides the
/// temp-dir default.
fn dump_payload(turtle: &str, source: &str) -> Option<std::path::PathBuf> {
    let dir = std::env::var("YUPANA_PROMOTE_DUMP_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    dump_payload_to(&dir, turtle, source)
}

/// [`dump_payload`] with the directory passed in.
///
/// Split so the whole of the write — the naming, the directory creation, the
/// best-effort contract — is testable without setting `YUPANA_PROMOTE_DUMP_DIR`:
/// parallel tests race on env vars, and this crate denies `unsafe_code`, which
/// `std::env::set_var` now requires.
fn dump_payload_to(
    dir: &std::path::Path,
    turtle: &str,
    source: &str,
) -> Option<std::path::PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("yupana-promote-{}.ttl", payload_slug(source)));
    std::fs::write(&path, turtle).ok()?;
    Some(path)
}

/// A filesystem-safe stem for a dump file, from the promotion's `source` string.
///
/// Anything outside `[A-Za-z0-9-]` becomes `-`, so a source carrying a path, a
/// URL or a shell metacharacter cannot escape the dump directory or produce a
/// name the shell would re-interpret. `.` is excluded too, deliberately: keeping
/// it would let a source spelling `..` survive into the name, which is harmless
/// only for as long as no separator ever survives with it. Bounded length keeps
/// the name under filesystem limits; an empty result falls back to a constant
/// rather than to a bare extension.
fn payload_slug(source: &str) -> String {
    let mut s: String = source
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    s.truncate(80);
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "payload".to_string()
    } else {
        s
    }
}

/// Append the retained-payload path to a failure message, when one was written.
fn with_payload(message: String, payload: Option<&std::path::Path>) -> String {
    match payload {
        Some(p) => format!("{message}\n  payload retained at: {}", p.display()),
        None => message,
    }
}

/// Stay safely under axum's 2 MiB default body limit: the Turtle is JSON-string
/// encoded (quotes/newlines escape to two bytes) before it travels, so leave
/// headroom for that inflation plus the JSON envelope.
const CHUNK_LIMIT: usize = 1_500_000;

/// Predicates whose SHACL property shape carries `sh:class`, so their object
/// must be TYPED IN THE PAYLOAD BEING VALIDATED. The store already holding that
/// type does not count.
///
/// Measured (aegis-sd5fj), the same `CodeSymbol` posted twice against the live
/// server: alone it is refused with `sh:class Class constraint not satisfied for
/// class CodeModule`; with its module's `a bobbin:CodeModule` triple in the same
/// payload it conforms. The module was already typed in the store for both. So a
/// chunk boundary that separates a module from its symbols refuses every symbol
/// after it — 2315 of them on the quipu repo's 71-chunk projection, at chunk
/// 2/71.
///
/// This is a second copy of a rule that lives in `shapes/code-edges.ttl`, and a
/// duplicated rule is how the neighbouring export bug got in. So it is CHECKED,
/// not merely written down: `class_constrained_predicates_match_the_shapes`
/// derives the set from the compiled shapes and fails on any drift — including
/// the moment one of that file's `TIGHTEN LATER: sh:class` comments is enabled,
/// which would otherwise reintroduce this bug on `calls`/`references`/`imports`.
const CLASS_CONSTRAINED_PREDICATES: &[&str] = &["bobbin:definedIn"];

/// The subject an `<IRI> a <Type>` statement types, if it is one.
fn typed_subject(stmt: &str) -> Option<&str> {
    let rest = stmt.strip_prefix('<')?;
    let gt = rest.find('>')?;
    let after = rest[gt + 1..].trim_start();
    let types = after
        .strip_prefix('a')
        .is_some_and(|r| r.starts_with(char::is_whitespace));
    types.then(|| &rest[..gt])
}

/// Index every type-declaring statement in `turtle` by the subject it types.
///
/// Statement granularity, not block granularity, on purpose: `to_turtle` gives
/// each module its own blank-line-separated block but runs all symbols together
/// into one contiguous block, so a block-keyed index could carry a module but
/// never an individual symbol — and `calls`/`references` are one shape edit away
/// from needing symbols.
fn definition_statements(turtle: &str) -> HashMap<&str, &str> {
    let mut defs = HashMap::new();
    let mut start = 0usize;
    let mut pos = 0usize;
    for line in turtle.split_inclusive('\n') {
        pos += line.len();
        if line.trim().is_empty() {
            start = pos;
            continue;
        }
        if line.trim_end().ends_with('.') {
            let stmt = turtle[start..pos].trim();
            if let Some(iri) = typed_subject(stmt) {
                defs.insert(iri, stmt);
            }
            start = pos;
        }
    }
    defs
}

/// Every IRI `text` names as the object of a class-constrained predicate.
fn class_constrained_objects(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for pred in CLASS_CONSTRAINED_PREDICATES {
        let mut rest = text;
        while let Some(i) = rest.find(pred) {
            rest = &rest[i + pred.len()..];
            let Some(lt) = rest.find('<') else { break };
            // Only the immediate object counts: anything but whitespace between
            // the predicate and the `<` means this IRI belongs to some other
            // statement.
            if !rest[..lt].trim().is_empty() {
                continue;
            }
            let Some(gt) = rest[lt + 1..].find('>') else {
                break;
            };
            out.push(&rest[lt + 1..lt + 1 + gt]);
            rest = &rest[lt + 1 + gt..];
        }
    }
    out
}

/// Assembles chunks that are each SELF-SUFFICIENT for SHACL: a chunk carries the
/// type declaration of everything it references through a class-constrained
/// predicate, repeated per chunk if need be (aegis-sd5fj, option 2).
///
/// Repetition rather than module-cohesive packing, on the measurement: ~85% of
/// this payload is `bobbin:calls` edges that by construction CROSS modules, so
/// cohesion cannot bound chunk size — the boundary just moves and the same class
/// of failure resurfaces elsewhere. A repeated definition is a few hundred bytes
/// against a 296k-edge payload, and it is immune to where the boundaries land.
///
/// Duplicate definitions across chunks are safe: `/knot` is bitemporal and every
/// IRI here is deterministic, so a re-declared module supersedes rather than
/// forking (the same property that makes a re-run of a partly-landed promotion
/// safe).
struct ChunkAssembler<'a> {
    header: &'a str,
    limit: usize,
    defs: HashMap<&'a str, &'a str>,
    cur: String,
    /// Definitions physically present in `cur`.
    present: HashSet<&'a str>,
    /// Definitions `cur` references but does not contain — appended on flush.
    /// Ordered so a chunk's bytes are a function of its content alone.
    needed: BTreeSet<&'a str>,
    needed_bytes: usize,
    chunks: Vec<String>,
}

impl<'a> ChunkAssembler<'a> {
    fn new(header: &'a str, limit: usize, defs: HashMap<&'a str, &'a str>) -> Self {
        Self {
            header,
            limit,
            defs,
            cur: String::from(header),
            present: HashSet::new(),
            needed: BTreeSet::new(),
            needed_bytes: 0,
            chunks: Vec::new(),
        }
    }

    /// What a definition costs when appended to a chunk.
    fn cost(&self, iri: &str) -> usize {
        self.defs[iri].len() + 2
    }

    /// The definitions `text` needs in its own payload, transitively — an
    /// injected definition can itself name a class-constrained object (a symbol
    /// carries `definedIn`), and a payload that is only one level self-sufficient
    /// fails exactly like the one this fixes.
    fn closure(&self, text: &str) -> BTreeSet<&'a str> {
        let mut out = BTreeSet::new();
        let mut work: Vec<&'a str> = self.known(text);
        while let Some(iri) = work.pop() {
            if !out.insert(iri) {
                continue;
            }
            work.extend(self.known(self.defs[iri]));
        }
        out
    }

    /// The class-constrained objects of `text` we hold a definition for, mapped
    /// onto the index's own keys so they outlive a borrowed piece.
    fn known(&self, text: &str) -> Vec<&'a str> {
        class_constrained_objects(text)
            .into_iter()
            .filter_map(|iri| self.defs.get_key_value(iri).map(|(k, _)| *k))
            .collect()
    }

    /// The definitions `text` itself supplies.
    fn supplies(&self, text: &str) -> HashSet<&'a str> {
        definition_statements(text)
            .into_keys()
            .filter_map(|iri| self.defs.get_key_value(iri).map(|(k, _)| *k))
            .collect()
    }

    /// Size of the current chunk if `piece` were appended to it, definitions and
    /// all. One implementation, used for both the does-it-fit test and the
    /// nothing-will-ever-fit test, so the two cannot disagree.
    fn projected(
        &self,
        piece: &str,
        need: &BTreeSet<&'a str>,
        supplies: &HashSet<&'a str>,
    ) -> usize {
        let extra: usize = need
            .iter()
            .filter(|i| {
                !self.present.contains(*i) && !self.needed.contains(*i) && !supplies.contains(*i)
            })
            .map(|i| self.cost(i))
            .sum();
        let freed: usize = supplies
            .iter()
            .filter(|i| self.needed.contains(*i))
            .map(|i| self.cost(i))
            .sum();
        self.cur.len() + 2 + piece.len() + self.needed_bytes + extra - freed
    }

    /// Would `piece` and its definitions fit in a chunk of their own?
    fn fits_alone(&self, piece: &str) -> bool {
        let need = self.closure(piece);
        let supplies = self.supplies(piece);
        let deps: usize = need
            .iter()
            .filter(|i| !supplies.contains(*i))
            .map(|i| self.cost(i))
            .sum();
        self.header.len() + 2 + piece.len() + deps <= self.limit
    }

    /// Append one statement-complete piece, starting a new chunk when full.
    fn push(&mut self, piece: &str) -> Result<()> {
        let need = self.closure(piece);
        let supplies = self.supplies(piece);
        if self.projected(piece, &need, &supplies) > self.limit {
            self.flush();
        }
        if self.projected(piece, &need, &supplies) > self.limit {
            let deps: usize = need
                .iter()
                .filter(|i| !supplies.contains(*i))
                .map(|i| self.cost(i))
                .sum();
            let carried = if deps > 0 {
                format!(
                    " plus {deps} bytes of type declarations it must carry with it (sh:class is \
                     validated against the payload, not the store)"
                )
            } else {
                String::new()
            };
            return Err(Error::Promote(format!(
                "a single Turtle statement is {} bytes{carried}, over the {} byte chunk limit — cannot split below statement granularity",
                piece.len(),
                self.limit
            )));
        }
        self.cur.push_str("\n\n");
        self.cur.push_str(piece);
        for iri in supplies {
            if self.needed.remove(iri) {
                self.needed_bytes -= self.cost(iri);
            }
            self.present.insert(iri);
        }
        for iri in need {
            if self.present.contains(iri) {
                continue;
            }
            if self.needed.insert(iri) {
                self.needed_bytes += self.cost(iri);
            }
        }
        Ok(())
    }

    /// Close the current chunk, appending the definitions it referenced but does
    /// not carry. A chunk is never emitted before this runs.
    fn flush(&mut self) {
        if self.cur.len() <= self.header.len() {
            return;
        }
        for iri in std::mem::take(&mut self.needed) {
            self.cur.push_str("\n\n");
            self.cur.push_str(self.defs[iri]);
        }
        self.needed_bytes = 0;
        self.present.clear();
        self.chunks
            .push(std::mem::replace(&mut self.cur, String::from(self.header)));
    }

    fn finish(mut self) -> Vec<String> {
        self.flush();
        self.chunks
    }
}

/// Split a Turtle document into chunks of whole statements, each chunk carrying
/// the prefix header. `to_turtle` separates entity blocks with blank lines and
/// never emits blank nodes, so a blank line is always a safe split point — but
/// the call/reference EDGE sections are contiguous single-line statements with
/// no blank lines between them (bobbin's edge section alone is ~6.9 MB), so an
/// oversized block is split further at statement boundaries: a line ending in
/// `.` completes a Turtle statement in this exporter's output.
///
/// Each chunk is also made SELF-SUFFICIENT for the server's SHACL pass — see
/// [`ChunkAssembler`]. Splitting alone is not enough: a syntactically complete
/// chunk can still be semantically incomplete, and quipu refuses it.
///
/// Errors only if a single STATEMENT, plus the type declarations it must carry,
/// exceeds `limit` — that genuinely cannot be split, and silently posting it
/// would just 413 downstream.
fn chunk_turtle(turtle: &str, limit: usize) -> Result<Vec<String>> {
    if turtle.len() <= limit {
        return Ok(vec![turtle.to_string()]);
    }
    let mut blocks = turtle.split("\n\n");
    // The first "block" is the @prefix header `to_turtle` puts at the top.
    let header = blocks.next().unwrap_or_default();
    let mut asm = ChunkAssembler::new(header, limit, definition_statements(turtle));

    for block in blocks {
        if block.trim().is_empty() {
            continue;
        }
        if asm.fits_alone(block) {
            asm.push(block)?;
            continue;
        }
        // Oversized block: regroup its lines into statement-complete pieces
        // (a line ending in `.` closes a statement in to_turtle's output).
        let mut piece = String::new();
        for line in block.lines() {
            if !piece.is_empty() {
                piece.push('\n');
            }
            piece.push_str(line);
            if line.trim_end().ends_with('.') && header.len() + 2 + piece.len() > limit / 2 {
                asm.push(&piece)?;
                piece.clear();
            }
        }
        if !piece.trim().is_empty() {
            asm.push(&piece)?;
        }
    }
    Ok(asm.finish())
}

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
    let chunks = match prepare(turtle, source)? {
        Prepared::Ready(chunks) => chunks,
        Prepared::Refused(refusal) => return Ok(refusal),
    };
    let total = chunks.len();
    let mut summary = WriteSummary {
        count: 0,
        tx_ids: Vec::new(),
        chunks: total,
    };
    for (i, chunk) in chunks.iter().enumerate() {
        let knot = write_knot(endpoint, chunk, source).map_err(|e| {
            // A server-side refusal names a focus node in a payload only yupana
            // held, so this failure needs the projection retained too.
            let dump = dump_payload(turtle, source);
            Error::Promote(with_payload(
                format!(
                    "chunk {}/{total} failed after {} chunk(s) landed — re-running is safe (deterministic IRIs supersede): {e}",
                    i + 1,
                    i
                ),
                dump.as_deref(),
            ))
        })?;
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
/// second. A transport failure leaves the prior snapshot current.
pub fn promote_snapshot(
    endpoint: &str,
    turtle: &str,
    source: &str,
    snapshot: &str,
) -> Result<Promotion> {
    match prepare(turtle, source)? {
        Prepared::Refused(refusal) => Ok(refusal),
        Prepared::Ready(_) => {
            let knot = write_knot_snapshot(endpoint, turtle, source, snapshot).map_err(|e| {
                let dump = dump_payload(turtle, source);
                Error::Promote(with_payload(
                    format!(
                        "atomic snapshot replacement failed; prior snapshot remains current: {e}"
                    ),
                    dump.as_deref(),
                ))
            })?;
            Ok(Promotion::Wrote(WriteSummary {
                count: knot.count,
                tx_ids: knot.tx_id.into_iter().collect(),
                chunks: 1,
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

#[cfg(test)]
#[path = "promote_test.rs"]
mod promote_test;
