#!/usr/bin/env bash
# probe-plate-session-scope.sh — does a trace record carry THIS session's work
# item, or whatever the plate happens to say? (aegis-1mp1ls, aegis-368cu.7)
#
# WHY A PROBE AND NOT ONLY A UNIT TEST. The unit tests around
# `pre_bash::action_record_fields` / `post_bash::outcome_fields` prove that the
# `item` field is always CLAIMED — that is the half a pure test can reach, and
# it is the half the unscoped fallback in `metrics::emit` defeats. They cannot
# prove the OTHER half: that the session actually threaded from the hook payload
# to `plate::current`. That is process-level (env + filesystem + a real payload),
# so it is measured here, against a BINARY, exactly as the bead requires.
#
# Usage:  scripts/probe-plate-session-scope.sh [path-to-yupana]        (default: yupana on PATH)
#
# Run it against the PRE-FIX binary too: arm 2 must FAIL there. A probe that has
# only ever been run against the fixed build cannot tell a working guard from an
# inert one — which is precisely how aegis-368cu.7's guard shipped inert.
#
# Exit: 0 all arms as expected · 1 an arm failed · 2 could not run the probe.
set -uo pipefail

BIN="${1:-$(command -v yupana || command -v hank)}"
[ -x "$BIN" ] || { echo "FATAL: no yupana binary (got '${BIN:-}')"; exit 2; }

PLATE="${SHANTY_ROOT:-}/crew/${SHANTY_AGENT:-}/plate.json"
[ -r "$PLATE" ] || { echo "FATAL: no readable plate at '$PLATE' — set SHANTY_ROOT/SHANTY_AGENT"; exit 2; }

# THE CONTROL THE BEAD ASKS FOR. 368cu.7's first attempt read a 2.5h-old plate,
# so what it measured as an abstention was STALENESS, not session scope — the
# guard would have looked identical had it been absent. Refuse to run on a plate
# old enough for the age backstop to be doing the work.
now=$(date +%s)
at=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("at",0))' "$PLATE")
age=$(( now - at ))
if [ "$age" -gt 600 ]; then
  echo "FATAL: plate is ${age}s old. Re-anchor (\`st anchor \$SHANTY_AGENT\`) first:"
  echo "       on a stale plate every arm abstains for the WRONG reason and the probe proves nothing."
  exit 2
fi
STORED=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("session") or "")' "$PLATE")
ITEM=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("item") or "")' "$PLATE")
[ -n "$ITEM" ] || { echo "FATAL: plate carries no item; nothing to attribute"; exit 2; }

echo "binary : $BIN ($("$BIN" --version 2>&1))"
echo "plate  : $PLATE"
echo "         item=$ITEM  session=${STORED:-null}  age=${age}s"
echo

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
OTHER="00000000-dead-4dea-dead-000000000000"

# Emit one record through the real hook and hand back its `item` field:
# "<absent>" when the key is not there at all, which is the abstention shape.
# $4, when given, overrides SHANTY_ROOT so an arm can present a differently
# stamped plate without touching the real one.
emit() {  # $1 hook (pre-bash|post-bash), $2 session_id or "" for none, $3 label, [$4 root]
  local spool="$TMP/$3.jsonl" payload
  payload=$(python3 - "$2" "$1" <<'PY'
import json,sys
sid, hook = sys.argv[1], sys.argv[2]
o = {"tool_name":"Bash","tool_input":{"command":"ssh build-01 uptime"},
     "tool_use_id":"toolu_probe1mp1ls",
     "hook_event_name":"PreToolUse" if hook=="pre-bash" else "PostToolUse",
     "tool_response":{}}
if sid: o["session_id"] = sid
print(json.dumps(o))
PY
)
  printf '%s' "$payload" | SHANTY_ROOT="${4:-$SHANTY_ROOT}" YUPANA_METRICS_PATH="$spool" \
    "$BIN" hook "$1" >/dev/null 2>&1
  python3 - "$spool" <<'PY'
import json,sys
try:
    rows=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]
except OSError:
    print("<no-spool>"); raise SystemExit
rows=[r for r in rows if r.get("kind") in ("action","action_outcome")]
print(rows[-1].get("item","<absent>") if rows else "<no-record>")
PY
}

rc=0
check() {  # $1 arm, $2 expected, $3 actual, $4 why
  if [ "$3" = "$2" ]; then printf 'PASS  %-46s item=%-22s %s\n' "$1" "$3" "$4"
  else printf 'FAIL  %-46s item=%-22s expected %s — %s\n' "$1" "$3" "$2" "$4"; rc=1; fi
}

if [ -n "$STORED" ]; then
  # ARM 1 — the plate's own session. The positive control: without it, arm 2's
  # abstention is indistinguishable from a plate that could never be read.
  check "1 my session"        "$ITEM"     "$(emit pre-bash  "$STORED" a1)"  "the reader IS the stamper"
  check "1 my session (post)" "$ITEM"     "$(emit post-bash "$STORED" a1p)" "outcome rows too"
  # ARM 2 — THE ONE THAT MATTERS. A plate stamped by a session that has since
  # died must not attribute this action. Omitted, never null, never stale.
  check "2 different session" "<absent>"  "$(emit pre-bash  "$OTHER" a2)"   "dead session's item must NOT ride"
  check "2 different (post)"  "<absent>"  "$(emit post-bash "$OTHER" a2p)"  "the outcome inherits it too"
else
  echo "SKIP  arms 1-2: plate carries no session (dispatcher-written)."
  echo "      Re-anchor in your own session to exercise them: st anchor \$SHANTY_AGENT"
  rc=2
fi

# ARM 3 — a DISPATCHER plate stores null, meaning "not session-scoped", and must
# STILL be read. Rejecting it would make every dispatched plate unreadable, and
# dispatch writes most plates: the guard would become an attribution outage.
D="$TMP/dispatched"; mkdir -p "$D/crew/${SHANTY_AGENT}"
python3 - "$PLATE" "$D/crew/${SHANTY_AGENT}/plate.json" <<'PY'
import json,sys
d=json.load(open(sys.argv[1])); d["session"]=None
json.dump(d,open(sys.argv[2],"w"))
PY
a3=$(emit pre-bash "$OTHER" a3 "$D")
check "3 dispatcher plate (session null)" "$ITEM" "$a3" "null means not-scoped and is still read"

echo
case $rc in
  0) echo "ALL ARMS AS EXPECTED. On a PRE-FIX binary arm 2 must FAIL — run it there too." ;;
  1) echo "AN ARM FAILED. On a pre-fix binary that is the expected negative result." ;;
  2) echo "INCONCLUSIVE — see SKIP above." ;;
esac
exit $rc
