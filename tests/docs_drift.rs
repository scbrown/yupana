//! Docs-drift guard (aegis-0hq0): the set of registered `yupana_*` MCP tools must
//! match what the docs claim. A tool count nobody enforces is a comment — and it
//! had already rotted three ways at once: `README.md` said 8, the spec's
//! Appendix D said 9 (omitting `yupana_communities`), `mcp-tools.md` said 10, and
//! the code has 10. This pins them together by NAME, so adding a tool without
//! documenting it (or vice versa) fails here naming the offender.
//!
//! Reads the source text (not the compiled server), so it needs no feature flag.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn read(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Collapse prose to a single spaced line so a phrase check is robust to
/// line-wrapping and Markdown blockquote (`> `) prefixes — a docs test must not
/// break just because a sentence wrapped.
fn flow(text: &str) -> String {
    text.replace("> ", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pull `yupana_*` identifiers out of a line, e.g. `async fn yupana_impact(` or a
/// `` | `yupana_impact` | … `` table row.
fn yupana_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("yupana_") {
        let tail = &rest[i..];
        let end = tail
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_'))
            .map_or(tail.len(), |(j, _)| j);
        names.insert(tail[..end].to_string());
        rest = &tail[end..];
    }
    names
}

/// The tools actually registered on the MCP server: every `async fn yupana_*`.
fn registered_tools() -> BTreeSet<String> {
    read("src/mcp/server.rs")
        .lines()
        .filter(|l| l.contains("async fn yupana_"))
        .flat_map(yupana_names)
        .collect()
}

/// The tools the `## Live tools` table in the MCP reference documents. Stops at
/// the next `##` so any later section is excluded. `yupana_promote` moved INTO this
/// table when it was wired (Phase 4); it is always registered, so it belongs here.
fn documented_live_tools() -> BTreeSet<String> {
    let md = read("docs/book/src/reference/mcp-tools.md");
    let start = md.find("## Live tools").expect("Live tools section");
    let after = &md[start + "## Live tools".len()..];
    let end = after.find("\n## ").map_or(after.len(), |e| e);
    after[..end]
        .lines()
        .filter(|l| l.trim_start().starts_with("| `yupana_"))
        .flat_map(yupana_names)
        .collect()
}

#[test]
fn registered_tools_match_the_mcp_reference() {
    let code = registered_tools();
    let docs = documented_live_tools();
    assert_eq!(
        code.len(),
        15,
        "expected 15 registered yupana_* tools, got {code:?}"
    );
    assert_eq!(
        code, docs,
        "MCP reference drifted from the registered tools.\n  only in code: {:?}\n  only in docs: {:?}",
        code.difference(&docs).collect::<Vec<_>>(),
        docs.difference(&code).collect::<Vec<_>>(),
    );
}

#[test]
fn spec_appendix_d_lists_every_registered_tool() {
    let spec = read("docs/yupana-spec.md");
    let code = registered_tools();
    // The count claimed in Appendix D's "MCP tools (N, …)" heading.
    assert!(
        spec.contains(&format!("MCP tools ({},", code.len())),
        "Appendix D's tool count is not {}",
        code.len()
    );
    for tool in &code {
        assert!(
            spec.contains(&format!("`{tool}`")),
            "Appendix D does not list the registered tool `{tool}`"
        );
    }
}

#[test]
fn readme_and_quickstart_state_the_right_tool_count() {
    // The count is spelled as a word in the prose; pin it to the registered count
    // and forbid the two stale numbers that were live (`eight`, `nine`).
    let n = registered_tools().len();
    assert_eq!(n, 15);
    for (file, rel) in [
        ("README.md", "README.md"),
        (
            "quick-start.md",
            "docs/book/src/getting-started/quick-start.md",
        ),
    ] {
        let text = flow(&read(rel));
        assert!(
            text.contains("fifteen `yupana_*` tools") || text.contains("fifteen MCP tools"),
            "{file} does not state fifteen tools"
        );
        assert!(
            !text.contains("eleven `yupana_*`") && !text.contains("eleven MCP tools"),
            "{file} still says eleven tools"
        );
    }
}
