#!/usr/bin/env bash
# Run inside an unfiltered workflow. The job always reports; later steps use the
# changed output to short-circuit expensive work when the paths are irrelevant.
set -euo pipefail

base=${PATH_GATE_BASE:?PATH_GATE_BASE is required}
head=${PATH_GATE_HEAD:?PATH_GATE_HEAD is required}
paths_text=${PATH_GATE_PATHS:?PATH_GATE_PATHS is required}

mapfile -t paths < <(printf '%s\n' "$paths_text" | sed '/^[[:space:]]*$/d')
[ ${#paths[@]} -gt 0 ] || { echo 'path-gate: no pathspecs supplied' >&2; exit 2; }

# GitHub uses all-zeroes for a push with no parent. Diff from the empty tree so
# the first run still gets an honest answer.
if [[ "$base" =~ ^0+$ ]]; then
  base=$(git hash-object -t tree /dev/null)
fi

git cat-file -e "$base^{commit}" 2>/dev/null || git cat-file -e "$base^{tree}"
git cat-file -e "$head^{commit}"

if git diff --quiet "$base" "$head" -- "${paths[@]}"; then
  changed=false
else
  status=$?
  [ "$status" -eq 1 ] || exit "$status"
  changed=true
fi

echo "changed=$changed"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "changed=$changed" >> "$GITHUB_OUTPUT"
fi
