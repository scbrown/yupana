# Tiers and Freshness

Full LSP-grade resolution on every keystroke is too expensive, so Yupana serves
tiered facts — and **every fact it serves is tagged** so a consumer never
mistakes an approximation for ground truth.

## Tier — how a fact was derived

- `treesitter` — fast, build-free, approximate. Always-on breadth; works on a
  syntactically-broken buffer. **This is the only tier served today** — every fact
  Yupana currently produces is `treesitter`, and `yupana status` advertises only it.
- `lsp` — precise defs/refs/types where a build resolves. *Planned (FR-2); not yet
  implemented or served.*
- `cpg` — control/data dependence from the code property graph. *Planned (Phase 2,
  FR-7); not yet implemented or served.*

## Freshness — how current a fact is

- `fresh` — reflects the latest observed edit.
- `stale` — known to be behind a pending edit.
- `recomputing` — a recompute is in flight.

Tree-sitter structure updates on save or debounced keystroke. Once the LSP and
CPG tiers land, agents that need certainty will ask for `lsp`/`cpg` and agents
that need breadth will take `treesitter` and know it — but today only
`treesitter` is served, so a fact's tier tells you which of these you actually
got, never a precision the build cannot provide.
