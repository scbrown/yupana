#!/usr/bin/env python3
"""Retrieval eval for the work-item briefing: per-problem F1 + ablation.

Builds a labeled corpus of work items in a fresh quipu store, organized as
PROBLEM CLASSES — each probe isolates one retrieval situation, so the
report says not just how good the scores are but WHERE retrieval breaks:

  core classes (gated):
    mixed             the original composite probe (phrase+term+provenance)
    phrase-match      near-duplicate label phrasing
    term-overlap      shares distinctive terms only
    provenance-only   lexically disjoint, linked by touched entities
    single-term-fp    a lone shared term must NOT retrieve (corroboration)
    hub-entity-trap   co-occurrence through an everyone-touches-it file
                      must NOT retrieve (hub-degree cap)
    crowded-cluster   more relevant items than the briefing cap
    no-neighbors      a genuinely novel item must retrieve NOTHING

  hard classes (reported, not gated — the lexical frontier, closed by the
  semantic arm when the model bundle is provisioned):
    mixed-collision   composite probe with a multi-term lexical collision
    multi-term-fp     a distractor corroborated by two shared terms
    paraphrase        relevant item in different words, no shared entity

Every arm runs the SHIPPED `yupana hook session-start` binary; ablation
arms remove one retrieval source via `$YUPANA_BRIEF_ABLATE` (feature
removal, never a reimplementation). Gates: core macro-F1 >= --min-f1
(default 0.9) and every ablation's OVERALL macro strictly below full.

Usage: scripts/e2e/eval_f1.py [--workdir DIR] [--profile release]
Run via `just e2e f1`.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from harness import YUPANA_ROOT, Rig, log, run  # noqa: E402

# ---------------------------------------------------------------------------
# The corpus: id -> (label, outcome, [entities]). Entities become CodeModule
# nodes with a filePath plus a GitCommit implements/modifies chain, so the
# provenance rung and the observed-scope plumbing are both exercised.
# Term collisions are DESIGNED (a distinctive term appearing in exactly the
# labels the probe's class needs) — comments mark each one.
# ---------------------------------------------------------------------------

CORPUS: dict[str, tuple[str, str | None, list[str]]] = {
    # -- mixed (legacy probe 1): each relevant item reachable one way only --
    "aegis-p0001": ("prove the grounded edit boundary", None, ["ent_e1"]),
    "aegis-a0002": ("prove the grounded edit boundary for tallies", "done", []),  # phrase
    "aegis-a0003": ("extend grounded boundary checks to imports", "done", ["ent_e3"]),  # terms
    "aegis-a0004": ("reject fabricated citations at the edit seam", None, ["ent_e1"]),  # prov
    # -- mixed-collision (legacy probe 2, hard): a5 collides on 2 terms --
    "aegis-p0002": ("weave pattern cache for the loom", None, ["ent_e2"]),
    "aegis-b0001": ("boundary conditions for weave tension", None, ["ent_e2"]),
    "aegis-b0002": ("cache the loom pattern weft", "done", []),
    "aegis-b0003": ("weave pattern cache for the loom shuttle", "done", []),
    "aegis-b0004": ("speed up shuttle threading", None, ["ent_e2"]),
    # -- multi-term-fp probe (hard): a5 doubles as legacy p2's collision.
    # No entities on purpose: the class isolates the LEXICAL failure mode,
    # so no provenance edge may rescue or muddy it.
    "aegis-a0005": ("pattern matching in the policy cache", None, []),
    "aegis-pm001": ("policy cache pattern precedence rules", "done", []),  # relevant
    "aegis-pc001": ("matching engine for pattern workloads", None, []),  # 2-term FP
    # -- phrase-match --
    "aegis-k0000": ("rotate the signing keys quarterly", None, ["ent_keys"]),
    "aegis-k0001": ("rotate the signing keys quarterly for verifiers", "done", []),
    # -- term-overlap --
    "aegis-w0000": ("debounce watcher events on save", None, ["ent_watch"]),
    "aegis-w0001": ("coalesce debounce intervals for the watcher", "done", []),
    "aegis-w0002": ("watcher debounce regression on large trees", None, []),
    # -- provenance-only: labels avoid the probe's terms entirely (and, per
    # a measured corpus bug, the "make ... faster" shape the paraphrase
    # probe uses — the model rightly scored the two as similar) --
    "aegis-c0000": ("shorten the cold boot path", None, ["ent_boot"]),
    "aegis-c0001": ("lazy-load grammars at process launch", "done", ["ent_boot"]),
    "aegis-c0002": ("profile allocations during init", None, ["ent_boot"]),
    # -- single-term-fp: d1 shares only "invalidation" and must be pruned --
    "aegis-o0000": ("cache invalidation for the overlay plane", None, ["ent_ovl"]),
    "aegis-o0001": ("overlay plane cache eviction on session close", "done", []),
    "aegis-d0001": ("invalidation of stale metric results", None, []),
    # -- paraphrase (hard): same intent, no shared distinctive term (>=5
    # chars) and no shared entity — reachable only semantically. ("pre-edit"
    # is shared vocabulary, but it splits to sub-5-char tokens, so no lexical
    # probe can use it; a paraphrase that shares zero domain words is not a
    # paraphrase, it's a different task.)
    "aegis-f0000": ("make the pre-edit check respond faster", None, ["ent_load"]),
    "aegis-f0001": ("reduce pre-edit hook latency", "done", ["ent_lat"]),
    # -- hub-entity-trap: t1..t5 share only the hub with the probe --
    "aegis-m0000": ("tighten markdown lint conventions", None, ["ent_hub", "ent_docs"]),
    "aegis-m0001": ("fix markdownlint violations in the book", "done", ["ent_hub", "ent_docs"]),
    "aegis-t0001": ("bump the toolchain pin", None, ["ent_hub"]),
    "aegis-t0002": ("add release automation recipe", None, ["ent_hub"]),
    "aegis-t0003": ("quiet recipe output by default", None, ["ent_hub"]),
    "aegis-t0004": ("install pre-commit dependencies", None, ["ent_hub"]),
    "aegis-t0005": ("rename build recipes for clarity", None, ["ent_hub"]),
    # -- crowded-cluster: seven relevant, briefing caps at five --
    "aegis-u0000": ("unify error envelope shapes across services", None, ["ent_env"]),
    "aegis-u0001": ("normalize the error envelope in auth services", None, []),
    "aegis-u0002": ("error envelope for gateway services", "done", []),
    "aegis-u0003": ("consistent envelope fields across worker services", None, []),
    "aegis-u0004": ("envelope version negotiation between services", None, []),
    "aegis-u0005": ("error envelope for the billing services", "done", []),
    "aegis-u0006": ("envelope schema for legacy services", None, []),
    "aegis-u0007": ("strict envelope validation in edge services", None, []),
    # -- no-neighbors: novel work, nothing should come back --
    "aegis-n0000": ("relicense the artwork attachments", None, ["ent_art"]),
}

ENTITY_PATHS = {
    "ent_e1": "src/lib.rs",
    "ent_e2": "src/weave.rs",
    "ent_e3": "src/other.rs",
    "ent_keys": "src/keys.rs",
    "ent_watch": "src/watch.rs",
    "ent_boot": "src/boot.rs",
    "ent_ovl": "src/overlay.rs",
    "ent_load": "src/gate.rs",
    "ent_lat": "src/hook.rs",
    "ent_hub": "justfile",
    "ent_docs": "docs/book.md",
    "ent_env": "src/envelope.rs",
    "ent_art": "assets/art.md",
}

# problem -> (probe id, gated?, relevant ids)
PROBLEMS: dict[str, tuple[str, bool, set[str]]] = {
    "mixed": ("aegis-p0001", True, {"aegis-a0002", "aegis-a0003", "aegis-a0004"}),
    "phrase-match": ("aegis-k0000", True, {"aegis-k0001"}),
    "term-overlap": ("aegis-w0000", True, {"aegis-w0001", "aegis-w0002"}),
    "provenance-only": ("aegis-c0000", True, {"aegis-c0001", "aegis-c0002"}),
    "single-term-fp": ("aegis-o0000", True, {"aegis-o0001"}),
    "hub-entity-trap": ("aegis-m0000", True, {"aegis-m0001"}),
    "crowded-cluster": (
        "aegis-u0000",
        True,
        {f"aegis-u000{i}" for i in range(1, 8)},
    ),
    "no-neighbors": ("aegis-n0000", True, set()),
    "mixed-collision": (
        "aegis-p0002",
        False,
        {"aegis-b0001", "aegis-b0002", "aegis-b0003", "aegis-b0004"},
    ),
    "multi-term-fp": ("aegis-a0005", False, {"aegis-pm001"}),
    "paraphrase": ("aegis-f0000", False, {"aegis-f0001"}),
}

ARMS = {
    "full": "",
    "-term-probes": "term-probes",
    "-provenance": "provenance",
    "-context": "context",
    # Included only when the embedding model bundle is present (see
    # model_dir()): on a lexical-only store the semantic source already
    # contributes nothing, so ablating it would measure nothing.
    "-semantic": "semantic",
}

# The all-MiniLM-L6-v2 ONNX bundle, mirrored by qdrant's fastembed on a host
# the sandbox proxy allows (HuggingFace's LFS CDN is not). `just e2e f1`
# fetches it best-effort; without it the eval runs lexical-only and says so.
MODEL_URL = (
    "https://storage.googleapis.com/qdrant-fastembed/"
    "sentence-transformers-all-MiniLM-L6-v2.tar.gz"
)


def model_dir() -> Path | None:
    """The model bundle dir, iff model + tokenizer + ONNX Runtime dylib are
    all present (quipu's `ort` is `load-dynamic`: the runtime ships via the
    `onnxruntime` PyPI wheel and is pointed at with `$ORT_DYLIB_PATH`)."""
    d = YUPANA_ROOT / "target" / "models" / "fast-all-MiniLM-L6-v2"
    dylib = d.parent / "libonnxruntime.so"
    ok = (d / "model.onnx").exists() and (d / "tokenizer.json").exists() and dylib.exists()
    if ok:
        import os

        os.environ["ORT_DYLIB_PATH"] = str(dylib)
    return d if ok else None


def corpus_ttl() -> str:
    lines = [
        "@prefix aegis: <http://aegis.gastown.local/ontology/> .",
        "@prefix rdfs:  <http://www.w3.org/2000/01/rdf-schema#> .",
        "",
    ]
    for entity, path in ENTITY_PATHS.items():
        lines.append(f'aegis:{entity} a aegis:CodeModule ; aegis:filePath "{path}" .')
    lines.append("")
    for iid, (label, outcome, entities) in CORPUS.items():
        node = iid.replace("-", "_")
        lines += [
            f"aegis:wi_{node} a aegis:WorkItem ;",
            f'    rdfs:label "{label}" ;',
            f'    aegis:identifier "{iid}" ;',
        ]
        if outcome:
            lines.append(f'    aegis:outcome "{outcome}" ;')
        lines.append('    aegis:sourceKind "declared" .')
        for i, entity in enumerate(entities):
            lines += [
                f"aegis:c_{node}_{i} a aegis:GitCommit ;",
                f"    aegis:implements aegis:wi_{node} ;",
                f"    aegis:modifies aegis:{entity} .",
            ]
        lines.append("")
    return "\n".join(lines)


def retrieved_ids(briefing: str, probe: str) -> set[str]:
    """The item ids the briefing surfaces as similar or related."""
    keep = []
    in_section = False
    for line in briefing.splitlines():
        if line.startswith(("Similar past work", "Related work items")):
            in_section = True
        elif line.strip() == "" or (line and not line.startswith(("-", "Similar", "Related"))):
            in_section = line.startswith(("Similar", "Related"))
        if in_section:
            keep.append(line)
    ids = set(re.findall(r"aegis-[a-z0-9]+", "\n".join(keep)))
    ids.discard(probe)
    return ids


def prf1(retrieved: set[str], relevant: set[str]) -> tuple[float, float, float]:
    if not relevant:
        # A novel item's correct answer is silence: recall is vacuously
        # perfect, and anything retrieved is pure false positive.
        return (1.0, 1.0, 1.0) if not retrieved else (0.0, 1.0, 0.0)
    tp = len(retrieved & relevant)
    precision = tp / len(retrieved) if retrieved else 0.0
    recall = tp / len(relevant)
    f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    return precision, recall, f1


def macro(scores: list[tuple[float, float, float]]) -> tuple[float, float, float]:
    if not scores:
        return (0.0, 0.0, 0.0)
    n = len(scores)
    return tuple(round(sum(s[i] for s in scores) / n, 3) for i in range(3))  # type: ignore


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--workdir", type=Path, default=None)
    ap.add_argument("--profile", choices=["release", "debug"], default="release")
    ap.add_argument("--min-f1", type=float, default=0.9, help="core macro-F1 floor")
    args = ap.parse_args()

    work = (args.workdir or YUPANA_ROOT / "target" / "e2e-f1").resolve()
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)

    rig = Rig(work, args.profile)
    rig.setup()
    rig.write_config(work_item_scope="advise")
    corpus = work / "corpus.ttl"
    corpus.write_text(corpus_ttl())
    run([rig.quipu, "knot", str(corpus), "--db", str(rig.db)])

    # Semantic arm: stage the embedding config where the server reads it and
    # backfill embeddings for the freshly knotted corpus at startup.
    model = model_dir()
    arms = dict(ARMS)
    if model:
        # Term probes auto-retire on a semantic store (see brief_sources),
        # so ablating them there measures an already-off feature; they are
        # ablation-proven by the lexical configuration of this same eval.
        arms.pop("-term-probes")
        (rig.work / ".bobbin").mkdir(exist_ok=True)
        (rig.work / ".bobbin" / "config.toml").write_text(
            "[quipu.embedding]\n"
            f'model_path = "{model / "model.onnx"}"\n'
            f'tokenizer_path = "{model / "tokenizer.json"}"\n'
            "dimension = 384\n"
        )
        log(f"semantic arm ON (model: {model})")
    else:
        arms.pop("-semantic")
        log(f"semantic arm SKIPPED — no model bundle; fetch via `just e2e f1` ({MODEL_URL})")
    rig.start_server(extra_args=["--embed-backfill"] if model else None)

    per_problem: list[dict] = []
    arm_core: dict[str, tuple[float, float, float]] = {}
    arm_hard: dict[str, tuple[float, float, float]] = {}
    arm_overall: dict[str, float] = {}
    try:
        for arm, ablate in arms.items():
            extra = {"YUPANA_BRIEF_ABLATE": ablate} if ablate else {}
            core_scores, hard_scores = [], []
            for problem, (probe, gated, relevant) in PROBLEMS.items():
                agent = f"eval-{probe}"
                rig.publish_plate(agent, probe)
                result = rig.session_start(
                    f"f1-{arm}-{probe}", tenant=agent, agent=agent, extra_env=extra
                )
                got = retrieved_ids(result["reason"], probe)
                p, r, f1 = prf1(got, relevant)
                (core_scores if gated else hard_scores).append((p, r, f1))
                per_problem.append({
                    "arm": arm,
                    "problem": problem,
                    "gated": gated,
                    "precision": round(p, 3),
                    "recall": round(r, 3),
                    "f1": round(f1, 3),
                    "retrieved": sorted(got),
                })
                if arm == "full":
                    log(
                        f"  {problem:>17}: P={p:.2f} R={r:.2f} F1={f1:.2f}  "
                        f"got {sorted(got)}"
                    )
            arm_core[arm] = macro(core_scores)
            arm_hard[arm] = macro(hard_scores)
            arm_overall[arm] = macro(core_scores + hard_scores)[2]
            log(
                f"  {arm:>13}: core macro-F1 {arm_core[arm][2]}  "
                f"hard macro-F1 {arm_hard[arm][2]}  overall {arm_overall[arm]}"
            )
    finally:
        rig.stop_server()

    full_core = arm_core["full"][2]
    failures = []
    if full_core < args.min_f1:
        failures.append(f"core macro-F1 {full_core} < {args.min_f1}")
    # Strictly-below is judged on the OVERALL macro: a source may earn its
    # place on the hard classes alone (the semantic source exists for them).
    for arm, overall in arm_overall.items():
        if arm != "full" and overall >= arm_overall["full"]:
            failures.append(
                f"ablation `{arm}` overall macro-F1 {overall} >= full "
                f"{arm_overall['full']} — the removed feature contributed "
                "nothing measurable"
            )

    lines = [
        "# Work-item briefing retrieval — per-problem F1 and ablation",
        "",
        "Core classes are gated; hard classes are the measured lexical",
        "frontier (closing them is what quipu's embedding backend is for).",
        "",
        "## Per problem (full arm)",
        "",
        "| problem | gated | precision | recall | F1 |",
        "| --- | --- | --- | --- | --- |",
    ]
    for row in per_problem:
        if row["arm"] == "full":
            lines.append(
                f"| {row['problem']} | {'yes' if row['gated'] else 'no'} | "
                f"{row['precision']} | {row['recall']} | {row['f1']} |"
            )
    lines += [
        "",
        "## Per arm (macro)",
        "",
        "| arm | core P | core R | core F1 | hard F1 | overall F1 |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for arm in arms:
        p, r, f1 = arm_core[arm]
        lines.append(
            f"| {arm} | {p} | {r} | {f1} | {arm_hard[arm][2]} | {arm_overall[arm]} |"
        )
    lines += [
        "",
        f"Gate: core full >= {args.min_f1}; every ablation's overall macro "
        "strictly below full.",
        "",
        f"Semantic arm: {'ON' if model else 'SKIPPED (no model bundle)'}.",
    ]
    lines += ["", "**FAILED:** " + "; ".join(failures)] if failures else ["", "**PASSED.**"]
    (work / "f1-report.md").write_text("\n".join(lines) + "\n")
    (work / "f1-report.json").write_text(
        json.dumps({"per_problem": per_problem, "core": arm_core, "hard": arm_hard}, indent=2)
    )
    log(f"report: {work / 'f1-report.md'}")
    for failure in failures:
        log(f"FAIL: {failure}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
