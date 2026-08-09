# Rename: hank → yupana

> **Status (2026-08-09):** yupana is the continuation of the repository
> formerly named **hank**. Full git history is preserved; the rename is
> one commit on top of hank's final `main` (`aee6beb`). The old
> repository is retained as a tombstone. (The name: a yupana is the
> Andean abacus — the calculating instrument used alongside quipus.)

## What changed in this repository

- Crate, binary, and package name: `hank` → `yupana` (Cargo.toml,
  Cargo.lock, CLI, env-var prefixes `HANK_*` → `YUPANA_*`).
- Renamed files: `docs/hank-spec.md` → `docs/yupana-spec.md`,
  `assets/hank-header.svg` → `assets/yupana-header.svg`,
  `.vale/styles/Vocab/hank/` → `.vale/styles/Vocab/yupana/`.
- All prose, identifiers, and doc references, case-preserving
  (`hank`/`Hank`/`HANK`). Full test suite passes after the rename
  (390 tests, 0 failures).

## Known debt inherited from hank main (pre-existing, not caused by the rename)

**Resolved (2032fb1).** `just check` was red on hank's final `main`:
four files stood above the file-size ratchet — `cli.rs` (523>510),
`cli_status.rs` (647>619), `promote.rs` (1022>709), and `policy.rs`
(514, over the hard limit unlisted). All four were paid down by
extraction into child modules and size-exempt `_test.rs` files; the
baseline regenerated strictly downward and the gate exits 0.

## Projects that need updating (reference map)

Recorded artifacts are exempt on principle: a trace recorded when the
binary was named `hank` keeps that name — facts true at write time.
Only live references update.

### quipu

- **Paper — done** (`docs/paper/`, `docs/design/paper.md`): the wild
  trace and census anchor now name Yupana, with a formerly-hank
  footnote.
- `justfile:101` — `ingest-repos` default includes `../hank`
  (functional: path breaks when the local clone is renamed).
- `README.md:317` — links the SARC conformance page at
  `scbrown.github.io/hank/...` (functional once Pages is enabled on
  yupana; the tombstoned repo keeps the old page alive meanwhile).
- `src/metrics.rs` — doc comments and tests use `hank`/
  `hank-policy-hook` as example client names (cosmetic; the client
  label is whatever the caller sends, but examples should match the
  live fleet).
- Test fixtures and doc-comment writer names across
  `src/governance*`, `src/lattice*`, `src/mcp/*`, `src/store/*`
  (~30 refs, cosmetic — scripted writer identities in tests).
- `benchmark/census/wild/README.md`, `BUILD_REPORT.md` — describe the
  recorded trace's provenance. The trace itself stays `hank`
  (recorded artifact); the prose should say "yupana (then hank)".
- Design docs: `policy-edit-hooks.md` (33 refs), `graph-labels.md`,
  `statement-identity.md`, `named-graphs.md`, `datalinks-3d.md`,
  `docs/book/src/reference/cli.md`, `CHANGELOG.md` (historical
  entries stay).

### NeuralAmplifier (heaviest consumer)

- `na.toml` — comments name the Quipu/Hank seam and the
  `NA_HANK_GUARD` override (**functional**: the env var itself must
  follow the binary's new prefix).
- `docs/hank-integration.md` — file rename + `docs/SUMMARY.md` link.
- `docs/knowledge-architecture.md` (62 refs), `VISION.md`,
  `docs/game-surface.md`, `docs/agent-play.md`, `docs/contract.md`,
  `docs/quipu-integration.md`, `docs/decision-inputs.md`,
  `docs/tenancy-and-isolation.md`, `docs/observability.md`,
  `docs/directives.md`, `docs/building-and-testing.md`,
  `docs/strategy-knowledge.md`, `docs/ontology/smac-ontology.md`,
  `README.md`, `AGENTS.md`, `orchestrator/README.md`.

### camayoc

- `README.md`, `AGENTS.md`, `docs/vision.md`, `docs/design/ingress.md`,
  `commands/bootstrap.md`, `competency/metrics-and-requirements.md`,
  `competency/verification-and-liveness.md` — sibling-repo references
  (cosmetic, ~15 refs).

### bobbin

- `src/storage/sqlite/tests.rs` — one fixture reference.
- `.github/workflows/release.yml:250` — one comment ("same gap hank
  had").
- `Local RAG and Context Injection.md` — one prose reference.

### shantytown

- `README.md`, `docs/harnesses.md` — sibling-repo references
  (cosmetic, 3 refs).

### GitHub-side (manual, owner-only)

- Enable Pages on yupana if the book should publish at
  `scbrown.github.io/yupana/`.
- Update the hank repo description to point here; optionally archive
  the tombstone once downstream references are migrated.
- CI secrets/tokens, if any were repo-scoped to hank.
