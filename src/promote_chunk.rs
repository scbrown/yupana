//! Turtle chunking for the promotion write path, split out of `promote` for
//! size (yupana #83). Splits entity blocks across multiple `/knot` posts under
//! the request-body limit, prefixes replicated, definitions kept with the
//! statements that need them (see `promote`'s module doc for the contract).

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::errors::{Error, Result};

/// Stay safely under axum's 2 MiB default body limit: the Turtle is JSON-string
/// encoded (quotes/newlines escape to two bytes) before it travels, so leave
/// headroom for that inflation plus the JSON envelope.
pub(super) const CHUNK_LIMIT: usize = 1_500_000;

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
pub(super) const CLASS_CONSTRAINED_PREDICATES: &[&str] = &["bobbin:definedIn"];

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
pub(super) fn chunk_turtle(turtle: &str, limit: usize) -> Result<Vec<String>> {
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
