# MCP Tools

Yupana exposes its capabilities as MCP tools, over stdio and streamable-HTTP.
Tools use the `yupana_*` naming convention, alongside Bobbin's `bobbin_*` and
Quipu's `quipu_*` on the same agent.

A parallel REST HTTP API (a per-tool endpoint for the broker and other
non-MCP consumers) is served by the [resident daemon](daemon.md) (`yupana
daemon`, FR-27/FR-31). With `[yupana.serve] use_daemon = true`, the graph tools
here become thin clients of the same resident engine.

Build with the `mcp` feature and run `yupana serve` (stdio) or `yupana serve --http`
(streamable-HTTP on `[yupana.serve] mcp_http_port`, default 3040).

```bash
cargo build --features mcp
yupana serve            # stdio, for a local agent
yupana serve --http     # streamable-HTTP at http://127.0.0.1:3040/mcp
```

## Live tools

| Tool | Purpose |
|------|---------|
| `yupana_status` | Base ref, tenant, available tiers, Quipu settings |
| `yupana_symbols` | Symbol tree for a file |
| `yupana_references` | Definition site(s) of a symbol, by name or by position |
| `yupana_analyze` | Files/symbols summary for a subtree |
| `yupana_callers` | Direct callers of a symbol (who calls it) |
| `yupana_callees` | Direct callees of a symbol (what it calls) |
| `yupana_communities` | Densely-connected symbol clusters (deterministic Louvain, FR-9) |
| `yupana_impact` | Blast radius — transitive callers, N hops; reconciles against a `cochange` set (FR-11) |
| `yupana_dataflow` | Intra-procedural data dependence within a function |
| `yupana_verify` | Verdict on a **proposed** edit buffer, before you write it (FR-23/FR-24) |
| `yupana_promote` | Promote a subtree's facts to Quipu — SHACL-validate, then write (needs the `quipu` feature) |
| `yupana_ingest` | Load generic (non-code) facts into the hot board graph (FR-35; needs the `game-state` feature) |
| `yupana_guard` | Check proposed orders against game-state policies (FR-37; needs the `game-state` feature) |
| `yupana_whatif` | Speculative, uncommitted impact of an order set over the board (FR-38; needs the `game-state` feature) |
| `yupana_path_check` | Conformance of a plan or work-in-progress against a blessed golden path under gp-grammar/1 (FR-41/FR-42; needs the `golden-path` feature) |

The last three are the **game-state harness**. They are registered on every
build, as `yupana_promote` is, and refuse with a message naming the feature when
it is absent — a tool that accepted a board and silently did nothing would be
worse than one that is missing, because the caller would believe it had guarded.
See [The Game-State Harness](../concepts/game-state.md).

## `yupana_references`

Give it `symbol` **or** a position (`at_file` + `at_line`), not both concerns at
once.

Names over-connect: a real tree has twelve `build`s, and asking by name gets all
twelve. When you are reading code you know *where* you are, so point at it:

```json
{ "at_file": "src/beta.rs", "at_line": 7 }
```

The position resolves to the innermost symbol enclosing that line, and the reply
holds **that symbol alone** — it does not re-expand to every symbol sharing its
name, which would restore the ambiguity the position was given to remove.

`at_line` is a **line, not a (line, column)**. The tree-sitter extractor records
lines, so a column would be accepted and silently ignored — an approximation
served as the finer tier, which FR-3 forbids. Column precision arrives with the
LSP tier (FR-2). Passing only one of `at_file`/`at_line` is an error rather than
a quiet downgrade to a name lookup, since that would answer "which one is here?"
with "all of them".

Position requests always take the transient build, never the resident daemon:
`/references` does not carry the node spans a position needs.

## `yupana_verify`

Pass the full proposed contents of a file; get back `ok` plus any violations.
Only violations the edit *introduces* are reported — the file's current contents
are the baseline, so pre-existing breakage is not blamed on the edit.

**Always read `unchecked` alongside `ok`.** At the tree-sitter tier there is no
type information, so `type-violation` is never decided, and method calls,
path-qualified calls, imports, locals, and closures are deliberately left alone
rather than guessed at. `ok: true` means "nothing this tier can see is wrong",
not "this compiles".

Every response carries a `tier` tag; structure-reading requests will accept a
`tenant` parameter as multi-tenancy lands (Phase 3).
