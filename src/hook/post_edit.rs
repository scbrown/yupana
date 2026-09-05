//! The `PostToolUse` advisory — Yupana's answer to "what did that edit reach?".
//! `yupana hook post-edit` reads the harness's `PostToolUse` JSON on stdin and
//! returns an advisory: which symbols in the edited file have callers elsewhere,
//! so the agent learns the blast radius of its own change synchronously, without
//! calling a tool. **Advisory only** — the blocking companion is
//! [`super::pre_edit`].
//! With `[yupana.serve] use_daemon = true` this is a thin client of the resident
//! daemon (FR-31, yupana #1 stage 5): the edited file's symbols are still
//! extracted fresh HERE (their content is what just changed), but their callers
//! come from the resident graph — no per-invocation `CodeGraph::build`. The
//! daemon being unusable falls back to the transient build with a stderr note;
//! like the MCP tools and unlike the pre-edit guard, fallback is silent to the
//! model — this is an advisory, not an enforcement surface, and a transient
//! answer is equally correct.

use super::{HookInput, ToolInput};
use crate::config::YupanaConfig;
use crate::daemon::client::{expected_same_root_daemon, fetch_edit};
use crate::extract::extract_symbols;
use crate::graph::{CodeGraph, Dir};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
/// Budget per localhost round-trip, same rationale as the MCP thin client:
/// generous against a resident graph, small enough that a wedged daemon costs
/// one slow query before the transient fallback answers.
const DAEMON_TIMEOUT: Duration = Duration::from_millis(500);
/// How many impacted symbols to list before summarizing the rest.
const MAX_LISTED: usize = 8;

/// Run the `post-edit` hook: read the harness payload from stdin and, if the
/// edit has cross-file impact, print the `PostToolUse` advisory envelope.
/// `tenant` is the session's overlay identity (the global `--tenant` flag).
pub fn run_post_edit(tenant: Option<&str>) -> anyhow::Result<()> {
    let mut buf = String::new();
    std::io::stdin().lock().read_to_string(&mut buf).ok();
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Two independent things happen here, and they are kept independent on
    // purpose. The blast-radius advisory bails early on anything it cannot
    // reason about — a non-Rust file, a file with no symbols, an edit landing
    // outside every symbol body. The AUDIT must not inherit those exits: a
    // constraint declared at the PAA has to be evaluated whether or not this
    // edit happened to have interesting callers, or its coverage would depend on
    // a property of the file that has nothing to do with the rule.
    let mut sections: Vec<String> = Vec::new();
    if let Some(text) = super::reread::advisory(&buf) {
        sections.push(text);
    }
    if let Some(text) = super::credential_output::advisory(&buf) {
        sections.push(text);
    }
    if let Some(audit) = super::paa::post_action_audit(&buf, &root) {
        sections.extend(audit);
    }
    // The `advise` rung of the capability ladder, delivered where the agent can
    // actually read it. `scope_arm` computes the same boundary at the gate, but
    // at `advise` it speaks through `systemMessage`, which reaches the
    // operator's pane and never the model — so the rung that exists to TELL an
    // agent where its work ends was telling nobody who could act on it. See
    // `scope_notice`.
    if let Some(text) = super::scope_notice::for_payload(&buf, &root) {
        sections.push(text);
    }
    // The DELEGATE line (aegis-2o9eo). Placed HERE rather than inside
    // `advisory_for` deliberately: that function returns early for anything
    // that is not a Rust file with symbols, and this boundary is
    // language-independent — the specimen case in the bead was edits across
    // several repos and file types, none of which advisory_for would have seen.
    let trajectory = super::delegate_line::advisory(&buf);
    if let Some(text) = advisory_for(&buf, &root, tenant) {
        sections.push(text);
    }

    let mut sections: Vec<String> = sections
        .into_iter()
        .filter_map(|section| super::advisory_for_session(&buf, section))
        .collect();
    // This channel applies its graph-declared once_per itself. Generic duplicate
    // advice suppression must not silently override an explicit edit scope.
    sections.extend(trajectory);
    if !sections.is_empty() {
        let envelope = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": sections.join("\n\n"),
            }
        });
        println!("{envelope}");
    }
    // A hook must never fail the harness: absence of output = nothing to say.
    Ok(())
}

/// Compute the advisory text for a hook payload, or `None` when there is nothing
/// useful to say (unparseable, non-Rust, or no cross-file impact).
#[must_use]
pub fn advisory_for(input_json: &str, default_root: &Path, tenant: Option<&str>) -> Option<String> {
    let input = HookInput::parse(input_json)?;
    if !matches!(
        input.tool_name.as_deref(),
        Some("Edit" | "Write" | "MultiEdit")
    ) {
        return None;
    }
    let file_path = input.tool_input.file_path.clone()?;
    let file = PathBuf::from(&file_path);
    if file.extension().and_then(OsStr::to_str) != Some("rs") {
        return None;
    }

    let root = input.root(default_root);
    let rel = file
        .strip_prefix(&root)
        .unwrap_or(&file)
        .display()
        .to_string();

    let source = std::fs::read_to_string(&file).ok()?;
    let symbols = extract_symbols(&source, "rust").ok()?;
    if symbols.is_empty() {
        return None;
    }

    // aegis-rcyd / #75: scope the advisory to the symbol(s) the edit actually
    // TOUCHED, not every symbol in the file. Without this, a comment- or
    // whitespace-only edit cries "N callers, re-check!" identically to a
    // signature change — alarm fatigue that trains agents to tune the advisory
    // out (the opposite of the "is it acted on?" goal). The post-edit `source`
    // already contains the replacement text, so locate it to get the changed
    // line span(s) and keep only symbols whose body overlaps. Fall back to the
    // whole file when we cannot localize (Write's full `content`, a pure
    // deletion, or an unlocatable replacement) — conservative: over-advise
    // rather than miss a real breaking change.
    let names: Vec<String> = match edited_line_spans(&input.tool_input, &source) {
        Some(spans) if !spans.is_empty() => {
            let scoped: Vec<String> = symbols
                .into_iter()
                .filter(|sym| {
                    spans
                        .iter()
                        .any(|&(lo, hi)| sym.start_line <= hi && sym.end_line >= lo)
                })
                .map(|sym| sym.name)
                .collect();
            // The edit landed outside every symbol body (e.g. an import or a
            // module-level comment) — nothing with callers to re-check.
            if scoped.is_empty() {
                return None;
            }
            scoped
        }
        _ => symbols.into_iter().map(|s| s.name).collect(),
    };

    let (mut per_symbol, files) =
        resident_feed(&root, &rel, tenant).or_else(|| transient_callers(&root, &rel, &names))?;
    per_symbol.sort();
    per_symbol.dedup();
    if per_symbol.is_empty() {
        return None;
    }

    Some(render(&rel, &per_symbol, &files))
}

/// The 1-based inclusive line span(s) the edit changed, located in the POST-edit
/// `source`. `Edit` → the line range(s) of `new_string` as it now sits in the
/// file (all occurrences — the same text may legitimately appear more than
/// once); `MultiEdit` → the union across its edits. Returns `None` when we
/// cannot localize (Write's whole-file `content`, or a pure deletion whose
/// replacement text is empty) so the caller falls back to whole-file scoping.
fn edited_line_spans(tool_input: &ToolInput, source: &str) -> Option<Vec<(usize, usize)>> {
    let mut needles: Vec<&str> = Vec::new();
    if let Some(ns) = tool_input.new_string.as_deref() {
        needles.push(ns);
    }
    for e in &tool_input.edits {
        if let Some(ns) = e.new_string.as_deref() {
            needles.push(ns);
        }
    }
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for needle in needles {
        if needle.is_empty() {
            continue; // deletion: no text to locate post-hoc
        }
        let added_lines = needle.matches('\n').count();
        let mut from = 0usize;
        while let Some(rel) = source[from..].find(needle) {
            let byte = from + rel;
            let start_line = source[..byte].matches('\n').count() + 1; // 1-based
            spans.push((start_line, start_line + added_lines));
            from = byte + needle.len().max(1);
        }
    }
    if spans.is_empty() {
        None
    } else {
        Some(spans)
    }
}

/// Per-symbol external-caller counts and their files. The shared shape both
/// sources produce, so the advisory renders identically either way.
type ExternalCallers = (Vec<(String, usize)>, BTreeSet<String>);

/// The FR-30 cycle against the RESIDENT daemon: ONE `POST /edit` records the
/// just-saved file in this tenant's overlay AND returns the advisory from the
/// fresh composed view. `None` to fall back — daemon not expected (silent), or
/// expected-but-unusable (stderr note; an advisory has no enforcement gap to
/// be loud about). On fallback the edit is not recorded anywhere, which is
/// fine: the overlay caches the tenant's edits, the file on disk is the record.
fn resident_feed(root: &Path, rel: &str, tenant: Option<&str>) -> Option<ExternalCallers> {
    let config = YupanaConfig::resolve(None, root).ok()?;
    let (host, port) = match expected_same_root_daemon(&config, root, DAEMON_TIMEOUT)? {
        Ok(addr) => addr,
        Err(reason) => {
            eprintln!(
                "yupana post-edit: daemon expected but unusable, transient fallback: {reason}"
            );
            return None;
        }
    };
    let tenant = tenant.unwrap_or("single-tenant");
    match fetch_edit(&host, port, tenant, rel, DAEMON_TIMEOUT) {
        Ok(reply) => Some((
            reply
                .advised
                .into_iter()
                .map(|a| (a.symbol, a.external_callers))
                .collect(),
            reply.files.into_iter().collect(),
        )),
        Err(reason) => {
            eprintln!("yupana post-edit: daemon edit feed failed, transient fallback: {reason}");
            None
        }
    }
}

/// External callers from a transient whole-root build — the pre-daemon path,
/// kept as the fallback.
fn transient_callers(root: &Path, rel: &str, names: &[String]) -> Option<ExternalCallers> {
    let graph = CodeGraph::build(root).ok()?;
    let mut per_symbol: Vec<(String, usize)> = Vec::new();
    let mut files: BTreeSet<String> = BTreeSet::new();
    for name in names {
        let external: Vec<_> = graph
            .direct(name, Dir::Callers)
            .into_iter()
            .filter(|caller| caller.file != rel)
            .collect();
        if !external.is_empty() {
            per_symbol.push((name.clone(), external.len()));
            for caller in &external {
                files.insert(caller.file.clone());
            }
        }
    }
    Some((per_symbol, files))
}

/// Format the advisory shown to the agent.
fn render(rel: &str, per_symbol: &[(String, usize)], files: &BTreeSet<String>) -> String {
    let mut out = format!(
        "Yupana (tree-sitter): your edit to {rel} touches symbol(s) with callers elsewhere \
         — re-check these still compile.\n"
    );
    for (name, count) in per_symbol.iter().take(MAX_LISTED) {
        out.push_str(&format!("  {name} <- {count} caller(s)\n"));
    }
    if per_symbol.len() > MAX_LISTED {
        out.push_str(&format!(
            "  ... and {} more\n",
            per_symbol.len() - MAX_LISTED
        ));
    }
    let file_list: Vec<&str> = files.iter().map(String::as_str).collect();
    out.push_str(&format!("Impacted files: {}", file_list.join(", ")));
    out
}

#[cfg(test)]
#[path = "post_edit_test.rs"]
mod post_edit_test;
