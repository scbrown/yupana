#!/usr/bin/env bash
# Refuse to publish anything that is not the version this run means to ship.
#
# Called by .github/actions/crates-publish before `cargo publish` (aegis-pz5crt,
# ported from the quipu fix for aegis-pb4rzi). It lives in a script rather than
# inline in the action for one reason: the refusals are the safety property, and
# a refusal that has never been observed is a claim. `--selftest` observes all of
# them.
#
# WHY THIS REPO NEEDS IT AT LEAST AS MUCH AS QUIPU DID. yupana's crates.yml was
# byte-identical to quipu's before that fix AND had no prerelease guard at all —
# its publish job carried no `if:` whatsoever, so a prerelease release event
# would have gone straight to `cargo publish`. quipu got that guard separately;
# this repo never did.
#
# It is also unpublished: crates.io has no `yupana` crate at all (the pre-rename
# name `hank` sits at 0.4.0 from the now-archived repository). So the first time
# this lane works it CLAIMS the name, and a crates.io version cannot be
# unpublished — only yanked. That is why the release job is gated on an explicit
# opt-in variable rather than simply being made to work.
#
# THE REFUSALS HOLD WITH TRUSTED PUBLISHING FULLY WORKING. It would be easy to
# call this lane safe today because crates.io TP is unconfigured, so a stray
# publish cannot succeed. That is not safety — it is an untested procedure
# standing behind someone else's outage, and it expires silently the moment TP
# is configured. Nothing below consults TP.
#
#   usage: crates-publish-guard.sh <expected-version> <actual-version> <ref-name>
#          crates-publish-guard.sh --selftest
#
#   exit 0  publish may proceed
#   exit 1  refused (the reason is on stderr)
#   exit 2  called wrong

set -uo pipefail

guard() {
    local expected="$1" actual="$2" ref="$3"

    # `cargo publish` publishes whatever is in Cargo.toml regardless of which
    # tag invoked it, so "which version are we publishing" is not knowable from
    # the tag and must be asserted rather than assumed.
    if [ "$actual" != "$expected" ]; then
        echo "REFUSED: the crate is at ${actual} but this run intends to publish ${expected}." >&2
        echo "         cargo publish would ship ${actual} under a tag nobody meant." >&2
        return 1
    fi

    # Checked on the VERSION, not on a GitHub release's `prerelease` flag: the
    # release lane calls this from a push, where there is no release object to
    # read, so a flag-based check would silently not apply on the path that
    # matters most.
    case "$actual" in
        *-*) echo "REFUSED: ${actual} is a prerelease version." >&2; return 1 ;;
    esac

    # A throwaway rehearsal tag must not reach the registry. On quipu the
    # documented rehearsal fired a real publish attempt (run 33959092337) and
    # failed only because TP was unconfigured; this repo has the same trigger
    # shape and, until now, fewer guards, so the same procedure would do the
    # same thing here — and here it would claim an unclaimed crate name.
    case "$ref" in
        rehearsal-*|*-rehearsal|test-*)
            echo "REFUSED: ref ${ref} looks like a rehearsal." >&2; return 1 ;;
    esac

    echo "ok: publishing ${actual} from ${ref}"
    return 0
}

selftest() {
    local failed=0
    check() {  # check <description> <want-rc> <expected> <actual> <ref>
        local desc="$1" want="$2"; shift 2
        local out rc
        out="$(guard "$@" 2>&1)"; rc=$?
        if [ "$rc" -eq "$want" ]; then
            printf '  PASS  %s (rc=%s)\n' "$desc" "$rc"
        else
            printf '  FAIL  %s — wanted rc=%s, got %s: %s\n' "$desc" "$want" "$rc" "$out"
            failed=1
        fi
    }

    # The CONTROL comes first. Every arm below asserts a refusal, and a guard
    # that refuses everything would pass all of them — including a guard broken
    # by a typo. Without this line the suite cannot tell "correctly strict" from
    # "uniformly broken".
    check "a real release publishes"                  0 "0.6.5" "0.6.5" "main"

    check "a version mismatch is refused"             1 "0.6.5" "0.6.6" "main"
    check "publishing an OLDER crate is refused"      1 "0.6.5" "0.4.0" "main"
    check "a prerelease version is refused"           1 "0.6.5-rc.1" "0.6.5-rc.1" "main"
    check "a prerelease build tag is refused"         1 "0.7.0-alpha" "0.7.0-alpha" "main"
    check "a rehearsal ref is refused"                1 "0.6.5" "0.6.5" "rehearsal-release-20260905"
    check "a trailing -rehearsal ref is refused"      1 "0.6.5" "0.6.5" "v1-rehearsal"
    check "a test-* ref is refused"                   1 "0.6.5" "0.6.5" "test-publish"
    check "an empty expected version is refused"      1 "" "0.6.5" "main"
    check "an empty actual version is refused"        1 "0.6.5" "" "main"

    # The rehearsal arm must refuse even when everything else is perfect —
    # otherwise it is the version check doing the work and the rehearsal guard
    # has never actually fired.
    check "rehearsal refused on a VALID version"      1 "0.6.5" "0.6.5" "rehearsal-anything"

    echo "selftest: $([ $failed -eq 0 ] && echo 'ALL PASS' || echo 'FAILED')"
    return $failed
}

case "${1:-}" in
    --selftest) selftest; exit $? ;;
    "") echo "usage: $0 <expected-version> <actual-version> <ref-name> | --selftest" >&2; exit 2 ;;
esac

if [ $# -ne 3 ]; then
    echo "usage: $0 <expected-version> <actual-version> <ref-name> | --selftest" >&2
    exit 2
fi
guard "$1" "$2" "$3"
