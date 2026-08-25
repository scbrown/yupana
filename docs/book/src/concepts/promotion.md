# Promotion to Quipu

Yupana holds the volatile, per-tenant, in-flight reality. When changes land on a
shared branch (commit/merge), the corresponding facts are **promoted** into
Quipu as a new bitemporal state — valid-time = commit time, transaction-time =
when learned. Quipu holds the settled, governed, versioned record; Yupana holds
what's in flight. Uncommitted churn never pollutes the governed graph.

## Export — the governed projection

`yupana export` is the projection Yupana promotes: the **precise, typed referential
structure** (modules, symbols, `definedIn`/`calls`/`imports`, and — as the
markdown extractor lands — `Document`/`Section` + `references`), emitted as RDF
Turtle in the `bobbin:` ontology. This is **not** Bobbin's chunking; it is
structure for reasoning and governance.

Module dependencies (`bobbin:imports`, `CodeModule → CodeModule`) are resolved
from `use`/`mod` declarations at the tree-sitter tier — best-effort by module
stem, so they over-connect on shared names like any tree-sitter-tier fact; the
`lsp` tier refines them.

```bash
yupana export src --repo myrepo --format turtle    # dump the referential graph
yupana promote --commit HEAD --to "$QUIPU_URL" .   # SHACL-validate + write it
```

`yupana promote` needs `--features quipu`; without it the binary says so rather
than pretending. It emits the Turtle, SHACL-validates in-process against
`shapes/`, and writes to `/knot` **only if it conforms** — a rejected promotion
exits non-zero so a script can't read it as landed.

## When promotion runs — `promote_on` and `--trigger`

Yupana installs no git hooks and owns no commit event of its own, so it does not
decide *when* a commit happened — the caller that has the event says so, and
`[yupana.quipu] promote_on` decides whether that event promotes:

```bash
# .git/hooks/post-commit — fires on every commit, promotes per policy
yupana promote --commit HEAD --trigger commit --to "$QUIPU_URL" .
```

| `promote_on` | `--trigger manual` (default) | `--trigger commit`, plain | `--trigger commit`, merge commit | `--trigger merge` |
|---|---|---|---|---|
| `manual` | promotes | skipped | skipped | skipped |
| `commit` | promotes | promotes | promotes | promotes |
| `merge` (default) | promotes | skipped | **promotes** | promotes |

Two things worth knowing. A `commit` event on a **merge commit** counts as a
merge — git knows (two or more parents) even when the hook does not — so the
default policy works from the simplest possible `post-commit` hook. And
`--trigger manual` always promotes: `promote_on` governs *automation*, not
authorization. `--to` remains the only thing that authorizes a write, and
`serve.read_only` the only thing that refuses one.

A skipped promotion exits **0** and prints `SKIPPED … Wrote nothing`. That is
deliberate: a hook that failed on every ordinary commit gets switched off within
a day, and the sentence carries the fact the exit code would have. An
unrecognised `promote_on` value is **refused**, not defaulted — a typo must not
be indistinguishable from the key working.

Large projections are **chunked**: Quipu's request body limit is ~2 MiB, so a
projection over the line is split on entity-block boundaries and posted as
multiple `/knot` writes (the output says so: `... in 3 chunks`). Validation is
still whole-graph and up front; a chunked write is not atomic across chunks,
but IRIs are deterministic and `/knot` supersedes, so re-running a failed
promotion converges instead of duplicating — the failure message names exactly
how many chunks landed.

Code and docs are one referential graph (spec §5.10): code leans real-time (the
live graph + edit hook), docs lean asynchronous (this export). Once in Quipu,
doc rot becomes a SPARQL query — "every `Document` referencing a `CodeSymbol`
that no longer exists."

- Facts are emitted as Turtle in the existing `bobbin:` code ontology and
  **SHACL-validated before write** (in-process via `rudof_lib`, FR-20) — Yupana
  never writes to Quipu without passing `shapes/code-edges.ttl`, the compiled-in
  shape set. It gates the structural edges (`calls`, `references`, `imports`,
  `dataDependsOn`, `controlDependsOn`, `hasTier`) and the `Section → references`
  edge (§5.10), and carries the node-shape constraints synced byte-for-byte
  from Quipu's `code-entities.ttl` so a shape drift is caught at Yupana rather
  than discovered as a Quipu refusal. A real `export` projection is
  round-trip-validated against these shapes in the test suite, so the emitter
  cannot drift from the gate unnoticed.
- Writes go through Quipu's existing surface (`quipu_knot` / `POST /knot` /
  `Store::transact`), honoring `valid_from`/`valid_to`, `actor`, and `source`
  (the commit SHA).

## Querying it back — dependency and blast radius

Once promoted, the dependency graph is queryable in Quipu. Store the **direct**
edges (`bobbin:calls`, `bobbin:imports`) and let SPARQL property paths do the
transitive work — never pre-compute and store a transitive closure that goes
stale. These queries are verified against live Quipu (`POST /query`, JSON body
`{"query": "…"}`).

**What does a symbol depend on?** (one hop)

```sparql
PREFIX bobbin: <http://aegis.gastown.local/ontology/>
SELECT ?dep WHERE { ?s bobbin:name "hbiw_alpha" . ?s bobbin:calls ?dep }
```

**Blast radius — what breaks if a symbol changes?** The transitive set of callers,
the `+` property path (this is the "if X dies, what breaks?" query; assert its
*members*, not a nonzero count):

```sparql
PREFIX bobbin: <http://aegis.gastown.local/ontology/>
SELECT ?affected WHERE { ?t bobbin:name "hbiw_beta" . ?affected bobbin:calls+ ?t }
```

Code entities do **not** suffer the alias-fragmentation that afflicts the
human-named infrastructure graph (a blast-radius query over fragmented nodes
returns a confident *subset*, worse than nothing): Yupana mints one deterministic
IRI per symbol (`…/code/<repo>/<file>::<scope…>::<symbol>`), so re-promotion
updates the same node rather than minting a synonym, and the `calls+` closure
is complete. The scope chain — enclosing module/impl/trait/class/function
names, with a trait impl written `Type@Trait` — is what keeps two same-named
symbols in one file on distinct IRIs (without it, 42 same-name collisions
across three real repos silently merged into single nodes, unioning different
symbols' call edges). It is sibling-independent: adding a second `run`
elsewhere in the file never renames the first.

## Branches as named graphs

Each branch's committed facts belong in an RDF **named graph**, bitemporally
versioned within. Quipu is a triple store today, so this is tracked as an
additive, default-graph-preserving quad-store extension —
[scbrown/quipu#36](https://github.com/scbrown/quipu/issues/36). Until it lands,
Yupana can fall back to a branch qualifier. See the
[Specification](../design/specification.md) §9 for the ontology extension and
the quad-store RFC sketch.
