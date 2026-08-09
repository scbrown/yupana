# Yupana

**Yupana** is an in-memory, multi-tenant code-analysis engine — the structural
signal for the Bobbin × Quipu stack. It extracts precise structure from a
codebase (AST, symbols, call graph, and — in later phases — control/data
dependence and LSP facts), keeps it hot per tenant, and serves it over MCP
(stdio and streamable-HTTP). A parallel REST HTTP API for non-MCP consumers is
planned for Phase 3 (FR-27).

> *Bobbin holds the thread. Quipu ties the knots. Yupana keeps the working coil —
> live, per-tenant, ready.*

## The three tools

- **Yupana** — extracts and serves live per-tenant structure.
- **[Quipu](https://github.com/scbrown/quipu)** — governs and versions the
  committed record (bitemporal RDF / SPARQL / SHACL).
- **[Bobbin](https://github.com/scbrown/bobbin)** — fuses everything with its
  embedding and co-change signals and serves explained context over MCP.

## Where to start

- New here? Read [Architecture](concepts/architecture.md), then
  [The Tenancy Model](concepts/tenancy-model.md).
- Want to run it? [Installation](getting-started/installation.md) and
  [Quick Start](getting-started/quick-start.md).
- Want the whole design? The [Specification](design/specification.md).

> **Status:** early Phase-1 scaffold. See the
> [phasing](design/specification.md) for what is built and what is next.
