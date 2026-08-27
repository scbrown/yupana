#!/usr/bin/env python3
"""Convert yupana's metrics spool into a Dogwood/Cedar replay trace.

WHAT THIS IS FOR
================

docs/work-scoped-governance.md gates the advise -> enforce promotion on REPLAY:
"run a candidate rule over recorded action records: would this have blocked
anything that actually happened?" That is the measured false-positive rate that
turns the promotion decision from a waiting game into a computation. The repo
specified it and never built the runner.

AWS open-sourced one on 2026-08-06. Dogwood extends Cedar with temporal
conditions that read an agent's own session event history — `formerly`,
`count_within`, `count_distinct_within`, `sum_within` — and ships
`dogwood replay <policy> --policy-schema <schema> --trace <events>`, which
prints ALLOW/DENY per event. That is exactly the missing runner, and the records
this script emits are exactly its input.

WHY OFFLINE ONLY, AND NOT AS AN ENFORCEMENT ENGINE
==================================================

Dogwood's own README is explicit that the reference interpreter is not for
production, and its stated weaknesses are this estate's three founding failure
classes almost word for word: it "accepts timestamps as provided and does not
validate them", it has "no authentication mechanism for events", and field
inconsistencies SILENTLY WEAKEN CHECKS. A system whose motivating incidents are
unattributable action, unattributable message, and a control that could not fail
cannot put a silently-weakening component on the enforcement path.

Offline is where that caveat costs nothing. The trace is our own spool, read
from our own disk; timestamp integrity and event authenticity are our problem
either way, and no agent's edit waits on the answer.

There is a second reason, and it is the deciding one: docs/design/paper.md
states that a policy is never DEFINED in yupana — authoring, SHACL validation
and verdict signing stay in quipu. Adopting Dogwood as the authoring language
would either break that or add a second authoring surface beside aegis:Policy.
Using it as an ANALYSIS tool over records breaks nothing.

THE MAPPING, AND ITS HONEST LIMITS
==================================

    spool kind      Dogwood action
    guard           Yupana::Action::"Edit"
    action          Yupana::Action::"<Verb>"     (from the resolved verb)
    audit           Yupana::Action::"Audit"

`session` is the temporal grouping key and `item` the work item — the two fields
the spool did not carry until this pass, and precisely why replay was not
runnable before. A record missing `session` CANNOT be placed in a window, so it
is dropped and COUNTED; the count is printed, because a replay that silently
discarded half its input would report a false-positive rate against a corpus
nobody could see.

Every dropped-record reason is reported separately. "No session" and
"unparseable line" need opposite fixes, and a single 'skipped' number would hide
which one is happening.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from collections import Counter
from pathlib import Path

# The spool kinds that describe an ACTION an agent took. Other kinds
# (`fail_open`, `served_from_cache`, `governed`, `scope`) describe the guard's
# own state rather than an agent's action, and replaying them as actions would
# invent traffic that never happened.
ACTION_KINDS = {"guard", "action", "audit"}


def spool_path(explicit: str | None) -> Path | None:
    """Where yupana's metrics spool lives — the same precedence
    `crate::metrics::resolve_path` implements, in the same order.

    Duplicated deliberately rather than shelled out of the binary: this script
    must be runnable against a spool copied off a host that has no yupana
    installed, which is the ordinary case when analysing an incident.
    """
    if explicit:
        return Path(explicit)
    if xdg := os.environ.get("XDG_STATE_HOME"):
        return Path(xdg) / "yupana" / "metrics.jsonl"
    if home := os.environ.get("HOME"):
        return Path(home) / ".local" / "state" / "yupana" / "metrics.jsonl"
    return None


def action_name(rec: dict) -> str | None:
    """The qualified Dogwood action for a spool record, or None to drop it."""
    kind = rec.get("kind")
    if kind == "guard":
        return 'Yupana::Action::"Edit"'
    if kind == "audit":
        return 'Yupana::Action::"Audit"'
    if kind == "action":
        verb = rec.get("verb")
        # ABSTAIN, NEVER GUESS — the resolver's rule, carried through. A record
        # whose verb the resolver declined to name must not be replayed under an
        # invented one: a rule derived from it would cite evidence that does not
        # exist.
        if not verb:
            return None
        return f'Yupana::Action::"{verb.capitalize()}"'
    return None


def cedar_string(value: object) -> str:
    """Render a value as a Cedar/Dogwood string literal."""
    return json.dumps(str(value), ensure_ascii=False)


def input_fields(rec: dict) -> dict[str, str]:
    """Policy-visible fields, absent when the spool did not resolve them."""
    keys = ("session", "item", "tenant", "result", "target_class", "rule", "tool", "mode")
    return {key: str(rec[key]) for key in keys if rec.get(key) is not None}


def record_literal(fields: dict[str, str]) -> str:
    return "{ " + ", ".join(f"{key}: {cedar_string(value)}" for key, value in fields.items()) + " }"


def convert(lines, out) -> Counter:
    """Write one Dogwood `.log` event per replayable record. Returns the drop tally."""
    tally: Counter = Counter()
    for line in lines:
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except ValueError:
            tally["unparseable"] += 1
            continue
        if rec.get("kind") not in ACTION_KINDS:
            tally["not-an-action"] += 1
            continue
        action = action_name(rec)
        if action is None:
            tally["resolver-abstained"] += 1
            continue
        session = rec.get("session")
        if not session:
            # Not placeable in a temporal window. Counted, never quietly
            # dropped: a replay whose corpus silently shrank would report a
            # false-positive rate against traffic nobody could inspect.
            tally["no-session"] += 1
            continue
        ts = rec.get("ts")
        if ts is None:
            tally["no-timestamp"] += 1
            continue

        fields = input_fields(rec)
        inputs = record_literal(fields)
        principal = f'Yupana::Agent::{cedar_string(rec.get("agent") or "unknown")}'
        resource = f'Yupana::Target::{cedar_string(rec.get("target") or rec.get("path") or "unknown")}'
        request_id = cedar_string(f"{session}-{tally['written']}")
        # Mirror input into both bags. Dogwood's reference interpreter warns
        # that supplying a field to only the request or logged bag silently
        # weakens Cedar or temporal checks respectively.
        out.write(
            f"@{ts} scope(principal: {principal}, resource: {resource}) "
            f"request_context(input: {inputs}) {action}::request("
            f"input: {inputs}, callerPrincipal: {principal}, "
            f"callerResource: {resource}, requestId: {request_id})\n"
        )
        tally["written"] += 1
    return tally


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--spool", help="metrics.jsonl (default: the usual precedence)")
    ap.add_argument("--out", help="trace file to write (default: stdout)")
    args = ap.parse_args()

    path = spool_path(args.spool)
    if path is None or not path.exists():
        print(f"no spool at {path or '<unresolvable>'} — nothing to replay", file=sys.stderr)
        return 3          # COULD NOT LOOK, distinct from "read it, it was empty"
    with path.open() as fh:
        lines = fh.readlines()

    if args.out:
        with open(args.out, "w") as out:
            tally = convert(lines, out)
    else:
        tally = convert(lines, sys.stdout)

    # THE DENOMINATOR IS THE POINT. A replay that reported only what it kept
    # would let a converter covering 5% of traffic look identical to one
    # covering 95% — the same reason pre_bash writes a record for its
    # abstentions.
    print(
        f"{tally['written']} event(s) written from {len(lines)} spool line(s)",
        file=sys.stderr,
    )
    for reason in sorted(k for k in tally if k != "written"):
        print(f"  dropped, {reason}: {tally[reason]}", file=sys.stderr)
    if tally["no-session"]:
        print(
            "  NOTE: records with no `session` predate the field and cannot be "
            "placed in a temporal window. They are not replayable, and a rule "
            "measured without them is measured against a smaller corpus than "
            "the fleet actually produced.",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
