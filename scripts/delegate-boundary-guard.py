#!/usr/bin/env python3
"""Delegate guardrail (aegis-2o9eo): did this session work PAST the hand-off line?

Stiwi: "it should be able to know that you're going much further than initial
investigations to create beads, which i think is where we should draw the line."

    investigate -> understand enough to write a GOOD bead -> DELEGATE
                                                          ^ the line is HERE

THE SIGNAL IS NOT DEPTH. Deep investigation is what produced the good findings —
a four-server differential, a max_over_time that caught a real thermal event
being closed as noise. A guard that suppresses those makes the fleet worse, and
the bead says so explicitly.

The signal is WRITE-shaped work, in a repo this agent does not shepherd, AFTER a
delegable artifact already exists (a bead has been filed this session). All three
conditions, together:

  * write-shaped   -- an Edit/Write, or a git commit/push. Reading, querying and
                      measuring are never flagged, at any depth.
  * not yours      -- ownership comes from the graph (the repo-ownership map), so
                      correcting an owner is a graph write, not a code change.
  * after a bead   -- before the first bead there is nothing to delegate TO, so
                      early implementation is not this failure.

Ownership is resolved from the knowledge graph and CACHED to a file. If the graph
cannot be reached the guard reports that it could not resolve ownership and stays
SILENT — it never guesses an owner, because accusing the wrong agent of absorbing
someone's workstream is worse than missing one.

Advisory. Exit is always 0.

Usage:
  delegate-boundary-guard.py <transcript.jsonl> [--agent NAME]
  delegate-boundary-guard.py --selftest
"""
import json
import os
import re
import sys
import urllib.request

GRAPH = os.environ.get("QUIPU_SERVER", "").rstrip("/")
NS = "http://aegis.gastown.local/ontology/"
CACHE = os.path.expanduser("~/.cache/yupana-repo-owners.json")


def fetch_owner_map():
    """{repo_label: owner} from the graph. Empty dict if unreachable."""
    if not GRAPH:
        return {}
    q = ("SELECT ?who ?repo WHERE { ?who <%sowns> ?repo }" % NS)
    try:
        req = urllib.request.Request(
            GRAPH + "/query", data=json.dumps({"query": q}).encode(),
            headers={"Content-Type": "application/json"})
        rows = json.load(urllib.request.urlopen(req, timeout=15)).get("rows", [])
    except Exception:
        return {}
    out = {}
    for r in rows:
        who = str(r.get("who", "")).rsplit("/", 1)[-1]
        repo = str(r.get("repo", "")).rsplit("/", 1)[-1]
        repo = re.sub(r"^repo[_-]", "", repo).lower()
        if who and repo:
            out.setdefault(repo, who)
    if out:
        try:
            os.makedirs(os.path.dirname(CACHE), exist_ok=True)
            json.dump(out, open(CACHE, "w"))
        except Exception:
            pass
    return out


def owner_map():
    m = fetch_owner_map()
    if m:
        return m, "graph"
    try:
        return json.load(open(CACHE)), "cache"
    except Exception:
        return {}, "unavailable"


def repo_of(path):
    """Which repo/agent-clone a path belongs to.

    A crew clone belongs to the crew member whose name it carries -- editing
    another agent's clone is the same transgression as editing their repo.
    """
    m = re.search(r"/crew/([A-Za-z0-9_-]+)/", path)
    if m:
        return ("crew", m.group(1).lower())
    m = re.search(r"/gt/([A-Za-z0-9_.-]+?)(?:-wt)?/", path)
    if m:
        return ("repo", m.group(1).lower())
    return (None, None)


def trajectory(transcript):
    evs = []
    with open(transcript, errors="ignore") as fh:
        for line in fh:
            try:
                rec = json.loads(line)
            except Exception:
                continue
            if rec.get("type") != "assistant":
                continue
            for c in (rec.get("message", {}) or {}).get("content") or []:
                if not isinstance(c, dict) or c.get("type") != "tool_use":
                    continue
                nm, inp = c.get("name"), (c.get("input") or {})
                if nm in ("Edit", "Write", "NotebookEdit"):
                    fp = inp.get("file_path")
                    if fp:
                        evs.append({"kind": "write", "path": fp})
                elif nm == "Bash":
                    cmd = str(inp.get("command", ""))
                    if re.search(r"\bbd\s+create\b", cmd):
                        evs.append({"kind": "bead"})
                    if re.search(r"\bgit\s+(commit|push)\b", cmd):
                        evs.append({"kind": "write", "path": cmd, "is_cmd": True})
    return evs


def analyse(evs, agent, owners):
    """Findings: write-shaped work in another owner's territory, after a bead."""
    seen_bead = False
    findings = []
    for e in evs:
        if e["kind"] == "bead":
            seen_bead = True
            continue
        if e["kind"] != "write" or not seen_bead:
            continue
        kind, name = repo_of(e["path"])
        if not name:
            continue
        if kind == "crew":
            owner = name                      # a crew clone belongs to that agent
        else:
            owner = owners.get(name)
            if owner is None:
                continue                      # unknown ownership -> never guess
        if owner.lower() != agent.lower():
            findings.append({"path": e["path"][:100], "territory": name, "owner": owner})
    return findings


def report(transcript, agent):
    owners, src = owner_map()
    if src == "unavailable":
        print("delegate-guard: ownership could not be resolved (graph unreachable, "
              "no cache) — staying SILENT rather than guessing an owner.")
        return
    f = analyse(trajectory(transcript), agent, owners)
    if not f:
        return
    print("⚠ PAST THE DELEGATE LINE — write-shaped work in territory you do not own,")
    print("  after a bead already existed to hand off with.")
    seen = set()
    for x in f:
        k = (x["territory"], x["owner"])
        if k in seen:
            continue
        seen.add(k)
        print(f"    {x['territory']} — owned by {x['owner']}")
    print(f"  {len(f)} write action(s). Advisory: investigating deeply is never flagged;")
    print("  reads, queries and measurements at any depth are silent. This fires only on")
    print("  EDITS/COMMITS in someone else's territory once you already had a delegable bead.")


def selftest():
    def W(p):
        return {"kind": "write", "path": p}
    B = {"kind": "bead"}
    owners = {"shantytown": "arnold", "yupana": "gennaro", "bobbin": "gennaro"}
    cases = [
        ("MUST FIRE   edit in ANOTHER owner's repo after a bead",
         [B, W("/home/x/gt/shantytown/shantytown/cli.py")], "gennaro", 1),
        ("MUST FIRE   edit in ANOTHER agent's crew clone after a bead",
         [B, W("/home/x/gt/beads_aegis/crew/arnold/CLAUDE.local.md")], "sattler", 1),
        ("must stay SILENT  edit in your OWN repo after a bead",
         [B, W("/home/x/gt/yupana/scripts/z.py")], "gennaro", 0),
        ("must stay SILENT  edit in your OWN crew clone",
         [B, W("/home/x/gt/beads_aegis/crew/gennaro/notes.md")], "gennaro", 0),
        ("must stay SILENT  edit BEFORE any bead exists (nothing to delegate yet)",
         [W("/home/x/gt/shantytown/cli.py")], "gennaro", 0),
        ("must stay SILENT  deep investigation — reads/queries only, no writes",
         [B, {"kind": "read", "path": "/anything"}], "gennaro", 0),
        ("must stay SILENT  repo of UNKNOWN ownership (never guess an owner)",
         [B, W("/home/x/gt/mystery-repo/f.py")], "gennaro", 0),
        ("MUST FIRE   git commit in another owner's worktree after a bead",
         [B, W("cd /home/x/gt/shantytown-wt/s && git commit -m x")], "gennaro", 1),
    ]
    npass = nfail = 0
    for name, evs, agent, expect in cases:
        got = len(analyse(evs, agent, owners))
        ok = got == expect
        print(f"  {'PASS' if ok else 'FAIL'}  {name}" + ("" if ok else f"  (expected {expect}, got {got})"))
        npass, nfail = npass + ok, nfail + (not ok)
    print(f"\n  {npass} passed, {nfail} failed")


if __name__ == "__main__":
    args = sys.argv[1:]
    if not args or args[0] == "--selftest":
        selftest()
    else:
        agent = os.environ.get("GT_ROLE", "unknown")
        if "--agent" in args:
            agent = args[args.index("--agent") + 1]
        report(args[0], agent)
    sys.exit(0)
