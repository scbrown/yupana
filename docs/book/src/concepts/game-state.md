# The Game-State Harness

FR-35..FR-39, behind the `game-state` Cargo feature. This is a real widening of
Yupana's mandate: from a *code* graph analyzer to a **general in-memory fact graph
and policy harness**. It reuses the machinery — copy-on-write overlays,
multi-tenancy, the impact walk, the Selector/Predicate policy shape — over a
different subject.

The driving consumer is
[NeuralAmplifier](https://github.com/scbrown/NeuralAmplifier), an LLM brain for
*Alpha Centauri*, but nothing here is game-specific. The design is in
[the addendum](../design/specification.md) (`docs/neuralamplifier-harness.md`).

## The layers

| Surface | Requirement | What it does |
|---------|-------------|--------------|
| `yupana_ingest` / `POST /ingest` | FR-35 | Load span-free facts into the hot board |
| — | FR-36 | The `graph-pattern` policy model over those facts |
| `yupana_guard` / `POST /guard` | FR-37 | Check proposed orders against those policies |
| `yupana_whatif` / `POST /whatif` | FR-38 | Ranked speculative impact, uncommitted |
| — | FR-39 | `(game_id, faction_id)` tenancy as a fog-of-war boundary |

## A different node type, on purpose

A code node is a symbol anchored to `file:start_line..end_line`. A board fact
("base Alpha holds 2 garrison units") has no file and no line. Rather than widen
`SymbolNode` with an optional anchor — which a consumer will read as present —
the harness adds a second node type whose identity is an opaque id and whose
provenance is an **adapter id + turn + faction**.

Everything it serves carries `tier: "engine-state"`, a peer of
`treesitter`/`lsp`/`cpg` rather than a rung above or below them. A board fact and
a code fact must be equally impossible to mistake for each other, in both
directions.

The tier is advertised by `yupana status` exactly when the `game-state` engine is
compiled in. That is not the empty-feature pattern the removed `lsp`/`cpg` flags
fell into: `game-state` gates `src/state/`, which *is* the ingestion path, so the
flag and the implementation are the same thing.

## Ingestion, and the fog-of-war routing decision

```json
POST /ingest
{
  "game_id": "g1", "faction_id": "gaians",
  "visibility": "shared",
  "provenance": {"adapter": "smac-worldview", "turn": 42},
  "entities": [
    {"name": "base_alpha", "type": "smac:BaseState",
     "attrs": {"smac:isBorderBase": true, "smac:garrisonCount": 2}}
  ],
  "edges": [{"source": "base_alpha", "target": "base_beta", "relation": "adjacent_to"}]
}
```

The node and edge JSON mirrors `quipu_episode` (`name`/`type`/`description`,
`source`/`target`/`relation`) so one adapter output feeds both stores. `attrs` is
Yupana's addition: scalars the pattern engine can compare.

`visibility` has **no default**, and that is deliberate:

- `shared` writes the game's **common-knowledge** base, which every faction in
  that game reads — the map, public treaties, observed sightings.
- `private` writes only the calling faction's copy-on-write overlay — its own
  units and bases, unexplored fog, plans.

Guessing here means one faction's private intel silently becoming common
knowledge, so a request omitting the field fails to parse rather than picking a
side.

Restating a board is idempotent: node ids are unique, so the same base arriving
each turn with new numbers replaces rather than accumulates. A **dangling edge is
refused** — a pattern traversing it would bind a variable to an id with no entity
behind it.

Ingest otherwise **merges**: a node the new request does not mention survives
from the last one. That is right for a caller sending patches and wrong for a
caller whose payload *is* the board — without a way to say so, a base razed
twenty turns ago goes on matching policy selectors forever, a stale second
source of board state behind a caller who believes it just stated the current
one. `"replace": true` on the request makes that ingest the *whole* of the
tenant's private layer rather than a patch on it. Default `false`, so every
existing caller keeps the additive behaviour; private layer only, because the
shared base is common knowledge and not one tenant's to clear.

## Policies: `graph-pattern`, and where the line to Quipu is

A policy is the code plane's shape with three additions: a `selector_lang`
discriminator, a `boundary` value of `"order"`, and the `engine-state` tier.
`match_type`, `gate`, `effect`, `claim`, `targets` and `label` are reused
unchanged — `MatchType` is literally the same Rust type, so `must-match` cannot
come to mean one thing over an AST and another over a board.

```text
pattern := clause { '.' clause } [ '|' filter { ',' filter } ]
clause  := ?var pair { ';' pair }
pair    := ('a' | name) term
filter  := ?var ('=' | '!=' | '<' | '<=' | '>' | '>=') literal
```

```json
{
  "label": "garrison-border-bases",
  "claim": "every border base retains >=1 garrison after the proposed orders apply",
  "boundary": "order", "effect": "deny",
  "selector":  {"selector_lang": "graph-pattern",
                "evidence_source": "?b a smac:BaseState ; smac:isBorderBase true"},
  "predicate": {"selector_lang": "graph-pattern", "match_type": "must-match",
                "evidence_source": "?b smac:garrisonCount ?n | ?n >= 1"}
}
```

Two things worth knowing before writing one:

- **No prefix expansion.** `smac:BaseState` is matched byte for byte against what
  the adapter ingested. Yupana has no prefix map, and inventing one would be a
  second, drifting copy of Quipu's.
- **A `name` predicate resolves to an attribute first, an outgoing edge second.**
  Edge traversal is outgoing only; a symmetric relation is ingested both ways.

**`selector_lang: "sparql"` is reserved for Quipu and is refused here** — not
skipped, not best-effort matched. Yupana is not an RDF store. If a predicate starts
wanting property paths or aggregation, that is the signal the policy belongs in
Quipu, not the signal to grow this grammar. Project the datalinks it needs into
the board instead.

## Guarding an order set

```json
POST /guard
{"game_id": "g1", "faction_id": "gaians",
 "policies": [ ... ],
 "orders": [{"id": "move-out", "effects": [
   {"op": "set_attr", "id": "base_alpha", "key": "smac:garrisonCount", "value": 0}]}]}
```

Orders carry **declared effects**. Yupana does not know that `MOVE` implies a
supply change and does not try to infer it — it applies exactly the deltas the
adapter states. That is what bounds FR-37's standing divergence risk: the gap is
"what the adapter declared vs. what the engine will do", which its author can
see and close, rather than "Yupana's reimplementation of the rules vs. the real
ones", which nobody can enumerate.

**The game engine remains the sole authority on legality and effects.** This
complements it and never replaces it: it can only subtract from, or annotate,
moves that are already legal. Because the post-order board is an approximation,
`deny` policies should be conservative — a false deny removes a legal, possibly
correct move.

The report has four lists, and reading only the first is a mistake:

| Field | Meaning |
|-------|---------|
| `violations` | `deny` policies that fired |
| `advisories` | `warn` policies that fired |
| `unevaluated` | policies that could not be compiled, or are not Yupana's to run |
| `vacuous` | policies whose **selector matched nothing** — never asked, not passed |

`vacuous` exists because a selector that has rotted away from the adapter's
vocabulary matches zero nodes and produces zero violations, which reads exactly
like a clean board. Every finding also carries `pre_existing`: a breach that
already held before the orders blames no order ids.

## What-if

`POST /whatif` speculates an order set onto a clone of the overlay and ranks what
it reaches — nearest first, then by degree — then throws the clone away. The
reply states `"committed": false` rather than leaving it implicit, because that
is the property a caller most needs to be able to check.

The ranking is **structural and domain-neutral**: which entities a change reaches,
how far, by which relations, over the adapter's own vocabulary. "Is this base
exposed" is a `graph-pattern` policy over the same speculative board, not a
hardcoded traversal — Yupana does not know what a supply line is, and putting a
slice of one game's rules inside a general engine would hide it from everyone
playing a different one.

Contrast with Quipu's `quipu_impact remove=true`: this is ephemeral, fast,
this-turn and tactical; that is persisted, durable, cross-game and strategic.
Blurring them is how the hot path acquires a database dependency it cannot
afford.

## Tenancy is a security boundary

The tenant is `(game_id, faction_id)`. One **shared base per game** — not per
process, which would make every fact in one game common knowledge in the next —
plus one copy-on-write overlay per faction. A tenant reads the base plus its own
overlay, never a sibling's.

When several factions in one game are LLM-driven, a leak between overlays is not
a tidiness bug: it is one player reading another's private intel, and it would be
**invisible in results** — the run just looks unusually well-informed. So
isolation is asserted three ways, because none of them is sufficient alone:

1. **By construction.** A view holds exactly one overlay reference, chosen from
   the tenant key. No API takes two keys and no method reaches a sibling, so a
   cross-tenant read is unrepresentable rather than merely checked.
2. **By routing.** A `shared` ingest carrying a faction is refused and counted —
   the one path by which private intel could reach the layer everybody reads.
3. **By count.** `isolation_report()` scans the shared bases for faction-stamped
   facts anyway. The first two are arguments; this is a measurement, and it is
   the only one that catches a leak arriving by a path nobody anticipated.

The tenant cap **refuses** rather than evicting. An evicted overlay is a
faction's private intel, and dropping it mid-game would silently widen that
faction's view back to the shared base — a correctness change disguised as a
resource policy. The code plane can evict because a developer's overlay costs
only a re-touch.

## The one operational trap

**A board lives in the process it was ingested into.** There is no store behind
these surfaces; that is what a hot graph means. Ingesting over `POST /ingest` and
then guarding through `yupana_guard` in a *different* process guards an empty
board.

So an empty board is **refused**, never reported as zero violations: `409
Conflict` over HTTP, an error result over MCP. Ingest and guard are separate
calls, and "nothing was ingested here" and "these orders are fine" would
otherwise be the same success response — a green light over a dead backend.

A guard with **no policies** is refused for the same reason: it cannot clear an
order set it never checked.
