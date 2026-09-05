#!/usr/bin/env bash
# Build privately, verify against this source, then atomically publish exact bytes.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
install_root=${YUPANA_INSTALL_ROOT:-${CARGO_INSTALL_ROOT:-$HOME/.local}}
cargo_bin=${CARGO_BIN:-cargo}
bin_dir="$install_root/bin"
canonical="$bin_dir/yupana"
legacy="$bin_dir/hank"
# An install is short-lived but a release build is large: use disk cache, not TMPDIR.
build_root=${YUPANA_INSTALL_BUILD_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/yupana/install-builds}
mkdir -p "$build_root" "$bin_dir"
build_dir=$(mktemp -d "$build_root/build.XXXXXX")
candidate=""
alias_tmp=""
cleanup() {
    test -z "$candidate" || rm -f -- "$candidate"
    test -z "$alias_tmp" || rm -f -- "$alias_tmp"
    rm -rf -- "$build_dir"
}
trap cleanup EXIT

# A wrapper can overwrite CARGO_TARGET_DIR. The command-line flag wins, and
# mktemp gives every concurrent installation its own output and Cargo lock.
"$cargo_bin" build --manifest-path "$repo_root/Cargo.toml" --locked --release \
    --all-features --bin yupana --example install-contract --target-dir "$build_dir" \
    --message-format=json > "$build_dir/artifacts.jsonl"
python3 "$repo_root/scripts/install-artifacts.py" "$build_dir/artifacts.jsonl" \
    "$build_dir" > "$build_dir/paths"
mapfile -t artifacts < "$build_dir/paths"
source_bin=${artifacts[0]}
contract=${artifacts[1]}

# Serialize publication, not compilation. The lock covers both names and the
# final readback, so another installer cannot replace them inside our check.
exec 9> "$bin_dir/.yupana-install.lock"
flock 9
test ! -d "$canonical" && test ! -d "$legacy" || {
    echo 'ERROR: an install destination is a directory' >&2; exit 1;
}
candidate=$(mktemp "$bin_dir/.yupana-candidate.XXXXXX")
install -m 0755 "$source_bin" "$candidate"
cmp -- "$source_bin" "$candidate"
# This checker comes from the SAME private build and derives its contract from
# Clap, including all nested subcommands. No list of old verbs can certify it.
"$contract" "$candidate"
sha=$(sha256sum "$candidate" | cut -d' ' -f1)
alias_tmp=$(mktemp "$bin_dir/.hank-yupana.XXXXXX")
rm -f -- "$alias_tmp"
ln -s yupana "$alias_tmp"
mv -Tf -- "$candidate" "$canonical"
candidate=""
mv -Tf -- "$alias_tmp" "$legacy"
alias_tmp=""
cmp -- "$source_bin" "$canonical"
test "$(readlink "$legacy")" = yupana
test "$(readlink -f "$legacy")" = "$(readlink -f "$canonical")"
"$contract" "$canonical"
printf 'Installed %s: %s\nSHA256: %s\n' "$("$canonical" --version)" "$canonical" "$sha"
printf 'Legacy alias: %s -> %s\n' "$legacy" "$(readlink "$legacy")"
