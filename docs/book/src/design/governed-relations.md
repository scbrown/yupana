# Governed Relations — Yupana ⋈ Quipu

> Yupana knows the *live* structure of the code. Quipu knows the *governed record*
> of everything related to it — docs, work items, commits, policies, decisions.
> They share one IRI scheme, so Yupana can tell you, at the moment you touch a
> symbol, **everything the governed graph already knows about what you changed.**

## The idea

Yupana exports its structure in the **same `bobbin:` code ontology that Quipu
stores** (FR-20; `src/export.rs` mirrors Quipu's `namespace.rs`). A `CodeSymbol`
IRI in Yupana's live graph *is* the same node in Quipu's governed graph. That
shared IRI is a free join key: Yupana's live blast radius on one side, Quipu's
committed neighborhood on the other, joined by identity.

This turns a single question into a general one. "Which docs reference this
symbol?" stops being a special case and becomes one filter over **"what governed
facts relate to these IRIs?"**

## The primitive

> Given a symbol — or the impacted set from an edit — ask Quipu for the governed
> neighborhood of those IRIs, and fold it back **tagged as the past.**

Yupana keeps ownership of the live structural answer (blast radius, dataflow). The
neighborhood is Quipu's governed answer. Yupana does the join and annotates its own
output — it never re-derives or re-owns Quipu's edges.

```text
impacted set (Yupana, live)          governed neighborhood (Quipu, committed)
  foo::bar  ─────────────┐         Section --references--> foo::bar   (docs/x.md#y)
  foo::baz  ─────────────┼──⋈──►   GitCommit --modifies--> foo::bar   (bead st-42)
  ...                    │         Policy --targets--> <type of foo::bar>
                         └──────►  Decision --targetRef--> foo::bar    (prior HITL)
```

## The join key: one ontology

- Yupana promotes / exports `CodeModule` / `CodeSymbol` with `definedIn` / `calls`
  / `imports` in the `bobbin:` ontology (FR-19/FR-20, `yupana export`).
- Quipu's `shapes/code-entities.ttl` defines the same `CodeModule` / `CodeSymbol`
  / `Document` / `Section`, and the governance plane
  (`shapes/governance.ttl`) extends that model in the same namespace.
- Because both sides mint the same IRIs, the join is identity — no mapping table,
  no fuzzy match.

## Tiering: this is the past, and it says so

Every fact Yupana serves carries a `tier` and `freshness` (FR-3). Governed
relations come back tagged `committed` (or `attested` for signed verdicts) —
**never mixed into the live `treesitter` / `lsp` structural tier.** This is the
present-vs-past split (§9) made legible at the fact level: Yupana annotates the
*present* (its live edit) with the *past* (Quipu's settled record), and the
consumer always knows which is which.

## Relation types this unlocks

One query, different predicate filters:

| Governed edge (Quipu) | Surfaced as |
|---|---|
| `Section --references--> CodeSymbol` | doc-staleness — "docs/x.md#y may be stale" |
| `GitCommit --modifies-->`, `Bead --implements-->` | traceability — "traces to bead st-42 / epic E-3" |
| `Policy --targets--> <type>`, `PlanStep --references-->` | governance — "a policy gates this; a plan step depends on it" |
| `Decision` / `Verdict --targetRef-->` | staleness of intent — "a prior decision was bound to this code" |
| cooccurrence / PageRank neighborhood | "these entities usually move together" |

## Principles

- **Borrow, don't derive (§9.6).** Quipu is the single source of truth for
  governed relations. Yupana *reads* them and tags them `committed`; it never
  promotes them back or treats them as its own structural facts. The day Yupana
  re-owns a Quipu edge is the day there are two sources of truth.
- **Optional, and it degrades.** Gate the enrichment behind the existing `quipu`
  feature. If Quipu is unreachable, the enrichment is simply *absent* — the code
  blast radius still answers. "Could not look" is not "nothing there" (the tier
  discipline, one layer up).
- **Latency by surface.** The advisory `post-edit` hook can afford a best-effort
  synchronous Quipu query. The blocking `pre-edit` guard cannot — it stays on the
  live graph (or a startup-loaded cache), never a synchronous Quipu round-trip
  (FR-31's sub-100ms budget).

## Surfaces

- **`yupana relations <symbol>` / `yupana_relations` (MCP)** — the governed
  neighborhood of a symbol, with an optional predicate filter, tier-tagged
  `committed`.
- **Folded into `yupana impact`** — the impact response gains a `governed_relations`
  section for the impacted set.
- **Folded into the `post-edit` hook** — the advisory gains a "governed relations
  touching your change" block.

## Worked example: doc-staleness

Doc-staleness is the `references` filter over the primitive. On an edit to
`foo::bar`, Yupana's blast radius yields the affected symbols; the join asks Quipu
for `Section --references-->` edges into that set; the advisory says:

```text
governed relations (committed):
  docs/architecture.md#data-flow  references foo::bar  — may be stale
```

Once the referential graph is in Quipu, the same fact is auditable over time as a
SPARQL query — *"every Document that references a CodeSymbol which no longer
exists"* (§5.10). The live hook is the in-the-moment view; the query is the
durable one.

## Status

- **Exists today:** `yupana export --format turtle` emits the code side
  (`CodeModule` / `CodeSymbol` + `definedIn` / `calls` / `imports`) and, via
  `src/docref.rs`, the doc→code `Section --references--> CodeSymbol` edges.
- **This design adds:** the *read* direction — Yupana querying Quipu by shared IRI
  to enrich its live output, tier-tagged `committed`, behind the `quipu` feature,
  exposed as `yupana_relations` and folded into `impact` / `post-edit`.
- **Boundary held:** Yupana owns structural references; embeddings and semantic
  retrieval stay in Bobbin, the governed record stays in Quipu (§5.10).
