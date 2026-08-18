# Replay: measuring a rule before it refuses anything

The promotion ladder in [work-scoped governance] gates the move from `advise` to
`enforce` on evidence, not on a date: *would this rule have blocked anything that
actually happened?* That question turns the decision from a waiting game into a
computation, and answering it needs two things — a corpus of recorded actions,
and a runner that replays a candidate rule over it.

Yupana has had the corpus design since the trace phase. It did not have the
records (the `pre-bash` hook was wired nowhere, and the `guard` records carried
no `session` to group by), and it never had the runner.

Both now exist.

## The runner is Dogwood, and it is used offline only

AWS open-sourced [Dogwood] on 2026-08-06: Cedar extended with temporal
conditions that read an agent's own session event history — `formerly`,
`count_within`, `count_distinct_within`, `sum_within` — plus

```bash
dogwood replay policy.dw --policy-schema schema.cedarschema --trace events.log
```

which prints ALLOW/DENY per event. That is the missing runner.

**It is not adopted as an enforcement engine, and the reasons are worth stating
rather than assuming.**

Its own README says the reference interpreter is not for production, and the
specific weaknesses it names are this estate's founding failure classes almost
word for word: it *"accepts timestamps as provided and does not validate them"*,
it has *"no authentication mechanism for events"*, and field inconsistencies
**silently weaken checks**. A system built around unattributable action,
unattributable message, and a control that could not fail cannot put a
silently-weakening component on the path that refuses things.

Offline, that caveat costs nothing: the trace is our own spool read from our own
disk, timestamp integrity is our problem either way, and no agent's edit waits
on the answer.

There is a second reason and it is the deciding one. [The paper plan] states
that a policy is never *defined* in yupana — authoring, SHACL validation and
verdict signing stay in quipu. Adopting Dogwood as the authoring language would
either break that or add a second authoring surface beside `aegis:Policy`. Using
it to *analyse records* breaks nothing.

## Running it

```bash
# 1. Convert the spool. Reads $YUPANA_METRICS_PATH / $XDG_STATE_HOME / $HOME
#    in the same order the emitter writes.
scripts/spool-to-dogwood.py --out /tmp/trace.log

# 2. Replay a candidate rule.
dogwood replay candidate.dw --policy-schema yupana.cedarschema --trace /tmp/trace.log
```

The converter maps `guard` records to `Yupana::Action::"Edit"`, `action` records
to the verb the resolver named, and `audit` records to
`Yupana::Action::"Audit"`. `session` becomes the temporal grouping key and
`item` the work item — the two fields that make a record replayable at all.

## Read the denominator

The converter prints, to stderr, how many events it wrote and how many records
it dropped **with a separate count per reason**:

```text
1284 event(s) written from 3011 spool line(s)
  dropped, no-session: 1502
  dropped, not-an-action: 210
  dropped, resolver-abstained: 15
```

That is not diagnostics; it is part of the result. A converter reporting only
what it kept would let one covering 5% of traffic look identical to one covering
95%, and the false-positive rate is measured *against the corpus that survived*.

Two of those reasons need opposite responses, which is why they are counted
separately:

- **`no-session`** — records predating the field. Not replayable, and a rate
  measured without them is measured against a smaller corpus than the fleet
  produced. These age out as new records accumulate.
- **`resolver-abstained`** — `crate::action` declined to name a verb. These are
  *correct* abstentions and must not be replayed under an invented verb: a rule
  derived from one would cite evidence that does not exist.

## The honest limits, carried with the numbers

From [work-scoped governance], and they travel with any rate this produces or
the rate misleads:

Replay measures false positives **only on traffic that happened**. It cannot see
work nobody has done yet, and it measures **no false negatives at all**. A
seeded adversarial corpus would give FN coverage against known attacks and none
against novel ones.

[work-scoped governance]: https://github.com/scbrown/yupana/blob/main/docs/work-scoped-governance.md
[Dogwood]: https://github.com/dogwood-policy/dogwood
[The paper plan]: https://github.com/scbrown/yupana/blob/main/docs/design/paper.md
