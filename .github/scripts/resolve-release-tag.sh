#!/usr/bin/env bash
# resolve-release-tag.sh — which version is this push publishing?
#
# PORTED FROM quipu (.github/scripts/resolve-release-tag.sh, PR166/111d8270) for
# aegis-snglaf. yupana carried the ORIGINAL `git tag --points-at HEAD` form and hit
# the identical failure on v0.8.1: the tag sat on the release branch head and main's
# HEAD was the merge commit above it, so the publish refused a valid release.
# Differences from the quipu original: TAG_PREFIX defaults to `v`, and the version is
# extracted by stripping that prefix rather than by `${tag##*-v}`.
#
# Prints `<tag>\t<version>` on stdout, or refuses.
#
# ── WHY THIS IS NOT `git tag --points-at HEAD` ──────────────────────────────────
#
# It was, and it refused EVERY REAL RELEASE (aegis-fxpkt2 / aegis-xx6xu). The old
# comment stated the assumption in as many words — "release-plz tags the very commit
# being built, so the two agree by construction" — and that holds only when the
# release PR is SQUASHED. Merged with a MERGE COMMIT, release-plz's tag sits on the
# PR's own commit and main's HEAD is the merge commit one above it, so nothing points
# at HEAD and the guard refuses a perfectly good release.
#
# Measured on both refusals:
#
#   quipu-ai-v0.3.36 -> 94edf1eb, merge 96473a88   trees IDENTICAL 00a51f5d
#   v0.8.1 -> f2304834, merge 4f3b3f8d   trees IDENTICAL 863e0899
#
# ── THE TEST THAT REPLACES IT: ANCESTRY **AND** TREE ────────────────────────────
#
# Two conditions, and dropping either one loses the property the guard exists for:
#
#   ANCESTRY  the tag must be reachable from HEAD (`git tag --merged HEAD`). A tag on
#             some other branch names code this push is not publishing.
#   TREE      the tag's tree must EQUAL HEAD's tree. The merge commit may sit above
#             the tag, but it must introduce NOTHING. This is what makes "the tag
#             names the code we are about to publish" true rather than approximately
#             true, and it is strictly stronger than points-at: a later commit that
#             changed anything is refused even though the tag is still an ancestor.
#
# THE FIX IS NOT "SWITCH TO SQUASH MERGES." That would make the old check pass by
# constraining how humans merge, and it would break again the first time somebody
# merged normally — silently, and only at release time.
#
# ── AND IT MUST NOT BE INVISIBLE UNTIL A RELEASE ────────────────────────────────
#
# The reason this survived two releases is that the publish job is SKIPPED on ordinary
# pushes, so it emits nothing until a release — and its first signal is a red Release
# workflow, which looks exactly like the benign asset-upload race (aegis-9wdzwv) that
# was triaged twice the same day. Two failures wearing one symptom. Hence `--selftest`:
# the logic is exercised on every run of the test suite, not only when it matters.

set -euo pipefail

PREFIX="${TAG_PREFIX:-v}"

resolve() { # resolve <repo-dir> -> prints "<tag>\t<version>" or fails with a message
  local d="$1" tag head_tree tag_tree
  tag=$(git -C "$d" tag --merged HEAD --sort=-creatordate 2>/dev/null | grep "^${PREFIX}" | head -1 || true)
  if [ -z "$tag" ]; then
    # ── A SHALLOW CLONE CANNOT ANSWER THIS QUESTION (aegis-pfqttr) ──────────────
    # `git tag --merged HEAD` computes ANCESTRY, and at fetch-depth: 1 there is no
    # ancestry to compute — so it returns nothing whether or not a tag exists. The
    # empty result is then indistinguishable from a genuine "no tag", and the
    # message below would state a finding that is not true.
    #
    # That is worse than the `points-at` bug this script replaced: that one named a
    # condition that WAS true. This would refuse every release while reporting the
    # wrong reason, and it would read as a NEW bug in the fix — sending the next
    # person to look at tags when the fault is in the checkout.
    #
    # Same class as aegis-9bgp, where a shallow clone nearly closed a live token
    # leak by confidently answering "not in history". The crew rule is that a script
    # about to conclude "X is not in history" must gate on depth FIRST. This is that
    # gate. Distinct exit code (2) and distinct wording, so the blind spot can never
    # be read as, or grepped as, the genuine no-tag case.
    if [ "$(git -C "$d" rev-parse --is-shallow-repository 2>/dev/null)" = true ]; then
      echo "this clone is SHALLOW, so ancestry cannot be computed and NO CONCLUSION about" >&2
      echo "reachable tags is available. This is a blind spot, not a finding: a ${PREFIX}*" >&2
      echo "tag may well exist and be reachable." >&2
      echo "Fix the CHECKOUT, not the tags — the publish job needs actions/checkout with" >&2
      echo "fetch-depth: 0." >&2
      return 2
    fi
    echo "no ${PREFIX}* tag is reachable from HEAD. Refusing to publish a version this run cannot name." >&2
    return 1
  fi
  # NOTE: the shallow gate deliberately covers only the NO-TAG conclusion. If a tag
  # WAS found, the tree comparison below is what makes the answer trustworthy, and it
  # is unaffected by depth: it compares two trees this clone already has.
  head_tree=$(git -C "$d" rev-parse 'HEAD^{tree}')
  tag_tree=$(git -C "$d" rev-parse "${tag}^{tree}")
  if [ "$head_tree" != "$tag_tree" ]; then
    echo "tag ${tag} is reachable from HEAD but names DIFFERENT CODE:" >&2
    echo "  tag tree  ${tag_tree}" >&2
    echo "  HEAD tree ${head_tree}" >&2
    echo "Something landed after the tag. Refusing to publish code the tag does not name." >&2
    return 1
  fi
  # PREFIX-aware, not "strip up to the last -v": quipu's tags carry a literal
  # `-v` and yupana's do not, so `${tag##*-v}` would return `v0.8.1` unchanged
  # here — a version string with a stray `v` that the caller then compares against
  # Cargo.toml and refuses. Stripping the PREFIX is correct for both schemes.
  printf '%s\t%s\n' "$tag" "${tag#"$PREFIX"}"
}

if [ "${1:-}" = "--selftest" ]; then
  fail=0; echo "resolve-release-tag selftest:"
  t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
  mk() { # mk <dir>
    git -C "$t" init -q "$1" && git -C "$t/$1" config user.email t@t && git -C "$t/$1" config user.name t
    echo base > "$t/$1/f"; git -C "$t/$1" add -A; git -C "$t/$1" commit -qm base
  }
  chk() { [ "$2" = "$3" ] && echo "  ok: $1" || { echo "  FAIL: $1 — expected '$3', got '$2'"; fail=1; }; }

  # 1. THE CASE THAT WAS BROKEN: tag one below HEAD, merge commit above it.
  mk a
  ( cd "$t/a"
    git checkout -qb rel; echo 0.8.1 > version; git add -A; git commit -qm "chore: release v0.8.1"
    git tag v0.8.1
    git checkout -q master 2>/dev/null || git checkout -q main
    git merge -q --no-ff -m "Merge pull request #67" rel ) >/dev/null 2>&1
  chk "a MERGE COMMIT above the tag resolves (this is what points-at refused)" \
      "$(resolve "$t/a" 2>&1 | cut -f2)" "0.8.1"

  # 2. The squash case must still work — the fix must not trade one for the other.
  mk b
  ( cd "$t/b"; echo 0.8.2 > version; git add -A; git commit -qm rel; git tag v0.8.2 ) >/dev/null 2>&1
  chk "a tag ON HEAD still resolves" "$(resolve "$t/b" 2>&1 | cut -f2)" "0.8.2"

  # 3. THE PROPERTY THE OLD CHECK HAD AND THIS MUST KEEP: code that landed after the
  #    tag is REFUSED. Ancestry alone would pass this; the tree test is what fails it.
  mk c
  ( cd "$t/c"; echo 0.8.3 > version; git add -A; git commit -qm rel; git tag v0.8.3
    echo later >> f; git add -A; git commit -qm "something else landed" ) >/dev/null 2>&1
  out=$(resolve "$t/c" 2>&1 || true)
  case "$out" in
    *"names DIFFERENT CODE"*) echo "  ok: a commit AFTER the tag is refused (ancestry alone would pass)" ;;
    *) echo "  FAIL: post-tag commit was not refused: $out"; fail=1 ;;
  esac

  # 4. No tag at all.
  mk d
  out=$(resolve "$t/d" 2>&1 || true)
  case "$out" in
    *"no v* tag is reachable"*) echo "  ok: no reachable tag is refused, and says so" ;;
    *) echo "  FAIL: missing tag not refused: $out"; fail=1 ;;
  esac

  # 5. A tag on ANOTHER branch must not be picked up.
  mk e
  ( cd "$t/e"
    git checkout -qb other; echo 9.9.9 > version; git add -A; git commit -qm other
    git tag v9.9.9
    git checkout -q master 2>/dev/null || git checkout -q main ) >/dev/null 2>&1
  out=$(resolve "$t/e" 2>&1 || true)
  case "$out" in
    *"no v* tag is reachable"*) echo "  ok: a tag on an unmerged branch is NOT reachable" ;;
    *) echo "  FAIL: picked up an unreachable tag: $out"; fail=1 ;;
  esac

  # 6. THE SHALLOW BLIND SPOT (aegis-pfqttr). Case `a` is the realistic shape: the
  #    tag sits on the commit BELOW a merge commit, so a depth-1 clone fetches the
  #    merge and not the tag — `tag --merged HEAD` then returns empty for a repo
  #    that genuinely has a reachable tag. Built as a REAL shallow clone rather
  #    than by faking the marker, because the thing under test is git's behaviour.
  git -C "$t" clone -q --depth 1 "file://$t/a" shallow 2>/dev/null || true
  if [ -d "$t/shallow" ]; then
    # CONTROL — if this clone is not actually shallow the arm below is vacuous: it
    # would pass by taking the ordinary no-tag path and prove nothing at all.
    chk "CONTROL: the fixture really is a shallow clone" \
        "$(git -C "$t/shallow" rev-parse --is-shallow-repository)" "true"
    # CONTROL — and the tag must genuinely be absent from it, or we are not
    # exercising the empty-result path we care about.
    chk "CONTROL: the tag is not present in the shallow clone" \
        "$(git -C "$t/shallow" tag --list 'v[0-9]*' | wc -l | tr -d ' ')" "0"
    out=$(resolve "$t/shallow" 2>&1 || true)
    case "$out" in
      *"no v* tag is reachable"*)
        echo "  FAIL: shallow clone reported the GENUINE no-tag finding — the blind spot is"
        echo "        being stated as a fact. This is the bug aegis-pfqttr describes."; fail=1 ;;
      *"clone is SHALLOW"*) echo "  ok: a shallow clone says SHALLOW, not 'no reachable tag'" ;;
      *) echo "  FAIL: unexpected output from a shallow clone: $out"; fail=1 ;;
    esac
    # The exit code must separate the two refusals for a CALLER, not only for a reader.
    # NB `cmd; rc=$?` is WRONG under `set -e` — the failing call exits the script
    # before the check runs, and the suite then reports a pass-shaped early exit.
    # Found by running it: the arm below silently never executed. Keep the `|| rc=$?`
    # form, which makes the call part of a condition.
    rc=0; resolve "$t/shallow" >/dev/null 2>&1 || rc=$?
    chk "a shallow refusal exits 2, distinct from a genuine no-tag refusal (1)" "$rc" "2"
    rc=0; resolve "$t/d" >/dev/null 2>&1 || rc=$?
    chk "a genuine no-tag refusal still exits 1" "$rc" "1"
  else
    echo "  FAIL: could not build the shallow fixture — arm 6 did not run, which is NOT a pass"
    fail=1
  fi

  [ "$fail" = 0 ] && echo "  ALL PASS" || echo "  FAILURES ABOVE"
  exit "$fail"
fi

resolve "${1:-.}"
