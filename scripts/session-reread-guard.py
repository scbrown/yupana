#!/usr/bin/env python3
"""Advisory replay of repeated successful text reads (aegis-sem1z).

Requires identical returned text for the exact same requested region, with the
prior result delivered before the new request. Requests without successful text
results do not establish context. Edits, possible shell writes, and recorded
compaction invalidate prior evidence. Known background-task output polling is
excluded; polling for new output is not forgetting previously read content.

This does not prove the harness retained content without an eviction marker,
and it deliberately misses subset reuse instead of accusing reads of new lines.
The offline advisory is not an automatically installed or blocking hook.

Usage: just session-guard <transcript.jsonl>
       just session-guard --selftest  # nonzero on regression
"""
import json
import sys

from session_trajectory import is_compaction, records

WHOLE_FILE = (0, float("inf"))


def region(inp):
    """Line range a Read covered. No offset/limit means the whole file."""
    off, lim = inp.get("offset"), inp.get("limit")
    if off is None and lim is None:
        return WHOLE_FILE
    start = off or 0
    return (start, start + lim) if lim else (start, float("inf"))


def overlaps(a, b):
    return a[0] < b[1] and b[0] < a[1]


def events(path):
    """Pair successful text results with requests; a request alone proves no read."""
    import hashlib

    out, pending = [], {}
    for rec in records(path):
        if rec is None:
            out.append({"kind": "compact"})
            pending.clear()
            continue
        if is_compaction(rec):
            out.append({"kind": "compact"})
            pending.clear()
            continue
        content = (rec.get("message") or {}).get("content") or []
        if not isinstance(content, list):
            continue
        for c in content:
            if not isinstance(c, dict):
                continue
            if c.get("type") == "tool_use":
                inp = c.get("input") or {}
                name, fp = c.get("name"), inp.get("file_path")
                if name == "Bash":
                    out.append({"kind": "bash", "cmd": str(inp.get("command", ""))})
                elif name in ("Edit", "Write", "NotebookEdit") and fp:
                    out.append({"kind": "edit", "path": fp})
                elif name == "Read" and fp and c.get("id"):
                    # Known background-task outputs are polled for changes,
                    # not reread because their earlier content was forgotten.
                    parts = str(fp).replace("\\", "/").split("/")
                    if "tasks" in parts and str(fp).endswith(".output"):
                        continue
                    begin = len(out)
                    out.append({"kind": "read_begin"})
                    pending[c["id"]] = (fp, region(inp), begin)
            elif c.get("type") == "tool_result":
                request = pending.pop(c.get("tool_use_id"), None)
                if request is None:
                    continue
                fp, span, begin = request
                body = c.get("content")
                if isinstance(body, list):
                    if not body or any(not isinstance(v, dict) or v.get("type") != "text"
                                       or not isinstance(v.get("text"), str) for v in body):
                        body = None
                    else:
                        body = "\n".join(v["text"] for v in body)
                if c.get("is_error") or not isinstance(body, str) or not body.strip():
                    out.append({"kind": "edit", "path": fp})  # evidence unavailable
                    continue
                digest = hashlib.sha256(body.encode()).hexdigest()
                out.append({"kind": "read", "path": fp, "region": span,
                            "digest": digest, "begin": begin})
    return out


def analyse(evs):
    """Return the redundant re-reads. seen[path] = list of live regions."""
    seen, findings = {}, []
    for i, e in enumerate(evs):
        if e["kind"] == "compact":
            seen.clear()          # everything before is no longer reliably in context
        elif e["kind"] == "edit":
            seen.pop(e["path"], None)   # content changed; a re-read is now correct
        elif e["kind"] == "bash":
            cmd = e["cmd"]
            for path in list(seen):
                base = path.rsplit("/", 1)[-1]
                if base and (base in cmd or path in cmd):
                    seen.pop(path, None)    # may have been written outside the Edit tool
        elif e["kind"] == "read":
            prior = seen.get(e["path"], [])
            # Exact regions deliberately miss some subset reuse. Overlap alone
            # cannot establish that the newly requested lines were already read.
            if any(e["region"] == p["region"] and e["digest"] == p["digest"]
                   and p["completed"] < e["begin"] for p in prior):
                findings.append({"index": i, "path": e["path"], "region": e["region"]})
            # A changed result can reflect another writer invisible to this
            # transcript. Never keep the older content as the current baseline.
            seen[e["path"]] = [{**e, "completed": i}]
    return findings


def report(path):
    f = analyse(events(path))
    if not f:
        return
    print("⚠ RE-READ CANDIDATE — identical successful text, same region")
    for x in f:
        lo, hi = x["region"]
        span = "whole file" if (lo, hi) == WHOLE_FILE else f"lines {lo}-{hi}"
        print(f"    {x['path']} ({span})")
    print("  Advisory: no intervening edit or compaction was recorded.")
    print("  Unreported context eviction cannot be ruled out. Silent after edits, compaction, and")
    print("  on a different region of the same file — those are all legitimate.")


# ── acceptance: the bead's discrimination test, executable ──────────────────
def selftest():
    next_id = iter(range(1000))

    def R(p, off=None, lim=None, body="same text", error=False):
        i = {"file_path": p}
        if off is not None:
            i["offset"] = off
        if lim is not None:
            i["limit"] = lim
        rid = str(next(next_id))
        return [{"type": "assistant", "message": {"content": [
                    {"type": "tool_use", "id": rid, "name": "Read", "input": i}]}},
                {"type": "user", "message": {"content": [
                    {"type": "tool_result", "tool_use_id": rid,
                     "content": body, "is_error": error}]}}]

    def E(p):
        return {"type": "assistant",
                "message": {"content": [{"type": "tool_use", "name": "Edit",
                                         "input": {"file_path": p, "old_string": "a",
                                                   "new_string": "b"}}]}}

    def B(cmd):
        return {"type": "assistant",
                "message": {"content": [{"type": "tool_use", "name": "Bash",
                                         "input": {"command": cmd}}]}}

    C = {"isCompactSummary": True}

    cases = [
        ("MUST FIRE   wasteful re-read, same whole file, nothing between",
         [R("/x.rs"), R("/x.rs")], 1),
        ("must stay SILENT  overlapping regions include new lines",
         [R("/x.rs", 10, 50), R("/x.rs", 30, 50)], 0),
        ("must stay SILENT  re-read after an EDIT",
         [R("/x.rs"), E("/x.rs"), R("/x.rs")], 0),
        ("must stay SILENT  re-read after COMPACTION",
         [R("/x.rs"), C, R("/x.rs")], 0),
        ("must stay SILENT  re-read of a DISJOINT region (live-data case)",
         [R("/x.rs", 44, 60), R("/x.rs", 30, 14)], 0),
        ("must stay SILENT  first read of a file",
         [R("/x.rs")], 0),
        ("must stay SILENT  different files",
         [R("/x.rs"), R("/y.rs")], 0),
        ("MUST FIRE   edit to ANOTHER file does not license a re-read",
         [R("/x.rs"), E("/y.rs"), R("/x.rs")], 1),
        ("must stay SILENT  file written via BASH between the reads (89% of real findings)",
         [R("/x.rs"), B("sed -i s/a/b/ /x.rs"), R("/x.rs")], 0),
        ("MUST FIRE   an UNRELATED bash command does not license a re-read",
         [R("/x.rs"), B("ls -la /tmp"), R("/x.rs")], 1),
    ]

    first, second = R("/x.rs"), R("/x.rs")
    cases.extend([
        ("must stay SILENT  failed prior read", [R("/x.rs", error=True), R("/x.rs")], 0),
        ("must stay SILENT  failed new read", [R("/x.rs"), R("/x.rs", error=True)], 0),
        ("must stay SILENT  externally changed content", [R("/x.rs"), R("/x.rs", body="new")], 0),
        ("must stay SILENT  parallel reads requested before either result",
         [first[0], second[0], first[1], second[1]], 0),
        ("must stay SILENT  image results",
         [R("/x.png", body=[{"type": "image"}]), R("/x.png", body=[{"type": "image"}])], 0),
        ("must stay SILENT  background task output polling",
         [R("/tmp/tasks/job.output"), R("/tmp/tasks/job.output")], 0),
        ("must stay SILENT  missing prior result", [first[0], R("/x.rs")], 0),
        ("MUST FIRE   successful matching text blocks",
         [R("/x.rs", body=[{"type": "text", "text": "x"}]),
          R("/x.rs", body=[{"type": "text", "text": "x"}])], 1),
    ])

    import tempfile, os
    npass = nfail = 0
    for name, recs, expect in cases:
        fd, tmp = tempfile.mkstemp(suffix=".jsonl")
        with os.fdopen(fd, "w") as fh:
            for group in recs:
                for r in group if isinstance(group, list) else [group]:
                    fh.write(json.dumps(r) + "\n")
        got = len(analyse(events(tmp)))
        os.unlink(tmp)
        ok = got == expect
        print(f"  {'PASS' if ok else 'FAIL'}  {name}" + ("" if ok else f"  (expected {expect}, got {got})"))
        npass, nfail = npass + ok, nfail + (not ok)
    print(f"\n  {npass} passed, {nfail} failed")
    return 0 if nfail == 0 else 1


if __name__ == "__main__":
    arg = sys.argv[1] if len(sys.argv) > 1 else "--selftest"
    if arg == "depth":
        from session_depth import main
        sys.exit(main(sys.argv[2:]))
    elif arg == "--selftest":
        sys.exit(selftest())
    else:
        report(arg)
    sys.exit(0)
