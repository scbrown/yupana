# The Stack: three tools, one job each

```text
        edit / save / file-watch
                 │
                 ▼
   ┌──────────────────────────┐   promote on commit/merge   ┌──────────┐
   │          YUPANA          │ ──────────────────────────► │  QUIPU   │
   │  base graph + overlays   │   (SHACL-validated Turtle)  │ EAVT log │
   │  tree-sitter + LSP + CPG │ ◄────────────────────────── │ SPARQL   │
   └────────────┬─────────────┘  SPARQL over committed code └──────────┘
                │ blast radius (per tenant)
                ▼
        ┌───────────────┐     trust boundary      ┌──────────┐
        │ Bobbin fusion │◄────────────────────────│  agents  │
        │ + serving     │────────────────────────►│          │
        └───────────────┘   explained context     └──────────┘
```

- **[Yupana](https://github.com/scbrown/yupana)** (this project) extracts and
  serves live, per-tenant code structure.
- **[Quipu](https://github.com/scbrown/quipu)** governs and versions the
  committed record: bitemporal RDF, queried with SPARQL, validated with SHACL.
- **[Bobbin](https://github.com/scbrown/bobbin)** fuses everything with its
  statistical and embedding signals and serves explained context over MCP.

The north star is the
[vision document](https://github.com/scbrown/yupana/blob/main/docs/vision.md);
the full build spec is the
[specification](https://github.com/scbrown/yupana/blob/main/docs/yupana-spec.md).

## What Yupana and Quipu unlock together

Yupana holds the *live* structure; Quipu governs the *committed* record.
Together they do things neither does alone:

- **Governed SPARQL over code.** Query committed structure as typed, validated
  facts: "every public function with no test", "modules that violate the
  layering", "who still calls this deprecated API". Not a cache you hope is
  fresh.
- **Impact over history.** Bitemporal facts answer what a change broke and when
  a coupling first appeared: blast radius that accounts for how the code got
  here, replayable at any point in time.
- **Ontology rules that block or steer changes.** Architectural constraints are
  authored as rules in Quipu (SHACL over the code graph). Yupana evaluates a
  proposed edit against them, live and per tenant, and warns or blocks before
  it lands. A new rule is a graph assertion, not a new bespoke linter.
- **Per-tenant parallel worlds.** A shared base plus copy-on-write overlays map
  onto Quipu named graphs, so a whole team edits at once without corrupting
  each other's view, over one always-queryable source of truth.
- **Agent trust boundaries.** Per-tenant blast radius scopes what an autonomous
  agent may touch: the structure defines the sandbox.
- **Code linked to intent.** Quipu provenance ties structural facts to the
  decisions and work items that produced them: "which decision does this
  module implement", "which tickets co-occur with this code path".
- **A decidable audit.** Every enforcement decision emits a trace record derived
  from the constraint set, plus an ed25519-signed verdict bound to what was
  actually checked. `quipu audit <trace>` then decides mechanically whether the
  trace satisfied the constraints, without access to the model, its prompts, or
  its developers. See [The Enforcement Trace](../reference/enforcement-trace.md)
  for the record, and [SARC Conformance](../design/sarc-conformance.md) for what
  the pair does and does not yet close.
