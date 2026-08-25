# Golden-Path Conformance Guard (addendum)

The FR-40..FR-42 addendum — blessed-trajectory projections and plan/progress
conformance under `gp-grammar/1` — lives at the repository root:

- **[`docs/golden-path-guard.md`](https://github.com/scbrown/yupana/blob/main/docs/golden-path-guard.md)**

**Status: BUILT (first cut), behind the `golden-path` Cargo feature.** The code
is `src/goldenpath/`; the surfaces are `yupana_path_check` over MCP and
`POST /path/check` on the daemon, both feature-gated. The operational reference
is [Golden-Path Conformance](../reference/golden-path.md).

Two choices the implementation made that the design left open, since they
change how the guard is used:

- **Projected paths are supplied per call**, like `StatePolicy`, rather than
  held resident — a stale resident copy would enforce yesterday's blessing.
- **Deviation is decidable only against a complete plan**, so FR-41's
  per-action flow reports *progress and hazards* rather than hard deviations.
