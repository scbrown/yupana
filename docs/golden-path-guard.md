# Addendum: Golden-Path Conformance Guard

> **Status: BUILT (first cut), behind the `golden-path` Cargo feature** —
> `src/goldenpath/` implements the grammar and the check; the surfaces are
> `yupana_path_check` over MCP and `POST /path/check` on the daemon (both
> feature-gated; the feature joined the CI matrix in the same change). Where
> the implementation made a choice the design left open, the section says so
> inline under **As built** — most notably: projected paths are supplied
> per call like `StatePolicy` (a stale resident copy would enforce
> yesterday's blessing), and deviation is decidable only against a complete
> plan, so FR-41's per-action flow reports progress and hazards rather than
> hard deviations. This addendum extends
> [yupana-spec.md](yupana-spec.md) and the game-state addendum
> ([neuralamplifier-harness.md](neuralamplifier-harness.md)) with the
> enforcement half of the golden-paths design. The ontology and blessing
> ladder live in camayoc (`docs/design/golden-paths.md`); the storage,
> pruning, and promotion mechanisms live in quipu
> (`docs/design/golden-paths-blessing.md`). FR numbering continues from the
> game-state addendum (last: FR-39).
>
> Per the lesson recorded at the top of the game-state addendum: when any part
> of this ships, this banner must be corrected in the same change — a reader
> who believes a capability is absent does not look for it, and one who
> believes it is present builds on a hole.

## Why yupana

A golden path is a blessed trajectory: a pruned, human-promoted record of how
verified-successful work actually went, stored and governed in quipu. Its
enforcement half is a *guard*: given an agent working a work item that
declared `followsPath`, evaluate each proposed action — and whole proposed
plans — against the path, and answer within a deadline. That is yupana's
existing shape three times over:

- the **pre-edit hook** (FR-30) already guards proposed actions inside a
  deadline + fail-open contract;
- the **game-state harness** (FR-35..39) already generalized the guard from
  code edits to arbitrary ingested fact graphs and declared-effect orders;
- the **quipu policy-projection path** (`src/hook/rule_planes.rs`) already
  delivers governed rules into the hot path, with projection freshness served
  on every verdict.

The conformance guard is those three seams composed, pointed at trajectory
steps instead of edits or board moves.

## New requirements

### FR-40 — Blessed-path projection

Project golden paths at blessing level L3 (advisory) and above from quipu
into the guard's rule registry, over the existing policy-projection path.
Each projected path carries:

- its **level** (L3 advisory / L4 blessed) — the level decides the effect
  ceiling below;
- its **conformance grammar version** — the step-matching contract is defined
  once, shared with quipu's backtest, and versioned; a guard verdict and a
  backtest must never silently disagree about what "conforming" means;
- its **exemplar citations** — a warn or deny must be able to say "because
  this concrete work succeeded this way," exactly as exemplar-carrying
  policies do today.

Projection freshness is served on every conformance verdict, as it is on rule
verdicts today. **As designed**, paths are a new rule plane beside the
existing policy planes, not a reuse of one: their applicability is keyed by
the tenant's declared `followsPath`, not by selector match.

**As built:** the projection is carried per call, exactly as the board
guard's `StatePolicy` list is — the request supplies the projected paths
(`ProjectedPath`: grammar version, level, pattern, dead ends, exemplars,
`projected_at`), because a stale resident copy would enforce yesterday's
blessing while looking current. `projected_at` is echoed on every verdict and
omitted rather than faked when the projection carries none. A
`constraint-backing` level does not even parse (`src/goldenpath/grammar.rs`) —
an unsigned L5 cannot enforce as if it were signed.

### FR-41 — Step-conformance verdicts

Given a tenant whose work item declared `followsPath <path>`, evaluate each
proposed action against the path's step grammar and return a verdict naming:

- the matched path step, or the deviation (which step was expected, what was
  proposed instead);
- any `deadEnd` hazard the proposed action matches — "exemplars tried this;
  it did not help" is served as advisory context even when nothing blocks;
- the path's level and this verdict's effect.

Effects by level, inside the existing deadline + fail-open contract of FR-30:

| Path level | Effect |
|---|---|
| L3 advisory | `warn` only — conformance and deviations are recorded, nothing blocks |
| L4 blessed | `warn` by default; `deny` is opt-in per tenant, exactly like the FR-30 guard's deny arm |
| L5 constraint-backing | out of scope for this addendum — gated on verdict signing (the game-state addendum's Phase-4 caveat); until verdicts are signed, a path-derived verdict is trusted-advisory and must not be presented as an authorization decision |

Deviation is not failure: an L3/L4 deviation verdict is *input to the
record* (quipu stores it; the promotion gates consume it as traffic), not a
judgment that the agent is wrong. Paths are demoted when conformers start
failing; that evidence only exists if deviation and conformance are both
recorded faithfully.

**As built:** under gp-grammar/1 gaps are allowed, so an OPEN trajectory
never hard-deviates — a future step could always match the next pattern
element. The check therefore takes a mode: `progress` (this FR's per-action
flow) reports pattern progress, dead-end hazards, and unevaluated steps, and
never denies; `deny` is reachable only in `plan` mode (FR-42), where the
submitted sequence is the whole intent and deviation is decidable — and only
for a blessed path with the caller's explicit `deny: true`.

### FR-42 — Plan pre-check (what-if over a step sequence)

Before executing, an agent submits a *planned* step sequence and receives the
whole plan's conformance report in one call: matched steps, deviations,
dead-end hazards, and the first point at which the plan leaves the path.
This generalizes `yupana_whatif` (FR-37) from one proposed order to an
ordered sequence, evaluated against the projected path rather than the board
rules. Same surface family: `yupana_path_check` over MCP, `POST /path/check`
over HTTP.

## Honesty rules (carried over, not optional)

- **An empty path registry is refused, not reported clean.** `followsPath`
  declared but no projected path loaded → refusal (409 / MCP error), never
  zero findings. Zero findings over a registry that was never loaded is a
  green light over a dead backend — the exact failure the game-state guard
  already refuses for an empty board.
- **Every verdict names its projection freshness.** A conformance verdict
  enforcing yesterday's blessing says so.
- **Unevaluable is reported as itself.** A step the grammar version cannot
  evaluate appears in an `unevaluated` list, exactly as `selectorLang
  "sparql"` refusals do today — never silently skipped.
- **No level inflation.** A verdict carries the path's level; an advisory
  path's verdict is never presented with blessed-path weight.

## Honesty / dependencies

- Everything here is **net-new engineering** on existing seams, not existing
  capability: the path rule plane, the step grammar, and the sequence
  evaluator do not exist today.
- FR-41 and FR-42 depend on FR-40; FR-40 depends on quipu's blessing pipeline
  producing projectable paths at all (quipu's
  `golden-paths-blessing.md`, itself design-only).
- The conformance grammar is a shared contract with quipu's backtest and must
  ship versioned from the first cut, or backtest-justified promotions and
  live verdicts will drift apart invisibly.
