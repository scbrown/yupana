#!/usr/bin/env bash
# arming: ci — Release.publish after the native archive matrix succeeds.
# Keep latest on the prior complete release while assets are being uploaded.
set -euo pipefail
tag=${1:?usage: publish-release-assets.sh VERSION_TAG (from the asset directory)}
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.+-][A-Za-z0-9.+-]+)?$ ]] || {
  echo 'invalid release tag' >&2; exit 1;
}

retry() {
  local n=1 delay
  until "$@"; do
    [ "$n" -lt 12 ] || { echo "giving up after $n attempts: $*" >&2; return 1; }
    delay=$((n * 15)); [ "$delay" -le 60 ] || delay=60
    echo "attempt $n failed: $* — retrying in ${delay}s" >&2
    sleep "$delay"
    n=$((n + 1))
  done
}

expected=()
sidecars=()
for platform in x86_64-linux-gnu aarch64-apple-darwin x86_64-apple-darwin; do
  archive="yupana-${tag}-${platform}.tar.gz"
  test -f "$archive" && test -s "$archive"
  test -f "$archive.sha256" && test -s "$archive.sha256"
  expected+=("$archive" "$archive.sha256")
  sidecars+=("$archive.sha256")
done
archives=(*.tar.gz)
test "${#archives[@]}" -eq 3
# Bind each checksum to its own archive before sha256sum reads any paths.
python3 - "${sidecars[@]}" <<'PY' > SHA256SUMS
import pathlib, re, sys
for name in sys.argv[1:]:
    text = pathlib.Path(name).read_text()
    match = re.fullmatch(r'([a-fA-F0-9]{64}) [ *]([^\n]+)\n?', text)
    if not match or match[2] != name.removesuffix('.sha256'):
        sys.exit(f'invalid checksum sidecar: {name}')
    print(f'{match[1]}  {match[2]}')
PY
sha256sum --check --strict SHA256SUMS
expected+=(SHA256SUMS)
state=$(mktemp)
trap 'rm -f "$state"' EXIT

view_release() {
  gh release view "$tag" --json isDraft,isPrerelease,assets,tagName > "$state"
}
check_remote() {
  view_release || return
  python3 - "$state" "$tag" "$1" "${expected[@]}" <<'PY'
import json, pathlib, sys
data = json.loads(pathlib.Path(sys.argv[1]).read_text())
if data.get('tagName') != sys.argv[2] or data.get('isDraft') is not (sys.argv[3] == 'true'):
    sys.exit('release tag/draft state changed; refusing to publish')
assets = {a['name']: a for a in data.get('assets', [])}
for name in sys.argv[4:]:
    asset = assets.get(name, {})
    if asset.get('state') != 'uploaded' or asset.get('size') != pathlib.Path(name).stat().st_size:
        sys.exit(f'remote asset is missing, unfinished, or the wrong size: {name}')
PY
}

if ! view_release; then
  retry gh release create "$tag" --draft --verify-tag --title "$tag" --generate-notes
  retry view_release
fi
# A malformed API reply is not a public release and never a successful no-op.
flags=$(python3 - "$state" "$tag" <<'PY'
import json, pathlib, sys
d = json.loads(pathlib.Path(sys.argv[1]).read_text())
if d.get('tagName') != sys.argv[2] or any(type(d.get(k)) is not bool for k in ('isDraft', 'isPrerelease')):
    sys.exit('invalid release metadata')
print(str(d['isDraft']).lower(), str(d['isPrerelease']).lower())
PY
)
read -r draft prerelease <<< "$flags"
if [ "$draft" = false ]; then
  # Re-runs must not clobber public assets and recreate a missing-asset window.
  # A legacy partial public release needs to be made draft before recovery.
  check_remote false || { echo 'existing public release is incomplete; restore draft state before retrying' >&2; exit 1; }
  echo "${tag} is already published with the complete asset set"
  exit 0
fi
retry gh release upload "$tag" "${expected[@]}" --clobber
retry check_remote true
latest=--latest
[ "$prerelease" = false ] || latest=--latest=false
# This is the ONLY publication step, after all seven uploads are observed.
retry gh release edit "$tag" --draft=false "$latest"
retry check_remote false
echo "published all platform archives on ${tag}"
