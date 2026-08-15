#!/usr/bin/env python3
"""Concurrency benchmark: N agents sharing one Yupana against one Quipu.

Answers the capacity question for the fast-plane/slow-plane split: each
simulated agent is what a real deployment runs — a `yupana hook pre-edit`
process per edit, every invocation projecting governed policy from the one
shared quipu-server. The sweep raises the number of concurrent agents and
measures, per level:

  - guard latency (p50 / p95 / max) and aggregate edits/second
  - how the projection was served: LIVE from quipu, from the durable CACHE
    (still enforced, age declared), or FAIL-OPEN (unguarded, loud)

The serving-source split is the honest capacity signal. Quipu serves /query
effectively one at a time, so as N grows the guard degrades *by design*:
live -> cached -> fail-open, never a silent wrong answer. "How many agents
can work at once" therefore has two answers, both reported: how many keep
fully-live projections, and how many stay enforced at all (live + cached).

Usage: scripts/e2e/bench.py [--levels 1,2,4,8,16,32] [--edits 10]
Run via `just e2e bench`.
"""

from __future__ import annotations

import argparse
import json
import shutil
import statistics
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from harness import (  # noqa: E402
    ANCHOR,
    FAKE_ITEM,
    REAL_ITEM_2,
    YUPANA_ROOT,
    Rig,
    log,
)

CLEAN_EDIT = (
    "pub fn caller() -> u32 {\n"
    f"    // implements {REAL_ITEM_2}\n"
    "    tally(4)\n"
    "}"
)
FABRICATED_EDIT = (
    "pub fn caller() -> u32 {\n"
    f"    // implements {FAKE_ITEM}\n"
    "    tally(5)\n"
    "}"
)


def agent_loop(rig: Rig, agent: int, edits: int) -> list[dict]:
    """One agent: `edits` sequential pre-edit guards under its own tenant id."""
    results = []
    env = rig.hook_env()
    env["BOBBIN_ROLE"] = f"agent-{agent}"
    for i in range(edits):
        # 1 in 5 edits cites a fabricated work item — enough denials to keep
        # the signing/spool path in the measurement without flooding it.
        new = FABRICATED_EDIT if i % 5 == 4 else CLEAN_EDIT
        payload = json.dumps(
            {
                "session_id": f"bench-{agent}",
                "cwd": str(rig.repo),
                "tool_name": "Edit",
                "tool_input": {
                    "file_path": str(rig.repo / "src" / "lib.rs"),
                    "old_string": ANCHOR,
                    "new_string": new,
                },
            }
        )
        started = time.time()
        proc = subprocess.run(
            [str(rig.yupana), "hook", "pre-edit"],
            input=payload,
            capture_output=True,
            text=True,
            cwd=rig.repo,
            env=env,
            timeout=120,
        )
        results.append(
            {
                "agent": agent,
                "ms": (time.time() - started) * 1000,
                "deny": '"deny"' in proc.stdout,
                "exit": proc.returncode,
            }
        )
    return results


def metrics_since(rig: Rig, offset: int) -> tuple[dict[str, int], int]:
    """Event-kind counts appended to the metrics spool past `offset` bytes."""
    path = rig.state / "metrics.jsonl"
    counts: dict[str, int] = {}
    if not path.exists():
        return counts, offset
    data = path.read_bytes()
    for line in data[offset:].decode(errors="replace").splitlines():
        try:
            kind = json.loads(line).get("kind", "?")
        except json.JSONDecodeError:
            continue
        counts[kind] = counts.get(kind, 0) + 1
    return counts, len(data)


def run_level(rig: Rig, n: int, edits: int, offset: int) -> tuple[dict, int]:
    started = time.time()
    with ThreadPoolExecutor(max_workers=n) as pool:
        batches = list(pool.map(lambda a: agent_loop(rig, a, edits), range(n)))
    wall = time.time() - started
    lat = sorted(r["ms"] for batch in batches for r in batch)
    total = len(lat)
    counts, offset = metrics_since(rig, offset)
    guards = counts.get("guard", 0)
    cached = counts.get("served_from_cache", 0)
    failed = counts.get("fail_open", 0)
    live = max(guards - cached - failed, 0)
    row = {
        "agents": n,
        "edits": total,
        "wall_s": round(wall, 2),
        "edits_per_s": round(total / wall, 2),
        "p50_ms": round(statistics.median(lat), 1),
        "p95_ms": round(lat[int(0.95 * (total - 1))], 1),
        "max_ms": round(lat[-1], 1),
        "denied": sum(r["deny"] for batch in batches for r in batch),
        "live": live,
        "cached": cached,
        "fail_open": failed,
    }
    log(
        f"  N={n:>3}: {row['edits_per_s']:>6} edits/s  "
        f"p50={row['p50_ms']}ms p95={row['p95_ms']}ms max={row['max_ms']}ms  "
        f"live/cached/fail-open = {live}/{cached}/{failed}"
    )
    return row, offset


def write_report(work: Path, rows: list[dict], edits: int) -> None:
    (work / "bench-report.json").write_text(json.dumps(rows, indent=2))
    lines = [
        "# Shared Yupana against one Quipu — concurrency sweep",
        "",
        f"Each agent issues {edits} sequential pre-edit guards (a process per "
        "edit, as deployed); every guard projects governed policy from the one "
        "quipu-server. `live/cached/fail-open` is how the projection was served "
        "— cached is still enforced (age declared in the verdict); fail-open is "
        "an unguarded edit, loudly reported.",
        "",
        "| agents | edits/s | p50 | p95 | max | denied | live | cached | fail-open |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for r in rows:
        lines.append(
            f"| {r['agents']} | {r['edits_per_s']} | {r['p50_ms']}ms | "
            f"{r['p95_ms']}ms | {r['max_ms']}ms | {r['denied']} | "
            f"{r['live']} | {r['cached']} | {r['fail_open']} |"
        )
    lines.append("")
    (work / "bench-report.md").write_text("\n".join(lines))
    log(f"report: {work / 'bench-report.md'}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--levels", default="1,2,4,8,16,32")
    ap.add_argument("--edits", type=int, default=10, help="edits per agent")
    ap.add_argument("--workdir", type=Path, default=None)
    ap.add_argument("--profile", choices=["release", "debug"], default="release")
    args = ap.parse_args()

    levels = [int(x) for x in args.levels.split(",")]
    work = args.workdir or YUPANA_ROOT / "target" / "e2e-bench"
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)

    rig = Rig(work, args.profile)
    rig.setup()
    log("seeding quipu store")
    rig.seed()
    rig.start_server()
    try:
        # Warm-up: one guard, so first-run costs (key checks, base-graph build
        # cold caches) don't land inside the N=1 row.
        agent_loop(rig, 0, 1)
        offset = (
            len((rig.state / "metrics.jsonl").read_bytes())
            if (rig.state / "metrics.jsonl").exists()
            else 0
        )
        rows = []
        for n in levels:
            row, offset = run_level(rig, n, args.edits, offset)
            rows.append(row)
            rig.scrape_metrics(f"n{n}")
    finally:
        rig.stop_server()
    write_report(work, rows, args.edits)
    return 0


if __name__ == "__main__":
    sys.exit(main())
