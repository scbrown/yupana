# Quick Start

Build the binary, then point it at a source tree.

```bash
# Build the base graph for a directory and print a summary
yupana analyze src

# Find the definition sites of a symbol by name
yupana refs authenticate src

# Show the base ref, active tiers, and configuration
yupana status

# Generate shell completions
yupana completions bash > yupana.bash
```

Every command accepts the global flags `--json`, `--quiet`, `--verbose`,
`--tenant <id>`, and `--config <path>`.

## Call graph and blast radius

```bash
# Direct callers and callees of a symbol
yupana callers authenticate src

# Blast radius: what changing a symbol transitively affects
yupana impact authenticate src --hops 5 --json

# Reconcile the structural impact against a co-change set (from Bobbin):
# corroborated = real coupling; co-change-only = possible refactoring smell
yupana impact authenticate src --cochange cochange.json

# Intra-procedural data dependence: where a value comes from / flows to
yupana dataflow authenticate src --var token           # what `token` depends on
yupana dataflow authenticate src --var token --forward # what `token` flows into

# Export the referential structure (modules, symbols, calls, imports) as governed RDF
yupana export src --repo myrepo --format turtle
```

The export is the governed projection of the live graph — precise, typed
referential structure in the `bobbin:` code ontology, **not** embedding chunks.
It is the substrate under Phase-4 promotion into Quipu; see the
[Specification](../design/specification.md) §5.10 and §9.

## The MCP server

Built with the `mcp` feature, `yupana serve` exposes fourteen `yupana_*` tools over MCP —
starting with `yupana_status`, `yupana_symbols`, `yupana_references`, and `yupana_analyze`
(the full set is in the [MCP Tools reference](../reference/mcp-tools.md)):

```bash
cargo run --features mcp -- serve         # stdio, for a local agent
cargo run --features mcp -- serve --http  # streamable-HTTP at :3040/mcp
```

See the [MCP Tools reference](../reference/mcp-tools.md).

## What works today

`analyze`, `refs`, `status`, the call-graph commands `callers`/`impact`,
`dataflow`, `verify` (the FR-23/FR-24 edit-buffer verdict), and the fourteen MCP tools
are live. Only `promote` is still declared with its final shape and prints a phase
notice until its engine lands — see the
[Specification](../design/specification.md).
