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

Freshness names **two different questions**, and Yupana keeps them apart:

- **Code-fact freshness** — is this structural fact current with the file it
  came from? The watch path tracks it per file (`Recomputing → Fresh`
  transitions in `src/watch/overlay_refresh.rs`), and since Phase 3
  (bobbin-052) the [resident daemon](../reference/daemon.md) *serves* it on
  the tenant-scoped `/symbols` reply — read before the view is composed, so
  the tag describes the state the symbols came from. Everywhere it is
  unknown, the field is **omitted rather than faked**: the untenanted path
  has no tenant to key the map by, a tenant that never had an edit absorbed
  has no note to report, and the on-demand (non-daemon) serve path rebuilds
  per request so no cached code fact can be stale. Absent, never `"fresh"`,
  never `"unknown"`.
- **Projection freshness** — are the governed rules a verdict enforced still
  the current ones? This is *served today*: the pre-edit guard's rule verdicts
  state `fresh` / `stale` (with the cache age in seconds) / `recomputing`
  against the policy projection from Quipu, and promoted verdicts carry
  `aegis:freshness`. It describes the policy registry, never a code fact.

Tree-sitter structure updates on save or debounced keystroke. Once the LSP and
CPG tiers land, agents that need certainty will ask for `lsp`/`cpg` and agents
that need breadth will take `treesitter` and know it — but today only
`treesitter` is served, so a fact's tier tells you which of these you actually
got, never a precision the build cannot provide.
