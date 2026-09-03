#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
GATE="$ROOT/.github/actions/path-gate/path-gate.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

git -C "$TMP" init -q
git -C "$TMP" config user.email path-gate@example.invalid
git -C "$TMP" config user.name path-gate-test
mkdir -p "$TMP/docs/book" "$TMP/src"
printf 'one\n' > "$TMP/docs/book/index.md"
printf 'one\n' > "$TMP/src/lib.rs"
git -C "$TMP" add .
git -C "$TMP" commit -qm base
base=$(git -C "$TMP" rev-parse HEAD)
printf 'two\n' >> "$TMP/docs/book/index.md"
git -C "$TMP" commit -qam docs
head=$(git -C "$TMP" rev-parse HEAD)

result=$(cd "$TMP" && PATH_GATE_BASE=$base PATH_GATE_HEAD=$head PATH_GATE_PATHS='docs/book/**' "$GATE")
[ "$result" = 'changed=true' ] || { echo "FAIL: relevant change: $result"; exit 1; }

result=$(cd "$TMP" && PATH_GATE_BASE=$base PATH_GATE_HEAD=$head PATH_GATE_PATHS='src/**' "$GATE")
[ "$result" = 'changed=false' ] || { echo "FAIL: irrelevant change: $result"; exit 1; }

if (cd "$TMP" && PATH_GATE_BASE=missing PATH_GATE_HEAD=$head PATH_GATE_PATHS='src/**' "$GATE") >/dev/null 2>&1; then
  echo 'FAIL: missing base was accepted'
  exit 1
fi

echo '3 passed'
