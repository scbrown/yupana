# Why Yupana, and how it compares

Structural code intelligence is not new, and the strongest tools each prove out
one signal. Yupana takes the best idea from each, then adds what none of them
have: **a whole team editing at once, governance, and time.**

## What it is for

- 🧵 **Correct while a team edits.** A shared base graph plus a copy-on-write
  overlay per developer or agent keeps every view right while many of them edit
  the same code at once. See [The Tenancy Model](tenancy-model.md).
- 🔀 **Fusion, not one signal.** Call and dataflow structure, plus historical
  co-change, plus embeddings. A coupling backed by a dataflow path is real; one
  without it is a refactoring smell, and only fusion tells them apart.
- 🪢 **Governed and time-travelable.** Committed facts promote into
  [Quipu](https://github.com/scbrown/quipu) as SHACL-validated, bitemporal RDF:
  a versioned source of truth, not a best-effort cache. See
  [Promotion to Quipu](promotion.md).
- 💥 **Blast radius as a primitive.** "What will this change break" is one
  query, per tenant, and it doubles as the incremental-update engine.
- ⚡ **Two-tier freshness.** Tree-sitter speed for breadth and LSP precision for
  depth, with every fact tagged by the tier that produced it so an agent knows
  what it is trusting. See [Tiers and Freshness](tiers-and-freshness.md).
- 🛡️ **Structure scopes the sandbox.** Per-tenant blast radius bounds what an
  autonomous agent may touch, and can act as a guardrail on what it generates,
  not only as context.
- 🪙 **Token-cheap.** Structural answers instead of whole files in context.

## How it compares

| | **codebase-memory** | **Joern (CPG)** | **LSP / multilspy** | **Embeddings / co-change** | **Yupana** |
|---|:--:|:--:|:--:|:--:|:--:|
| Fast structural graph, low token cost | ✅ | ⚠️ | ❌ | ✅ | ✅ |
| Call graph + **dataflow / taint** | ⚠️ | ✅ | ⚠️ | ❌ | ✅ |
| Precise LSP-grade types | tiered | ❌ | ✅ | ❌ | tiered |
| Incremental freshness on edit | ✅ | ❌ | ✅ | ❌ | ✅ *(frontier-bounded)* |
| **Correct while a team edits concurrently** | ❌ | ❌ | ❌ | ❌ | ✅ *(per-tenant overlays)* |
| **Governed, versioned, time-travel record** | ❌ | ❌ | ❌ | ❌ | ✅ *(→ Quipu)* |
| Blast radius scopes an **agent trust boundary** | ❌ | ❌ | ❌ | ❌ | ✅ |

Each of these proves one piece. [multilspy](https://github.com/microsoft/multilspy)
shows that LSP facts can also guard what a model generates;
[Joern](https://joern.io) established the Code Property Graph and dataflow;
codebase-memory is a lean standalone analyzer with content-hash incremental
freshness. Yupana is closest in spirit to codebase-memory, extended with
Joern-style dataflow, LSP precision, tenancy, and a governed projection into
Quipu.

> The advantage is not any single signal. It is **fusion, governance, time and
> tenancy**, kept correct while a whole team edits.

## What is built today

- **Structure:** `analyze`, `refs`, `callers`, `impact` (with `--cochange`
  reconciliation), intra-procedural `dataflow`, and `verify`, which returns a
  verdict on a proposed edit buffer. See the [CLI reference](../reference/cli.md).
- **Serving:** an MCP server (`yupana serve`) exposing the `yupana_*` tools, and
  a resident daemon (`yupana daemon`) that keeps the base graph and per-tenant
  overlays hot. See [MCP Tools](../reference/mcp-tools.md) and
  [Resident Daemon](../reference/daemon.md).
- **Governance, around the graph:** the pre-edit policy guard (scopes,
  structural and text rules, tripwires, and the work-item scope ladder), the
  record-only Bash action hook, session-start work-item briefings, the
  game-state harness, and the golden-path conformance guard. See
  [Pre-Edit Policy Guard](../reference/policy-guard.md).

Rules that come from Quipu start at the advise tier: they warn, and they block
only once their own enforcement gates are met. `yupana exemplar` drafts rule
candidates from an example of what should have been caught, and
`yupana verifier` prints the key that signs verdicts, for registering in Quipu.
The full phasing is in the
[specification](https://github.com/scbrown/yupana/blob/main/docs/yupana-spec.md#12-milestones--phasing).
