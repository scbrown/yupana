#!/usr/bin/env bash
# Install published bytes, never build the invoking checkout.
set -euo pipefail

usage() {
    echo 'Usage: install-release.sh VERSION'
    echo 'Install a checksummed published release (Linux x86_64). VERSION may start with v.'
    echo 'YUPANA_INSTALL_ROOT overrides the default ~/.local install prefix.'
}
if [[ ${1:-} == --help || ${1:-} == -h ]]; then usage; exit 0; fi
[[ $# == 1 && $1 =~ ^v?[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9.-]+)?$ ]] || { usage >&2; exit 2; }
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || {
    echo 'ERROR: no supported release archive for this platform' >&2; exit 1;
}
version=${1#v}
tag="v$version"
archive="yupana-$tag-x86_64-linux-gnu.tar.gz"
url="https://github.com/scbrown/yupana/releases/download/$tag/$archive"
install_root=${YUPANA_INSTALL_ROOT:-${CARGO_INSTALL_ROOT:-$HOME/.local}}
bin_dir="$install_root/bin"
mkdir -p "$bin_dir"
stage=$(mktemp -d)
candidate=""
alias_tmp=""
cleanup() {
    test -z "$candidate" || rm -f -- "$candidate"
    test -z "$alias_tmp" || rm -f -- "$alias_tmp"
    rm -rf -- "$stage"
}
trap cleanup EXIT
curl --fail --show-error --silent --location --proto '=https' --tlsv1.2 \
    --max-time 300 "$url" --output "$stage/$archive"
curl --fail --show-error --silent --location --proto '=https' --tlsv1.2 \
    --max-time 30 "$url.sha256" --output "$stage/$archive.sha256"
# Validate the checksum's filename and extract only a regular binary. Never
# unpack archive paths or follow an archive-supplied link on the host.
python3 - "$stage" "$archive" <<'PY'
import hashlib, pathlib, re, sys, tarfile
root, name = pathlib.Path(sys.argv[1]), sys.argv[2]
fields = (root / (name + '.sha256')).read_text().split()
if len(fields) != 2 or not re.fullmatch(r'[0-9a-fA-F]{64}', fields[0]) or fields[1].lstrip('*') != name:
    raise SystemExit('ERROR: invalid release checksum manifest')
if hashlib.sha256((root / name).read_bytes()).hexdigest() != fields[0].lower():
    raise SystemExit('ERROR: release checksum mismatch')
with tarfile.open(root / name, 'r:gz') as archive:
    members = archive.getmembers()
    if sorted(m.name for m in members) != ['hank', 'yupana']:
        raise SystemExit('ERROR: unexpected release archive members')
    binary, alias = archive.getmember('yupana'), archive.getmember('hank')
    if not binary.isfile() or not alias.issym() or alias.linkname != 'yupana':
        raise SystemExit('ERROR: invalid release binary or compatibility alias')
    (root / 'yupana').write_bytes(archive.extractfile(binary).read())
PY
chmod 0755 "$stage/yupana"
verify() {
    [[ $("$1" --version) == "yupana $version" ]] || {
        echo 'ERROR: release version mismatch' >&2; return 1;
    }
    local capability
    for capability in exemplar verifier verdicts; do "$1" "$capability" --help >/dev/null; done
}
verify "$stage/yupana"
exec 9> "$bin_dir/.yupana-install.lock"
flock 9
canonical="$bin_dir/yupana"
legacy="$bin_dir/hank"
test ! -d "$canonical" && test ! -d "$legacy" || {
    echo 'ERROR: an install destination is a directory' >&2; exit 1;
}
candidate=$(mktemp "$bin_dir/.yupana-candidate.XXXXXX")
install -m 0755 "$stage/yupana" "$candidate"
cmp -- "$stage/yupana" "$candidate"
verify "$candidate"
alias_tmp=$(mktemp "$bin_dir/.hank-yupana.XXXXXX")
rm -f -- "$alias_tmp"
ln -s yupana "$alias_tmp"
mv -Tf -- "$candidate" "$canonical"
candidate=""
mv -Tf -- "$alias_tmp" "$legacy"
alias_tmp=""
cmp -- "$stage/yupana" "$canonical"
test "$(readlink "$legacy")" = yupana
verify "$canonical"
printf 'Installed release %s from %s\n' "$tag" "$url"
sha256sum "$canonical"
printf 'Legacy alias: %s -> %s\n' "$legacy" "$(readlink "$legacy")"
