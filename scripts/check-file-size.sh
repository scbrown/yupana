#!/usr/bin/env bash
# Keep source files small (CLAUDE.md; docs/yupana-spec.md §7.2).
#
# Takes its file list from ARGUMENTS when given, and falls back to the staged
# set otherwise. That fallback used to be the ONLY source, which made the
# control inert in the place it was most often run: `pre-commit run --all-files`
# leaves the index empty, so the loop body never executed and the hook reported
# "Passed" having examined nothing — while four source files sat over the hard
# limit (yupana #83). Reading arguments means `--all-files` checks all files and a
# real commit still checks exactly what is being committed, since pre-commit
# passes the staged set.
set -euo pipefail
WARN_LIMIT=400
ERROR_LIMIT=500
SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASELINE="${FILE_SIZE_BASELINE:-$SELF_DIR/file-size-baseline.txt}"

# THE RATCHET (aegis-1gy64). Five files were already over the hard limit, so this hook
# failed on EVERY CI run — and a check that is always red is not a check. It made yupana's
# CI permanently red, which is why the mcp+quipu arms going red on cc2c213 was invisible:
# the run went from one failure to three and nothing about the result changed. The lint
# was correct and useless at the same time.
#
# So: files listed in the baseline are FROZEN AT THEIR SIZE, not exempted. A listed file
# may shrink, never grow. A file not listed may not exceed the limit at all. Existing debt
# can only go down; new debt cannot be added. That makes the signal TRUE today, which is
# the prerequisite for anyone ever gating on it.
baseline_lines() { # $1=path -> frozen size, or empty
  [ -r "$BASELINE" ] || return 0
  awk -F'\t' -v f="$1" '$1==f {print $2; exit}' "$BASELINE"
}

if [ "${1:-}" = "--update-baseline" ]; then
    # Rewrite the baseline from the current tree. Keeps the header comment.
    tmp="$BASELINE.$$"
    awk '/^#/ {print; next} {exit}' "$BASELINE" > "$tmp" 2>/dev/null || true
    git ls-files '*.rs' | while read -r f; do
        case "$f" in *tests.rs|*_test.rs|tests/*) continue;; esac
        [ -f "$f" ] || continue
        n=$(wc -l < "$f")
        [ "$n" -gt "$ERROR_LIMIT" ] && printf '%s\t%s\n' "$f" "$n"
    done | sort >> "$tmp"
    mv -f "$tmp" "$BASELINE"
    echo "check-file-size: baseline rewritten from the working tree -> $BASELINE"
    exit 0
fi

if [ "$#" -gt 0 ]; then
    files=("$@")
else
    mapfile -t files < <(git diff --cached --name-only --diff-filter=ACM)
fi

errors=0; warnings=0; checked=0
for file in "${files[@]:-}"; do
    [[ "$file" == *.rs ]] || continue
    [ -f "$file" ] || continue
    # Tests are exempt (CLAUDE.md). The name-suffix forms cover in-crate test
    # modules; the `tests/` prefix covers the integration suite, where a file is
    # named for the BINARY it drives (`tests/cli.rs`) and so matches neither
    # suffix. Without it the exemption missed the largest test files in the repo.
    if [[ "$file" =~ tests\.rs$ ]] || [[ "$file" =~ _test\.rs$ ]] || [[ "$file" == tests/* ]]; then continue; fi
    checked=$((checked + 1))
    lines=$(wc -l < "$file")
    frozen="$(baseline_lines "$file")"
    if [ -n "$frozen" ]; then
        # Grandfathered. The only failure is GROWTH past the frozen size.
        if [ "$lines" -gt "$frozen" ]; then
            echo "ERROR: $file has $lines lines, above its frozen baseline of $frozen — grandfathered files may shrink, never grow"
            errors=$((errors + 1))
        elif [ "$lines" -le "$ERROR_LIMIT" ]; then
            echo "NOTICE: $file is now $lines lines, under the $ERROR_LIMIT limit — drop it from $(basename "$BASELINE") (scripts/check-file-size.sh --update-baseline)"
        fi
    elif [ "$lines" -gt "$ERROR_LIMIT" ]; then echo "ERROR: $file has $lines lines (limit: $ERROR_LIMIT)"; errors=$((errors + 1))
    elif [ "$lines" -gt "$WARN_LIMIT" ]; then echo "WARNING: $file has $lines lines (warn: $WARN_LIMIT)"; warnings=$((warnings + 1)); fi
done
# Report the SUBJECT COUNT, always (yupana #84). A green check is two claims — "I
# looked" and "it was fine" — and this hook only ever made the second, which is
# how #83 passed over an empty input set. `checked 0 files` is self-diagnosing in
# a CI log; `Passed` is not. Print it even on success: the empty case is the one
# worth seeing, and it is exactly the case a failure-only message never reaches.
echo "check-file-size: examined $checked file(s) in this invocation (pre-commit batches; totals are the sum of these lines)"
if [ "$errors" -gt 0 ]; then echo "$errors file(s) exceed $ERROR_LIMIT lines."; exit 1; fi
if [ "$warnings" -gt 0 ]; then echo "$warnings file(s) approaching limit."; fi
exit 0
