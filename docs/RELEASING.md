# Releasing yupana

Releases are cut by `release-plz` from conventional commits on `main`. The
`Release` workflow then builds the binary, attaches it to the GitHub release,
and — since aegis-pz5crt — publishes the crate.

## The crates.io lane

**`yupana` is not on crates.io yet.** The registry has no `yupana` crate at all;
the pre-rename name `hank` sits at `0.4.0`, published from the repository that
is now an archived tombstone. So the first successful run of this lane does not
resume anything — it **claims the name**.

That is why the publish is gated:

```text
repository variable   CRATES_PUBLISH_ENABLED
unset / not "true" -> the lane runs in DRY-RUN mode and publishes nothing
"true"             -> the lane publishes
```

Setting it is a deliberate act, and the first publish is Stiwi's under his own
token. A crates.io version cannot be unpublished (only yanked) and a name cannot
be released, so this is not a decision the release automation should make on
anyone's behalf.

The gate is a **variable rather than a comment** because a constraint that lives
only in prose does not survive the merge that lands it. It is deliberately not a
skipped job either: a skipped job is invisible, whereas the dry-run branch says
in the log that it published nothing.

## Why the publish lives in `release.yml`, not `crates.yml`

`crates.yml` triggers on `release: [published]`. `release-plz` creates the
release with `GITHUB_TOKEN`, and GitHub does not trigger workflows from events
raised by `GITHUB_TOKEN` — the same mechanism that stops its tag pushes
triggering the `binary` job, which that workflow already documents at length.

So on a real release `crates.yml` **does not run at all**. On quipu that exact
shape left crates.io six versions behind while the publish workflow showed a
green run (aegis-pb4rzi); here it left the crate unpublished. `crates.yml`
remains as the manual lane only.

## Do not read a green publish run as a publish

The dry-run branch skips both authentication and `cargo publish`, so it goes
green without exercising either. A publish job whose success is compatible with
the registry not moving is a check that cannot fail. Both ends are now handled:
the dry-run branch says in the log that it published nothing, and the real
branch ends by polling crates.io and **failing** if the registry does not serve
the version it was asked to publish.

**Verify a publish by the registry, not by the workflow.** crates.io requires a
`User-Agent`; without one it answers `403`, which reads as "blocked" or "absent"
rather than "you forgot a header".

```bash
curl -s -H 'User-Agent: your-name (contact)' \
  https://crates.io/api/v1/crates/yupana | jq -r '.crate.max_version'
```

## The guards

`scripts/ci/crates-publish-guard.sh` refuses, and every refusal holds **with
Trusted Publishing fully working**. Do not reason that the lane is safe because
TP is unconfigured: a procedure whose safety depends on another system being
broken is not safe, it is untested, and it expires silently when that system is
fixed.

| refusal | why |
|---|---|
| version ≠ the version the caller declared | `cargo publish` ships whatever `Cargo.toml` holds, regardless of which tag invoked it |
| a prerelease **version** | checked on the version, not a release object's `prerelease` flag — the release lane runs from a push, where there is no release object |
| a ref named `rehearsal-*`, `*-rehearsal`, `test-*` | a throwaway rehearsal tag must not reach the registry |

`expected-version` is required with no default: the release lane reads the **git
tag**, the manual lane takes an **operator input**. Deriving it from `Cargo.toml`
would compare a number to itself.

Run `scripts/ci/crates-publish-guard.sh --selftest` to see all of them, plus a
**control** arm asserting that a valid publish is permitted — without which the
suite could not tell a correctly strict guard from a uniformly broken one. CI
runs it in `Pre-commit checks`.

## Rehearsing

Do not rehearse by pushing a throwaway tag named for a release. The guard
refuses `rehearsal-*` and `test-*` refs by name, which is the intended
behaviour: on quipu the documented rehearsal fired a real publish attempt (run
`33959092337`) and failed only because TP was unconfigured.

To exercise the lane, dispatch `crates.yml` with an explicit `version` and
`dry_run: true` — the default.
