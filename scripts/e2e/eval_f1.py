#!/usr/bin/env python3
"""Retrieval eval for the work-item briefing: F1 with an ablation study.

Builds a labeled corpus of work items in a fresh quipu store — two probe
items, each with a relevance-judged cluster reachable through DIFFERENT
retrieval mechanisms (whole-phrase match, distinctive-term probes,
provenance co-occurrence) plus term-collision distractors that punish
precision — then runs the SHIPPED `yupana hook session-start` binary per
probe and scores the item ids the briefing surfaces against the judgments.

The ablation arms re-run the same binary with one retrieval source removed
via `$YUPANA_BRIEF_ABLATE` (feature removal, not a reimplementation):

  full            all sources
  -term-probes    /context queried with the full label only
  -provenance     co-occurrence (related items) off
  -context        the /context pipeline off entirely

The gate: full-arm macro-F1 >= --min-f1 (default 0.85), and every ablation
arm strictly below full — each feature has to EARN its place by measurably
hurting when removed.

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
# The labeled corpus. Ids share the `aegis-` prefix; entities carry filePath
# so provenance also exercises the observed-scope plumbing.
#
# Probe 1 cluster (grounding): each relevant item is reachable by exactly the
# mechanism named, so each ablation loses a known judgment:
#   a2  phrase   label contains probe 1's label verbatim
#   a3  term     shares distinctive terms (grounded/boundary), no phrase, no edge
#   a4  prov     shares entity E1 with the probe, label lexically disjoint
#   b1  --       DISTRACTOR for probe 1 ("boundary" collision), relevant to probe 2
# Probe 2 cluster (weaving) mirrors it: b3 phrase, b1+b2 term, b4 prov,
# a5 distractor ("pattern"/"cache" collision).
# ---------------------------------------------------------------------------

PROBES = {
    "aegis-p0001": {
        "label": "prove the grounded edit boundary",
        "agent": "eval-p1",
        "relevant": {"aegis-a0002", "aegis-a0003", "aegis-a0004"},
    },
    "aegis-p0002": {
        "label": "weave pattern cache for the loom",
        "agent": "eval-p2",
        "relevant": {"aegis-b0001", "aegis-b0002", "aegis-b0003", "aegis-b0004"},
    },
}

ITEMS = {
    "aegis-a0002": ("prove the grounded edit boundary for tallies", "done", None),
    "aegis-a0003": ("extend grounded boundary checks to imports", "done", "ent_e3"),
    "aegis-a0004": ("reject fabricated citations at the edit seam", None, "ent_e1"),
    "aegis-a0005": ("pattern matching in the policy cache", None, "ent_e3"),
    "aegis-b0001": ("boundary conditions for weave tension", None, "ent_e2"),
    "aegis-b0002": ("cache the loom pattern weft", "done", None),
    "aegis-b0003": ("weave pattern cache for the loom shuttle", "done", None),
    "aegis-b0004": ("speed up shuttle threading", None, "ent_e2"),
}

ARMS = {
    "full": "",
    "-term-probes": "term-probes",
    "-provenance": "provenance",
    "-context": "context",
}


def corpus_ttl() -> str:
    lines = [
        "@prefix aegis: <http://aegis.gastown.local/ontology/> .",
        "@prefix rdfs:  <http://www.w3.org/2000/01/rdf-schema#> .",
        "",
        'aegis:ent_e1 a aegis:CodeModule ; aegis:filePath "src/lib.rs" .',
        'aegis:ent_e2 a aegis:CodeModule ; aegis:filePath "src/weave.rs" .',
        'aegis:ent_e3 a aegis:CodeModule ; aegis:filePath "src/other.rs" .',
        "",
    ]
    probe_entity = {"aegis-p0001": "ent_e1", "aegis-p0002": "ent_e2"}
    for pid, spec in PROBES.items():
        node = pid.replace("-", "_")
        lines += [
            f"aegis:wi_{node} a aegis:WorkItem ;",
            f'    rdfs:label "{spec["label"]}" ;',
            f'    aegis:identifier "{pid}" ;',
            '    aegis:sourceKind "declared" .',
            f"aegis:c_{node} a aegis:GitCommit ;",
            f"    aegis:implements aegis:wi_{node} ;",
            f"    aegis:modifies aegis:{probe_entity[pid]} .",
            "",
        ]
    for iid, (label, outcome, entity) in ITEMS.items():
        node = iid.replace("-", "_")
        lines += [
            f"aegis:wi_{node} a aegis:WorkItem ;",
            f'    rdfs:label "{label}" ;',
            f'    aegis:identifier "{iid}" ;',
        ]
        if outcome:
            lines.append(f'    aegis:outcome "{outcome}" ;')
        lines.append('    aegis:sourceKind "declared" .')
        if entity:
            lines += [
                f"aegis:c_{node} a aegis:GitCommit ;",
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
    tp = len(retrieved & relevant)
    precision = tp / len(retrieved) if retrieved else 0.0
    recall = tp / len(relevant) if relevant else 0.0
    f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    return precision, recall, f1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--workdir", type=Path, default=None)
    ap.add_argument("--profile", choices=["release", "debug"], default="release")
    ap.add_argument("--min-f1", type=float, default=0.85)
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
    rig.start_server()

    rows = []
    try:
        for arm, ablate in ARMS.items():
            extra = {"YUPANA_BRIEF_ABLATE": ablate} if ablate else {}
            scores = []
            for probe, spec in PROBES.items():
                rig.publish_plate(spec["agent"], probe)
                # A fresh projection cache per invocation would hide nothing
                # here, but a STALE one would: the scope map is projected per
                # run and the corpus never changes mid-eval.
                result = rig.session_start(
                    f"f1-{arm}-{probe}", tenant=spec["agent"], agent=spec["agent"],
                    extra_env=extra,
                )
                got = retrieved_ids(result["reason"], probe)
                p, r, f1 = prf1(got, spec["relevant"])
                scores.append((p, r, f1))
                log(
                    f"  {arm:>13} {probe}: got {sorted(got)} "
                    f"P={p:.2f} R={r:.2f} F1={f1:.2f}"
                )
            macro = [sum(s[i] for s in scores) / len(scores) for i in range(3)]
            rows.append({
                "arm": arm,
                "precision": round(macro[0], 3),
                "recall": round(macro[1], 3),
                "f1": round(macro[2], 3),
            })
    finally:
        rig.stop_server()

    full_f1 = next(r["f1"] for r in rows if r["arm"] == "full")
    failures = []
    if full_f1 < args.min_f1:
        failures.append(f"full-arm macro-F1 {full_f1} < {args.min_f1}")
    for r in rows:
        if r["arm"] != "full" and r["f1"] >= full_f1:
            failures.append(
                f"ablation `{r['arm']}` scored {r['f1']} >= full {full_f1} — "
                "the removed feature contributed nothing measurable"
            )

    lines = [
        "# Work-item briefing retrieval — F1 and ablation",
        "",
        "Macro-averaged over the labeled probes; each ablation arm re-runs the",
        "shipped binary with one retrieval source removed (`YUPANA_BRIEF_ABLATE`).",
        "",
        "| arm | precision | recall | F1 |",
        "| --- | --- | --- | --- |",
    ]
    for r in rows:
        lines.append(f"| {r['arm']} | {r['precision']} | {r['recall']} | {r['f1']} |")
    lines += ["", f"Gate: full >= {args.min_f1} and every ablation strictly below full."]
    lines += ["", "**FAILED:** " + "; ".join(failures)] if failures else ["", "**PASSED.**"]
    (work / "f1-report.md").write_text("\n".join(lines) + "\n")
    (work / "f1-report.json").write_text(json.dumps(rows, indent=2))
    log(f"report: {work / 'f1-report.md'}")
    for failure in failures:
        log(f"FAIL: {failure}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
