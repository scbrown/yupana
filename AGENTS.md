# yupana - Agent Instructions

## Project Overview

In-memory, multi-tenant code-analysis engine — the structural signal for the
Bobbin × Quipu stack. Yupana extracts precise structure (AST, symbols, call graph,
and later control/data dependence and LSP facts), keeps it hot per tenant, and
serves it over MCP/HTTP. See `docs/yupana-spec.md` for the full design and
`docs/vision.md` for the north star.

Sibling repos: [`scbrown/bobbin`](https://github.com/scbrown/bobbin) (fusion +
serving) and [`scbrown/quipu`](https://github.com/scbrown/quipu) (governed
bitemporal graph). Keep Yupana's stack coherent with theirs.

## Conventions

- **Always use `just` instead of raw commands.** The justfile is configured with
  quiet output by default to save context; use `verbose=true` when debugging.
- **Prefer subcommands over separate recipes.** Group related operations under a
  single recipe with a subcommand argument (e.g. `just docs build`).
- **Keep source files small.** The pre-commit file-size check warns at 400 lines
  and fails at 500 (tests exempt). One responsibility per module (see the layout
  in `docs/yupana-spec.md` §7.2).
- **Tag every fact.** Everything Yupana serves carries a `tier` (FR-3) — never
  present a tree-sitter approximation as LSP-precise. FR-3's `freshness` half
  splits in two: **code-fact** freshness is tracked by the watch path
  (`src/watch/overlay_refresh.rs`) but not yet served — a query response omits
  it rather than faking a `fresh` tag until Phase 3 wires it through; the
  verdict surface already serves **projection** freshness (how current the
  projected policy registry is — `src/hook/rule_planes.rs`, `src/verdict.rs`).
  Don't mistake one for the other.
- **Don't let a feature ship dark.** When a phase wires a Cargo feature (`mcp`,
  `quipu`, `cpg`, `lsp`), add it to the CI matrix in the same change.

## Build Commands

```bash
just --list          # Show available commands
just setup           # Install pre-commit hooks
just build           # cargo build
just test            # cargo test
just lint            # cargo clippy -- -D warnings -A missing-docs
just check           # Run all pre-commit hooks (pre-push gate)
```

## Documentation Commands

```bash
just docs build      # Build the mdBook
just docs serve      # Serve locally with hot reload
just docs lint       # Lint markdown
just docs check      # Full docs quality gate
```

## Quality Requirements

### Before Every Push

You MUST run and pass the full quality gate:

```bash
just check           # pre-commit hooks: fmt, clippy, markdownlint, file-size
just test            # all tests green
```

**Do NOT push if any check fails.** Fix the issues and re-run.

- New functionality must include tests.
- User-facing changes must update the docs (`just docs build` must be clean) and
  the README if quick-start or usage changes.
- Clippy runs with `-D warnings`; resolve every lint before merge.

## Landing the Plane (Session Completion)

**Work is NOT complete until `git push` succeeds.**

1. Run quality gates — `just check` and `just test` must pass.
2. Build docs — `just docs build` must succeed if docs changed.
3. Commit (conventional commits: `<type>: <description>`) and push.
4. Verify — all changes committed AND pushed.

**CRITICAL RULES:**

- NEVER stop before pushing — that leaves work stranded locally.
- NEVER say "ready to push when you are" — YOU must push.
- If push fails, resolve and retry until it succeeds.
