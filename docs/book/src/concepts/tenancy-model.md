# The Tenancy Model

A team means there is no single "the AST." Each developer sits at some
branch/commit plus an uncommitted working delta, and those deltas diverge.
Yupana resolves this with **shared base + per-tenant overlay (copy-on-write)**.

- **Shared base.** The full structural graph is computed once at a baseline
  commit and held read-only in memory.
- **Per-tenant overlay.** Each session gets a lightweight overlay: only touched
  files are re-parsed, only affected edges recomputed, layered over the base.
  Queries resolve against `base + overlay`. An overlay is invisible to other
  tenants — isolation is automatic.
- **Content-hash sharing.** With structural sharing, N developers cost *one
  base plus N small deltas*, not N full graphs.

## Blast radius is the incremental-update primitive

The overlay is not just the edited file — it is the edited file **plus its
frontier**. When a signature changes, every reference to it (possibly in files
the developer never opened) now has different facts. So the updater queries the
base graph for the references and dependents of the changed symbols — the *same*
reachability query that answers "what does this change affect?" for a consumer.

**One primitive, two uses — build it once.**

See the [Specification](../design/specification.md) §5.5 for the frontier
algorithm, eviction policy, and high-fan-in handling.

## Implementation state

The engine above exists as a library (yupana #2): `graph::Base` (shared,
read-only, `Arc`-shared, built at a resolved commit with per-file content
hashes), `graph::Overlay` (owned re-parses of touched files only —
`O(touched)`, never `O(repo)`), and `graph::TenantRegistry` /
`graph::TenantView` (the per-query `base + overlay` composition, walked by the
same FR-12 BFS as every other graph). Isolation is structural: a view composes
exactly one tenant's overlay, the base is immutable, and interned parses are
shared by content hash (FR-15) without sharing any view state. The
`tests/overlay_isolation_tests.rs` suite pins §6.3 absolute isolation,
masking/revert/deletion, and the cost shape.

The [resident daemon](../reference/daemon.md) wires this live: it holds the
registry (base at the startup `HEAD`), `POST /edit` is the FR-30 feed (the
post-edit hook calls it per save), query endpoints take `tenant=`, and
`/status` reports the base commit and active overlays.

The FR-16 frontier is `graph::update_frontier` (yupana #3): editing a symbol
perturbs its transitive callers AND callees, so the recompute is bounded to
that reachable frontier — the *second* caller of the one `reachable()` BFS
(FR-12), never a second traversal. The base also keeps a call-site index
(`callers_of_name`) keyed by callee name, which is what lets an overlay-NEW
symbol (one with zero base definitions) find its base callers — the case a
naive per-file update misses.

On-disk edits drive this automatically (FR-17, yupana #5): a `notify` watcher
(`watch::OverlayRefresh`), `.gitignore`-filtered and debounced, touches the
tenant's overlay on the fast tier and runs `update_frontier` on the deferred
heavy tier, tracking per-file freshness (`recomputing` while the frontier is
pending, `fresh` after).

Overlay lifecycle (FR-18/§14.2, yupana #6) is the registry's job: sessions open
on first touch and close explicitly (`close_session`), `reset` clears a tenant
to base, and live overlays are capped at `[yupana.tenancy].max_overlays` — a new
overlay past the cap evicts one per `overlay_eviction` (`lru`, or oldest-created
as the `on_session_close` backstop), always logged. A symbol whose direct
fan-in exceeds `high_fanin_threshold` has its frontier cascade clipped to one
hop, so a widely-referenced signature edit cannot blow the recompute budget —
also logged, never a silent truncation. **This completes Phase 3.**
