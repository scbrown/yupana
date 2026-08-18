//! The `PostToolUse` advisory — Yupana's answer to "what did that edit reach?".
//!
//! `yupana hook post-edit` reads the harness's `PostToolUse` JSON on stdin and
//! returns an advisory: which symbols in the edited file have callers elsewhere,
//! so the agent learns the blast radius of its own change synchronously, without
//! calling a tool. **Advisory only** — the blocking companion is
//! [`super::pre_edit`].
//!
//! With `[yupana.serve] use_daemon = true` this is a thin client of the resident
//! daemon (FR-31, yupana #1 stage 5): the edited file's symbols are still
//! extracted fresh HERE (their content is what just changed), but their callers
//! come from the resident graph — no per-invocation `CodeGraph::build`. The
//! daemon being unusable falls back to the transient build with a stderr note;
//! like the MCP tools and unlike the pre-edit guard, fallback is silent to the
//! model — this is an advisory, not an enforcement surface, and a transient
//! answer is equally correct.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{HookInput, ToolInput};
use crate::config::YupanaConfig;
use crate::daemon::client::{expected_same_root_daemon, fetch_edit};
use crate::extract::extract_symbols;
use crate::graph::{CodeGraph, Dir};

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
    if let Some(text) = advisory_for(&buf, &root, tenant) {
        sections.push(text);
    }

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
// Test names here shout the invariant they pin — `is_NEVER_observable`,
// `daemon_EXPECTED_but_DOWN`, `is_DOWN_not_UP`. That capitalisation is the same
// emphasis the prose and comments use throughout this repo, and it is load-
// bearing in a test name: it says which word the assertion turns on. Allowed
// explicitly, and scoped to tests, so the lint stays live everywhere else
// rather than being switched off crate-wide (yupana #83).
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn advises_on_cross_file_impact() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn mid() { leaf(); }\n").unwrap();

        let payload = serde_json::json!({
            "tool_name": "Edit",
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": { "file_path": dir.path().join("a.rs").to_str().unwrap() },
        })
        .to_string();

        let text = advisory_for(&payload, dir.path(), None).expect("expected an advisory");
        assert!(text.contains("leaf"));
        assert!(text.contains("b.rs"));
    }

    #[test]
    fn scopes_advisory_to_the_edited_symbol() {
        // a.rs defines `leaf` (edited) and `other` (untouched), both called from b.rs.
        // The file on disk is POST-edit, as the real hook sees it.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn leaf() {\n    let x = 2;\n}\nfn other() {}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn m() { leaf(); other(); }\n").unwrap();

        let payload = serde_json::json!({
            "tool_name": "Edit",
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": {
                "file_path": dir.path().join("a.rs").to_str().unwrap(),
                "old_string": "let x = 1;",
                "new_string": "let x = 2;",
            },
        })
        .to_string();

        let text = advisory_for(&payload, dir.path(), None).expect("expected an advisory");
        assert!(
            text.contains("leaf"),
            "advises on the edited symbol: {text}"
        );
        assert!(
            !text.contains("other"),
            "must NOT cry wolf on the untouched symbol: {text}"
        );
    }

    #[test]
    fn no_advice_when_edit_is_outside_every_symbol() {
        // A top-of-file comment edit touches no symbol body — the cry-wolf case.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "// header updated\nfn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn m() { leaf(); }\n").unwrap();

        let payload = serde_json::json!({
            "tool_name": "Edit",
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": {
                "file_path": dir.path().join("a.rs").to_str().unwrap(),
                "old_string": "// header",
                "new_string": "// header updated",
            },
        })
        .to_string();
        assert!(
            advisory_for(&payload, dir.path(), None).is_none(),
            "a comment-only edit must not advise on symbols it did not touch"
        );
    }

    #[test]
    fn edited_line_spans_locates_edit_and_falls_back() {
        let src = "aaa\nbbb\nccc\n";
        let ti = ToolInput {
            new_string: Some("bbb".into()),
            ..Default::default()
        };
        assert_eq!(edited_line_spans(&ti, src), Some(vec![(2, 2)]));
        // Deletion → None (unlocatable post-hoc).
        let ti = ToolInput {
            new_string: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(edited_line_spans(&ti, src), None);
        // No diff info (Write) → None → whole-file fallback.
        assert_eq!(edited_line_spans(&ToolInput::default(), src), None);
    }

    #[test]
    fn quiet_when_no_external_callers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn leaf() {}\nfn mid() { leaf(); }\n",
        )
        .unwrap();
        // leaf's only caller (mid) is in the same file → no cross-file impact.
        let payload = serde_json::json!({
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": { "file_path": dir.path().join("a.rs").to_str().unwrap() },
        })
        .to_string();
        assert!(advisory_for(&payload, dir.path(), None).is_none());
    }

    #[test]
    fn quiet_on_non_rust_or_garbage() {
        assert!(advisory_for("not json", Path::new("."), None).is_none());
        let payload = serde_json::json!({ "tool_input": { "file_path": "README.md" } }).to_string();
        assert!(advisory_for(&payload, Path::new("."), None).is_none());
    }

    // Project config expecting a daemon at 127.0.0.1:port. Written as the
    // PROJECT config so it wins over any developer user config for these keys.
    fn write_daemon_config(root: &Path, port: u16) {
        let bobbin = root.join(".bobbin");
        std::fs::create_dir_all(&bobbin).unwrap();
        std::fs::write(
            bobbin.join("config.toml"),
            format!(
                "[yupana.serve]\nuse_daemon = true\nbind_address = \"127.0.0.1\"\n\
                 mcp_http_port = {port}\n"
            ),
        )
        .unwrap();
    }

    fn edit_payload(root: &Path, file: &str) -> String {
        serde_json::json!({
            "tool_name": "Edit",
            "cwd": root.to_str().unwrap(),
            "tool_input": { "file_path": root.join(file).to_str().unwrap() },
        })
        .to_string()
    }

    #[test]
    fn daemon_EXPECTED_but_DOWN_falls_back_to_the_transient_advisory() {
        // Port 1 never listens. The advisory must still be produced (transient
        // fallback) — a down daemon degrades performance, never the advisory.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn mid() { leaf(); }\n").unwrap();
        write_daemon_config(dir.path(), 1);

        let text = advisory_for(&edit_payload(dir.path(), "a.rs"), dir.path(), None)
            .expect("the transient fallback must still advise");
        assert!(text.contains("leaf"));
        assert!(text.contains("b.rs"));
    }

    // Serving the router needs axum (`mcp` feature); the down/fallback quadrant
    // above runs feature-free.
    #[cfg(feature = "mcp")]
    #[tokio::test(flavor = "multi_thread")]
    async fn daemon_up_and_same_root_advises_from_the_RESIDENT_view_and_feeds_the_overlay() {
        use crate::daemon::{http, ResidentEngine};
        // The tenant layer anchors to a COMMIT, so the fixture is a real repo.
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.rs"), "fn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn mid() { leaf(); }\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "base"]);

        let engine = ResidentEngine::build(dir.path(), None).unwrap();
        let observer = engine.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, http::router(engine)).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        write_daemon_config(dir.path(), port);

        // A REAL edit to a.rs (differs from base, still defines `leaf`): the
        // daemon reads it from disk, so it must not match the baseline or the
        // FR-15 base-hit would make it a no-op.
        std::fs::write(dir.path().join("a.rs"), "fn leaf() {}\nfn added() {}\n").unwrap();
        // `late.rs` is UNCOMMITTED and untouched: the tenant view composes
        // over base@HEAD, so it must not appear — a transient build would see
        // it. Its absence below proves who answered.
        std::fs::write(dir.path().join("late.rs"), "fn late() { leaf(); }\n").unwrap();

        let payload = edit_payload(dir.path(), "a.rs");
        let root = dir.path().to_path_buf();
        let text = tokio::task::spawn_blocking(move || advisory_for(&payload, &root, Some("t1")))
            .await
            .unwrap()
            .expect("an up, same-root daemon must advise");
        assert!(text.contains("b.rs"), "{text}");
        assert!(
            !text.contains("late.rs"),
            "`late.rs` is uncommitted and untouched — its presence means a \
             transient build answered, not the tenant view: {text}"
        );

        // And the advisory FED the overlay (FR-30): the daemon now holds the
        // edit as tenant t1's touch of a.rs.
        let reg = observer.registry().expect("repo ⇒ tenant layer");
        let reg = reg.read().unwrap();
        let overlay = reg.overlay("t1").expect("the edit created t1's overlay");
        assert!(overlay.is_touched("a.rs"));
    }
}
