# Policy edit hooks — the yupana side

This documents yupana's half of the **performant edit hooks for policy** design.
The write-time enforcement half lives in quipu and is already implemented; see
[quipu `docs/design/policy-edit-hooks.md`](https://github.com/scbrown/quipu/blob/main/docs/design/policy-edit-hooks.md)
for the full split. This page records what yupana owns, why none of it ships yet,
and the backlog.

## Deciding principle: evidence locality

Evaluate a policy where its evidence is already hot.

- **Governed-fact policies** — the claim reasons over quipu's committed graph
  (provenance, prior verdicts, workflow state). Quipu evaluates these at its
  pre-commit gate.
- **Structural-evidence policies** — the claim reasons over the call graph,
  reachability, blast radius, or symbols. **Yupana** holds that hot, per tenant,
  at the `boundary:"action"` edit-time seam (`yupana hook pre-edit` + the resident
  daemon). Yupana evaluates these against a hot **projection** of quipu's canonical
  policies.

Canonical policy definitions, SHACL validation, verdict signing, and the
human-owned `VerifierRegistration` root of trust stay in quipu regardless. Yupana
holds only a projected read cache — a policy is never *defined* in yupana.

## What yupana owns

- A **hot projection** of quipu's `boundary:"action"` policies whose claims are
  structural, refreshed through yupana's existing watch/daemon machinery. Strictly
  one-directional (quipu canonical → yupana cache) with invalidation on quipu
  policy writes. If it diverged, yupana could allow what quipu would deny — so the
  projection is a cache, never a source of truth.
- **Edit-time evaluation** at `hook pre-edit`: an edit that violates a structural
  `deny` policy is blocked at edit time, against the resident graph, with a
  tier-tagged verdict.
- **Verdict promotion**: yupana's edit-time verdicts promote back into quipu as
  signed, bitemporal facts, extending the existing `commit → touched` promote
  path.

## Two rule sources, one engine

Structural policies reach the guard from two sources, evaluated by the same
`rules` engine (`src/rules.rs`) — a Selector (a tree-sitter `.scm` capture) + a
Predicate (a regex + a `must-match` / `must-not-match` / `must-exist` direction)
over the text an edit **introduces**:

- **Local config** (`[[yupana.policy.rules]]`) — ships today, no quipu needed. The
  operational plane, the same status the capability `scopes` have: yupana-local,
  authoritative, always fresh. This is where "no ticket id in a comment" lives
  for a standalone yupana.
- **Projected from quipu** (`quipu` feature) — the governed plane. Yupana fetches
  quipu's `boundary:"action"`, `tree-sitter`-tier policies and decodes them into
  the *same* `Rule` shape (field-for-field congruence with `aegis:Selector` +
  `aegis:Predicate`), so a projected policy is just a `Rule` the engine already
  runs. Opt-in via `[yupana.quipu] enabled + endpoint`. Yupana never *defines* a
  governed policy — the projection is one-directional (quipu canonical → yupana
  cache), and a verdict declares the cache's freshness.

Both are `Mode`-staged (advise-first) and fail open: a rule set that will not
compile, or a quipu that cannot be reached, is a LOUD allow, never a silent pass.

## What ships now, and what remains

The `cpg`/`lsp` lesson still binds — a feature must gate real code and never
advertise precision it lacks. What ships is real and tier-tagged; what does not
is named honestly below.

**Reframed:** projection is HTTP, not a crate pin. Like promotion (`POST /knot`),
Yupana reads policies over quipu's `POST /query` (W3C `sparql-results+json`), so
`--features quipu` needs no `quipu` *crate* dependency — H-DEP's "pin the dep" is
not a prerequisite for projection. The commented-out `quipu = { git = … }` line
stays commented; the RDF/`ureq` crates the feature already pulls are enough.

## Backlog

- **H-DEP** ✅ (reframed) Projection + promotion run over HTTP; no quipu crate
  pin needed. `--features quipu` is in the CI matrix.
- **H-PROJECTION** ✅ `src/project.rs`: decode quipu's structural policies into
  `Rule`s, a `ProjectionRegistry` with sync-state freshness. *Remaining:* the
  resident/async refresh cache (FR-31) — today's projection fetches per edit,
  which is the daemon-side optimization, not the semantics.
- **H-EDIT-EVAL** ✅ `hook pre-edit` evaluates projected policies; a `deny`-effect
  policy blocks under `Enforce`, tier- and freshness-tagged.
- **H-FRESHNESS** ✅ (slice) A verdict declares its freshness: local config is
  `fresh` (evidence is the exact proposed edit); a projection reports its real
  sync state (`fresh`, or `stale` when a refresh fails). The full copy-on-write /
  frontier code-graph freshness (Phase 3) is only needed for *graph-consulting*
  structural rules, which are not shipped — the buffer-local rules are fresh by
  construction.
- **H-PROMOTE-VERDICT** ✅ `src/verdict.rs`: ed25519 signing that MIRRORS quipu's
  `signing.rs` (same `ring`, same canonical `v1|…` message, same hex encodings),
  a `VerdictShape`-conformant signed `aegis:Verdict` Turtle, and `promote_verdict`
  over `/knot`. `yupana verifier` prints the public key a human registers as
  `aegis:publicKey`. *Remaining:* wiring the trigger (promote a verdict on the
  post-commit `commit → touched` path) and key rotation — the signing + promotion
  primitives are done and interop-tested against the shared scheme.
