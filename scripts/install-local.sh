#!/usr/bin/env bash
# Install one feature-complete Yupana build under both current and legacy names.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
install_root=${YUPANA_INSTALL_ROOT:-${CARGO_INSTALL_ROOT:-$HOME/.local}}
cargo_bin=${CARGO_BIN:-cargo}
bin_dir="$install_root/bin"
canonical="$bin_dir/yupana"
legacy="$bin_dir/hank"

mkdir -p "$bin_dir"
"$cargo_bin" install --path "$repo_root" --locked --root "$install_root" \
    --all-features --force

test -x "$canonical" || {
    echo "ERROR: cargo install produced no executable at $canonical" >&2
    exit 1
}

help=$($canonical --help)
for command in exemplar verifier verdicts; do
    printf '%s\n' "$help" | grep -Eq "^[[:space:]]+${command}([[:space:]]|$)" || {
        echo "ERROR: installed yupana lacks required command: $command" >&2
        exit 1
    }
done

# Create the alias off-path, then atomically replace any older regular binary.
alias_tmp=$(mktemp "$bin_dir/.hank-yupana.XXXXXX")
trap 'rm -f "$alias_tmp"' EXIT
rm -f "$alias_tmp"
ln -s yupana "$alias_tmp"
mv -Tf "$alias_tmp" "$legacy"
trap - EXIT

test "$(readlink "$legacy")" = yupana
test "$(readlink -f "$legacy")" = "$(readlink -f "$canonical")"
test "$($legacy --version)" = "$($canonical --version)"

echo "Installed $($canonical --version): $canonical"
echo "Legacy alias: $legacy -> $(readlink "$legacy")"
