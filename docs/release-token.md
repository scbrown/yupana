# Release PR credential

The `release-plz` job in `.github/workflows/release.yml` uses two credentials:

| step | credential | why |
|---|---|---|
| `release-plz release` (tags, creates the GitHub release) | `GITHUB_TOKEN` | its `releases_created` output drives the inline `binary` and `crates` jobs |
| `release-plz release-pr` (opens/updates the release PR) | `RELEASE_PLZ_TOKEN` | the PR head must trigger its required checks |
| re-checkout with `persist-credentials: true` | `RELEASE_PLZ_TOKEN` | the changelog corrector's plain `git push` uses the stored credential |
| changelog corrector (`gh pr list`, `git push`) | `RELEASE_PLZ_TOKEN` | its push becomes the PR's final head |

Everything else, including `verify-changelog` and `binary`, keeps `GITHUB_TOKEN`.

## Why

This is a public repository with contributor approval policy
`first_time_contributors`. When `GITHUB_TOKEN` authors a release PR, every
run on its head parks at `action_required`. Nothing reports until someone
approves each run. Branch protection requires seven contexts with
`strict: true`: Format, Build, Pre-commit checks, Book builds,
Clippy (default), Test (default) and Build relevant docs. So a parked release
PR is BLOCKED and shows nothing red.

The corrector makes this worse. Each push to `main` regenerates the release
PR, and the corrector then pushes a fixed section. Each resulting head needs
approving again. Approvals per release, from the interim approver's log and
by hand:

| release | release PR | automated approvals | by hand |
|---|---|---|---|
| v0.7.0 | #53 | 2 | |
| v0.8.0 | #55 | 14 | |
| v0.8.1 | #67 | 4 | |
| v0.8.2 | #71 | 3 | |
| v0.9.0 | #74 | 3 | |
| v0.9.1 | #77 | 3 | |
| v0.9.2 | #81 | 9 | 6 |
| v0.9.3 | #85 | 3 | |

The fix is the identity. **Do not relax the contributor approval policy.**
The same switch governs outside fork PRs, so relaxing it would let a
stranger's fork run Actions unattended.

## Provision

Create a **fine-grained personal access token** on the `scbrown` account:

- Resource owner: `scbrown`.
- Repository access: **Only select repositories**, and select **yupana**.
- Repository permissions: **Contents: Read and write** and
  **Pull requests: Read and write**. Metadata read is automatic.
- No Actions, Workflows or Administration permission. The token does not
  change workflow files or bypass branch protection.
- Set an expiration, and arrange renewal before it expires.

Store it as the repository **Actions secret `RELEASE_PLZ_TOKEN`**. Enter the
value only in Settings, never in a commit, issue, command argument or log.
Without the secret, the first step fails with a named error before release-plz
runs. It never falls back to `GITHUB_TOKEN` silently.

One token scoped to several repositories also works if you choose that. The
secret still has to be set on each repository.

## Why publication keeps `GITHUB_TOKEN`

Events authored by a PAT trigger workflows. `crates.yml` still listens for
`release: published`, and the inline `crates` job also publishes. If
`release` ran under the PAT, both publishers could fire and race. Splitting
`release` from `release-pr` isolates the PR's authentication without changing
how release events are delivered.

## Activation and acceptance

**Provision the secret BEFORE merging this workflow change.** The preflight
step fails every push to `main` until the secret exists.

After the next push to `main` updates a release PR:

1. Record the Release run and the PR number. Read the PR's `headRefOid` after
   the corrector finishes.
2. Read the required contexts from branch protection. Do not trust the list
   above; it may have changed.
3. Check that every required context reported and succeeded for that exact
   SHA. Zero checks is a failure.
4. Check that none of those runs concluded `action_required`, and that the
   interim approver did not approve them. Look for their run IDs in its log.
   A green run that the approver unparked proves the workaround, not this fix.
5. Check that the `release` step still used `GITHUB_TOKEN`, and that the
   binary and crates jobs behaved as before.

```bash
gh pr view PR --repo scbrown/yupana --json author,headRefOid,url
gh api repos/scbrown/yupana/branches/main/protection/required_status_checks
gh api --paginate 'repos/scbrown/yupana/actions/runs?head_sha=SHA&per_page=100' \
  --jq '.workflow_runs[] | [.id,.name,.conclusion,.run_attempt] | @tsv'
```

Once this passes, the interim approver finds nothing to approve on this
repository and can be retired here. If activation fails, keep the run IDs and
revert through a reviewed PR. The approver keeps releases moving meanwhile.
