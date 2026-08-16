# Eval note: corroborated similar-work retrieval for the work-item briefing

**Status:** measured, 2026-08-15, against yupana at the commits cited
below. Every number reproduces with `just e2e f1` (the harness prints the
same tables to `target/e2e-f1/f1-report.md`); the semantic arm needs the
model bundle, which the recipe provisions best-effort. Companion docs:
`docs/book/src/reference/e2e-grounding-eval.md` (how to run),
`docs/work-scoped-governance.md` (the scope ladder the briefing serves),
provisional D § 11 (the disclosed mechanisms).

## What is being measured

`yupana hook session-start` briefs an agent at work-item assignment time;
the briefing's similar/related-work sections are a retrieval problem:
given the item's label and graph neighborhood, surface the prior work the
agent should reuse or coordinate with. Retrieval draws on four sources —
whole-label match, distinctive-term probes, provenance co-occurrence, and
(when the store has an embedding provider) semantic similarity via quipu
`/search` — combined by corroboration voting: phrase 2, each term 1,
provenance 2, semantic 2; single-vote candidates are pruned only when a
corroborated candidate exists, and the result is never blanked.

The eval scores the **shipped binary** on a labeled corpus organized as
problem classes, each probe isolating one retrieval situation. Ablation
arms re-run the same binary with one source removed via
`$YUPANA_BRIEF_ABLATE` — feature removal of the real code path, never a
reimplementation. The gate: core macro-F1 ≥ 0.9 and every ablation's
overall macro strictly below full. A source whose removal changes nothing
fails the build; contribution is proven the way the installer proves
refusal gates — by a degradation only that removal causes.

## Results

Per problem, full configuration (semantic arm on):

| problem | gated | P | R | F1 |
| --- | --- | --- | --- | --- |
| mixed composite | yes | 1.00 | 0.67 | 0.80 |
| phrase-match | yes | 1.00 | 1.00 | 1.00 |
| term-overlap | yes | 1.00 | 1.00 | 1.00 |
| provenance-only | yes | 1.00 | 1.00 | 1.00 |
| single-term FP | yes | 1.00 | 1.00 | 1.00 |
| hub-entity trap | yes | 1.00 | 1.00 | 1.00 |
| crowded-cluster | yes | 1.00 | 0.71 | 0.83 |
| no-neighbors | yes | 1.00 | 1.00 | 1.00 |
| mixed-collision | no | 1.00 | 0.75 | 0.86 |
| multi-term FP | no | 1.00 | 1.00 | 1.00 |
| paraphrase | no | 1.00 | 1.00 | 1.00 |

Per arm (macro-averaged):

| arm | core F1 | hard F1 | overall F1 |
| --- | --- | --- | --- |
| full (semantic) | 0.954 | 0.952 | **0.954** |
| − provenance | 0.667 | 0.800 | 0.703 |
| − /context pipeline | 0.438 | 0.222 | 0.379 |
| − semantic (= lexical config) | 0.979 | 0.489 | 0.845 |

The lexical configuration is itself a valid deployment (stores without an
embedding provider) and is separately ablation-proven: there, term probes
carry the term-overlap class and the corroboration threshold carries both
FP classes (0.87 → 0.94 macro when voting landed).

## Findings the gate forced

**No semantic veto.** The obvious upgrade — let a low semantic score
demote a lexically nominated candidate — was implemented, measured, and
removed: on the serving surface a term-corroborated true positive scored
0.352 while the term-corroborated false positive scored 0.384. The model
ranks the false one higher; no veto threshold keeps one and prunes the
other. A low semantic score is therefore not evidence against a
corroborated candidate. (Support survives because it is judged at the top
of the ranking, where the model is reliable.)

**Query-relative acceptance, not absolute floors.** Self-similarity on
the serving surface ranges ~0.2–0.75 per query (entity text carries
type/provenance boilerplate the bare query lacks), so two rounds of
absolute cosine floors misfired in both directions. The shipped rule:
support within 80% of the query's top non-self score, top ≥ 0.4, self
excluded from setting the scale.

**Term probes retire on a semantic store.** The strictly-below gate
flagged `-term-probes` *beating* full: on a semantic store the collisions
term probes admit cost more than the recall they add. They now run only
in the lexical configuration — the substring-fallback compensation they
were built to be. Cost, visible above: the mixed composite's
jargon-only sibling (findable only by term probes) drops recall to 0.67
under the semantic configuration.

**Hub entities poison co-occurrence.** Before the degree cap, the
hub-entity trap scored P = 1/6: five unrelated items retrieved because
their commits touched the same justfile-shaped file as the probe's.
Sharing a hub evidences shared residence, not shared work; entities
touched by more than `HUB_DEGREE_CAP` distinct items now contribute
nothing.

**One ground-truth correction.** The provenance-only probe originally
shared the "make … faster" sentence shape with the paraphrase probe; the
model scored them similar and the eval called it a false positive. The
model was right — both labels described performance work — and the corpus
label, not the retrieval, was fixed.

## Threats to validity

The corpus is small (36 items, 11 probes) and synthetic, with collisions
designed rather than sampled; the semantic thresholds were calibrated on
the same corpus they are scored on; one embedding model (all-MiniLM-L6-v2)
and one serving surface were measured, and the no-veto finding is a claim
about that surface, not about semantic models in general. The remaining
sub-1.0 scores are structural, not noise: the crowded cluster measures the
briefing's deliberate 5-item cap, and the mixed composite measures the
term-probe retirement trade.

Next: re-run the same harness over a snapshot of real tracker history
(labels + commit provenance already have the corpus's shape), and derive
relevance labels from the verdict spool — items whose briefed ground the
agent then edited are confirmed positives.
