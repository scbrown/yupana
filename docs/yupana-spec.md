# Yupana — Product Requirements & Build Specification

**Version:** 0.2
**Status:** Living (Phases 1–2 implemented; Phase 3 next)
**Last Updated:** 2026-07-18
**Owning vision:** [`docs/vision.md`](./vision.md) — *Bobbin × Yupana × Quipu: A Governed, Multi-Signal, Multi-Tenant Code Intelligence Layer (v0.2)*

> **New here / picking this up?** Start with **Appendix D (Implementation
> Status)** for exactly what is built, **Appendix E (Design Decision Log)** for
> why the architecture is the way it is, and **Appendix F (Handoff & Next
> Steps)** for what to build next and how.

---

## 1. Executive Summary

Yupana is an **in-memory, multi-tenant code-analysis engine** written in Rust. It
extracts precise structure from a codebase — AST, symbols, call graph, control-
and data-dependence, and LSP-grade type/reference facts — keeps that structure
hot in memory, and serves it over MCP (stdio and streamable-HTTP; a parallel REST
API is Phase 3, FR-27). It does so **per
tenant**, so an entire team can edit concurrently without corrupting each
other's view of the graph, using a **shared-base-plus-copy-on-write-overlay**
model in which *blast radius doubles as the incremental-update primitive*.

Yupana is the third peer in an existing stack:

- **Bobbin** (`scbrown/bobbin`, v0.6.0) — the fusion/serving layer. Retrieval is
  LanceDB hybrid (vector + keyword) search; coupling is FP-Growth co-change
  mining over git history. Bobbin's mission is unchanged; it gains Yupana's
  structural facts as a new signal to fuse and explain.
- **Quipu** (`scbrown/quipu`, v0.3.3) — the governed, bitemporal knowledge graph
  (RDF model over a SQLite EAVT fact log, SPARQL 1.1, SHACL via `rudof`). Quipu
  becomes the settled home for *committed* structural facts under a code
  ontology it already partially defines (`shapes/code-entities.ttl`).
- **Yupana** (this spec) — new. Owns the language toolchains, holds the volatile
  per-tenant working graph, and feeds three consumers: Bobbin (fusion), Quipu
  (promotion on commit), and the Gas Town broker/Aegis (per-tenant blast radius
  as a trust boundary).

The north star, restated as an engineering contract: **Yupana extracts and serves
live per-tenant structure; Quipu governs and versions the committed record;
Bobbin fuses everything and serves it.**

This document specifies what Yupana must do (functional requirements), how well
(non-functional requirements), how it is built (architecture and technology
choices, matched to Bobbin and Quipu), how it integrates (MCP surface, config,
Quipu promotion), and in what order (phasing). It deliberately reconciles the
vision with what the two existing peers *actually* implement today — most
importantly, that Quipu is a **triple store, not a quad store**, so the vision's
"branches as named graphs" needs a concrete design (§9.4), not an assumption.

---

## 2. Problem Statement

### 2.1 What Bobbin cannot answer today

Bobbin answers *"what code is relevant to this?"* using two signals: embedding
similarity and statistical co-change. Both are excellent at surface plausibility
and historical correlation. Neither knows the **actual structure or semantics**
of the code — a call edge, a type, a dataflow path, a definition site. Bobbin
can tell you two files tend to change together; it cannot yet tell you *why*, or
whether the coupling is real or coincidental.

A co-change edge with no structural explanation is a refactoring smell; a
co-change edge backed by a dataflow path is real coupling. **No single signal
makes that distinction** — which is exactly the gap Yupana fills.

### 2.2 Why this belongs in a new tool, not in Bobbin

1. **Toolchain quarantine.** LSP servers, tree-sitter grammars, and any
   CPG/dataflow machinery (potentially JVM-flavored, from Joern) are stateful,
   heavy, and must never link into Bobbin's retrieval path.
2. **Different lifecycle.** Bobbin's path is interactive and per-query. Yupana's is
   incremental, event-driven, and on-edit — a hot resident graph updated by a
   file-watcher, not rebuilt per request.
3. **Three consumers, not one.** Yupana's facts feed Quipu, Bobbin, *and* the
   broker's blast radius. Bury extraction inside Bobbin and the other two must
   route through Bobbin to get structural facts. As a peer, Yupana serves all three
   directly.
4. **Precedent.** The stack is already decomposed into peers (Quipu, Aegis,
   polecat). The strongest recent project in this space (codebase-memory) chose
   the standalone-analyzer design deliberately.

### 2.3 The multi-tenant reality

A team means there is **no single "the AST."** Each developer sits at some
branch/commit **plus an uncommitted working delta**, and those deltas diverge.
Rebuilding the whole graph per developer is wasteful (most of it is identical);
sharing one mutable graph is wrong (A's experiment corrupts B's view). Any
credible code-intelligence layer for a *team* of agents and humans must solve
this, and none of the source tools (multilspy, Joern, codebase-memory) do.

### 2.4 The routing rule this implies

Bobbin is on the request path **only when fusion or ranking adds value.**
Multi-signal context retrieval goes through Bobbin. Single-signal, analysis-only
queries — edit verification, blast radius, live structure lookups — go **straight
to Yupana**, and policy consumers like the broker read Yupana directly. Verification
and blast radius skip Bobbin because there is only one signal, so there is
nothing to fuse.

The boundary is not dogmatic: the lightweight parsing Bobbin already does
(chunking for embeddings, git-history co-change) **stays in Bobbin**. Yupana owns
the *heavy, precise, toolchain-bound* analysis, not all parsing.

---

## 3. Relationship to Bobbin and Quipu

| Concern | Bobbin (v0.6.0) | Quipu (v0.3.3) | Yupana (this spec) |
|---|---|---|---|
| Mission | Fuse + serve context | Govern + version committed facts | Extract + serve live structure |
| State | Per-query, index on disk | Append-only bitemporal log | Hot in-memory, per-tenant |
| Freshness | Re-index on change | On commit/merge (promotion) | On save / debounced keystroke |
| Primary store | LanceDB (+ SQLite coupling) | SQLite EAVT triple log | In-memory graph (+ overlay cache) |
| Signals | Embeddings, co-change | Committed structure, time | Structure, semantics, dataflow |
| Interface | MCP + HTTP + CLI | MCP handlers + REST + CLI | MCP + CLI (parallel HTTP API: Phase 3) |
| On request path? | When fusion helps | For governed/temporal queries | For single-signal analysis |

**Data flow (steady state):**

```text
        edit / save / file-watch
                 │
                 ▼
   ┌───────────────────────────┐        promote on commit/merge
   │           YUPANA            │ ───────────────────────────────► ┌──────────┐
   │  base graph + overlays    │        (SHACL-validated Turtle)   │  QUIPU   │
   │  tree-sitter + LSP + CPG  │ ◄─────────────────────────────── │ EAVT log │
   └────────────┬──────────────┘        SPARQL over committed code │ SPARQL   │
                │                                                   └──────────┘
   structural   │ blast radius                 governed history
   facts +      │ (per tenant)                        ▲
   verdicts     │                                     │ fuse
                ▼                                      │
        ┌───────────────┐   broker/Aegis         ┌────┴─────┐
        │ Bobbin fusion │◄──── (trust boundary) ─│  agents  │
        │ + serving     │────────────────────────►│ (polecat)│
        └───────────────┘   explained context    └──────────┘
```

Where each stolen idea lives (from the vision, made concrete):

| Idea (source) | Home | Realized as |
|---|---|---|
| LSP defs/refs/types (*multilspy*) | Yupana | §5.2 reference/definition resolution |
| CPG + dataflow/taint (*Joern*) | Yupana builds → Quipu stores | §5.3 call graph + dataflow |
| Structural graph, community detection (*codebase-memory*) | Yupana → Quipu → Bobbin | §5.3, §9, Bobbin fusion |
| Token-efficient structural recall | Bobbin over Yupana/Quipu | Bobbin serves structure, not files |
| Convention/decision memory | Quipu | Quipu episodes (out of Yupana scope) |
| Monitor-guided verification (*multilspy*) | Yupana (served directly) | §5.7 edit verification |
| Blast-radius-as-trust-boundary | Broker/Aegis consumes; Yupana computes | §5.4 + §5.9 |

---

## 4. User Personas

### Persona 1: Autonomous Coding Agent (polecat)

- **Needs:** provably-connected references (not "probably relevant"); blast
  radius before it edits; a boolean "will this edit compile / is this identifier
  real" check on its own proposed buffer.
- **Constraints:** limited context window; must not corrupt other tenants; must
  operate inside a capability sandbox scoped by *its own* tenant's live graph.

### Persona 2: Human Developer (direct + via Bobbin)

- **Needs:** "where is this defined / who references it" with ground truth; "what
  will this change break"; explained coupling ("these two files change together
  *because* of this dataflow path").
- **Constraints:** sits at a working copy with uncommitted edits; expects
  sub-second answers; does not want to stand up a language server per query.

### Persona 3: Bobbin (the fusion layer)

- **Needs:** structural facts with confidence/tier tags to fuse with co-change
  and embeddings; a way to flag retrieved code that will not compile in the
  current overlay.
- **Constraints:** async, per-query; consumes Yupana as a signal source, not a
  dependency it must route others through.

### Persona 4: The Broker / Aegis (policy consumer)

- **Needs:** per-tenant blast radius to scope the provisioned execution
  environment for a polecat — autonomous edits safe *by construction*, not by
  review.
- **Constraints:** must read the *right* tenant's live graph, never a stale
  shared one.

---

## 5. Functional Requirements

Requirements are grouped by capability and tagged `FR-N`. Each capability maps to
a numbered capability from the vision (§"The concrete capability set").

### 5.1 Extraction engine (tree-sitter + LSP tiers)

**FR-1: Fast structural extraction (tree-sitter).**

- Parse source files with tree-sitter, reusing the exact grammar set Bobbin
  already ships: Rust, TypeScript/TSX, Python, Go, Java, C/C++.
- Extract a symbol tree (functions, methods, classes, structs, enums,
  interfaces, modules, fields, constants, type aliases) and intra-file call
  edges, with byte/line spans.
- Tree-sitter extraction is **always-on breadth**: it must work build-free, on a
  syntactically-broken buffer, incrementally (tree-sitter's incremental reparse).

**FR-2: Precise semantic extraction (LSP).**

- Run a language server per supported language behind one language-agnostic
  client interface (the multilspy idea), yielding defs, refs, types, hover,
  document/workspace symbols.
- LSP facts are **precision where a build exists**; they are computed on save or
  on-demand when a query needs them, never on every keystroke.
- Absence of a resolvable build must degrade to tree-sitter facts, not fail.

**FR-3: Confidence / freshness tier tags (crux — see risk §14.5).**

- Every served fact MUST carry a `tier` ∈ {`treesitter`, `lsp`, `cpg`,
  `engine-state`} and a `freshness` ∈ {`fresh`, `stale`, `recomputing`}. Agents
  must be able to tell a tree-sitter-fast-but-approximate fact from an
  LSP-precise one.
  - `engine-state` (FR-35) is a fact an engine ADAPTER stated, not one Yupana
    derived from source; `src/types.rs::Tier` ships it and
    `docs/neuralamplifier-harness.md` fixes that hyphenated spelling as the
    wire value (`game-state` is the Cargo feature, `engine-state` is the tier).
    It is a **peer** of the code tiers, not a rung above or below them: nothing
    it carries is span-anchored, so a consumer reading one as if it pointed at
    a `file:line` is wrong in exactly the way FR-3 exists to prevent — and the
    confusion must be impossible in both directions.
- **Status (aegis-8yrn):** the `tier` half is served on every response.
  **The `freshness` half splits in two, and both halves are now served — but
  they answer different questions and can disagree, so they must never be
  conflated:**
  - **Code-fact freshness** (is this structural fact current with the file it
    came from?) is *tracked AND served*, since Phase 3 (bobbin-052). The watch
    path maintains real `Recomputing → Fresh` transitions per file
    (`src/watch/overlay_refresh.rs::freshness_of`, tenant-keyed through
    `src/daemon/mod.rs::freshness_of`), and the resident daemon's
    tenant-scoped `/symbols` reply serves the tracked value — read BEFORE the
    view is composed (`src/daemon/tenanted.rs::symbols_for`) so the tag
    describes the state those symbols came from rather than one that moved
    underneath the read. The wire field is
    `daemon::wire::FileSymbols::freshness`, and
    `src/daemon/http_test.rs::symbols_serve_code_fact_freshness_and_omit_it_when_unknown`
    pins the rule.
    **Wherever it is unknown the field is omitted — never `"fresh"`, never
    `"unknown"`.** Three paths cannot answer and all three say so by absence:
    the untenanted `/symbols` query has no tenant to key the map by; a tenant
    that never had an edit absorbed has no note to report; and the on-demand
    (non-daemon) serve path rebuilds per request, so no cached code fact can
    be stale and nothing tracked it. A fabricated tag would be
    indistinguishable from a measured one, which is the failure FR-3 exists to
    prevent. `types::Fact`/`types::Freshness` remain the typed carrier.
  - **Projection freshness** (are the governed rules this verdict enforced still
    the current ones?) is *already served* on the verdict surface: the pre-edit
    guard states `fresh` / `stale` (with the cache age in seconds) /
    `recomputing` on every rule verdict (`src/hook/rule_planes.rs`), and
    promoted verdicts carry `aegis:freshness` (`src/verdict.rs`). This is a
    property of the policy projection from Quipu, not of any code fact — the
    two must never be conflated.

### 5.2 Ground-truth reference & definition resolution *(multilspy → Yupana; cap. 1)*

**FR-4:** Given a symbol or a `(file, line, col)` position, return its definition
site(s) and all reference sites, each with span, tier, and tenant-resolved
truth (base + overlay, see §5.5).

**FR-5:** Resolution must be served to Bobbin so it can turn "probably relevant"
into "provably connected," and to agents directly for navigation.

### 5.3 Call-graph & dataflow extraction *(Joern + codebase-memory → Yupana; cap. 2)*

**FR-6: Call graph.** Build inter-procedural call edges (caller → callee) with
multi-strategy resolution (direct, method, dynamic/virtual best-effort), matching
codebase-memory's approach. Tag each edge with the resolution strategy and tier.

**FR-7: Code Property Graph.** Construct AST + control-flow + data/program-
dependence merged into one queryable graph (the Joern CPG idea). See §14.1 for
the JVM-vs-Rust build decision this forces (resolve in Phase 2).

**FR-8: Dataflow / taint.** Support source→sink reachability over the CPG so a
dataflow path can corroborate (or refute) a co-change edge.

**FR-9: Community detection.** Run deterministic Louvain community detection over
the structural graph (Quipu already exposes this via `quipu_project`; Yupana
computes it live over the in-memory graph for the hot path).

### 5.4 Blast-radius / impact analysis *(Joern reachability + co-change → Yupana; cap. 3)*

**FR-10:** Given a symbol/file/change set, compute the structurally-reachable
impacted set (forward: dependents; backward: dependencies) over the call/dataflow
graph, bounded by max hops and optional predicate filters — the same shape as
Quipu's `quipu_impact` but over Yupana's live per-tenant graph.

**FR-11:** Reconcile the structural reachable set with Bobbin's historical
co-change set; surface edges that appear in one but not the other (structural-
only = new/unexercised coupling; co-change-only = a refactoring smell).

> **Invariant — Yupana borrows co-change, it never derives it.** The co-change set
> is always a *required input* supplied by the caller (Bobbin), never something
> Yupana mines itself. Yupana must not walk git history, run FP-Growth, or store a
> co-change signal — that is Bobbin's owned signal (statistical, over the settled
> past) and the routing rule (§2.4) keeps it there. Reconciliation is a
> *stateless annotation* on Yupana's own structural output, not ownership of a
> second temporal signal. The day Yupana derives co-change is the day it becomes a
> second source of truth (the risk in §9.6). The implementation enforces this:
> `reconcile()` takes the co-change set as a parameter with no fallback path.

**FR-12 (crux):** The blast-radius reachability query MUST be implemented as a
single primitive reused for two purposes: (a) answering *"what does this change
affect?"* for a consumer, and (b) answering *"what must I recompute?"* for the
incremental updater (§5.5). **One primitive, two uses — build it once.**

### 5.5 Per-tenant live graph *(the tenancy model → Yupana; cap. 4)*

**FR-13: Shared base.** Compute the full structural graph once at a baseline
commit (e.g. `main`), held **read-only** in memory.

**FR-14: Copy-on-write overlays.** Each tenant (developer/agent session) gets a
lightweight overlay: only touched files are re-parsed, only affected edges are
recomputed, layered over the shared base. Queries resolve against `base +
overlay`. An overlay MUST be invisible to other tenants (isolation is automatic).

**FR-15: Content-hash structural sharing.** Use content-hash keys (the codebase-
memory trick) so that N developers cost *one base + N small deltas*, not N full
graphs. Identical subtrees across tenants share storage.

**FR-16: Frontier-bounded incremental update.** Updating an overlay is **not** just
the edited file — it is the edited file *plus its frontier*. On edit: re-parse X
(cheap) → find changed symbols → **query the base graph for references/dependents
of those symbols** (this is FR-12's primitive) → recompute facts for that bounded
frontier → store as overlay. Naive per-file incremental update is wrong because
the consequences are non-local.

**FR-17: Tiered freshness.** Tree-sitter structure updates on save or debounced
keystroke; LSP/dataflow facts update on save or on-demand. (This is exactly the
tree-sitter-everywhere + LSP-for-a-subset split codebase-memory ships.)

**FR-18: Overlay lifecycle.** Overlays are created per session, evicted on session
close, and support explicit reset to base. Very-high-fan-in symbols (widely-
referenced signatures) may cascade the frontier; §14.2 requires an eviction and
special-handling policy.

### 5.6 Promotion to Quipu *(codebase-memory → Quipu; caps. 5, 6, 7 — see §9)*

**FR-19:** When changes land on a shared branch (commit/merge), promote the
corresponding structural facts into Quipu as a new bitemporal state
(valid-time = commit time; transaction-time = when learned).

**FR-20:** Promoted facts MUST be emitted as Turtle in the **existing `bobbin:`
code ontology** (`https://bobbin.dev/ontology#`, namespace constructors in
Quipu's `src/namespace.rs`) and validated against `shapes/code-entities.ttl`
(extended per §9.2) before write. Yupana never writes to Quipu without passing
SHACL.

**FR-21:** Promotion writes via Quipu's existing surface — `quipu_knot` (MCP) /
`POST /knot` (REST) / `Store::transact` (in-process) — honoring
`valid_from`/`valid_to`, `transactions.actor` (= the promoting identity), and
`source` (= the commit SHA). Yupana does **not** stand up its own triple store
(§14.4).

**FR-22:** Uncommitted overlay churn MUST NOT be promoted. Yupana holds the
in-flight reality; Quipu holds only the settled record. Enforced structurally:
`yupana promote --commit <ish>` projects `export::to_turtle_at` over the
**committed git tree** at that ref, never the working tree — an in-flight
overlay edit or unsaved buffer cannot reach a promotion by construction (the
yupana #15 committed-tree slice).

### 5.7 Monitor-guided edit verification *(multilspy monitors → Yupana, served directly; cap. 8)*

**FR-23:** Given a proposed edit (an edited buffer), re-run analysis on that
buffer against the base graph Yupana already holds and return a boolean verdict
plus violations: `identifier-does-not-exist`, `wrong-arity`, `type-violation`,
`unresolved-import`.

**FR-24:** Verification is single-signal and boolean — agents call Yupana directly,
**not** through Bobbin. Bobbin may still *consume* verdicts like any other Yupana
fact (e.g. to flag retrieved code that will not compile in the current overlay);
that is the normal Yupana→Bobbin flow, not verification living in Bobbin.

### 5.8 Static-analysis-as-trust-boundary *(Yupana blast radius → Broker/Aegis; cap. 9)*

**FR-25:** Expose per-tenant blast radius in a form the Gas Town broker/Aegis can
consume to scope a polecat's provisioned execution environment. Capability
scoping MUST be computed against the *requesting tenant's* live graph, never a
stale shared one (this is why the live per-tenant state must live in Yupana).

### 5.9 Interfaces — the interface model

Yupana has **two interaction modes**, and they want different shapes. Conflating
them is the mistake to avoid:

- **Query mode** (pull) — an agent asks a discrete question ("what's the blast
  radius of this symbol"). Request/response, agent-initiated, structured. **MCP
  is ideal** and it is what agents and Bobbin already speak.
- **Edit-reactive mode** (push/synchronous) — the agent *changes a file* and
  Yupana responds *at the moment of the edit* (impact, verification). This is
  LSP-shaped (an edit stream in, a verdict out) and MCP-pull fits it poorly.

The resolution is to serve each mode with the surface that fits, over one
resident engine:

| Surface | Consumer | Shape | Requirement |
|---|---|---|---|
| **Harness hook** (`yupana hook …`) | in-harness agents (Claude Code) | synchronous, edit-reactive, automatic | FR-30 |
| **MCP tools** | agents, Bobbin | pull, on-demand queries | FR-26 |
| **HTTP API** *(Phase 3)* | broker / daemon backplane | the resident engine all surfaces share | FR-27 |
| **CLI** | humans, scripts, CI | one-shot | FR-28 |
| **LSP server** (optional) | human editors | unsaved-buffer precision + push diagnostics | FR-32 |

**FR-26: MCP server.** Expose Yupana's capabilities as MCP tools (§12) over both
stdio and streamable-HTTP transports, using `rmcp` exactly as Bobbin does
(`#[tool_router]` / `#[tool]` / `Parameters<T>` / `schemars`).

**FR-27: HTTP API *(Phase 3, with the FR-31 resident daemon).*** Expose the same
capabilities over a local Axum HTTP server for the broker and non-MCP consumers,
mirroring Quipu's REST-parallel-to-MCP pattern. This is the resident engine's
shared backplane, so it lands **with** that engine (FR-31), not before it: a
REST facade over a per-request transient graph build would carry the daemon's
latency without its benefit. Until then every capability is already reachable
over TCP via the **streamable-HTTP MCP transport** (`yupana serve --http`, mounted
at `/mcp`); the gap FR-27 closes is protocol ergonomics for non-MCP consumers, not
reach. Tracked in §12 Phase 3.

**FR-28: CLI.** Provide a `yupana` binary (clap, like Bobbin) with subcommands for
serving, one-shot analysis, and inspection (§Appendix A).

**FR-29: Config.** Read from the shared `.bobbin/config.toml` under a new `[yupana]`
table (§11), with the same resolution order Quipu uses (flags > project toml >
user toml > defaults).

**FR-30: Harness hook adapter — the edit-reactive interface.** Provide
`yupana hook <event>` adapters that read an agent harness's hook payload on stdin
and respond synchronously. The edit tool call *is* the `didChange` event; the
hook makes Yupana's response automatic — the agent never has to remember to call a
tool. For Claude Code (Bobbin already integrates this way for context injection):

- **`yupana hook post-edit`** (`PostToolUse` on `Edit|Write|MultiEdit`) — after the
  edit lands, update the overlay and return the cross-file blast radius as
  injected `additionalContext`. *Advisory by default.* (Implemented; with a
  resident daemon it feeds the tenant overlay via `POST /edit`, FR-30/31, and
  falls back to a transient build otherwise.)
- **`yupana hook pre-edit`** (`PreToolUse`) — before the edit lands, verify the
  *proposed* buffer (§5.7 / FR-23) and, for **capability-scoped agents**
  (polecats), optionally `deny` with a reason so the model revises. This is where
  the §5.8 trust boundary becomes concrete: **blocking guard is opt-in**, never
  the default (a wrong hard-deny is worse than none).

The adapter is a thin, harness-specific translation layer; the core engine and
its facts stay harness-agnostic.

**FR-31: Resident daemon (the latency prerequisite).** The hook fires
synchronously in the agent's loop, so a `pre-edit` guard has a sub-100ms budget
(§6.1). A cold full-graph build per edit blows it. Therefore the hook (and the
streamable-HTTP MCP surface, and the broker) must be **thin clients of a resident
`yupana serve` engine** holding the base + per-tenant overlays — never rebuilding
per invocation (Bobbin's hooks hit its resident server the same way). **This
makes the Phase-3 resident overlay a hard prerequisite of the hook interface, not
a nice-to-have** — the hook use case is the forcing function for the hot overlay.

**FR-32: LSP server surface (optional).** Optionally *expose* an LSP server (Yupana
already *consumes* LSP internally for the precise tier) so human editors get
unsaved-buffer precision and pushed diagnostics natively. Justified only if
human-in-editor is a target consumer; deferred behind the agent/Bobbin/broker
consumers. Note the tenant/edit-sync input differs by consumer: **agents** edit
on disk (picked up by the file-watcher, §5.5) or via the harness hook (FR-30);
**editors** stream unsaved `didChange` (this surface).

### 5.10 Unified referential structure — code and docs

**The synergy:** code and docs are one referential graph, not two. A function
calls a function; a doc section *references* a symbol; a code comment links to an
ADR. These are the same *kind* of fact — a typed edge between named entities —
and Yupana's machinery (reference resolution, blast radius, monitor-guided
verification) applies to all of them. Yupana's differentiated job is to be the one
tool that builds the **complete, precise referential graph spanning code and
docs**. This is explicitly **not chunking**: Bobbin chunks code+docs into
embedding windows for *retrieval*; Yupana emits *precise, typed referential
structure* for *reasoning and governance*. Complementary, not redundant.

**Two clocks — the same graph, two update disciplines:**

| | Real-time (live) | Asynchronous (export) |
|---|---|---|
| Trigger | edit hook / MCP query | commit / merge / on-demand |
| Home | Yupana in-memory overlay (the present) | Quipu governed graph (the record) |
| Code | blast radius + guard, in-loop | committed structure, bitemporal |
| Docs | "your code edit made `docs/x.md#y` stale" | full doc→code reference graph, versioned |

The distinction the doc case forces: **a doc going stale is a warning, not a
blocker.** You never hard-block an agent mid-edit because a README drifted. So
the doc side leans *asynchronous* (export, caught on commit/CI) while the
real-time hook still fires the *code→doc* staleness note in the moment. Same
underlying graph; code leans live, docs lean export.

**It reuses the existing ontology.** `shapes/code-entities.ttl` already defines
`Document` and `Section` (alongside `CodeModule`/`CodeSymbol`). Yupana adds
`Section → references → CodeSymbol` edges into that model — additive, no new
entity design.

**Doc rot becomes a query.** Once the referential graph is in Quipu, SPARQL
answers "every `Document` that references a `CodeSymbol` which no longer exists,"
auditable over time. That is capability 7 (SPARQL-over-code) extended to docs for
free.

**Boundary discipline** (so this stays Yupana's job and not everyone's): Yupana owns
*building the structural referential graph*. It does **not** do chunking or
embeddings (Bobbin), prose/style linting (Vale), doc semantic retrieval (Bobbin),
or governed-intent storage (Quipu owns the record). Yupana only cares about
*structural references between docs and code*.

**FR-33: Doc→code reference extraction.** Parse markdown (tree-sitter / the
`langs-extra` set) and extract references to code symbols — inline code spans,
code fences, and `src/…#L..` links — resolved against the code graph and
tier-tagged. Emits `Section → references → CodeSymbol` edges. Feeds both the
live hook (code→doc staleness) and the export (FR-34).

**FR-34: Export the referential structure.** Provide `yupana export` — the governed
projection of the live graph.

- `yupana export --format turtle` emits the referential structure (modules,
  symbols, `definedIn`/`calls`, and — as FR-33 lands — `Document`/`Section` +
  `references`) as Turtle in the `bobbin:` ontology, validating against
  `shapes/code-entities.ttl`. *(Implemented for the code side.)*
- `yupana export --to quipu` promotes it (SHACL-validate → `quipu_knot`,
  bitemporal). This **is** Phase-4 promotion (§9); the Turtle dump is the
  substrate under it. Decoupling "produce the governed projection" from "serve
  live" keeps the present (overlay) and the record (Quipu) cleanly separated.

---

## 6. Non-Functional Requirements

### 6.1 Performance

| Metric | Target |
|---|---|
| Base graph build (tree-sitter tier), 100K LOC | < 30 s cold |
| Overlay update on single-file save (tree-sitter) | < 150 ms p95 |
| Frontier recompute, typical (non-hot symbol) | < 500 ms p95 |
| Reference/definition lookup (served) | < 50 ms p95 (base+overlay hit) |
| Blast radius, 5 hops, live graph | < 300 ms p95 |
| Edit verification verdict | < 200 ms p95 |
| LSP-precise fact (on-demand, warm server) | < 1 s p95 |

### 6.2 Scalability & memory

| Metric | Target |
|---|---|
| Codebase size | up to 1M LOC base graph |
| Concurrent tenants | ≥ 32 overlays on one base |
| Overlay cost | O(touched files + frontier), not O(repo) |
| Memory | base + Σ overlays within a configurable budget; content-hash sharing (FR-15) is the primary lever |

Overlay memory and hot-symbol churn are the top scaling risk (§14.2): the spec
requires an eviction policy and a high-fan-in special case, and requires Yupana to
`log` when it bounds or truncates coverage rather than silently degrading.

### 6.3 Correctness & staleness semantics

- Every fact carries a tier and freshness tag (FR-3). A served fact must never
  present a tree-sitter approximation as LSP-precise.
- Tenant isolation is absolute: no overlay is ever observable by another tenant.
- Promotion to Quipu is all-or-nothing per commit and must pass SHACL; a
  validation failure blocks the write and surfaces the violations (it does not
  write partial facts).

### 6.4 Reliability & portability

- Graceful handling of unparseable files, missing language servers, and
  build-free repos (degrade tier, never crash).
- Same platform matrix as Bobbin: macOS (ARM64/x86_64), Linux (x86_64/ARM64).
- Single binary for the Rust core; language servers and any JVM extractor are
  external processes managed behind a boundary (§14.1).

### 6.5 Security & privacy

- Local-first, matching Bobbin/Quipu: no code leaves the machine during normal
  operation. Language servers run locally.
- The HTTP surface honors the same read-only / bearer-token guards Quipu uses
  (`http_auth.rs` pattern) for any write-ish endpoint (e.g. promotion trigger).

---

## 7. Technical Architecture

### 7.1 High-level components

```text
┌────────────────────────────────────────────────────────────────────┐
│              MCP (rmcp: stdio + HTTP/axum)  ·  CLI (clap)            │
├────────────────────────────────────────────────────────────────────┤
│                            Query / Serve layer                       │
│   refs · defs · callgraph · dataflow · blast-radius · verify         │
│   (all resolve against base + tenant overlay, tier/freshness tagged) │
├────────────────────────────────────────────────────────────────────┤
│                        Tenancy layer (the hard part)                 │
│  ┌────────────────┐   ┌──────────────────────────────────────────┐  │
│  │  Shared base   │   │  Per-tenant overlays (copy-on-write)      │  │
│  │  graph (RO)    │◄──│  touched files + frontier, content-hashed │  │
│  └────────────────┘   └──────────────────────────────────────────┘  │
│        ▲   blast-radius primitive (FR-12): one query, two callers    │
├────────┼─────────────────────────────────────────────────────────────┤
│        │                 Extraction layer                            │
│  ┌───────────┐  ┌───────────────┐  ┌────────────────────────────┐    │
│  │ tree-sitter│  │  LSP client   │  │  CPG / dataflow (Phase 2)  │    │
│  │  (breadth) │  │ (multilspy-ish)│  │  Rust traversals or Joern  │    │
│  └───────────┘  └───────────────┘  │  behind a process boundary │    │
│                                     └────────────────────────────┘    │
├────────────────────────────────────────────────────────────────────┤
│   File-watch (notify)   ·   Git baseline (gix/git2)   ·   Overlay    │
│   cache (in-mem + optional rusqlite spill)                           │
├────────────────────────────────────────────────────────────────────┤
│         Promotion boundary  →  Quipu (quipu_knot / REST / in-proc)   │
│         emits bobbin: Turtle, SHACL-validated before write           │
└────────────────────────────────────────────────────────────────────┘
```

### 7.2 Proposed source layout (`src/`)

Mirrors Bobbin's module-per-concern style (one file/dir per responsibility, a
thin `main.rs` that inits tracing + parses the CLI):

```text
src/
  main.rs            # tracing init, CLI parse+dispatch (#[tokio::main])
  cli/               # one module per subcommand (serve, analyze, refs, impact, verify, promote, status)
  config.rs          # [yupana] table, load_merged (defaults < user < project < flags)
  errors.rs          # thiserror error type + Result alias
  extract/
    treesitter.rs    # grammar registry, symbol tree, intra-file calls
    lsp/             # language-agnostic LSP client (multilspy idea), per-language servers
    cpg.rs           # CPG construction + dataflow (Phase 2; Joern boundary or Rust traversals)
    resolve.rs       # multi-strategy call resolution, import resolvers
  graph/
    base.rs          # shared read-only base graph (petgraph-backed)
    overlay.rs       # copy-on-write overlay, content-hash sharing
    tenant.rs        # tenant/session registry, base+overlay resolution
    blast.rs         # FR-12 reachability primitive (impact + frontier)
    community.rs     # Louvain over the live graph
  serve/
    refs.rs · impact.rs · verify.rs · callgraph.rs · dataflow.rs
  watch.rs           # notify-based file-watch, debounce, tier scheduling
  promote/
    ontology.rs      # bobbin: IRI minting (reuse Quipu namespace constructors)
    turtle.rs        # emit facts as Turtle (oxrdf/oxttl)
    quipu.rs         # #[cfg(feature="quipu")] promotion via quipu_knot / Store::transact
  mcp/               # rmcp server (server.rs handlers, tools.rs DTOs) — Bobbin pattern
  http/              # axum server: streamable-HTTP MCP transport today;
                     #   parallel REST handlers land in Phase 3 (FR-27)
  types.rs           # Fact, Tier, Freshness, Symbol, Edge, Tenant, Overlay
```

### 7.3 Core data model (`types.rs`)

```rust
enum Tier { TreeSitter, Lsp, Cpg }
enum Freshness { Fresh, Stale, Recomputing }

enum SymbolKind { // matches shapes/code-entities.ttl sh:in enumeration
    Function, Method, Class, Interface, Enum, Struct,
    Variable, Constant, Module, Property, Field, Constructor, TypeAlias,
}

enum EdgeKind {   // §9.2 predicates
    Calls, References, DefinedIn, Imports,
    DataDependsOn, ControlDependsOn,
}

struct Fact { subject: Iri, edge: EdgeKind, object: Iri, tier: Tier, freshness: Freshness }
struct Overlay { tenant: TenantId, base_commit: Oid, touched: HashMap<PathBuf, FileFacts>, frontier: HashSet<SymbolId> }
```

### 7.4 The blast-radius primitive (FR-12), made concrete

```text
fn reachable(seed: &[SymbolId], dir: Direction, hops: u32, view: &TenantView) -> ReachSet
    // dir = Forward  → dependents  → "what does this change affect?"  (consumer)
    // dir = Backward → dependencies → context for recompute
    // Called by serve/impact.rs AND by graph/overlay.rs::update_frontier.
    // Same traversal, same code, two callers.
```

### 7.5 Data flow

**Baseline build:** walk repo (respect `.gitignore` via `ignore`) → tree-sitter
parse each file → symbol tree + intra-file calls → resolve inter-procedural calls
→ (Phase 2) CPG/dataflow → hold read-only base keyed by content hash.

**Overlay update (on save):** notify event → debounce → re-parse touched file →
diff symbols vs base → `reachable(changed, Backward+Forward)` to bound the
frontier → recompute frontier facts (tree-sitter now, LSP on demand) → write
overlay delta.

**Serve:** request carries a `tenant` (and optionally a position) → resolve
`base + overlay` → return tier/freshness-tagged facts.

**Promote (on commit/merge):** diff committed change vs base → emit `bobbin:`
Turtle for the affected facts → SHACL-validate → `quipu_knot` with valid-time =
commit time, source = SHA → advance the base to the new commit.

---

## 8. Technology Choices

Yupana most resembles **Bobbin** on the serving side (async, MCP, tree-sitter,
file-watch) and borrows **Quipu's** graph and RDF crates for the analysis and
promotion sides. Versions below are pinned to what the two peers already use, so
the three build against a coherent dependency set.

| Concern | Choice | Version | Matches |
|---|---|---|---|
| Language / edition | Rust, **edition 2021** | — | Bobbin (Yupana is closest to Bobbin's rmcp serving core; see note) |
| Async runtime | `tokio` (full) | `1` | Bobbin |
| MCP SDK | `rmcp` (server, transport-io, streamable-http, axum) | `0.12` | Bobbin |
| JSON schema | `schemars` | `1.0` | Bobbin |
| CLI | `clap` (derive, env) + `clap_complete` | `4` | Bobbin |
| Tree-sitter | `tree-sitter` + rust/ts/python/go/java/cpp grammars | `0.25` / `0.24`/`0.23`/`0.25`/`0.23`/`0.23`/`0.23` | Bobbin (identical grammar set) |
| Graph algorithms | `petgraph` | `0.7` | Quipu |
| Datalog (optional, for derived edges) | `datafrog` | `2` | Quipu |
| RDF model / Turtle | `oxrdf` / `oxttl` / `oxrdfio` | `0.3` / `0.2` / `0.2` | Quipu |
| SPARQL (if Yupana ever parses queries) | `spargebra` | `0.4` | Quipu |
| SHACL (validate before promotion) | `rudof_lib` (behind `shacl`/`quipu` feature) | `0.2.8` | Quipu |
| Overlay spill / cache (optional) | `rusqlite` (bundled) | `0.33` | Both |
| HTTP server | `axum` + `tower-http` (cors, trace) | `0.8` / `0.6` | Both |
| File-watch | `notify` | `6` | Bobbin |
| Git baseline | shell out to `git` (decided, §15.2) | — | Bobbin (`index/git.rs` shells to git); behind `src/git.rs`, reversible |
| Error handling | `thiserror` (+ `anyhow` in bins only) | `2` / `1` | Both (Quipu is thiserror-only; Bobbin uses both) |
| Serialization | `serde` / `serde_json` / `toml` | `1` / `1` / `0.8` | Both |
| Logging | `tracing` + `tracing-subscriber` | `0.1` / `0.3` | Bobbin |
| Hashing | `sha2` / `hex` | `0.10` / `0.4` | Bobbin (content-hash sharing) |
| Quipu integration | `quipu` git dep, pinned by rev, `default-features = false`, optional | rev-pinned | Bobbin's exact pattern |

**Edition note.** Bobbin is edition 2021; Quipu is edition 2024. Yupana sits on
Bobbin's serving stack (`rmcp`, async, `notify`, `tracing`) and shares Bobbin's
request-path role, so **edition 2021** is the default choice for compatibility
with that surface. This is a reversible decision; revisit if a 2024-only
dependency becomes compelling (§16, open question 1).

**Feature flags** (mirroring both peers' feature discipline):

- `quipu` — gates the entire promotion path (`dep:quipu`, `oxttl`, `rudof_lib`).
  Off by default so Yupana compiles and serves without the promotion toolchain, and
  — critically — **CI builds and tests both with and without it**, the single
  most-emphasized convention in Bobbin (the "don't let a feature ship dark" rule).
- `lsp` — gates the real language-agnostic JSON-RPC client and column-precise
  definition/reference resolution for Rust plus TypeScript/JavaScript. It landed
  atomically with `Tier::served()` and dedicated CI arms; runtime server/build
  absence degrades to explicitly tagged tree-sitter facts.
- `cpg` — **planned, not yet a Cargo feature** (aegis-qe5z). Its former empty
  feature was removed because it advertised a tier without an implementation.

**Lints.** Adopt Quipu's in-manifest `[lints.rust]` / `[lints.clippy]` block
verbatim (`unsafe_code = "deny"`, `unused_must_use = "deny"`, `missing_docs =
"warn"`, plus the ~25 clippy warns) so Yupana matches house style from commit one.

**The `quipu` dependency**, following Bobbin's Cargo.toml comment discipline
exactly: pin by `rev` (not `branch`, because `Cargo.lock` is gitignored and a
branch dep would float to tip on a fresh CI checkout), use `default-features =
false` to keep Quipu's `onnx`/`shacl` off unless Yupana explicitly needs them, and
document the chosen rev and why bumping it is a migration, not a version bump.

---

## 9. The Code Ontology & Quipu Promotion

This is where Yupana meets Quipu, and where the vision needs the most reconciliation
with reality.

### 9.1 What already exists (build on it, don't reinvent)

Quipu already ships a code ontology and SHACL contract:

- **Namespace:** `bobbin: <https://bobbin.dev/ontology#>` (and the SHACL file's
  `bobbin: <http://aegis.gastown.local/ontology/>` target class prefix). IRI
  constructors live in Quipu `src/namespace.rs`: `code_module_iri`,
  `code_symbol_iri`, etc., minting IRIs like `bobbin:code/{repo}/{path}::{symbol}`.
- **Classes (in `shapes/code-entities.ttl`):** `CodeModule` (requires `filePath`,
  `repo`, `language`), `CodeSymbol` (requires `name`, `definedIn` → CodeModule;
  `symbolKind` enumerated), `Document`, `Section`, `Bundle`.
- **Bobbin↔Quipu type mapping** (`bobbin-quipu-mapping.toml`): `CodeSymbol` →
  `aegis:SoftwareComponent`, `CodeModule` → `aegis:CodeRepository`, etc., surfaced
  predicates `aegis:dependsOn`, `aegis:ownedBy`, `aegis:runsOn`.

Yupana promotes into **this** model. It mints the **same** IRIs so Bobbin's and
Yupana's facts about the same symbol reconcile on a shared identifier.

### 9.2 What Yupana adds (ontology extension)

The existing shapes cover *entities* (modules, symbols) but not the *structural
edges* Yupana exists to produce. Yupana contributes new predicates and their SHACL
shapes (to be added to `code-entities.ttl`, or a sibling `code-edges.ttl`):

| Predicate | Domain → Range | Meaning | Source tier |
|---|---|---|---|
| `bobbin:calls` | CodeSymbol → CodeSymbol | caller invokes callee | tree-sitter / cpg |
| `bobbin:references` | CodeSymbol → CodeSymbol | use site of a definition | lsp |
| `bobbin:imports` | CodeModule → CodeModule | module dependency | tree-sitter |
| `bobbin:dataDependsOn` | CodeSymbol → CodeSymbol | data-dependence edge | cpg |
| `bobbin:controlDependsOn` | CodeSymbol → CodeSymbol | control-dependence edge | cpg |
| `bobbin:hasTier` | Fact → literal | provenance/confidence tag | (all) |

Following the vision's guidance — *"start permissive, tighten deliberately"* (a
good code ontology over-constrained will reject legitimate facts from messy real
code) — these shapes begin with minimal cardinality/datatype constraints and add
`sh:class` domain/range checks only once real promoted data validates cleanly.

**Sample shape (new edge, in the existing SHACL style):**

```turtle
@prefix sh:     <http://www.w3.org/ns/shacl#> .
@prefix bobbin: <http://aegis.gastown.local/ontology/> .

bobbin:CallsShape a sh:NodeShape ;
    sh:targetSubjectsOf bobbin:calls ;
    sh:property [
        sh:path bobbin:calls ;
        sh:class bobbin:CodeSymbol ;   # range: callee is a CodeSymbol
        sh:minCount 1 ;
    ] .
```

### 9.3 Bitemporal promotion

Promotion uses Quipu's bitemporal model directly (Quipu `concepts/temporal-model`):

- **valid-time** (`--timestamp` / `valid_from`) = the commit's author/commit time.
- **transaction-time** (`transactions.timestamp`, monotonic tx id) = when Yupana
  learned/promoted the fact.
- A signature change that removes an edge is a **retraction** (close `valid_to`),
  not a delete — Quipu's log is append-only, so code archaeology ("what called
  this function as of last March?") is answerable via `--valid-at`.

This gives capability 6 (bitemporal code archaeology) and capability 7
(SPARQL-over-code) for free, because they are Quipu features once the facts are
in the graph. **Sample SPARQL over promoted code:**

```sparql
# Who called authenticate() as of 2026-03-01?  (valid-time travel)
SELECT ?caller WHERE {
  ?caller <http://aegis.gastown.local/ontology/calls>
          <http://aegis.gastown.local/ontology/code/yupana/src%2Fauth.rs::authenticate> .
}
# executed with valid_at = 2026-03-01
```

### 9.4 Branches as named graphs (make Quipu a quad store)

The vision proposes modeling each branch's committed facts as an **RDF named
graph**, bitemporally versioned within. **Quipu today is a triple store, not a
quad store** — there is no `GRAPH` / quad handling in its SPARQL engine or EAVT
schema. The recommended resolution is to **add quad support to Quipu** and make
named graphs the branch axis, rather than reifying a branch qualifier onto every
promoted edge.

This is the right call because a quad store is a **strict superset** of a triple
store, so the change is *additive* and can be made non-breaking:

- Add a graph term `g` to Quipu's `facts` identity. Existing facts migrate into
  the **default graph** (`g = NULL`/sentinel); nothing is deleted or rewritten.
- SPARQL without a `GRAPH` clause keeps hitting the default graph; `spargebra`
  already parses `GRAPH` / `FROM` / `FROM NAMED`, so the evaluator in
  `src/sparql/` gains graph-scoped BGP matching without a new query language.
- Bobbin (pinned to an old Quipu rev, `default-features = false`) is insulated
  during the transition.

**Why it's worth a Quipu-core change, not just a Yupana convenience:** named graphs
pay off well beyond branches. Quipu already has a `docs/design/group-
isolation.md`, per-source provenance (`transactions.source`, episode
`prov:wasGeneratedBy`), and a `FederatedProvider` — all of which want the same
primitive: a first-class way to partition the graph. Branches are simply the
first customer. One quad column serves branch scoping, group isolation, and
provenance/federation at once, which is *less* total complexity than solving each
separately (a branch-qualifier hack in Yupana *plus* group isolation *plus* source
scoping).

**Where the design care goes:** the interaction of three axes — `graph ×
valid-time × transaction-time`. Each fact already carries two time dimensions;
adding a graph dimension means the index permutations (`idx_eavt/aevt/vaet`),
retraction semantics (does closing `valid_to` scope to a graph?), the `datafrog`
reasoner (which graphs does a rule range over?), and SHACL targeting (which graph
do shapes validate?) each grow a graph-awareness question. None are individually
hard; together they are the surface to design deliberately. **Decide
default-graph-is-union vs. default-graph-is-distinct early** — it is the dataset
semantics choice that is painful to reverse later.

**Sequencing (does not block Yupana).** Yupana Phases 1–3 (extraction, dataflow,
tenancy) never touch Quipu. Only Phase 4 (promotion) cares. So the quad work is a
**Phase 4 enabler tracked on the Quipu side** (see §9.5 for the RFC sketch), not a
Yupana dependency. If quads land first, Yupana promotes each branch's committed facts
directly into a named graph named for the branch (bitemporally versioned within).
If they are not ready when Phase 4 starts, Yupana falls back to **branch-as-
qualifier** (a reified `bobbin:onBranch` term on each edge, queries adding a
`?fact bobbin:onBranch "main"` constraint) — heavier queries, no Quipu change —
and migrates to named graphs when they arrive. The config `branch_model` key
(§11) selects between them.

**Status (GH #4): the qualifier fallback is IMPLEMENTED; named-graph refuses.**
`src/promote_branch.rs` attaches `bobbin:onBranch "<branch>"` to every entity a
promotion declares, so `?m bobbin:onBranch "main"` is answerable today with zero
Quipu change. `branch_model = "named_graph"` **refuses the promotion**, naming
quipu#36, rather than degrading to the qualifier — an operator who asked for the
partitioned model must not be left believing their branches are partitioned when
nothing partitions them. The **default moved to `"qualifier"`** in the same
change: while neither model existed, defaulting to the preferred one was right;
now that exactly one is implementable, the other would refuse every promotion out
of the box.

Two limits, stated because the fallback is easy to over-read:

- **It answers membership, not per-branch structure.** Promoted IRIs are
  deterministic and branch-independent by design (`code/{repo}/{path}` — that is
  what makes a re-promotion supersede instead of fork), so two branches promoting
  the same module write the *same* subject and accumulate both branch values on
  it. That answers "which branches is this module on". It cannot answer "what did
  the call graph look like on `feature` versus `main`", because both branches'
  `calls` edges land on one set of subjects with nothing to tell them apart.
  Distinguishing them needs per-branch IRIs (which forks the graph) or named
  graphs (quipu#36). This is exactly why §9.4 calls named-graph the preferred
  design and this the fallback.
- **An undeterminable branch is OMITTED, never invented.** A promotion of a bare
  SHA that is not a branch tip emits no qualifier and says so on stderr — the
  same absent-beats-wrong rule FR-3 freshness follows.

**Migration, qualifier → named_graph, once quads land.** The qualifier triples
are additive and carry no structure of their own, so the migration is: register
the branch graphs, re-promote each branch's HEAD with `branch_model =
"named_graph"` (deterministic IRIs mean this supersedes rather than duplicates),
then retract the `bobbin:onBranch` triples as a single predicate sweep. Nothing
else in the projection changes shape.

### 9.5 Quipu quad-store RFC (sketch, Quipu-side follow-up)

> **Tracked as [scbrown/quipu#36](https://github.com/scbrown/quipu/issues/36)** —
> *"store: add named-graph (quad) support — additive, default-graph-preserving."*

A short design note to raise in `scbrown/quipu` (natural home:
`docs/design/group-isolation.md` or a new `docs/design/named-graphs.md`):

- **Schema:** add `g INTEGER` (interned graph IRI, nullable = default graph) to
  `facts`; extend the primary key and the EAVT/AEVT/VAET index permutations to be
  graph-aware (or add a `GEAVT`-style permutation). Keep it nullable so the
  migration is a column-add, not a rewrite.
- **SPARQL dataset semantics:** define the active dataset (default graph = union
  of all graphs, or a distinct empty default) and wire `GRAPH ?g { … }`,
  `FROM`, and `FROM NAMED` through the evaluator. Pick union-vs-distinct once.
- **Bitemporality:** `valid_from`/`valid_to`/`tx` stay per-fact; retraction and
  time-travel scope *within* a graph. Confirm `Store::speculate` savepoints and
  contradiction detection are graph-local.
- **SHACL / reasoner:** decide the graph a shape targets by default (all graphs,
  or the default graph) and the graphs a `datafrog` rule ranges over.
- **MCP/REST:** `quipu_knot` / `POST /knot` gain an optional `graph` parameter;
  `quipu_query` honors `GRAPH`. Backward compatible when omitted.
- **Migration:** existing `data/quipu.db` facts move to the default graph in
  place; no downstream break for Bobbin's pinned rev.

### 9.6 Two graph engines — keep the split honest

Yupana's in-memory graph serves interactive dataflow/reachability queries that are
genuinely painful over RDF/SPARQL. Quipu serves governed/temporal/cross-domain
queries. The rule (from the vision's risks): **Yupana's transient store must never
become a second source of truth for committed facts.** Committed truth lives in
Quipu; Yupana holds only what is in flight plus a read-only projection of the base.

### 9.7 Downstream: promotion feeds work-item co-occurrence in Quipu

Yupana's promotion emits more than entity facts — at commit time it can write the
**provenance edge `commit → touched entities`** (valid-time = commit time,
`source` = SHA, `actor` = committer). That provenance is the substrate for a
Quipu-side capability distinct from Bobbin's statistical co-change:
**governed, provenance-based work-item co-occurrence** (ticket/epic ↔ code).

Keep the three notions of "coupling" distinct — they are different mechanisms
answering different questions, and conflating them recreates the two-engines
problem:

| Signal | Owner | Mechanism | Question |
|---|---|---|---|
| Structural coupling | **Yupana** | call/dataflow reachability | "what is wired to this" |
| Statistical co-change | **Bobbin** | FP-Growth over git history | "what *tends to* change together" |
| Work-item co-occurrence | **Quipu** | deterministic SPARQL over provenance edges | "what work *did* touch this, and what else did it touch" |

The loop closes cleanly: **Yupana promotes the `commit → entity` provenance →
Quipu aggregates it (with `bead → commit`) into ticket/epic co-occurrence →
Bobbin fuses all three signals.** This generalizes FR-11's structural-vs-co-change
reconciliation into multi-signal corroboration: coupling backed by structure
*and* co-change *and* a shared work item is strong; coupling in only one is weak.
The same borrow-don't-derive invariant applies to Quipu (no statistical mining
there — that stays Bobbin's). Tracked Quipu-side as
[scbrown/quipu#37](https://github.com/scbrown/quipu/issues/37); Yupana's obligation
is only to promote the provenance edge in Phase 4.

**Status (GH #5): the edge is produced inside Yupana at promotion time.**
`src/promote_provenance.rs` emits, per promoted commit, a `bobbin:GitCommit`
node (`hash` / `repo` / `author` / `date` as a typed `xsd:dateTime` / `rdfs:label`)
and one `bobbin:modifies` statement per touched `CodeModule`, gated by
`GitCommitShape` + `ModifiesShape` in `shapes/code-edges.ttl`, synced from
Quipu's registry. Three deliberate limits:

- **`aegis:implements` is NOT emitted.** The commit → work-item link needs a
  declared project-prefix vocabulary Yupana does not hold, and the tracker-aware
  ingest lane already owns it with an abstention rule tuned against measured
  false matches. Re-deriving that heuristic here is how two producers drift; the
  chain still closes because both predicates join on the **commit IRI**.
- **Module granularity.** "Touched" means the commit changed the file, which is
  exactly true. Symbol-level touch would need a per-symbol diff and would
  over-claim if guessed from a file-level one.
- **Valid-time rides as a fact, not as a transaction field.** Verified against
  Quipu `main`: `tool_knot` takes `turtle` / `timestamp` / `actor` / `source` /
  `shapes` / `replace_snapshot` / `snapshot` / `graph` and has **no `valid_from`
  parameter**, so the valid-time axis is not settable over `/knot`. The commit's
  authored time therefore rides as `bobbin:date`. Putting it in `timestamp`
  would falsify transaction time ("when learned"), which is the axis that IS
  correct today.

**Divergence with the pre-existing out-of-tree ingest lane, and the fix.**
Measured, not assumed. camayoc's `scripts/ingest_git_provenance.py` mints under
`BASE = http://aegis.gastown.local/code/`; Yupana mints under
`http://aegis.gastown.local/ontology/code/…`, the base its own entities live at.
Those are different IRIs for the same referents, so this is **not** a
double-write — the two lanes produce disjoint populations that never collide and
never join. Quipu's own `src/namespace.rs` records the measurement (2026-08-23):
subjects under `CODE_BASE` number **0**, subjects under the ontology base
**10,425**, and it warns that building against `CODE_BASE` forks the code graph.
The resolution is a one-line `BASE` repoint on the camayoc side, which that note
already asks for; afterwards both lanes mint identical commit and module IRIs and
`/knot` supersedes per `(s, p, o)`, so they converge rather than duplicate.
Yupana's `rdfs:label` spelling matches the ingest's (`<repo>@<sha[:12]>`) so that
convergence does not leave two labels on one node.

### 9.8 Bounded transitive paths over the promoted graph (Quipu-side follow-up)

> **Sketched Quipu-side in `quipu/docs/design/statement-identity.md`.** Nothing
> is required of Yupana; this records what the promoted graph cannot answer yet, so
> §9.3's capability claims stay honest.

§9.3 claims bitemporal code archaeology comes "for free once the facts are in the
graph." That holds for the one-hop question it demonstrates — *who called
`authenticate()` as of 2026-03-01* — but not for its transitive form, which is
the one a blast radius actually asks. Quipu today offers two half-answers:
SPARQL property paths give `calls+` with **no depth cap**, and `quipu impact`
gives a hop-bounded BFS that is a fixed function rather than something a query
composes. `quipu/src/impact.rs` says so in its own header: *"property paths
cannot express a depth cap, so we walk the store directly."*

What the promoted graph needs is therefore a depth bound on path expressions plus
the traversed path returned, not just the endpoint pair.

**This does not relitigate §9.6.** Yupana's interactive dataflow and reachability
stay in Yupana's in-memory graph; that split is deliberate and unchanged. The gap
here is confined to the *governed, committed* projection — the queries Quipu owns
because they are temporal and cross-domain, which is exactly where Yupana's
transient store must not answer. Concretely: transitive `calls` archaeology at a
`--valid-at`, and cross-repo reachability spanning promotions that no single
tenant view holds.

Nothing blocks Yupana's Phase 4 promotion, and the ontology needs no change — the
edges are already the right ones. The recommendation is only that a transitive
archaeology query be written against real promoted data before §9.3's wording is
treated as satisfied.

## 10. MCP & HTTP Tool Surface

Tool naming mirrors the peers: Bobbin uses bare snake_case function names that
clients namespace as `bobbin_*`; Quipu uses explicit `quipu_*`. Yupana uses
**`yupana_*`** for clarity alongside both on the same agent.

| Tool | Purpose | Routes to |
|---|---|---|
| `yupana_definition` | Definition site(s) of a symbol/position | §5.2 |
| `yupana_references` | All reference sites of a symbol | §5.2 |
| `yupana_callers` / `yupana_callees` | Call-graph neighbors | §5.3 |
| `yupana_dataflow` | Source→sink dataflow paths | §5.3 |
| `yupana_impact` | Blast radius (forward/backward, N hops) | §5.4 |
| `yupana_symbols` | Symbol tree for a file/module | §5.1 |
| `yupana_verify` | Verdict on a proposed edit buffer | §5.7 |
| `yupana_status` | Base commit, tenant overlays, tiers, freshness | §5.5 |
| `yupana_promote` | Trigger promotion of a commit to Quipu (write-guarded) | §5.6 |

Every tool response carries `tier` per FR-3 (the `freshness` half is Phase 3 —
see FR-3), and every request that reads structure accepts a `tenant` parameter
(defaulting to a single-tenant session in Phase 1). Registration follows Bobbin's `rmcp` pattern exactly:
`#[tool_router]` impl, `#[tool(description = …)]` async fns taking
`Parameters<Req>` where `Req: Deserialize + schemars::JsonSchema`, responses
serialized with `serde_json::to_string_pretty` into `CallToolResult::success`.
The HTTP API will expose a parallel endpoint per tool (Quipu's REST-mirrors-MCP
pattern) for the broker — **Phase 3, alongside the FR-31 resident daemon** (FR-27).
Today the broker reaches these same tools over the streamable-HTTP MCP transport.

> **Refinement — name-based today, position-based for the LSP tier.** The
> current tools resolve by symbol *name* (the tree-sitter tier). The precise
> LSP tier (FR-2/FR-4) wants **position-based** variants — `(file, line, col)` —
> so `yupana_definition` can disambiguate overloads and shadowing the way a
> language server does. MCP carries positions fine; the tools were simply
> designed name-first. Add position variants when the `lsp` tier lands.

---

## 11. Configuration

Yupana shares Bobbin/Quipu's `.bobbin/config.toml` under a new `[yupana]` table, with
the same resolution order (compiled defaults < `~/.config/bobbin/config.toml` <
`.bobbin/config.toml` < CLI flags). No new environment variables beyond what
Bobbin defines (e.g. `BOBBIN_ROLE` for tenant identity, reused).

```toml
[yupana]
# Baseline the shared read-only graph is built at.
base_ref = "main"

# Which extraction tiers to run.
enable_lsp = true          # (Phase 2/3 — not yet read) LSP precision where a build resolves
enable_cpg = false         # (Phase 2 — not yet read) CPG/dataflow

# Languages (default = Bobbin's grammar set). RESTRICTS `yupana analyze`.
languages = ["rust", "typescript", "python", "go", "java", "cpp"]

[yupana.freshness]
# Debounce keystroke-driven tree-sitter updates (ms); LSP/CPG on save/on-demand.
debounce_ms = 300
lsp_on = "save"            # (LSP tier — not yet read) "save" | "on_demand"

[yupana.tenancy]                          # (Phase 3 — none of these keys are read yet)
max_overlays = 32
# Symbols with fan-in above this get special frontier handling (§14.2).
high_fanin_threshold = 200
overlay_eviction = "on_session_close"   # "on_session_close" | "lru"

[yupana.serve]
bind_address = "127.0.0.1"
mcp_http_port = 3040       # distinct from Bobbin's server and Quipu's 3030
read_only = false          # write guard: when true, yupana REFUSES mutating operations (promotion)

[yupana.quipu]               # (Phase 4) promotion target (feature = "quipu")
enabled = false
promote_on = "merge"       # §14.3: WHEN an automated promotion runs. The caller declares the event
                           # (`yupana promote --trigger commit|merge`); this decides whether it promotes.
                           # A `commit` event on a merge commit counts as a merge. `manual` (the default
                           # trigger) always promotes — this governs automation, not authorization.
branch_model = "qualifier" # §9.4: "qualifier" (implemented — bobbin:onBranch per entity, zero Quipu change)
                           # | "named_graph" (preferred, needs Quipu quads — quipu#36; REFUSES until then)
shapes_path = "shapes/"    # NOT READ — shapes are compiled in (include_str!), so a path cannot gate a write
```

---

## 12. Milestones / Phasing

Phasing follows the vision's five phases. Each is a checklist with an exit
criterion; every phase must keep the `quipu` feature compiling both on and off
(Bobbin's dark-feature rule) and must land docs + tests per §13.

### Phase 1 — Yupana, single-tenant *(explained retrieval, no new store)*

- [x] Project scaffold: Cargo (edition 2021), `[lints]` block, `just` + pre-commit + CI (now nine feature arms), mdBook skeleton.
- [x] Tree-sitter extraction (Bobbin's grammar set): symbol tree + intra-file calls (`src/extract/`).
- [ ] LSP client (multilspy-style) for ≥ Rust + one more language: the shared
      client and definition/reference position slice are implemented; types,
      hover, and document/workspace symbols remain on yupana #1.
- [x] Tier tagging (FR-3) on every served response from day one — pinned by `tests/tier_tags.rs`. (Freshness
      tagging was Phase 3; code-fact freshness now serves on the daemon's
      tenant-scoped `/symbols`, see FR-3's status block above.)
- [x] Single-tenant in-memory graph; `yupana_references` / `yupana_symbols` / `yupana_callers` over MCP (stdio + HTTP). Definition-site lookup shipped *as* `yupana_references` ("find the definition site(s) of a symbol by name") rather than a separate `yupana_definition`, so there are three tools here, not four.
- [x] CLI: `serve`, `analyze`, `refs`, `status` (`src/cli.rs`).
- **Exit (met, at the tree-sitter tier):** Bobbin fuses Yupana's references with its co-change/embeddings; "probably relevant" becomes "provably connected." *Precision* awaits the LSP rung above.

### Phase 2 — Dataflow & blast radius

- [x] Call graph (FR-6): tree-sitter call-site extraction, by-name resolution, in-memory `CodeGraph`.
- [x] Blast-radius primitive (FR-10, FR-12) with forward/backward reachability (`reachable()`, one primitive).
- [x] `yupana_impact`, `yupana_callers`, `yupana_callees` (MCP) and `yupana callers` / `yupana impact` (CLI).
- [x] Resolve the JVM/Rust CPG decision (§14.1): **Rust-native traversals** (Joern not adopted).
- [x] Intra-procedural data dependence (FR-8, first slice): `src/dataflow.rs`, `yupana dataflow` (CLI) and `yupana_dataflow` (MCP).
- [x] Reconcile structural reachable set with Bobbin co-change (FR-11): `src/reconcile.rs`, `yupana impact --cochange` (CLI) and the `cochange` param on `yupana_impact` (MCP), partitioning into corroborated / structural-only / co-change-only.
- [x] Edit-reactive harness hook (FR-30, prototype): `yupana hook post-edit` emits a synchronous cross-file blast-radius advisory as Claude Code `PostToolUse` context (builds transiently until the Phase-3 resident daemon lands).
- [x] Referential-structure export (FR-34, code side): `yupana export --format turtle` emits `CodeModule`/`CodeSymbol` + `definedIn`/`calls`/`imports` as Turtle in the `bobbin:` ontology (the substrate under Phase-4 promotion; doc→code references and `--to quipu` fold in later).
- [ ] *Deferred to the `cpg` feature (post-exit):* deeper CPG — control dependence + inter-procedural taint (FR-7, remainder of FR-8).
- **Exit (met):** structural blast radius, reconciled with history, served to agents and Bobbin. Co-change mining stays in Bobbin; Yupana reconciles a supplied co-change set (the routing rule).

### Phase 3 — Multi-tenancy *(the hard phase)*

- [x] Shared base + copy-on-write overlays (FR-13, FR-14): `graph::{Base, Overlay, TenantRegistry, TenantView}`, wired into the resident daemon (yupana #2).
- [x] Content-hash structural sharing (FR-15): per-file content hashes on the base + the registry's parse-intern cache (identical bytes across tenants share one `ParsedFile`).
- [x] Frontier-bounded incremental update reusing the Phase-2 blast primitive (FR-16): `graph::update_frontier` walks the composed view via the one `reachable()` BFS; the base's `callers_of_name` index closes the overlay-new-name case (yupana #3).
- [x] File-watch (`notify`) + debounce + tiered scheduling (FR-17): `src/watch/` — `.gitignore`-filtered `notify` watcher, debounced tiers; `OverlayRefresh` touches the tenant overlay (fast) then runs `update_frontier` (heavy), with per-file freshness (yupana #5).
- [x] Overlay lifecycle + high-fan-in handling + eviction (FR-18, §14.2): `TenantRegistry` open/close/reset, `max_overlays` cap with `overlay_eviction` (lru / on_session_close backstop) — logged, never silent — and a `high_fanin_threshold` guard that clips a hot signature's cascade to one hop (yupana #6).
- [x] `tenant` parameter across the MCP/HTTP surface; `yupana_status` shows overlays (yupana #2 daemon wiring).
- [x] Parallel REST HTTP API beside the MCP mount (FR-27): the resident daemon (`yupana daemon`) serves `/status`, `/callers`, `/impact`, `/references`, `/symbols`, `/dataflow`, `/measure`, `/edit`, each mirroring the `yupana_*` tool payloads, for the broker and non-MCP consumers (yupana #1).
- **Exit (met):** N developers edit concurrently; each sees a correct, isolated `base + overlay`; overlays cost O(touched + frontier), capped and evicted under a logged policy.

### Phase 4 — Promote to Quipu

- [x] Extend the code ontology with edge shapes (§9.2) and `Section → references → CodeSymbol` (§5.10); start permissive: `shapes/code-edges.ttl` covers `calls`/`references`/`imports`/`dataDependsOn`/`controlDependsOn`/`hasTier` + `Section→references`, permissive (nodeKind IRI, `sh:class` deferred), with node shapes synced from Quipu's `code-entities.ttl` (yupana #13).
- [x] Turtle emission of the referential structure (`yupana export --format turtle`, FR-34, code side) — extend to docs (FR-33) and wire `--to quipu`.
- [x] Doc→code reference extraction (FR-33) folded into the **export**: `src/docref.rs`
      scans markdown for code-symbol mentions and `src/export.rs` emits
      `Section → references → CodeSymbol`. Not yet wired into the live edit hook.
- [x] SHACL-validate (`rudof`) before every write (FR-20): `promote::validate` runs `rudof_lib` in-process against `code-edges.ttl` and refuses the whole promotion on any violation (all-or-nothing); a real `export` projection is round-trip-validated in the test suite so the emitter cannot drift from the gate (yupana #14).
- [x] Promote via `quipu_knot` / `POST /knot`, bitemporal, committed-tree-only (FR-19, FR-21, FR-22): `src/promote.rs::promote` SHACL-validates the projection in-process against the compiled-in `code-edges.ttl`, refuses the whole promotion on any violation, then POSTs the Turtle to `/knot` (chunked under the body limit; deterministic IRIs, so a re-run supersedes rather than duplicating). `yupana promote <commit-ish> --to <url>` reads a commit, never the working tree, which is what keeps uncommitted churn out of the graph. Graded ✅ in Appendix D.
- [x] Promote **on commit/merge** — the trigger (FR-19, §14.3): `yupana promote
      --trigger commit|merge` is the event a git hook or CI step declares, and
      `[yupana.quipu] promote_on` decides whether that event promotes
      (`src/promote_trigger.rs`, read by `cli_promote::trigger_admits`). Yupana
      installs no git hooks and owns no commit event of its own, so the event
      comes from the caller that has one; git supplies the merge/non-merge
      distinction a `post-commit` hook cannot. `manual` (the default trigger)
      always promotes — the key governs automation, not authorization. The key
      is no longer exempted in `src/config_test.rs`'s live-control guard. GH #3.
- [x] Branch modeling per §9.4, qualifier half: `src/promote_branch.rs` attaches
      `bobbin:onBranch "<branch>"` to every promoted entity, needing no Quipu
      change, and `branch_model = "named_graph"` REFUSES naming quipu#36 rather
      than degrading to it silently. Default moved to `"qualifier"`.
- [ ] Branch modeling per §9.4, named-graph half: promote each branch into
      `GRAPH bobbin:branch/<b>` once Quipu quad support (§9.5, quipu#36) has
      landed, and migrate the qualifier triples out. Blocked on quipu#36.
- **Exit:** committed structure lives in Quipu, SHACL-validated, bitemporally queryable; uncommitted churn never pollutes it.

### Phase 5 — Consumption & guardrails

- [x] Per-tenant blast radius wired into the broker/Aegis capability-scoping path (FR-25): `[yupana.policy.scopes.<tenant>]` gives each tenant writable-path globs and blast-radius ceilings, evaluated against that tenant's graph.
- [x] `yupana_verify` monitor-guided edit verification as a direct surface (FR-23, FR-24): `yupana verify` + the `yupana_verify` MCP tool. Tree-sitter tier decides `identifier-does-not-exist`, `wrong-arity`, and `unresolved-import`; `type-violation` is reported as unchecked until the LSP tier lands.
- [x] `yupana hook pre-edit` guard (FR-30): blocking `deny` opt-in for capability-scoped polecats, off by default, always fail-open. Contract pinned in `docs/book/src/reference/policy-guard.md`. Proposed-buffer *verification* (FR-23) is now an opt-in arm of the guard (`[yupana.policy] verify = true`), inside the same deadline and fail-open contract.
- [ ] Bobbin consumes verdicts to flag won't-compile retrieved code.
- **Exit:** structure defines the polecat sandbox, per tenant; agents get a boolean guard on their own edits.

---

## 13. Testing & Dev Tooling

Adopt both peers' conventions so Yupana is a first-class citizen of the stack from
commit one:

- **`just` is the only entrypoint** (never raw `cargo`); justfile quiet by
  default with `verbose=true` to override; group related ops under subcommands
  (`just docs build`).
- **`just check` is the pre-push gate** — pre-commit hooks (trailing-whitespace,
  EOF, yaml/json, merge-conflict, large-files, markdownlint-cli2) + `cargo fmt
  --check` + clippy `-D warnings`. **Do not push if it fails.**
- **CI matrix builds/tests/clippies both feature arms** (`--features quipu` and
  `--no-default-features`) — Bobbin's hardest-won lesson; dropping either arm
  re-creates the dark-feature bug.
- **Tests:** inline `#[cfg(test)]` unit tests colocated with modules (Quipu
  style) + `tests/` integration tests via `assert_cmd`/`predicates`/`tempfile`
  driving the `yupana` binary (Bobbin style). New functionality ships with tests;
  tests are part of `just check`. Integration tests must **skip gracefully** when
  a language server or optional toolchain is unavailable (Bobbin's
  `try_indexed_project` pattern).
- **Docs:** mdBook under `docs/book/` with the peers' IA (getting-started /
  concepts / architecture / reference / tutorials); user-facing changes MUST
  update docs and README; `just docs build` must be clean; Vale + markdownlint +
  prettier for prose.
- **Release:** conventional commits + `release-plz` + `git-cliff` (Quipu's
  automated versioning/changelog).
- **"Landing the plane":** work is not complete until `git push` succeeds.

---

## 14. Risks & Mitigations

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| 14.1 | **JVM/Rust fork for CPG.** Joern is JVM/Scala; the stack is Rust. | High | **Decided (Phase 2): Rust-native traversals.** Rather than embed Joern (a heavy JVM dep + serialization seam), Yupana reimplements the traversals it needs, keeping the stack coherent. Started with intra-procedural data dependence (`src/dataflow.rs`, tree-sitter tier); a deeper CPG with inter-procedural taint can grow behind the `cpg` feature. Joern is not adopted. |
| 14.2 | **Overlay memory & churn.** Per-tenant overlays + a large base must stay in budget; frontier recompute on hot (high-fan-in) symbols can cascade. | High | Content-hash sharing (FR-15) as the primary lever; `high_fanin_threshold` special-casing; explicit overlay eviction policy (`on_session_close`/`lru`); `log` any bounded/truncated coverage — never degrade silently. |
| 14.3 | **When to promote to Quipu.** Every commit? Only merges to tracked branches? Promotion cost vs. history completeness. | Medium | `promote_on = commit\|merge\|manual` config; default `merge`. Bitemporality lets promotion be lazy but not free. |
| 14.4 | **Two graph engines drift.** Yupana's transient store could become a second source of truth for committed facts. | Medium | Hard rule (§9.6): committed truth lives in Quipu; Yupana holds in-flight + a read-only base projection only. Promotion is the one-way boundary. |
| 14.5 | **Freshness/staleness semantics.** Agents must know if a fact is tree-sitter-approximate or LSP-precise. | Medium | Mandatory `tier` + `freshness` tag on every fact (FR-3), surfaced in every MCP/HTTP response. |
| 14.6 | **Build-free vs build-required.** Joern's fuzzy parser needs no build; LSP needs a resolvable build for precise types. | Medium | Serve both: tree-sitter always-on breadth, LSP precision where a build exists; degrade tier, never fail; the ontology carries facts of differing confidence. |
| 14.7 | **Ontology design cost.** Over-constrained SHACL rejects legitimate facts from messy real code. | Medium | Start permissive (§9.2), tighten deliberately once real promoted data validates cleanly. |
| 14.8 | **Named-graph gap → Quipu quad-store work.** Quipu is a triple store; branches want named graphs. The fix is a Quipu-core change (add a graph column, graph-aware SPARQL), whose real cost is the `graph × valid-time × tx-time` interaction. | Medium | §9.4/§9.5: add quads *additively* (default-graph-preserving, non-breaking); sequence as a Phase-4 enabler tracked on the Quipu side, **not** on Yupana's critical path; `branch_model = "qualifier"` is the zero-Quipu-change fallback if quads aren't ready. Decide default-graph union-vs-distinct early. |
| 14.9 | **Query-surface sprawl.** Resist standing up CPGQL *and* SPARQL *and* many MCP tools as permanent interfaces. | Low | Consolidate on SPARQL-over-Quipu for committed queries + Yupana's `yupana_*` MCP surface for live analysis. No second query language. |
| 14.10 | **`quipu` dep instability.** Quipu is pre-1.0; API drifts (Bobbin is pinned to a rev a full minor behind tip). | Medium | Pin `quipu` by `rev`, `default-features = false`, document the rev and why bumping it is a migration; CI compiles the `quipu` feature so drift can't ship dark. |

---

## 15. Open Questions

1. **Edition.** Default is 2021 (Bobbin's serving stack). Adopt 2024 (Quipu) only
   if a 2024-only dependency becomes compelling.
2. **Git access.** ~~`gix` vs `git2` vs shelling out~~ — **Decided: shell out to
   the system `git`** (matches Bobbin's `index/git.rs`; zero dependency; single
   binary preserved), behind the `src/git.rs` boundary so a later swap to
   `gix`/`git2` is localized and reversible. Resolves the baseline commit
   (`resolve_commit`) and the commit-diff (`changed_paths`); degrades gracefully
   outside a repo (§6.4). Content *at* a historical ref is read via
   `list_files_at` + `read_blob_at` — the base graph (`build_at_ref`) and
   promotion (`export::to_turtle_at`) both read committed-tree content, never
   the working tree.
3. **CPG realization.** Joern-as-subprocess vs Rust-native traversals (§14.1) —
   the single biggest architectural fork; resolve early in Phase 2.
4. **Branch model.** Named graphs (via Quipu quad support, §9.4/§9.5) are the
   preferred path; branch-as-qualifier is the fallback. The open item is
   *sequencing*: does the Quipu quad work land before Yupana Phase 4, and what are
   the default-graph dataset semantics (union vs. distinct)? Freeze before the
   promotion schema is.
5. **Promotion trigger.** On every commit vs only merges to tracked branches
   (§14.3) — trades promotion cost against history completeness.
6. **Tenant identity.** Reuse `BOBBIN_ROLE`/Gas Town crew identity as the tenant
   key, or mint a Yupana-native session id? Affects broker capability scoping.
7. **Overlay persistence.** Pure in-memory vs `rusqlite` spill for large overlays
   / crash recovery — do we need durability for in-flight state at all?
8. **LSP server management.** Bundle/vendor language servers, or discover
   system-installed ones? Affects portability and the build-free story.

---

## 16. Glossary

| Term | Definition |
|---|---|
| **Base graph** | The full structural graph at a baseline commit, held read-only in memory and shared across tenants. |
| **Overlay** | A per-tenant copy-on-write delta over the base: touched files + recomputed frontier facts. |
| **Frontier** | The bounded set of symbols whose facts must be recomputed after an edit — the edited symbols plus their references/dependents. |
| **Blast radius** | The reachable set answering "what does this change affect?" — and, reused, "what must I recompute?" (FR-12). |
| **Tier** | Provenance/precision of a fact: `treesitter` (fast, approximate), `lsp` (precise, build-required), `cpg` (dataflow). |
| **Freshness** | Whether a served fact is `fresh`, `stale`, or `recomputing`. |
| **CPG** | Code Property Graph — AST + control-flow + data/program-dependence merged into one queryable graph (Joern's idea). |
| **Promotion** | Writing committed structural facts from Yupana into Quipu as a new bitemporal state, SHACL-validated. |
| **Tenant** | A developer/agent session sitting at the base commit plus its own uncommitted working delta. |
| **Bitemporal** | Two time axes: valid-time (when true in the world = commit time) and transaction-time (when Quipu learned it). |
| **Named graph** | An RDF quad's graph component; the preferred branch axis. Not supported by Quipu today — §9.4/§9.5 propose adding quad support additively. |
| **LSP** | Language Server Protocol — the source of precise defs/refs/types. |
| **Monitor-guided verification** | Re-running analysis on an edited buffer to return a boolean "is this edit valid" verdict (multilspy monitors). |

---

## Appendix A: CLI Reference (Draft)

```text
USAGE:
    yupana <COMMAND>

COMMANDS:
    serve       Run the MCP server (stdio, or streamable-HTTP with --http)
    analyze     One-shot: build the base graph and print stats
    refs        Definitions and references for a symbol/position
    callers     Callers / callees of a symbol
    impact      Blast radius (forward/backward, N hops) for a change
    dataflow    Intra-procedural data dependence within a function
    export      Emit the referential structure as Turtle (→ Quipu)
    hook        Agent-harness hook adapter (edit-reactive)
    verify      Verdict on a proposed edit buffer
    promote     Promote a commit's structural facts into Quipu
    status      Base commit, tenant overlays, tiers, freshness
    completions Generate shell completions
    help        Print help

GLOBAL FLAGS:
    --json      Machine-readable output
    --quiet     Suppress non-essential output
    --verbose   Detailed progress
    --tenant    Tenant/session id (default: single-tenant)
    --config    Path to config file

EXAMPLES:
    yupana serve
    yupana analyze
    yupana refs authenticate src
    yupana impact authenticate src --hops 5
    yupana verify --file src/auth.rs --buffer /tmp/edited.rs
    yupana promote --commit HEAD
```

## Appendix B: Sample promoted Turtle (facts Yupana emits into Quipu)

```turtle
@prefix bobbin: <http://aegis.gastown.local/ontology/> .
@prefix xsd:    <http://www.w3.org/2001/XMLSchema#> .

bobbin:code/yupana/src%2Fauth.rs::authenticate
    a bobbin:CodeSymbol ;
    bobbin:name "authenticate" ;
    bobbin:symbolKind "function" ;
    bobbin:definedIn bobbin:code/yupana/src%2Fauth.rs ;
    bobbin:calls bobbin:code/yupana/src%2Fdb.rs::lookup_user ;
    bobbin:dataDependsOn bobbin:code/yupana/src%2Ftoken.rs::verify .

bobbin:code/yupana/src%2Fauth.rs
    a bobbin:CodeModule ;
    bobbin:filePath "src/auth.rs" ;
    bobbin:repo "yupana" ;
    bobbin:language "rust" .

# §9.7 provenance, emitted by the same promotion:
bobbin:code/yupana/commit/2f1c9a4b1d0e…
    a bobbin:GitCommit ;
    rdfs:label "yupana@2f1c9a4b1d0e" ;
    bobbin:hash "2f1c9a4b1d0e…" ;
    bobbin:repo "yupana" ;
    bobbin:author "Dev <dev@example.com>" ;
    bobbin:date "2026-08-25T09:14:02+00:00"^^xsd:dateTime .

bobbin:code/yupana/commit/2f1c9a4b1d0e…
    bobbin:modifies bobbin:code/yupana/src%2Fauth.rs .

# §9.4 branch qualifier, under the (default, implemented) `qualifier` model:
bobbin:code/yupana/src%2Fauth.rs bobbin:onBranch "main" .
```

(Promoted with `source` = commit SHA and `actor` = the promoting process, via
`POST /knot`, after SHACL validation against `code-entities.ttl` +
`code-edges.ttl`. The commit's authored time rides as `bobbin:date` rather than a
`valid_from` transaction field, because `/knot` has no such parameter — see §9.7.
Under `branch_model = "named_graph"` (§9.4) these facts would instead be written
into the branch's named graph — `GRAPH bobbin:branch/main { … }` — once Quipu
quad support (§9.5, quipu#36) has landed; until then that setting REFUSES rather
than degrading to the qualifier.)

## Appendix C: Sample `yupana_impact` response (MCP)

```json
{
  "tenant": "strider",
  "seed": "src/auth.rs::authenticate",
  "direction": "forward",
  "hops": 5,
  "reachable": [
    { "symbol": "src/api/login.rs::handler", "distance": 1, "via": "calls", "tier": "lsp", "freshness": "fresh" },
    { "symbol": "src/api/session.rs::refresh", "distance": 2, "via": "dataDependsOn", "tier": "cpg", "freshness": "fresh" }
  ],
  "cochange_only": [ "docs/auth.md" ],
  "structural_only": [ "src/api/session.rs::refresh" ]
}
```

---

## Appendix D: Implementation Status

A snapshot of what is actually built, reconciled against the source tree
2026-08-25. The body of this spec (§§1–11) is the *design*; this appendix is
the *state* — so its numbers are **recomputed** from `find`/`wc` and
`cargo test -- --list`, never carried forward from the previous revision.

**Two of those numbers are now pinned by tests**, because this appendix had
rotted by roughly 4× on the file count and 29× on the test count before anyone
noticed: `tests/docs_drift.rs` pins the MCP tool count by name, and
`tests/appendix_d_drift.rs` pins the source-file, source-line and test counts
below within a tolerance band. A band rather than an equality, deliberately —
an exact pin would make every commit that adds one test edit this appendix,
which trains people to bump the number without re-deriving anything else. The
band catches the failure that actually happened (order-of-magnitude drift) and
resets to reality every time it fires.

**Phases:** Phase 1 (single-tenant structure + MCP) is complete **except the
LSP tier** (§12, GH #1) — everything it promised serves at the tree-sitter
tier. Phase 2 (call graph, blast radius, intra-procedural dataflow, co-change
reconciliation) is complete. **Phase 3 (multi-tenancy) is complete** — §12
marks its exit met: shared base + copy-on-write overlays, content-hash
structural sharing, frontier-bounded incremental update, the file-watcher, and
overlay lifecycle/eviction all ship, with `tests/overlay_isolation_tests.rs`
holding the isolation property. Phase 4 (promotion) is complete but for the
commit/merge *trigger* and branch modeling; Phase 5 is partial. Two capability
drops landed outside the phase numbering entirely — the game-state harness
(FR-35..FR-39) and the golden-path guard (FR-40..FR-42) — each behind its own
Cargo feature and its own addendum.

**Source layout (`src/`, 162 `.rs` files, ~42,950 lines):** the 400-line soft
cap is a warn-not-fail target and 24 non-test files currently exceed it, led by
`promote.rs` (625), `export.rs` (605) and `hook/rule_planes.rs` (567). Those
three are the only entries in `scripts/file-size-baseline.txt`: the ratchet
freezes them **at exactly their current size** — they may shrink but never grow
— while any file not listed must stay under the hard limit outright. Tests are
exempt from the check (`*_test.rs`, `*tests.rs`, `tests/`).

| Module | Role | Status |
|---|---|---|
| `extract/` | tree-sitter symbol + call-site extraction | done (Rust always; 6 more behind `langs-extra`) |
| `graph/` | `CodeGraph` + `reachable()` (FR-12), `symbol_at` position lookup, tenant base/overlay/view | done |
| `daemon/` | the resident engine + REST API (FR-27): `/status` `/callers` `/callees` `/impact` `/references` `/symbols` `/dataflow` `/measure` `/edit` `/health`, plus `/ingest` `/guard` `/whatif` and `/path/check` behind their features | done |
| `watch/` | `notify` watcher + debounce + tiered scheduling (FR-17), per-file code-fact freshness | done |
| `dataflow.rs` | intra-procedural data dependence | done |
| `reconcile.rs` | structural-vs-co-change partition (FR-11) | done |
| `export.rs` / `docref.rs` | referential structure → Turtle (FR-34) incl. `Section → references → CodeSymbol` (FR-33) | done |
| `promote.rs` (+ `_chunk`, `_payload`) | SHACL-validate in-process, then `POST /knot`, chunked (FR-19/20/21/22) | done (`quipu` feature) |
| `hook/` | harness adapters: `post-edit`, `pre-edit` guard, `pre-bash` action record, `session-start` briefing; the scope ladder (observed + derived rungs), rule planes, tripwire arm | done |
| `policy.rs` / `policy_items.rs` / `rules.rs` / `textrules.rs` | capability scopes, blast-radius ceilings, rule catalogue (§5.8/FR-25) | done |
| `project*.rs` | the Quipu projections the guard reads: rules, scopes, exposure, grounding, tripwires, queries | done (`quipu` feature) |
| `projection_cache.rs` | durable projection cache with a TTL past which the guard fails open loudly | done |
| `trace.rs` / `attribution.rs` / `constraint.rs` / `action.rs` / `plate.rs` | the Σ-derived trace record: `ConstraintEvaluation`, the SARC §9.6 attribution tuple, action resolution | done |
| `verdict.rs` / `verdict_spool.rs` / `audit.rs` | signed verdicts, local spool, deferred promotion (`yupana verdicts`) | done (`quipu` feature) |
| `brief*.rs` / `grounding*.rs` / `turn_grounding.rs` / `recurrence.rs` / `exemplar.rs` | the work-item briefing (CONTEXT consumer) and turn-grounding evidence | done (`quipu` feature) |
| `tripwire.rs` / `project_tripwire.rs` | governed path-boundary tripwires projected from quipu | done (`quipu` feature) |
| `state/` | game-state ingestion, `graph-pattern` policy plane, order guard, what-if, per-(game, faction) tenancy (FR-35..FR-39) | done (`game-state` feature) |
| `goldenpath/` | blessed-trajectory projections + plan/progress conformance under `gp-grammar/1` (FR-40..FR-42) | done (`golden-path` feature) |
| `verify/` | proposed-buffer verdicts (FR-23/FR-24) | tree-sitter tier done; `type-violation` reported unchecked pending LSP |
| `mcp/` | `rmcp` server (`server`/`tools`/`handlers`/`transport`) | done (`mcp` feature) |
| `config.rs` | `[yupana]` config table + the phased-key anti-drift guard | done |
| `cli*.rs` / `render.rs` | CLI surface | done |
| `types.rs` / `errors.rs` | fact model (Tier/Freshness/…) + errors | done |

**CLI commands:** `analyze`, `refs` (`--at FILE:LINE`), `callers`, `impact`
(`--cochange`), `communities`, `census`, `changed`, `dataflow`, `export`,
`verify`, `exemplar`, `status`, `watch`, `completions`, `hook <post-edit |
pre-edit | pre-bash | session-start>`, `serve` (`mcp` feature), `daemon`
(`mcp` feature), `promote` / `verifier` / `verdicts` (`quipu` feature).

**MCP tools (15, `mcp` feature):** `yupana_status`, `yupana_symbols`,
`yupana_references`, `yupana_analyze`, `yupana_callers`, `yupana_callees`,
`yupana_impact` (with `cochange`), `yupana_communities`, `yupana_dataflow`,
`yupana_verify`, `yupana_promote` (writes to Quipu; needs the `quipu` feature),
`yupana_ingest`, `yupana_guard`, `yupana_whatif` (the game-state harness; need
the `game-state` feature), `yupana_path_check` (the golden-path conformance
guard, FR-41/FR-42; needs the `golden-path` feature). Over stdio +
streamable-HTTP. **This count is pinned by name** in `tests/docs_drift.rs`.

**Cargo features:** `default = []`; `mcp`, `langs-extra`, `quipu`,
`game-state`, `golden-path` — all off by default. **Every one of them except
`langs-extra` is in the CI matrix**, and as both a solo and an `mcp`-combined
arm: nine arms on clippy and nine on test (`default`, `mcp`, `langs-extra`,
`quipu`, `mcp+quipu`, `game-state`, `mcp+game-state`, `golden-path`,
`mcp+golden-path`). That is the dark-feature rule from §14.10 made mechanical:
a feature joins the matrix in the same change that wires it.

`langs-extra` gates REAL extractors — TypeScript, TSX, Python, Go, Java and C++
all produce modules, symbols and call edges (measured 2026-08-04 against a
7-language probe repo, 3–4 symbols each). A build WITHOUT it is Rust-only and
says so in `yupana status` (`languages`); it does not error on the other five,
it silently extracts nothing from them, which is why the deployed binary was
Rust-only for an undated period without anyone noticing. This paragraph
previously read "deps are declared but extractors are Rust-only so far" — that
was true early in Phase 1, went stale, and is the likeliest reason a release was
hand-built without the flag.

`cpg` remains planned and is **not a feature yet**. `lsp` returned as a feature
only with its real JSON-RPC client, `Tier::served()` arm, and CI matrix. This
preserves the lesson from the former empty `cpg = []` / `lsp = []` flags: a
feature that can be enabled without an implementation advertises a lie.

**Tests: 788** (`cargo test --all-features -- --list`), of which 2 are
`#[ignore]`d and both declare why in the attribute: `shape_agreement`'s Layer 2
verdict-agreement test needs a live `QUIPU_URL`, and `promote_test`'s chunk
soak needs `YUPANA_CHUNK_SOAK_PAYLOAD` and runs in minutes. `cargo test` on
default features runs 474. The
Rust-free replay-converter suite (`tests/spool_to_dogwood.py`, 8 tests) runs
under the `replay-converter-tests` pre-commit hook, so `just check` and CI's
pre-commit job both cover it. Quality gate green: `cargo fmt`, `clippy -D
warnings` (all nine arms), markdownlint, mdBook, file-size ratchet.

**Not yet built:**

- **Remaining LSP precision surfaces** (FR-2; `lsp` feature) — GH #1. Precise
  definition/reference positions are implemented; types, hover,
  document/workspace symbols, and `verify`'s `type-violation` remain.
- **CPG control-dependence + inter-procedural taint** (FR-7, remainder of
  FR-8; planned `cpg` feature) — GH #6.
- **FR-32, the optional LSP *server* surface** — exposing Yupana *as* a
  language server to human editors. Distinct from FR-2 above: that one
  *consumes* language servers for precision, this one *is* one.
- **Branch modeling** (§9.4) — GH #4.

**Phase 4 (graph-export → Quipu) — status (verified by mechanism against
`src/`; issue cross-refs renumbered to the post-rename range, which is 1–9):**

| Section | Status | Evidence |
|---|---|---|
| FR-34 `yupana export` (Turtle) | ✅ Implemented | `src/export.rs::to_turtle` (`:32`) and `to_turtle_at` (`:57`), tested |
| FR-33 doc→code references | ✅ Implemented | `src/docref.rs` scans markdown for code-symbol mentions; `src/export.rs` emits `Section → references → CodeSymbol`. Ticked at §12 Phase 4. Not wired into the live edit hook. |
| FR-20 SHACL-validate before every write | ✅ Implemented | `src/promote.rs::validate` runs `rudof_lib` **in-process** against `CODE_EDGE_SHAPES` (`include_str!`, `:43`) and refuses the whole promotion on any violation. In-process on purpose: validating against the server you are about to write to proves only that the server agrees with itself. `tests/shape_agreement.rs` Layer 1 proves the shapes can both accept and refuse, on every `quipu` CI arm. |
| FR-19/21/22 `yupana promote` (the write path) | ✅ Implemented (`quipu` feature) | `src/promote.rs::promote` (`:463`) — validate → `POST /knot`, all-or-nothing per chunk, deterministic IRIs so a re-run supersedes rather than duplicating. Reads the **committed** tree only (FR-22), asserted by `src/export_test.rs::to_turtle_at_promotes_the_committed_tree_not_working_churn` (`:248`). |
| FR-21 full bitemporal fields | 🟡 Partial | `actor` + the resolved commit SHA as `source` on every transaction (`promote.rs:276-283`). `valid_from` is not set from the commit's author time; `/knot` times the transaction as *when learned*. `src/cli_promote.rs` says so inline. |
| FR-19 promote **on commit/merge** | ✅ Implemented | `src/promote_trigger.rs::decide` is the `promote_on` × trigger decision table; `cli_promote::trigger_admits` reads the key and short-circuits before any tree is read. The caller declares the event (`yupana promote --trigger commit\|merge`) because yupana installs no git hooks; `git::is_merge_commit` upgrades a `commit` event on a two-parent commit to a merge, so the default policy works from the simplest hook. A decline exits **0** — a `post-commit` hook that failed every ordinary commit would be switched off — and prints `SKIPPED … Wrote nothing`. `promote_on` is delisted from `config_test.rs`'s phased allowlist. GH #3. |
| §9.7 commit→touched-entities provenance edge | ✅ Implemented (in yupana) | `src/promote_provenance.rs::commit_turtle` emits the `bobbin:GitCommit` node and one `bobbin:modifies` statement per touched `CodeModule`, appended to the projection at promotion time — §9.7's *placement* requirement, previously met only by an out-of-tree hourly job. `git::commit_touched_paths` uses `log -1 --name-only -m --first-parent` so an ordinary commit, a **merge** (a bare `diff-tree` on a merge prints nothing) and the **root** commit all answer correctly. Edges are filtered against the projection, so one can never point at an entity the same payload does not declare. `aegis:implements` deliberately NOT emitted (the work-item vocabulary belongs to the tracker-aware lane); module granularity; valid-time carried as `bobbin:date` because `/knot` has no `valid_from` parameter. GH #5. |
| §9.4 branch modeling (named-graph vs qualifier) | 🟡 Qualifier implemented; named-graph refuses | `src/promote_branch.rs` attaches `bobbin:onBranch "<branch>"` to every entity a promotion declares (`git::branch_for` resolves it, or ABSTAINS — no qualifier rather than a guess), gated by `shapes/code-edges.ttl::OnBranchShape`. Default `branch_model` moved to `"qualifier"`, the implemented model. `"named_graph"` REFUSES the promotion naming quipu#36, rather than silently writing under the fallback's semantics. The qualifier answers branch MEMBERSHIP, not per-branch structure — promoted IRIs are branch-independent by design, so both branches' edges land on one subject; that gap is what quipu#36 closes. GH #4. |

Pre-existing Phase 1/2 spec-gaps also remain open (out of the graph-export
scope): the remaining FR-2 LSP surfaces (GH #1) and FR-7/FR-8 CPG (GH #6).
FR-4 column-granularity definition/reference positions are implemented behind
the `lsp` feature, with name-based tree-sitter fallback when no build resolves.

## Appendix E: Design Decision Log

The load-bearing decisions and *why*, so they are not re-litigated blind.

1. **Stack pinned to Bobbin + Quipu.** Edition 2021, `rmcp` 0.12, tree-sitter
   0.25, `petgraph`, `oxrdf`/`oxttl`/`rudof` (behind `quipu`), Quipu's clippy
   lint block verbatim. Rationale: the three tools must build against a coherent
   dependency set (§8).
2. **`Cargo.lock` committed** (unlike Bobbin). Rationale: the spec's own
   float-to-tip lesson (§14.10) — a gitignored lock let a feature ship dark.
3. **CPG = Rust-native, not Joern** (§14.1). Reimplement the traversals we need;
   no JVM dependency or serialization seam. Started with intra-procedural data
   dependence.
4. **MCP is the query interface; the harness hook is the edit-reactive
   interface** (§5.9). Yupana has two modes — pull (MCP) and push/synchronous
   (LSP-shaped) — and forcing edit-streaming over MCP is the mistake. The
   filesystem watcher / harness hook is the edit source, not a protocol the agent
   speaks. Exposing LSP is an optional, deferred surface for human editors.
5. **The hook makes Phase 3 a hard prerequisite** (FR-31). A synchronous guard
   has a sub-100ms budget; a cold build per edit blows it → the resident overlay
   is required, not optional.
6. **Co-change stays Bobbin's; Yupana borrows it, never derives it** (§5.4
   invariant). `reconcile()` takes co-change as a required input with no fallback.
   Prevents a second source of truth.
7. **Reconciliation lives in Yupana** (not Bobbin) because the broker reads Yupana
   directly for a reconciled blast radius (§5.8); it is a stateless annotation,
   not fusion.
8. **Export is decoupled from serving** (§5.10). `yupana export` produces the
   governed projection (Turtle); `--to quipu` = Phase-4 promotion. The present
   (overlay) and the record (Quipu) stay cleanly separated.
9. **Code and docs are one referential graph** (§5.10) — *not* chunking (that is
   Bobbin's). Code leans real-time; docs lean asynchronous (export).
10. **Branches → RDF named graphs via an additive Quipu quad store** (§9.4/§9.5),
    not a branch-qualifier hack. Tracked as [quipu#36](https://github.com/scbrown/quipu/issues/36).
11. **Docs publish via a `gh-pages` branch** (`peaceiris`), because the Actions
    integration token lacks `pages: write`. One owner-only toggle remains (see
    Appendix F).
12. **Git access = shell out to `git`** (§15.2), not `gix`/`git2`. Matches
    Bobbin's `index/git.rs`, adds no dependency, keeps the single binary, and
    lives behind `src/git.rs` so a swap is localized. Resolves the baseline
    commit + commit-diff; degrades gracefully outside a repo.

**Tracked Quipu-side follow-ups:** [quipu#36](https://github.com/scbrown/quipu/issues/36)
(quad store / named graphs) · [quipu#37](https://github.com/scbrown/quipu/issues/37)
(provenance-based work-item co-occurrence — fed by Yupana's promotion, §9.7).

## Appendix F: Handoff & Next Steps

**Repo state.** `main` is current; the working branch
`claude/yupana-project-spec-qyw6qg` mirrors it. Every push runs CI (green) and
redeploys the mdBook to `gh-pages`.

**One owner-only action outstanding.** GitHub Pages is not yet enabled. Toggle:
**Settings → Pages → Deploy from a branch → `gh-pages` / `(root)`** → the book
goes live at `https://scbrown.github.io/yupana/`. No token available to the agent
can do this (the Pages REST endpoint is blocked and the integration lacks
`pages: write`).

**Next build: Phase 3 — multi-tenancy (the lynchpin).** It is the hard phase and
now has a concrete forcing function (the hook's latency budget, decision E-5).
Recommended order:

1. **Resident engine + local API.** Turn `yupana serve` into a daemon holding the
   base graph, reachable over a local socket/HTTP (FR-27/FR-31). Make the hook
   and the streamable-HTTP MCP surface *thin clients* of it (today they build
   transiently). This alone is the biggest latency win and unblocks the guard.
2. **Shared base + copy-on-write overlays** (FR-13/14). One read-only base graph;
   a per-tenant overlay of touched files + recomputed frontier. `tenant` already
   threads through the CLI/MCP surface; bind it to a working-tree path / session.
3. **Frontier-bounded incremental update** (FR-16) reusing `graph::reachable()` —
   the FR-12 primitive is already built; this is its second caller.
4. **Content-hash structural sharing** (FR-15, `sha2` already a dep) and overlay
   eviction / high-fan-in handling (§14.2).
5. **File-watcher** (`notify`, already a dep) as the on-disk edit source for
   agents (§5.5).

**Then:** `pre-edit` guard (FR-30, needs #1); `yupana export --to quipu` + doc→code
references (FR-33/34, Phase 4); and the `lsp`/`cpg` precision tiers. (The
`langs-extra` extractors are DONE — see Appendix E's Cargo-features note.)

**Beyond code (Phase 4+):** a general in-memory fact graph + policy harness for
the NeuralAmplifier project — generic non-code ingestion, a game-state policy
model, `yupana_guard`/`yupana_whatif`, and per-game tenancy (FR-35..FR-39). Scoped in
[neuralamplifier-harness.md](neuralamplifier-harness.md).

**Known imprecision to keep in mind.** Call/reference resolution is *by name*
(tree-sitter tier), so it over-connects on common names (`build`, `new`,
`write`). This is expected — the `lsp`/`cpg` tiers are what refine it. Every
served fact is already tier-tagged (FR-3) so consumers know.

**How to work here.** `just check` + `just test` before every push; clippy is
`-D warnings` with Quipu's lint block; keep files < 400 lines; new Cargo features
join the CI matrix in the same change (the "don't ship dark" rule). See
`AGENTS.md`.

---

*Yupana: live per-tenant code structure — the missing structural signal for the
Bobbin × Quipu stack.*
