# Specification

The full PRD-style build specification lives at the repository root:

- **[`docs/yupana-spec.md`](https://github.com/scbrown/yupana/blob/main/docs/yupana-spec.md)**

It covers the problem statement and the three-tool split, personas and user
stories, functional and non-functional requirements, the technical architecture
and proposed source layout, the code ontology and bitemporal promotion into
Quipu, the MCP/HTTP/CLI surface, technology choices (pinned to Bobbin and
Quipu), phasing, risks, and open questions.

## Phasing at a glance

- **Phase 1 — Yupana, single-tenant.** Tree-sitter structure + LSP facts served
  over MCP; Bobbin fuses them.
- **Phase 2 — Dataflow & blast radius.** CPG/dataflow; blast radius as a served
  capability and the incremental-update primitive.
- **Phase 3 — Multi-tenancy.** Shared base + copy-on-write overlays,
  content-hash sharing, frontier-bounded incremental updates.
- **Phase 4 — Promote to Quipu.** Code ontology, SHACL-validated bitemporal
  promotion, branches as named graphs.
- **Phase 5 — Consumption & guardrails.** Per-tenant blast radius into the
  broker trust boundary; monitor-guided edit verification.
