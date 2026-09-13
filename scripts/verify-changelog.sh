#!/usr/bin/env bash
# verify-changelog.sh — guard against release-plz's changelog commit mis-selection.
#
# WHY: release-plz's own commit selection for the CHANGELOG is
# unreliable on this repo — it has picked a wrong base (documenting only the 4
# newest commits of a 17-commit release, v0.3.6) and, separately, dumped
# full-history for an empty release (v0.3.7, and v0.3.5 before it). git-cliff with
# THIS repo's cliff.toml over `<prev-tag>..<head>` produces the correct delta every
# time. This script regenerates that correct delta and fails if the newest section
# of CHANGELOG.md is missing any commit git-cliff attributes to the release —
# mechanizing the manual "diff before merge" workaround so a bad changelog can't be
# merged unnoticed (v0.3.5's under-documented section was).
#
# Usage:
#   scripts/verify-changelog.sh                 # verify HEAD's CHANGELOG newest section
#   scripts/verify-changelog.sh --at <ref>      # verify CHANGELOG.md as of <ref> (for tests)
#   scripts/verify-changelog.sh --range A..B    # override the git-cliff range explicitly
#
# Exit: 0 = every expected commit is documented; 1 = commits missing; 2 = usage/env error.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CLIFF_CONFIG="${CLIFF_CONFIG:-cliff.toml}"
AT_REF=""
RANGE_OVERRIDE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --at) AT_REF="$2"; shift 2 ;;
    --range) RANGE_OVERRIDE="$2"; shift 2 ;;
    -h|--help) grep '^#' "$0" | sed 's/^# \?//'; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

command -v git-cliff >/dev/null 2>&1 || { echo "ERROR: git-cliff not installed" >&2; exit 2; }

# The CHANGELOG content to check (working tree, or as-of a ref for tests).
if [[ -n "$AT_REF" ]]; then
  changelog_content="$(git show "${AT_REF}:CHANGELOG.md")"
  head_ref="$AT_REF"
else
  changelog_content="$(cat CHANGELOG.md)"
  head_ref="HEAD"
fi

# Newest version section = from the first `## [x]` heading to the next one.
newest_ver="$(printf '%s\n' "$changelog_content" | grep -m1 -oE '^## \[[0-9]+\.[0-9]+\.[0-9]+\]' | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' || true)"
[[ -n "$newest_ver" ]] || { echo "ERROR: no versioned section found in CHANGELOG.md" >&2; exit 2; }
# VERSIONED headings only. `/^## \[/` also matches `## [Unreleased]`, so on a
# changelog that carries one — yupana's does — n==1 selects that empty block and the
# verifier checks the wrong section: everything reads as omitted. `newest_ver` above
# already greps for a versioned heading; this must agree with it (aegis-snglaf).
newest_section="$(printf '%s\n' "$changelog_content" | awk '/^## \[[0-9]+\./{n++} n==1' )"

# Range = <prev-tag>..<head-of-release>.
#  - HEAD of range: the historical v<newest_ver> tag when that released section
#    exists, otherwise the package-qualified quipu-ai-v<newest_ver> tag.
#  - prev tag: the newest package-qualified ancestor, falling back to historical
#    v-tags only when there is no such ancestor. This mirrors release-plz's renamed
#    package boundary without making the historical changelog unverifiable.
if [[ -n "$RANGE_OVERRIDE" ]]; then
  range="$RANGE_OVERRIDE"
else
  rel_head="$head_ref"
  for candidate in "v${newest_ver}" "quipu-ai-v${newest_ver}"; do
    if git rev-parse -q --verify "refs/tags/${candidate}" >/dev/null 2>&1 \
       && git merge-base --is-ancestor "$candidate" "$head_ref" 2>/dev/null; then
      rel_head="$candidate"
      break
    fi
  done
  prev_tag="$(git tag --list 'quipu-ai-v[0-9]*' --sort=-v:refname \
    | grep -vx "quipu-ai-v${newest_ver}" \
    | while read -r t; do git merge-base --is-ancestor "$t" "$rel_head" 2>/dev/null && { echo "$t"; break; }; done \
    || true)"
  if [[ -z "$prev_tag" ]]; then
    prev_tag="$(git tag --list 'v[0-9]*' --sort=-v:refname \
      | grep -vx "v${newest_ver}" \
      | while read -r t; do git merge-base --is-ancestor "$t" "$rel_head" 2>/dev/null && { echo "$t"; break; }; done)"
  fi
  [[ -n "$prev_tag" ]] || { echo "ERROR: no release tag found as ancestor of ${rel_head}" >&2; exit 2; }
  range="${prev_tag}..${rel_head}"
fi

# Expected commits = git-cliff over the range, narrowed to commits that change the
# packaged crate. release-plz uses `git_only = true`, so `cargo package` is the
# shared definition of release content: docs/CI-only commits do not ship and do not
# belong in the crate changelog. The helper is also used by fix-changelog.sh so the
# generator and verifier cannot silently diverge.
# An empty cliff result is meaningful (nothing releasable), but a package-list
# failure is not. Keep those outcomes distinct so the guard fails closed.
cliff_hashes="$(git-cliff --config "$CLIFF_CONFIG" "$range" 2>/dev/null \
  | grep -oE '\[[0-9a-f]{7}\]' | tr -d '[]' | sort -u || true)"
# Independently inspect the raw window BEFORE trusting git-cliff's filtered list.
# Otherwise generator and verifier can silently agree to omit a bare squash title.
bare_hashes="$(printf '%s\n' "$newest_section" | python3 scripts/check-release-window.py \
  --range "$range" --section-stdin)" || exit $?
expected="$bare_hashes"
if [[ -n "$cliff_hashes" ]]; then
  packaged="$(printf '%s\n' "$cliff_hashes" | scripts/filter-packaged-commits.py)" \
    || { echo "ERROR: could not determine packaged release content" >&2; exit 2; }
  expected="$(printf '%s\n%s\n' "$bare_hashes" "$packaged" | sed '/^$/d' | sort -u)"
fi
# Actual = hashes present in the newest CHANGELOG section.
actual="$(printf '%s\n' "$newest_section" | grep -oE '\[[0-9a-f]{7}\]' | tr -d '[]' | sort -u || true)"

missing="$(comm -23 <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") || true)"
# Drop release-MECHANICS commits from "missing": they are not release CONTENT and
# cannot be required to appear in the changelog (release-plz's own "chore: release
# vX", or a hand splice like fb1cfea) — they document the release, they are not
# documented BY it.
#
# The test is "touches ONLY release-mechanics files", NOT "touches CHANGELOG.md".
# The looser form exempted any commit that happened to also edit the changelog, so a
# substantive commit that did both stopped being required and could go undocumented
# with the guard still green. Observed live on #60: a fix(ci) commit that also
# touched CHANGELOG.md was dropped from the required set, and the section
# release-plz generated for it documents nothing but an unresolvable placeholder.
MECHANICS_RE='^(CHANGELOG\.md|Cargo\.toml|Cargo\.lock)$'
missing="$(printf '%s\n' "$missing" | while read -r h; do
  [[ -z "$h" ]] && continue
  files="$(git show --name-only --format= "$h" 2>/dev/null | grep -c . || true)"
  mech="$(git show --name-only --format= "$h" 2>/dev/null | grep -cE "$MECHANICS_RE" || true)"
  # Every file it touched is release mechanics, and it touched at least one.
  if [[ "$files" -gt 0 && "$files" -eq "$mech" ]]; then continue; fi
  echo "$h"
done)"

# EXTRA = documented here but NOT attributed to this release by git-cliff. This is
# the OTHER direction, and it is the one that has actually bitten: release-plz's
# full-history dump (v0.3.7 in #42, v0.3.17 in #59) puts every commit in the project
# under one heading. Checking only `missing` cannot see it — a section with 220
# commits too many satisfies "nothing is missing" and passed clean, printing the
# 1-vs-221 discrepancy on its own summary line before returning OK. A guard that
# fails safe in one direction and is silently blind in the other is worse than no
# guard, because the green check is read as verification.
extra="$(comm -13 <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") || true)"
# Keep only hashes that resolve to a real commit. git-cliff renders a literal
# [0000000] for synthetic entries it cannot attribute (v0.3.11 has one, for
# "Update Cargo.toml dependencies"). That is changelog noise, not a commit
# documented under the wrong version, and counting it here would fail a release
# for a defect that does not exist.
extra="$(printf '%s\n' "$extra" | while read -r h; do
  [[ -z "$h" ]] && continue
  # `if`, not `&&`: a non-resolving hash makes git exit 128, and under `set -e`
  # that status escapes the loop and kills the script instead of skipping the line.
  if git cat-file -e "${h}^{commit}" 2>/dev/null; then echo "$h"; fi
done)"

exp_n="$(grep -c . <<<"$expected" || true)"
act_n="$(grep -c . <<<"$actual" || true)"
mis_n="$(grep -c . <<<"$missing" || true)"
ext_n="$(grep -c . <<<"$extra" || true)"

echo "changelog-verify: version ${newest_ver}, range ${range}"
echo "  git-cliff commits: ${exp_n} · documented in CHANGELOG: ${act_n} · missing: ${mis_n} · extra: ${ext_n}"
rc=0
if [[ "$mis_n" -gt 0 ]]; then
  echo "FAIL — the newest CHANGELOG section is missing commits git-cliff attributes to this release:" >&2
  while read -r h; do [[ -n "$h" ]] && echo "  - $h $(git log -1 --format='%s' "$h" 2>/dev/null)"; done <<<"$missing" >&2
  rc=1
fi
if [[ "$ext_n" -gt 0 ]]; then
  echo "FAIL — the ${newest_ver} section documents ${ext_n} commit(s) that do not belong to this release" >&2
  echo "       (git-cliff attributes only ${exp_n} commit(s) to ${range}):" >&2
  while read -r h; do [[ -n "$h" ]] && echo "  - $h $(git log -1 --format='%s' "$h" 2>/dev/null)"; done \
    <<<"$(printf '%s\n' "$extra" | head -10)" >&2
  [[ "$ext_n" -gt 10 ]] && echo "  ... and $((ext_n - 10)) more" >&2
  # The signature of the full-history dump: nothing to release, everything documented.
  if [[ "$exp_n" -eq 0 ]]; then
    echo "" >&2
    echo "NOTE: git-cliff attributes NO commits to this range — there is nothing to release." >&2
    echo "      A release PR proposing ${newest_ver} over an empty delta should be CLOSED, not merged." >&2
  fi
  rc=1
fi
if [[ "$rc" -ne 0 ]]; then
  echo "" >&2
  echo "Fix: regenerate with the tool that is correct here —" >&2
  echo "  git-cliff --config ${CLIFF_CONFIG} ${range}" >&2
  echo "and replace the ${newest_ver} section with its output" >&2
  exit 1
fi
echo "OK — the ${newest_ver} section matches git-cliff exactly (${exp_n} commit(s), none missing, none extra)."
