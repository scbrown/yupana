# Architecture

Yupana is a peer of Bobbin and Quipu, not a feature of either. It quarantines the
heavy language toolchains (tree-sitter grammars, LSP servers, any CPG/dataflow
machinery), holds a hot structural graph, and feeds three consumers: Bobbin
(fusion), Quipu (promotion on commit), and the Gas Town broker/Aegis (per-tenant
blast radius as a trust boundary).

```text
   MCP (rmcp: stdio + HTTP/axum)  ·  CLI (clap)
   (a parallel REST HTTP API is Phase 3 — FR-27)
                    │
              Query / Serve layer
   refs · defs · callgraph · dataflow · blast-radius · verify
   (resolve against base + tenant overlay, tier/freshness tagged)
                    │
              Tenancy layer
   shared read-only base  +  per-tenant copy-on-write overlays
                    │
              Extraction layer
   tree-sitter (breadth) · LSP (precision) · CPG/dataflow (Phase 2)
                    │
   file-watch (notify) · git baseline · overlay cache
                    │
   Promotion boundary → Quipu (SHACL-validated Turtle)
```

## The routing rule

Bobbin is on the request path **only when fusion or ranking adds value.**
Multi-signal retrieval goes through Bobbin. Single-signal, analysis-only queries
— edit verification, blast radius, live structure lookups — go straight to Yupana,
and policy consumers like the broker read Yupana directly.

See the [Specification](../design/specification.md) §7 for the full component
diagram and the proposed source layout.
