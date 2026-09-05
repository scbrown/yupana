#!/usr/bin/env python3
"""Guard-latency percentiles from the yupana metrics spool (aegis-x894x2).

The success measure for the resident-daemon work is spool p90/p99 BEFORE vs
AFTER. That is only meaningful if both numbers are produced the same way, so
the method lives here rather than in two hand-typed shell pipelines.

Two properties this file exists to guarantee:

* **Malformed lines are recovered, not silently dropped.** The spool is
  appended by ~10 concurrent agents. `jq` ABORTS at the first bad line, so a
  naive `jq` pipeline silently reports only the rows BEFORE it — measured
  2026-09-05, that read 265 guard rows out of a true 771 and would have
  produced a confident, wrong baseline. Every reader of this spool needs the
  recovering parse.
* **A window is stated, never assumed.** `--since` makes the AFTER sample an
  explicit slice rather than the whole accumulating file, which would be
  contaminated by the BEFORE rows it is meant to be compared against.
"""

import argparse
import json
import sys
from datetime import datetime, timezone

DEC = json.JSONDecoder()


def records(path):
    """Yield every recoverable record, counting what could not be recovered.

    Returns (records, stats). Two distinct corruption shapes, which must not be
    conflated: a MISSING NEWLINE leaves both records intact and recoverable,
    while a TORN write has genuinely lost bytes. Reporting them as one number
    would hide whether the spool is lossy or merely untidy.
    """
    recs, stats = [], {"lines": 0, "clean": 0, "missing_newline": 0, "torn": 0}
    with open(path, errors="replace") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            stats["lines"] += 1
            try:
                recs.append(json.loads(line))
                stats["clean"] += 1
                continue
            except Exception:
                pass
            pos, got, whole = 0, [], True
            while pos < len(line):
                try:
                    obj, pos = DEC.raw_decode(line, pos)
                    got.append(obj)
                except Exception:
                    whole = False
                    break
            recs.extend(got)
            stats["missing_newline" if whole and len(got) > 1 else "torn"] += 1
    return recs, stats


def pct(sorted_vals, q):
    """Nearest-rank percentile. Stated explicitly so BEFORE and AFTER cannot
    differ by interpolation choice."""
    if not sorted_vals:
        return None
    idx = min(len(sorted_vals) - 1, max(0, int(round(q * len(sorted_vals) + 0.5)) - 1))
    return sorted_vals[idx]


def ts(v):
    return datetime.fromtimestamp(v, timezone.utc).strftime("%Y-%m-%d %H:%M:%SZ")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("spool", nargs="?",
                    default=f"{sys.path[0] and ''}{__import__('os').path.expanduser('~')}"
                            "/.local/state/yupana/metrics.jsonl")
    ap.add_argument("--since", type=int, default=None,
                    help="only rows with ts > this epoch second (the AFTER window)")
    ap.add_argument("--until", type=int, default=None)
    ap.add_argument("--label", default="spool")
    args = ap.parse_args()

    recs, stats = records(args.spool)

    guards = [r for r in recs if r.get("kind") == "guard" and isinstance(r.get("duration_ms"), (int, float))]
    if args.since is not None:
        guards = [r for r in guards if r.get("ts", 0) > args.since]
    if args.until is not None:
        guards = [r for r in guards if r.get("ts", 0) <= args.until]

    print(f"=== {args.label} :: {args.spool}")
    print(f"lines={stats['lines']} clean={stats['clean']} "
          f"missing_newline={stats['missing_newline']} torn={stats['torn']}")
    if stats["torn"]:
        print(f"  !! {stats['torn']} torn line(s): bytes genuinely lost, not just untidy")

    if not guards:
        print("no guard rows with duration_ms in window — REPORT THIS, do not read it as 0ms")
        return 1

    ds = sorted(r["duration_ms"] for r in guards)
    tss = [r.get("ts", 0) for r in guards if r.get("ts")]
    print(f"window   : {ts(min(tss))} .. {ts(max(tss))}" if tss else "window   : unknown")
    print(f"kind:guard duration_ms  n={len(ds)}")
    print(f"  p50={pct(ds,.50)}ms  p90={pct(ds,.90)}ms  p99={pct(ds,.99)}ms  "
          f"max={ds[-1]}ms  mean={sum(ds)/len(ds):.0f}ms")

    # The tail's attribution is the whole thesis of the bead: if the fail-opens
    # are not projection, a projection daemon is the wrong fix.
    fo = [r for r in recs if r.get("kind") == "fail_open"]
    if args.since is not None:
        fo = [r for r in fo if r.get("ts", 0) > args.since]
    if fo:
        by = {}
        for r in fo:
            k = r.get("fail_kind", "unknown")
            by[k] = by.get(k, 0) + 1
        print(f"fail_open n={len(fo)}  by fail_kind: {by}")

    fresh = {}
    for r in guards:
        f = r.get("policy_freshness")
        if f:
            fresh.setdefault(f, []).append(r["duration_ms"])
    for f, v in sorted(fresh.items()):
        v.sort()
        print(f"  policy_freshness={f:8s} n={len(v):4d} p50={pct(v,.50)}ms p90={pct(v,.90)}ms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
