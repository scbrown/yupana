# Code slices as Quipu share bundles

A Quipu **share bundle** is a git-storable, content-addressed directory holding a canonical RDF
graph, the shapes that describe it, and a lineage manifest. This page is the measured workflow for
turning a Yupana code-graph slice into one, and for importing it into somebody else's store. For
the consumer side — pulling a bundle somebody hands YOU into the graph Yupana reads — see
[Pulling a Share](share-pull.md).

Yupana is the **producer of structural code facts and nothing more**. It does not write manifests,
compute share ids, or canonicalize RDF: Quipu authors the bundle from the graph it holds, so a
bundle built this way carries `producer.name = "quipu"`. That is by design, not a gap — one
implementation of the hashing rules means one set of rules.

## The pipeline

```bash
# 1. Yupana emits the governed code projection. --repo is REQUIRED for anything shared:
#    the repo name is a segment of every entity IRI (see "Promotion to Quipu").
yupana export src --repo myrepo --format turtle > slice.ttl

# 2. Quipu ingests it and holds it.
quipu knot slice.ttl --db store.db
quipu shapes load code-entities <quipu>/shapes/code-entities.ttl --db store.db

# 3. Quipu AUTHORS the bundle. Always pass --shapes (see the warning below).
quipu share --output ./bundle \
  --construct 'CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }' \
  --shapes code-entities --db store.db
```

That writes `bundle/` containing exactly:

| file | what |
|---|---|
| `manifest.json` | schema `…/share-manifest/v1`, `share_id`, `store_id`, `tx_anchor`, `graph_hash`, `shapes_hash`, `scope`, `parent_share`, `producer` |
| `export.nt` | canonical N-Triples — the bytes `graph_hash` covers |
| `shapes.ttl` | the shape sets named by `--shapes` |

A code slice selected by CONSTRUCT records `scope.kind = "construct"`.

Import is **REST-only** — there is no `quipu import` subcommand:

```bash
curl -s http://your-quipu/import -X POST -H 'Content-Type: application/json' -d @- <<JSON
{"manifest": …, "export_ntriples": "…", "shapes_turtle": "…", "source": "yupana-code-slice:myrepo"}
JSON
```

The importer verifies the schema, the three payload basenames, both content hashes and the
`share_id` **before parsing any RDF**, then stages into a per-source named graph. It never writes
ROOT directly. A second byte-identical import returns `unchanged`, which is a success.

## Hash the canonical form, never the producer's Turtle

`yupana export` serializes in **filesystem traversal order**. The same code, checked out twice into
differently-named directories, produces the same triples in a **different byte order** — measured.
So:

> **A `graph_hash` taken over Yupana's raw Turtle is not stable across checkouts. Taken over
> Quipu's canonical `export.nt`, it is.**

Two independently-created checkouts of identical content yield byte-differing `slice.ttl` and an
**identical `graph_hash`**. Hashing the producer's output is the obvious shortcut and it is exactly
the thing that breaks; it is written down here so nobody re-derives it as an optimization.

This is also why "export twice and compare bytes" is a **weak** test — it passes for a canonical
producer *and* for a path-dependent one, so it cannot tell them apart. The discriminating test is
cross-condition: same content, two checkout paths, same hash required.

## Local shapes decide admission — bundled shapes are evidence

The bundled `shapes.ttl` does **not** grant a bundle the right to assert its vocabulary in a
stranger's store. Measured, one variable at a time:

| bundle `shapes.ttl` | consumer's local shapes | result |
|---|---|---|
| present | `code-entities` loaded | `staged`, all triples accepted |
| **empty** | `code-entities` loaded | `staged` — **identical**; the bundled shapes changed nothing |
| present | none | **`quarantined`**, 0 accepted, blocker `off_vocabulary` naming `CodeModule`/`CodeSymbol` |
| present, then adopted with `quipu shapes load` | now loaded | `staged` |

The consumer's own shapes always decide. That is deliberate: a bundle must not be able to widen a
stranger's vocabulary just by shipping a file that says it may.

**Receiving a quarantined slice** — the recovery path is the last row: read the bundle's
`shapes.ttl`, review what it would admit, `quipu shapes load` it deliberately, re-import. Quarantine
is not a rejection; the triples are staged and named, and the response tells you which types
blocked promotion.

> ⚠️ **Always pass `--shapes`.** `quipu share` with no `--shapes` writes a **zero-byte
> `shapes.ttl` and exits 0**. That bundle passes every integrity check and imports cleanly on any
> consumer that already holds the shapes — i.e. on your own team, who will never see the problem —
> while being the one bundle a fresh consumer can **never** adopt, because the recovery path above
> needs a file with content in it. Filed upstream against Quipu; until it refuses, passing
> `--shapes` is the whole mitigation.

## Outward sharing

Every entity IRI embeds the ontology base host. That host **is entity identity**, not decorative
metadata: rewriting or stripping it changes what the bundle asserts and breaks merging against any
store holding the unrewritten form. Publishing a bundle outside the network it names is therefore a
governance decision, not a scrub — do not automate it here.

## Regression

The whole pipeline — cross-condition hash, bundle integrity, the four refusal arms, quarantine, and
idempotence — is pinned by an end-to-end regression that stands up two independent Quipu stores and
round-trips a real slice between them. Reproduce it with the commands on this page; the arms that
matter are the cross-condition hash and the bare-consumer quarantine, because each fails in the
reassuring direction if you only run the easy version.
