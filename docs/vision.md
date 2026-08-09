# Bobbin × Yupana × Quipu: A Governed, Multi-Signal, Multi-Tenant Code Intelligence Layer

**Vision Document — v0.2**

---

## Thesis

Bobbin today answers *"what code is relevant to this?"* using two signals: embedding similarity (LanceDB hybrid search) and statistical co-movement (FP-Growth co-change mining over git history). Both are excellent at surface plausibility and historical correlation. Neither knows the *actual structure or semantics* of the code — a call edge, a type, a dataflow path, a definition site. Bobbin can tell you two files tend to change together; it cannot yet tell you *why*, or whether the coupling is real or coincidental.

Multispy, Joern, and codebase-memory each prove out a signal class Bobbin is missing. But the lesson from studying them — especially codebase-memory, which deliberately ships as a standalone analyzer rather than a bolt-on — is that this work does not belong *inside* Bobbin. It belongs in a peer tool. So the stack becomes three components with one responsibility each:

- **Yupana** — a new, in-memory code-analysis engine that extracts precise structure (AST, call graph, dataflow, LSP facts), keeps it hot, serves it, and does so *per tenant* so a team of developers can each edit without corrupting each other's view.
- **Quipu** — the governed, bitemporal substrate where *committed* structural facts live under a formal code ontology, queryable via SPARQL.
- **Bobbin** — unchanged in mission: the fusion and serving layer. It gets richer inputs (Yupana's structure, Quipu's history) to fuse with its own statistical and embedding signals, and serves explained context to agents over MCP.

The north star: **Yupana extracts and serves live per-tenant structure; Quipu governs and versions the committed record; Bobbin fuses everything and serves it — making autonomous agents better-informed, safely-bounded, and correct even when a whole team is editing at once.**

---

## The three sources, distilled

The point isn't to adopt these tools — it's to extract the *idea* each one proves out and re-home it in the Bobbin/Yupana/Quipu stack.

| Source | The idea worth stealing | What it contributes |
|---|---|---|
| **multilspy** (Microsoft) | A language-agnostic client over LSP yielding *precise, live* semantic facts — definitions, references, types, symbols — over one interface. Its origin (Monitor-Guided Decoding) proves a second idea: analysis facts can act as **generation guardrails**, constraining an LM to type-valid, arity-correct output. | Ground-truth semantics for Yupana to serve, and the notion of facts-as-guardrails (Bobbin's guard mode), not just facts-as-context. |
| **Joern** (joernio) | The **Code Property Graph** — AST + control-flow + data/program-dependence merged into one queryable graph — plus a **taint/dataflow engine** and a fuzzy parser that works without a build. | True structural reasoning: call graphs, control/data dependence, and **dataflow-based blast radius** — Yupana's heavy-analysis core, and the primitive that feeds capability scoping. |
| **codebase-memory** (DeusData/codebase-memory-mcp) | A very recent (arXiv preprint 28 Mar 2026; now ~32k stars) proof that structural code intelligence can be a lean, standalone analyzer: a single static C binary, tree-sitter across ~66 languages with tiered LSP-grade type inference for a subset, parallel extraction with multi-strategy call resolution and Louvain community detection, kept fresh by **content-hash incremental re-indexing on file changes**. | The closest existing analog to Yupana: in-memory, fast, serves structural answers at a fraction of the token cost of dumping files — and a proven incremental-freshness strategy Yupana can adopt directly. |

There is also a "second flavor" of codebase-memory worth naming — the **decision/convention memory** pattern (architecture rationale, conventions, gotchas persisted across sessions). That's intent, not structure, and it maps cleanly onto Quipu as typed, SHACL-validated knowledge. Worth carrying as a smaller parallel track.

---

## Where the pieces stand

**Bobbin** — a code context engine with MCP integration. Retrieval is LanceDB hybrid (vector + keyword) search; coupling is FP-Growth association-rule mining over co-change history. It already sits in the agent loop. Strengths: semantic recall and historical coupling. Under this vision its mission is *unchanged* — it becomes the fusion/serving layer and gains richer inputs.

**Quipu** — an ontology-enforced bitemporal knowledge graph in Rust/SQLite with RDF/SPARQL/SHACL. Already enforces shapes (SHACL), tracks both valid and transaction time, and is a component of the capability manifest system: Gas Town beads declare tool/data needs, a broker resolves them against Aegis-defined SHACL shapes in Quipu, and a scoped execution environment is provisioned for polecat agents. Under this vision it gains a **code ontology** and becomes the governed home for committed structural facts.

**Yupana** — new. The in-memory code-analysis and serving engine. Owns the language toolchains (LSP, tree-sitter, and any CPG/dataflow construction), holds the hot structural graph, serves it, and manages the multi-tenant reality of a team editing concurrently. Spiritually closest to codebase-memory, extended with dataflow (Joern), LSP precision (multilspy), tenancy, and a governed projection into Quipu.

---

## Three tools, one responsibility each — the component split

The central design question was: do these stolen ideas go into Bobbin, or into pluggable peers? The answer is peers. Here is where each idea plugs in.

| Stolen idea | Where it lives | Why not just Bobbin |
|---|---|---|
| LSP-precise facts: defs/refs/types (*multilspy*) | **Yupana** | Running a language-server per language is stateful, toolchain-heavy work — wrong layer for a retrieval engine |
| CPG + dataflow/taint, reachability (*Joern*) | **Yupana** builds → **Quipu** stores derived edges (on commit) | Heaviest analysis, JVM-flavored; quarantine it behind a fact-emitting boundary |
| Structural graph: call edges, routes, community detection (*codebase-memory*) | **Yupana** builds/serves → **Quipu** stores committed → **Bobbin** fuses | One idea spanning three layers — it doesn't map to a single tool |
| Token-efficient structural recall (*codebase-memory*) | **Bobbin** (serve) over **Yupana**/Quipu | This *is* Bobbin's mission — realized by serving structure instead of dumping files |
| Convention/decision memory (*codebase-memory, 2nd flavor*) | **Quipu** | Typed, SHACL-validated intent — governance, not extraction |
| Monitor-guided edit verification (*multilspy monitors*) | **Yupana** (served directly) | Single-signal analysis on the *edited* buffer — nothing for Bobbin to fuse, so it skips Bobbin exactly like blast radius does |
| Blast-radius-as-trust-boundary (native) | **Broker/Aegis** consumes; **Yupana** computes; **Quipu** stores | Pure policy — Bobbin isn't even in this path |

**Why Yupana is a separate tool, not a Bobbin feature:** it quarantines the toolchains (LSP servers, tree-sitter grammars, anything JVM from Joern) so they never link into Bobbin; it has a different lifecycle (incremental, event-driven, on-edit) versus Bobbin's interactive per-query path; and its facts feed three consumers — Quipu, Bobbin, *and* the broker's blast radius. Bury extraction inside Bobbin and the other two must route through Bobbin to get structural facts. As a peer, Yupana serves all of them. It also matches how the stack is already decomposed (Quipu, Aegis, polecat are separate peers), and it mirrors the design the strongest recent project in this space (codebase-memory) chose deliberately.

**The routing rule this implies:** Bobbin is on the request path only when fusion or ranking adds value. Multi-signal context retrieval goes through Bobbin. Single-signal, analysis-only queries — edit verification, blast radius, live structure lookups — go straight to Yupana, and policy consumers like the broker read Yupana directly too. That's why both verification and blast radius skip Bobbin: there's only one signal, so there's nothing to fuse.

Note the boundary is not dogmatic: the lightweight parsing Bobbin already does (chunking for embeddings, git-history co-change) stays in Bobbin. Yupana owns the *heavy, precise, toolchain-bound* analysis, not all parsing.

---

## The multi-tenant model — Yupana's hard problem

A team means there is no single "the AST." Each developer sits at some branch/commit **plus an uncommitted working delta**, and those deltas diverge. Rebuilding the whole graph per developer is wasteful (most of it is identical); sharing one mutable graph is wrong (A's experiment corrupts B's view). The model that resolves this:

**Shared base + per-tenant overlay (copy-on-write).** Compute the full structural graph once at a baseline commit (e.g. `main`), held read-only in memory. Each developer's session gets a lightweight **overlay**: only the files they've touched are re-parsed, and only the affected edges are recomputed and layered over the shared base. Queries resolve against `base + overlay`. With content-hash structural sharing (the codebase-memory trick), N developers cost *one base + N small deltas*, not N full graphs, and isolation is automatic — an overlay is invisible to other tenants.

**The overlay is not just the edited file — it's the edited file plus its frontier.** This is the crux. If a developer changes a function signature in file X, every *reference* to it — possibly in files they never opened — now has different (or invalid) type/dataflow facts. So updating the overlay means: re-parse X (cheap, tree-sitter incremental) → find the symbols that changed → **query the base graph for the references and dependents of those symbols** → recompute facts for that bounded frontier → store as overlay. Naive per-file incremental update is wrong precisely because the consequences are non-local; you need the base graph in memory to compute the frontier — which is exactly what Yupana provides.

**The key insight: blast radius is the incremental-update primitive.** The reachability query that answers *"what does this change affect?"* for a developer is the *same operation* that answers *"what do I need to recompute?"* for the updater. One primitive, two uses — build it once.

**Tier the freshness.** Full LSP-grade type/dataflow resolution on every keystroke is too expensive, so serve two tiers: fast tree-sitter structure (call edges, symbol tree) updated on save or debounced keystroke; precise LSP/dataflow facts updated on save or on-demand when a query needs them. This is exactly the tree-sitter-everywhere + LSP-for-a-subset split codebase-memory already ships.

**Yupana is the present; Quipu is the past.** Yupana holds the volatile, per-tenant, working reality — base plus overlays — and serves the *current* structural truth for a given developer's or agent's context. When changes actually land on a shared branch (commit/merge), the corresponding facts get **promoted** into Quipu as a new bitemporal state (valid-time = commit time; transaction-time = when learned). Quipu holds the settled, governed, versioned record; Yupana holds what's in flight. Clean division of labor, and it means uncommitted churn never pollutes the governed graph.

**Branches are a third axis — model them as named graphs.** Quipu's bitemporality gives a time axis, but tenancy adds a *parallel-worlds* (branch) axis. RDF **named graphs** are the natural mechanism: each branch's committed structural facts are a named graph, bitemporally versioned within. Yupana's per-developer overlays are the not-yet-promoted frontier of some branch.

**Tenancy makes capability scoping correct.** If blast radius scopes what a polecat may touch, and the graph is now per-tenant, then capability scoping must be computed against *the right tenant's* live graph — not a stale shared one. Yupana serving per-tenant blast radius is what keeps the trust boundary correct in a team setting; this is another reason the live per-tenant state must live in Yupana.

---

## The concrete capability set

Buildable capabilities, each tied to a source idea and a home.

**1. Ground-truth reference & definition resolution** *(multilspy → Yupana).* Precise "where is this defined / who references it," served to Bobbin to turn "probably relevant" into "provably connected."

**2. Call-graph & dataflow extraction** *(Joern + codebase-memory → Yupana).* Structural coupling (call edges, control/data dependence) for Bobbin to fuse with FP-Growth co-change.

**3. Blast-radius / impact analysis** *(Joern reachability + co-change → Yupana).* Reachable set computed structurally and reconciled with historical co-change. Doubles as Yupana's incremental-update primitive and as the input to capability scoping.

**4. Per-tenant live graph** *(the tenancy model → Yupana).* Shared base + copy-on-write overlays, content-hash sharing, frontier-bounded incremental updates, tiered freshness.

**5. Governed structural knowledge graph** *(codebase-memory → Quipu).* Committed facts promoted from Yupana into Quipu under a SHACL-validated code ontology; ingestion validates every fact against the shape of the domain.

**6. Bitemporal code archaeology + branch-aware named graphs** *(Quipu).* Query historical structure and the provenance of edges; each branch a named graph, bitemporally versioned.

**7. SPARQL-over-code** *(Joern's CPGQL idea, re-homed in Quipu).* Express committed-code queries as SPARQL against the governed graph rather than standing up a second query language and store.

**8. Monitor-guided edit verification** *(multilspy monitors → Yupana, served directly).* An agent's proposed edit is checked by re-running analysis on the edited buffer against the base graph Yupana already holds, returning "identifier doesn't exist / wrong arity / type violation." It's single-signal and boolean — nothing to fuse — so agents call Yupana directly rather than routing through Bobbin. Bobbin can still consume verdicts like any other Yupana fact (e.g. to flag retrieved code that won't compile in the current overlay), but that's the normal Yupana→Bobbin flow, not verification living in Bobbin.

**9. Static-analysis-as-trust-boundary** *(Yupana blast radius → Broker/Aegis).* Per-tenant blast radius scopes the provisioned execution environment for polecat — autonomous edits safe by construction, not by review.

**10. Decision/convention memory** *(codebase-memory 2nd flavor → Quipu).* Architecture rationale, conventions, and gotchas as typed, validated knowledge so agents stop re-asking and re-violating settled decisions.

---

## The differentiating insight

Most code-intelligence tools pick one signal and go deep. Bobbin already had two of three. The differentiated move is **fusion + governance + time + tenancy**:

- **Fusion** lets Bobbin answer *why* two things are coupled and rank by corroborated evidence. A co-change edge with no structural explanation is a refactoring smell; a co-change edge backed by a dataflow path is real coupling. No single signal makes that distinction.
- **Governance** (Quipu + SHACL) makes the committed graph typed and validated, not a best-effort cache — the arbiter when Yupana's extractors (tree-sitter vs LSP vs CPG) disagree.
- **Time** (Quipu bitemporality) makes every committed fact a historical record — impact analysis that accounts for how the code got here, replayable at any point.
- **Tenancy** (Yupana overlays + Quipu named graphs) adds the parallel-worlds axis, so the whole thing stays correct while a team edits — and blast radius does double duty as both a product feature and the incremental-update engine.

The trust-boundary tie-in is the part no off-the-shelf tool offers, because it depends on the existing Gas Town/Aegis/polecat machinery: **structure defines the sandbox, per tenant.**

---

## Suggested phasing

**Phase 1 — Yupana, single-tenant.** Stand up the in-memory analyzer: tree-sitter structure + LSP facts for one working copy, served over MCP. Bobbin fuses these with its existing co-change/embeddings. Immediate payoff: explained retrieval and precise references, no new store.

**Phase 2 — Dataflow & blast radius in Yupana.** Add Joern-style CPG/dataflow (embed Joern behind a process boundary, or reimplement the needed traversals in Rust). Deliver structural blast radius, reconciled with historical co-change.

**Phase 3 — Multi-tenancy.** Shared base + copy-on-write overlays, content-hash sharing, frontier-bounded incremental updates via file-watch, tiered freshness. This is the hard phase; blast radius from Phase 2 is the reused primitive.

**Phase 4 — Promote to Quipu.** Design the code ontology (SHACL shapes for functions, edges, modules, flows), promote committed facts on commit/merge, branches as named graphs, bitemporality on, SPARQL-over-code.

**Phase 5 — Consumption & guardrails.** Wire per-tenant blast radius into capability scoping (the polecat trust boundary) and ship Yupana's monitor-guided guard mode as a direct verification surface.

---

## Risks & open questions

- **The JVM/Rust fork, now scoped to Yupana.** Joern is JVM/Scala; the stack is Rust-centric. Running Joern as a subprocess extractor inside Yupana is pragmatic but adds a heavy dependency and a serialization seam; reimplementing just the CPG/dataflow traversals you need in Rust keeps Yupana coherent. Resolve early (Phase 2). Isolating this in Yupana is itself a reason Yupana exists.
- **Overlay memory & churn.** Per-tenant overlays plus a large base must stay within budget; content-hash sharing helps but frontier recomputation on hot files (widely-referenced signatures) can cascade. Decide overlay eviction and whether very-high-fan-in symbols get special handling.
- **When to promote to Quipu.** On every commit? Only on merge to a tracked branch? Promotion cost vs. history completeness is a real tradeoff; bitemporality lets you be lazy but not free.
- **Two graph engines.** Yupana's in-memory graph serves interactive dataflow queries that are genuinely painful over RDF/SPARQL; Quipu serves governed/temporal/cross-domain queries. Keep the split honest — don't let Yupana's transient store drift into a second source of truth for committed facts.
- **Freshness tiers & staleness semantics.** Agents must know whether a served fact is tree-sitter-fast-but-approximate or LSP-precise. The fact model needs a confidence/tier tag.
- **Build-free vs build-required.** Joern's fuzzy parser works without a build; LSP needs a resolvable build for precise types. Yupana likely wants both — fuzzy/tree-sitter for always-on breadth, LSP for precision where a build exists — with the ontology representing facts of differing confidence.
- **Ontology design cost.** A good code ontology is real modeling work; over-constrained SHACL will reject legitimate facts from messy real-world code. Start permissive, tighten deliberately.
- **Query surface sprawl.** Resist standing up CPGQL *and* SPARQL *and* many MCP tools as permanent interfaces. Consolidate on SPARQL-over-Quipu plus Yupana's and Bobbin's MCP surfaces.

---

## The one-line version

Build **Yupana** as an in-memory, multi-tenant code analyzer that extracts precise structure and serves it — using a shared-base-plus-overlay model where blast radius doubles as the incremental-update engine; promote committed facts into **Quipu** as a governed, bitemporal, branch-aware code graph; and let **Bobbin** fuse all of it with its statistical and embedding signals. Multispy, Joern, and codebase-memory each prove out one piece — the value is fusing them, governing the result, and keeping it correct while a whole team edits.
