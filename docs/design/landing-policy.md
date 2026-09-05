# Governed landing policy — single-writer as data, not as a name in a script

Status: design, advise-tier first. Tracking item `aegis-unvg1x`.

## What this is

A repository can declare, *in the graph*, who is allowed to land code on its
protected branch and under what evidence. Yupana evaluates that declaration at
the pre-action gate it already hosts, and signs the verdict with the identity it
already has.

The rule is **data**. Extending single-writer from one repository to a second is
a graph write. It is not a code change, not a deploy, and not an edit to a
guard script that names repositories and agents in its source.

## Why the obvious implementation is the wrong one

The host already enforces this rule for one repository, and the enforcement
works. Its defect is not correctness — it is that the *policy* and the
*mechanism* are the same artifact:

- the repository is a regular expression in the guard's source;
- the owner is a name in a cache file the guard refreshes out of band;
- adding a second governed repository means editing and redeploying the guard.

So the guard cannot answer the question an operator actually asks — "which
repositories are governed, by what rule, and who says so" — because the answer
is distributed across a script, a cache file and a deploy. Moving the rule into
the graph makes that question a query.

**There is no new subcommand.** The rule rides the policy circuit yupana
already runs: the pre-action Bash gate, the projection cache, and the
certify/verdicts spool. A new top-level verb would have been a second
enforcement path with its own freshness story, its own failure modes and its own
bypasses — which is the defect above, rebuilt one layer up.

## The vocabulary

Nothing here mints a new `rdf:type`. `Policy`, `Selector`, `Predicate` and
`GitRepo` are already governed classes, and `aegis:targets` carries a string
literal, so the target name costs no schema change.

### The policy entity

```turtle
aegis:policy_repo-single-writer-landing a aegis:Policy ;
    rdfs:label          "Protected-branch landings require the declared owner" ;
    aegis:targets       "aegis:LandingAction" ;
    aegis:boundary      "action" ;
    aegis:selector      aegis:selector_landing-command ;
    aegis:predicate     aegis:predicate_agent-is-repo-owner ;
    aegis:effect        "deny" ;
    aegis:constraintClass "hard" ;
    aegis:verificationPoint "PAG" .
```

### The two atoms

`selector_landing-command` selects the action from the live Bash command — the
same `evidenceSource "bash-command"` the memory-headroom policy uses.

`predicate_agent-is-repo-owner` is the new predicate vocabulary this design
adds, and the only genuinely new thing in it. Its `evidenceSource` is
`repo-owner`: the evidence is not a substring of the command and not a local
reading, it is a **fact resolved from the graph** — the target repository's
`aegis:owned_by`, compared against the acting agent.

### The per-repository fact

```turtle
aegis:repo_quipu aegis:landingPolicy "single-writer" .
```

| value | who may land on a protected ref |
| --- | --- |
| `single-writer` | the declared owner only, and the owner must cite a work item |
| `any-owner-with-bead` | any agent, and the agent must cite a work item |
| *absent* | ungoverned — this policy does not apply |

## The decision procedure

The evaluator abstains unless it can positively identify a **landing**: a `push`
or `merge` verb, against a named repository, onto a protected ref. Everything
else — a topic branch, a tag, a fetch, a command it cannot parse — is not this
policy's business and is allowed without a verdict.

Having identified a landing, resolution is three-valued, and the three values
are the whole design:

| resolution | meaning | outcome |
| --- | --- | --- |
| **Governed** | the graph (or a fresh-enough cache) declares a landing policy for this repository | evaluate it |
| **Ungoverned** | the graph answered, and this repository declares no landing policy | **allow** |
| **Unknown** | the graph could not be asked and no usable cache exists | **refuse**, loudly, naming the override |

Collapsing `Ungoverned` into `Unknown` is the failure this table exists to
prevent. They are both "no policy found", and treating them alike means every
landing on every repository on the host is refused the moment the graph blinks —
which is how a guard gets removed rather than fixed.

Collapsing them the other way is the failure the *refusal* exists to prevent: if
an unreachable graph reads as "ungoverned", then making the graph unreachable is
the bypass.

### The blast radius of failing closed

`Unknown` refuses, but only for a landing verb onto a protected ref. Topic
branches, tags and every other ref allow unconditionally, and so does every
command that is not a landing. So the fail-closed path is exactly:

> landing on a protected branch, while the graph is unreachable, with a cold
> projection cache.

That is the highest-stakes action on the host, it is rare, and the refusal names
a single-use expiring override. This is a deliberately narrow door, not a
host-wide brake.

### Staleness is not freshness

A cache-served resolution is `Stale` and says so, carrying its age. Past the
configured TTL the cache is refused rather than served, and resolution degrades
to `Unknown`. A retired rule that keeps firing out of a cache is worse than no
rule, because nothing outside the process can falsify it.

## Attestation

Every decision — allow as well as refuse — is a signed action-certification
record on the existing spool, promoted by `yupana verdicts`. The attestation
happens *at* the gate rather than being reconstructed from a log afterwards, so
the record and the decision cannot drift apart.

An override is the same object: a signed verdict carrying its reason, single
use and time-limited, promoted to the graph. The exception is therefore
attributed in the same place as the rule it excepts, rather than only in a
host-local log file.

## Sequencing, and what is deliberately not done yet

This is the first **block**-tier policy on this circuit, so it inherits that
gate: it runs in **advise** mode, beside the existing host guard, until the
false-positive rate is *measured* at zero — comparing this evaluator's verdicts
against the host guard's log line by line. Only then does the tier flip.

Until the flip, the host guard remains the enforcing authority. Afterwards it
becomes a shim that defers to the verdict and keeps its own refusal only for the
case where yupana is absent — which must fail closed, because "the enforcing
binary is missing" is not a reason to permit an unattributed landing.

## Honest limits

- **This is not a security boundary and cannot be one.** Every agent here runs
  as one operating-system user and authenticates to the forge as one account, so
  anyone able to land is able to write the override. The claim is narrower and
  still worth making: it converts an *accidental* second writer into a
  deliberate, dated, attributed one.
- **The acting agent is self-reported.** It comes from the session environment,
  not from a credential. A policy keyed on identity is only as good as that
  report, and nothing in this design improves it.
- **`exemplar` does not draft this policy today.** Policy-by-example is the
  intended authoring path, but the extractor is built for source text — it
  derives selectors from tree-sitter node kinds. Drafting a policy from guard
  log lines needs an extractor that does not exist yet; the policy below was
  authored against the memory-headroom policy's proven shape instead. Recorded
  because the gap is real work, not an oversight.
