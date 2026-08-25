#!/usr/bin/env bash
set -euo pipefail

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/tools"

cat > "$tmp/tools/cargo" <<'FAKE_CARGO'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" > "$FAKE_CARGO_ARGS"
root=""
while [ "$#" -gt 0 ]; do
    if [ "$1" = --root ]; then root=$2; shift 2; else shift; fi
done
test -n "$root"
mkdir -p "$root/bin"
cat > "$root/bin/yupana" <<'FAKE_YUPANA'
#!/usr/bin/env bash
case "${1:-}" in
  --version) echo 'yupana 0.6.4' ;;
  --help) printf 'Commands:\n  exemplar  draft policy\n  verifier  show key\n  verdicts  promote spool\n' ;;
  *) exit 0 ;;
esac
FAKE_YUPANA
chmod 0755 "$root/bin/yupana"
FAKE_CARGO
chmod 0755 "$tmp/tools/cargo"

export FAKE_CARGO_ARGS="$tmp/cargo.args"
YUPANA_INSTALL_ROOT="$tmp/prefix" CARGO_BIN="$tmp/tools/cargo" \
    "$(dirname "$0")/install-local.sh" >/dev/null

grep -q -- '--locked' "$FAKE_CARGO_ARGS"
grep -q -- '--all-features' "$FAKE_CARGO_ARGS"
grep -q -- '--force' "$FAKE_CARGO_ARGS"
test -x "$tmp/prefix/bin/yupana"
test -L "$tmp/prefix/bin/hank"
test "$(readlink "$tmp/prefix/bin/hank")" = yupana
test "$(readlink -f "$tmp/prefix/bin/hank")" = \
     "$(readlink -f "$tmp/prefix/bin/yupana")"

echo 'PASS: all-features install produced one executable under yupana and hank'
