# Read-time entity legend

`yupana hook post-read` annotates completed reads with entity labels from the
resident keyword index. It is off by default. This is advisory feedback, so a
missing allocation, daemon, session ID, snapshot or response means silence.
It never contacts Quipu from the hook or falls back to loading a gazetteer file.
Build with `quipu,mcp` for the hook and resident HTTP surface.

## Context allocation

The calling context pipeline must add `remaining_context_bytes` to the completed
tool payload. It reserves prior signpost/advisory output and separators first.
The legend spends at most the smaller of that remainder and 600 UTF-8 bytes,
including its relation annotations. Missing or zero allocation produces no output.
An independently installed hook therefore does not create another 600-byte budget.

```json
{
  "hook_event_name": "PostToolUse",
  "session_id": "example-session",
  "tool_name": "Read",
  "tool_input": {"file_path": "notes.md"},
  "tool_response": {"file": {"content": "build-01.example runs demo-service"}},
  "remaining_context_bytes": 200
}
```

The adapter must reserve the actual prior bytes, invoke this stage with the
remaining allocation, and combine the returned `additionalContext` into the
existing envelope. Supplying a constant 600 to every independent hook does not
establish a shared budget. Ordinary harness payloads without this adapter remain
silent, even when the enable flag is set. DP signpost, prefetch, search augmentation
and command routing stay in DP. Only its entity-legend stage is replaced during
an approved handover; avoid enabling both entity legends.

## Resident data and activation

The daemon holds first-class `keywords::KeywordIndex` data shared through
`ResidentEngine::keywords()`. Label matching is compiled once per complete
snapshot replacement. Atomic replacement makes new or renamed labels visible
to subsequent requests without per-hook compilation.

| Setting | Meaning |
| --- | --- |
| `YUPANA_READ_LEGEND=1` | Enable the advisory in this agent's environment |
| `YUPANA_KEYWORDS_NAMESPACE` | Required ontology class namespace in the daemon environment |
| `YUPANA_KEYWORDS_DICT` | Precision word list, default `/usr/share/dict/words` |
| `[yupana.quipu] enabled`, `endpoint` | Configure the daemon's background data source |
| `[yupana.serve] use_daemon`, `bind_address`, `mcp_http_port` | Configure the hook's resident connection |

The hook accepts only numeric loopback bind addresses and follows no redirects.
The background producer refreshes every five minutes, with a 60-second total
request budget. It queries the ported governed entity classes, excluding bulk
code kinds, Formula and credentials. Every class must succeed without truncation
or exceeded row bounds before replacing the index. Failure retains the prior
snapshot with its original timestamp; after ten minutes it is refused.

There is no startup file export or hourly cron requirement. Namespace omission
leaves this producer inactive. Existing daemon policy refresh is unchanged.
The keyword snapshot reports its observation time; these are projected Quipu
entity facts, rather than tree-sitter code facts.

## Matching and bounded output

The index preserves DP's case-insensitive ASCII boundary rules, filename-stem
guard, longest-first non-overlap, precision dictionary and evidence stoplist.
Multiple IRIs under one label display `ambiguous N`. At most five entity labels
are shown. Keywords are deduplicated within a session across calls and snapshot
replacements. Up to two existing directed relations between unambiguous shown
entities can use the same remaining budget; co-occurrence never creates an edge.

`Read` responses and Bash `br show` or markdown `cat`, `head`, `tail` and `sed`
responses are eligible. Failed or interrupted calls stay silent. The current
adapter supports the Claude completed-tool envelope. Other payload schemas
require their own tested translation before activation.

## Shared pipeline entry

`yupana hook post-read-pipeline` combines DP signpost and the resident legend in
one process. During an authorized handover it replaces the existing `dp signpost`
entry; do not install it alongside that entry or an independent `post-read` entry.
Source delivery does not install or activate it. Other DP hooks remain unchanged.

With `YUPANA_READ_LEGEND=1`, it disables `DP_LEGEND` only in its DP child,
runs `dp signpost`, and subtracts its actual context UTF-8 bytes plus a newline
separator from `DP_LEGEND_MAX_BYTES` (default and maximum 600). It passes that
remainder to the in-process read adapter. Exhaustion adds nothing; prior output
is preserved even if signpost alone exceeds the cap. Other DP envelope fields
survive. Empty, malformed, unavailable or over-budget legend replies preserve
DP's exact output bytes.

With the Yupana flag off, DP retains its original environment and legacy legend
flag, allowing rollback without removing the pipeline entry. `YUPANA_DP_BIN`
optionally supplies an executable path, never a shell command. `--config` selects
Yupana's configuration as on the ordinary hook. Input is bounded to 256 KiB and
accepted DP output to 64 KiB. The DP child has a one-second deadline and is killed
and reaped on timeout; the resident lookup retains its 40 ms deadline. Anonymous
temporary files avoid pipe deadlocks. Builds without `quipu` stay silent and
cannot replace a working signpost entry.

The shared allocation covers these two stages. Independent harness hooks do not
report their output bytes to this pipeline and are outside this allocation.

The resident `POST /keywords` API accepts `session_id`, `text`, `reference` and
`remaining_bytes`, and returns context, shown dedup keys, raw hit count and the
snapshot's `generated_at`. Input is bounded to 128 KiB of read text. Snapshot,
match and session capacities are bounded. Saturation refuses additional output
instead of evicting active dedup state and repeating annotations. Session state
expires after 24 hours or daemon restart, so a restart also starts a new dedup
window and must be accounted for in a continuing trial.

## Measurement and handover

Successful resident decisions emit `read_legend` events into the Yupana metrics
spool with the frozen DP scorer's `timestamp`, `session_id`, `shown`, `bytes`,
`budget` and `latency_us` fields. Unavailable resident data emits a silent
`read_legend_unknown` event. Filter this event kind before using a trial scorer;
do not mix these events into action or guard denominators.

Source tests and an isolated replay establish candidate behavior. They do not
establish an installed hook, delivery to an agent, or a valid continuing trial.
Live handover requires a functioning context allocation adapter, durable per-agent
activation, real observed delivery, and the original trial's acceptance criteria.
Keep the preceding measurement available until that handover is authorized.
