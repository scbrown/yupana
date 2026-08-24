# Golden-Path Conformance

FR-40..FR-42, behind the `golden-path` Cargo feature (in the CI matrix). A
**golden path** is a blessed trajectory — a pruned, human-promoted record of
how verified-successful work actually went, authored and governed in Quipu.
This plane evaluates work against one: given a declared `followsPath` and a
sequence of steps, it answers where the work stands — matched, deviating,
hazard-adjacent — under the versioned conformance grammar shared with Quipu's
backtest (`gp-grammar/1`).

The full design addendum, including the blessing ladder and the division of
labor with camayoc and quipu, is
[`docs/golden-path-guard.md`](https://github.com/scbrown/yupana/blob/main/docs/golden-path-guard.md).

## Surfaces

- **`yupana_path_check`** over MCP (registered on every build; refuses with a
  message naming the feature when `golden-path` is absent).
- **`POST /path/check`** on the [resident daemon](daemon.md), mounted only on
  a build that can serve it.

Both take the same request: the `followsPath` IRI the work declared, the steps
as v1 signatures (`action_kind` + `target_class`), the **projected paths
supplied per call**, a `mode`, and an optional `deny` opt-in.

Paths are supplied per call, like the board guard's `StatePolicy` list,
because a stale resident copy would enforce yesterday's blessing while looking
current. "Empty registry" is therefore a per-call property: a request
declaring a `followsPath` that is not in the supplied set is **refused**,
never reported clean — `409 Conflict` over HTTP, an error result over MCP, so
a caller gating on status alone cannot mistake "nothing was loaded" for "this
plan conforms". A grammar version this build does not implement refuses the
same way.

## The two modes

Under gp-grammar/1 gaps are allowed, so an **open** trajectory never
hard-deviates — a future step could always match the next pattern element.
Deviation is decidable only against a complete intent, which is what splits
the check in two:

- **`mode: "plan"`** (default, FR-42) — the submitted sequence is the whole
  intent. The report names how much of the pattern matched and the **first
  deviation point** (which pattern step was expected, after which submitted
  step). This is the only mode in which deviation is decidable, and so the
  only mode that can ever deny.
- **`mode: "progress"`** (FR-41) — the work is in flight. The report says how
  far along the path the work is, which `deadEnd` hazards it has brushed
  ("exemplars tried this; it did not help"), and which steps the grammar could
  not evaluate. It never denies.

## Effects are capped by blessing level

| Path level | Effect |
|---|---|
| `advisory` (L3) | `warn` only |
| `blessed` (L4) | `warn` by default; `deny` only when the caller passed `deny: true` **and** the mode is `plan` |
| constraint-backing (L5) | does not even parse — an unsigned L5 cannot enforce as if it were signed; gated on verdict signing |

Deviation is not failure: a deviation verdict is input to the record (quipu
stores it; the promotion gates consume it as traffic), not a judgment that the
agent is wrong. Paths are demoted when conformers start failing, and that
evidence only exists if deviation and conformance are both recorded
faithfully.

## The report

An answerable check returns a `PathCheckReport`: the grammar version and path
IRI it evaluated, the path's `level` and the `mode`, `matched` out of
`pattern_len`, the `first_deviation` (plan mode), `hazards` (which submitted
step matched which dead end, with its note), `unevaluated_steps`, the
resulting `effect`, the path's **exemplar citations** (a warn must be able to
say "because this concrete work succeeded this way"), and `projected_at` —
echoed from the projection, omitted rather than faked when the projection
carries none.
