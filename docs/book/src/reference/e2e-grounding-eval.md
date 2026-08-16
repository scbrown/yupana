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

A second group proves work-item scope isolation — the provenance ladder of
`docs/work-scoped-governance.md`, driven by the tracker's plate and the
observed scope projected from quipu's commit-provenance chain:

| scenario | proves |
| --- | --- |
| declared deny | a static `[yupana.policy.scopes.*]` entry hard-denies outside itself |
| observed in-scope | the item's own ground admits the edit silently |
| observed advise | out-of-scope is named (rung, item, right move) without blocking |
| unknown scope | no rung answers → one advisory per session, never a silent allow |
| observed enforce | `work_item_scope = "enforce"` denies outside the item's boundary |
| enforce in-scope | the hard boundary does not over-block the item's own ground |
| assignment briefing | `session-start` injects ground, central entities, similar successful work, and rules before the first edit |

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

## The retrieval eval (`just e2e f1`)

`scripts/e2e/eval_f1.py` scores the briefing's similar/related-item
retrieval against a labeled corpus organized as **problem classes**, so the
report says not just how good the scores are but *where* retrieval breaks.
Core classes (gated, default floor 0.9 macro-F1): a mixed composite,
phrase match, term overlap, provenance-only linkage, a single-term
distractor that corroboration must prune, a hub-entity trap that the
provenance rung's hub-degree cap must ignore, a crowded cluster larger
than the briefing cap, and a no-neighbors probe where the only correct
answer is silence. Hard classes (reported, not gated — the lexical
frontier): multi-term collisions and a paraphrase, the two shapes only a
semantic backend separates.

The semantic arm runs when the model bundle is provisioned, which
`just e2e f1` does best-effort: the all-MiniLM-L6-v2 ONNX bundle from
qdrant's fastembed mirror on `storage.googleapis.com` and the ONNX
Runtime dylib extracted from the `onnxruntime` PyPI wheel
(`scripts/e2e/extract_ort.py`, quipu's `ort` is `load-dynamic` via
`$ORT_DYLIB_PATH`) — both hosts commonly allowed where HuggingFace's LFS
CDN is not. With it, the briefing gains a fourth retrieval source (quipu
`/search`, query embedded server-side, supporting hits within 80% of the
query's top non-self score) and term probes retire — measured, they cost
more precision on a semantic store than the recall they add. Measured
macros: lexical overall 0.845; semantic overall 0.954, with the
multi-term-FP and paraphrase classes both at 1.0. The full write-up —
per-problem tables, the no-veto and term-probe-retirement findings, and
threats to validity — is the eval note at
`docs/briefing-retrieval-eval.md` (repo root docs, outside this book).

Each arm re-runs the shipped binary; ablations remove one retrieval source
via `$YUPANA_BRIEF_ABLATE` — feature removal against the real code path,
never a reimplementation. The gate fails the run unless the full arm's
core macro-F1 clears the floor and every ablation scores strictly below
it, so each retrieval source has to measurably earn its place.

Retrieval is corroboration-scored: sources vote (phrase hit 2, each term
hit 1, provenance co-occurrence 2) and single-vote candidates are pruned
whenever anything better exists — a lone shared term is how a lexical
distractor sneaks in. Measured effect: full-arm macro-F1 0.87 → 0.94.
The residual false-positive class is the multi-term lexical collision,
which only a semantic backend separates: configure quipu's embeddings
(`[embedding] model_path`/`tokenizer_path`, `--embed-backfill`) and the
same probes ride the vector path with no yupana changes.
