# E2E Grounding Eval

`just e2e` stands up the real pair — a `quipu-server` on a freshly seeded
store, and this repo's `yupana` pre-edit guard pointed at it — and proves
the grounding-integrity loop end to end: governed policy projected from
Quipu, hallucinated references rejected in the agent's loop at a declared
tier, and every verdict returned to Quipu as a signed fact.

It needs the sibling checkouts `../quipu` (the governed store) and
`../camayoc` (the policy pack `shapes/policies/edit-grounding.ttl` that the
harness loads). Both sides are built in release automatically.

## The eval (`just e2e run`)

`scripts/e2e/harness.py` seeds the store with the camayoc pack, two
`aegis:WorkItem` records, and yupana's registered verifier key, then drives
the guard through eight scenarios. Each maps to an aspect of the grounding
cluster disclosure:

| scenario | proves |
| --- | --- |
| clean, cited edit | a bounded pass — the guard does not cry wolf |
| hallucinated call | a reference that exists nowhere is denied, tier declared |
| fabricated work-item citation | id-shaped but resolving to nothing is its own violation class |
| uncited edit | the `must-ground` discipline denies untracked work |
| freshness declared | a live projection says `verdict freshness: fresh` |
| quipu down, cache warm | last-known policy still enforced, cache age named |
| quipu down, no cache | loud fail-open — never a silent allow |
| verdict return | spooled verdicts land in quipu signed, with tier + freshness |

Everything observable is captured under the workdir (default `target/e2e`):
per-scenario guard stdout/stderr with `RUST_LOG=debug`, the metrics spool,
the verdict spool, quipu's Prometheus `/metrics` snapshots, and a scored
`report.md`/`report.json`. The run exits nonzero if any check fails.

## The bench (`just e2e bench`)

`scripts/e2e/bench.py` answers the capacity question: how many concurrent
agents can share one Yupana against one Quipu? Each simulated agent is what
a deployment actually runs — a `yupana hook pre-edit` process per edit,
every guard projecting policy live from the single quipu-server — under its
own tenant id.

The sweep reports, per concurrency level, guard latency (p50/p95/max),
aggregate edits/second, and the projection serving split:

- **live** — projected from quipu on this guard
- **cached** — quipu could not answer in time; the durable cache enforced
  last-known policy and the verdict declared its age
- **fail-open** — no servable projection; the edit went unguarded, loudly

The split is the honest capacity signal: the guard degrades live → cached →
fail-open by design, so "how many agents fit" has two answers — how many
keep fully-live projections, and how many stay enforced at all.
