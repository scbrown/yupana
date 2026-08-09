# Addendum: Game-State & Policy Harness (NeuralAmplifier-driven)

> **Status: BUILT, behind the `game-state` Cargo feature** (yupana #78–#82). This addendum
> extends [yupana-spec.md](yupana-spec.md) with the net-new capabilities the
> [NeuralAmplifier](https://github.com/scbrown/NeuralAmplifier) project needs from Yupana. It
> continues the FR numbering (last core FR is FR-34).
>
> Each section below still describes the DESIGN; what shipped is in
> [The Game-State Harness](book/src/concepts/game-state.md), and the code is `src/state/`.
> Where the implementation made a choice the design left open, the section says so inline
> under **As built**. This heading previously read "design intent … nothing here is built",
> which is the one claim in a spec that must never go stale in the optimistic direction: a
> reader who believes a capability is absent does not look for it, and one who believes it is
> present when it is not builds on a hole.

## What shipping changed about the honesty caveats

The three caveats in [Honesty / dependencies](#honesty--dependencies) below are **not**
retired by the implementation — two of them are now enforced by it:

- **Not a SPARQL store.** `selectorLang "sparql"` is REFUSED at compile time with a message
  naming Quipu, and appears in the guard report's `unevaluated` list. It is never skipped:
  a policy silently not evaluated reports a clean board it never looked at.
- **The guard sees an approximated post-order board.** Bounded rather than removed: an order
  carries DECLARED effects and yupana applies exactly those, so the divergence is between the
  adapter and the engine — visible to the adapter's author — rather than between yupana's own
  reimplementation of the rules and the real ones.
- **Phase-4 gating.** Unchanged. Verdict signing is still unkeyed, so engine-observed facts
  remain trusted-advisory, not cryptographically trusted.

One caveat the design did not anticipate, found while building: **a board lives in the
process it was ingested into.** Ingesting over `POST /ingest` and guarding through
`yupana_guard` in another process guards an empty board. Both `guard` and `whatif` therefore
REFUSE an empty board (409 over HTTP, an error over MCP) rather than returning zero
findings — zero findings over a board that was never loaded is a green light over a dead
backend.

## Why Yupana

NeuralAmplifier is an LLM brain for *Alpha Centauri*. Its knowledge lives in Quipu (a governed
bitemporal graph — the SMAC datalinks and learned strategy). But three needs are a poor fit for
a persisted graph and a natural fit for Yupana's **hot, per-tenant, copy-on-write in-memory
graph**:

1. A live board graph rebuilt every turn from the game's fog-limited world view.
2. A fast **policy guardrail** over proposed moves — the strategic analog of `yupana_verify` /
   the pre-edit hook (FR-23/24/30), evaluated against game state rather than code.
3. **What-if** analysis over a proposed move — `yupana_impact` (FR-11) generalized from the call
   graph to the board.

This is a real widening of Yupana's mandate: from a *code* graph analyzer to a **general
in-memory fact graph + policy harness**. It reuses Yupana's existing machinery (COW overlays,
multi-tenancy, impact BFS, the `rules::Rule` policy shape, the Quipu policy-projection path) but
requires a non-code ingestion capability Yupana does not have today.

## New requirements

### FR-35 — Generic (non-code) fact ingestion

Yupana's in-memory graph is built only from tree-sitter-parsed source; nodes are span-anchored
`CodeSymbol`/`CodeModule`. Add a **generic `Node`/`Edge` type not tied to source spans**, plus an
ingestion seam:

- `yupana_ingest` (MCP) + `POST /ingest` — `{ entities[], edges[], tenant, provenance }`, mirroring
  the `quipu_episode` node/edge JSON shape so one adapter output can feed both stores.
- A new **tier** value `"engine-state"` alongside `tree-sitter | lsp | cpg` (FR-3). Provenance for
  these facts = the adapter id + turn + faction (not a `file:line`).
- Gated behind a new `game-state` Cargo feature; joins the CI matrix in the same change (the
  "don't ship dark" rule).

**As built.** `src/state/graph.rs` + `src/state/ingest.rs` + `src/state/overlay.rs`.
Attribute values are a closed set of three scalars (bool/number/string), not arbitrary JSON:
the pattern engine compares them with `<`/`>=`, and an ordering over arbitrary JSON has no
defined answer. `visibility` (`shared` | `private`) has NO default — it routes a fact to the
game's common-knowledge base or to the calling faction's overlay, and guessing means one
faction's intel silently becoming common knowledge. A dangling edge is refused rather than
stored.

### FR-36 — Game-state policy selector/predicate model

Generalize the Quipu-authored policy model (`aegis:Policy`/`Selector`/`Predicate`, whose fields
already map 1:1 to `rules::Rule`) from code to game state. **1:1 reuse:** `matchType`, `gate`,
`effect` (`warn`|`deny`), `claim`/`targets`/`label`. **New:**

- `selectorLang ∈ { "tree-sitter", "graph-pattern", "sparql" }` — a discriminator. Code policies
  keep `"tree-sitter"` (`.scm` over the AST). Game-state policies use `"graph-pattern"`: a compact
  ASK-style pattern over the generic node/edge graph (FR-35). **Yupana is not an RDF/SPARQL store** —
  full SPARQL stays Quipu's job; any datalinks a predicate references are projected from Quipu
  first.
- `boundary "order"` (a new value beside `"action"`) — evaluated at pre-apply of proposed orders.
- `tier "game-state"`.

```turtle
aegis:policy_garrison_border a aegis:Policy ;
    rdfs:label "garrison-border-bases" ;
    aegis:targets "BaseState" ;
    aegis:claim "every border base retains >=1 garrison after the proposed orders apply" ;
    aegis:boundary "order" ; aegis:effect "deny" ;
    aegis:selector  [ aegis:selectorLang "graph-pattern" ;
                      aegis:evidenceSource "?b a smac:BaseState ; smac:isBorderBase true" ] ;
    aegis:predicate [ aegis:selectorLang "graph-pattern" ; aegis:matchType "must-match" ;
                      aegis:evidenceSource "?b smac:garrisonCount ?n | ?n >= 1" ] .
```

**As built.** `src/state/pattern.rs` (+ `pattern_parse.rs`) and `src/state/policy.rs`. The
grammar is the one sketched above; `MatchType` is imported from `crate::rules`, not
redeclared, so `must-match` cannot diverge between the two planes. **Yupana performs no prefix
expansion** — `smac:BaseState` matches what the adapter ingested, byte for byte; a prefix map
here would be a second, drifting copy of Quipu's. A `name` predicate resolves against the
subject's attributes first and its OUTGOING edges second.

One spelling correction, because it is a wire value: FR-36 and FR-37 above say `tier
"game-state"`, but FR-35 fixes the tier as `"engine-state"` and that is what shipped.
`game-state` is the CARGO FEATURE; `engine-state` is the tier. Two names for two things, and
a consumer discriminating on the wrong string would silently match nothing.

### FR-37 — `yupana_guard`: move/policy verify surface

The `(game_state + proposed_orders)` analog of `yupana_verify` (FR-23/24):

- `yupana_guard` (MCP) + `POST /guard` — `{ game_state, proposed_orders, tenant } → { violations[],
  advisories[] }`, each `{ policy, tier, claim, offending_order_ids[] }`.
- Evaluation: apply the proposed orders to a **COW overlay** of the hot board graph → run each
  policy's selector → evaluate the gated predicate on the *post-order* overlay → `deny` routes to
  `violations`, `warn` to `advisories`, each carrying `tier "game-state"`.
- **Complements, never replaces** the engine's own legality gate: it can only subtract or annotate
  *legal* moves.

**As built.** `src/state/orders.rs` + `src/state/guard.rs`. The report carries two lists the
design did not name, both for the same reason — a violation list alone cannot distinguish
"checked and clean" from "never checked": `unevaluated` (policies that would not compile, or
are Quipu's) and `vacuous` (policies whose SELECTOR matched nothing). Findings also carry
`pre_existing`; a breach that already held before the orders blames no order ids, because
denying a move for a condition it did not cause is a false deny.

### FR-38 — `yupana_whatif`: what-if / impact over state

Generalize `yupana_impact` (FR-11, BFS blast-radius over the call graph) to the board:

- `yupana_whatif` (MCP) + `POST /whatif` (or a `speculate` flag on `yupana_guard`) — speculatively
  apply an order-set to a COW overlay (the analog of Quipu's `speculate()` SAVEPOINT) and return a
  ranked downstream-impact set: bases exposed, own units entering enemy threat range, reachability
  / zone-of-control / supply shifts, opponent next-turn reach — **without committing**.
- Contrast to keep clear: `yupana_whatif` = ephemeral live board, fast, this-turn, tactical;
  Quipu `quipu_impact remove=true` = persisted knowledge, durable, cross-game.

**As built.** `src/state/whatif.rs`. Implemented as `POST /whatif` + `yupana_whatif`, not as a
`speculate` flag on the guard — they answer different questions and returning one shape for
both invited a caller to read an impact set as a verdict. The ranked set is STRUCTURAL and
domain-neutral: which entities a change reaches, how far, by which relations. "Bases exposed"
and the rest are `graph-pattern` policies over the same speculative board rather than
hardcoded traversals — yupana does not know what a supply line is, and burying one game's rules
in a general engine hides them from everyone playing a different one. Both surfaces share one
overlay and one apply path, so what the guard denies and what the what-if shows can never come
from different boards.

### FR-39 — Per-game / per-faction tenancy as an isolation boundary

Yupana is already multi-tenant (per-developer COW overlays over a shared base graph). Map that
directly to games:

- Tenant = `(game_id, faction_id)`; per-turn overlays for the guard/what-if speculation.
- **Shared base graph** = public / common-knowledge facts (map size, public treaties, tech known
  to ≥3, observed sightings). **Per-faction COW overlay** = that faction's private intel (own
  units/bases, unexplored fog, plans). A tenant reads the base + its own overlay, **never a
  sibling's** — fog-of-war isolation falls out of the existing architecture. When several factions
  are LLM-driven in one game, this is a **security** boundary, not just organization.

**As built.** `src/state/registry.rs`. Bases are per GAME, not one per process — a shared base
across games would make every fact in one common knowledge in the other, the same leak one
level up. Isolation is asserted three ways: by construction (a view holds exactly one overlay
reference; no API takes two tenant keys), by routing (a `shared` write carrying a faction is
refused), and by COUNT (`isolation_report()` scans the shared bases for faction-stamped facts
anyway — the first two are arguments, this is the measurement, and it is the only one that
catches a leak arriving by a path nobody anticipated). The tenant cap REFUSES rather than
evicting: an evicted overlay is a faction's private intel, and dropping it would silently widen
that faction's view back to the shared base.

## Honesty / dependencies

- All of FR-35..FR-39 is **net-new engineering**, not integration of existing capability. It is
  gated twice: by Yupana **Phase 4** (HTTP-only Quipu promotion; the `quipu` crate dep is still
  commented out; verdict signing is unkeyed → engine-observed/state facts are trusted-advisory,
  not cryptographically trusted) **and** by the non-code ingestion of FR-35, which does not exist
  today. FR-36/37/38 depend on FR-35.
- **Yupana is not a SPARQL store.** Game-state selectors are a compact graph-pattern subset over the
  native node/edge index; full SPARQL stays in Quipu.
- **The guard sees an *approximated* post-order board.** Applying proposed orders to a COW overlay
  re-implements a slice of the engine's order semantics outside the engine — a divergence risk. So
  `deny` policies must be conservative; the **game engine remains the sole authority** on legality
  and effects. This is exactly why `yupana_guard` complements, and never replaces, that authority.

See NeuralAmplifier's [knowledge-architecture.md](https://github.com/scbrown/NeuralAmplifier/blob/main/docs/knowledge-architecture.md)
for how the brain consumes these surfaces.
