#!/usr/bin/env bash
# Make a release: the tarball, SHA256SUMS, and (with PIPKIN_SIGN_KEY set to a gpg key id) a detached
# signature over SHA256SUMS. A downloader verifies with scripts/verify-release.sh.
#
#   scripts/release.sh [--engine native]       -> dist/release/
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
if [ "${1:-}" = --engine ] && [ "${2:-}" = native ]; then shift 2; fi
[ $# -eq 0 ] || { echo 'Release supports only the built-in native engine.' >&2; exit 2; }
revision=$(git rev-parse --verify HEAD 2>/dev/null) || { echo 'release requires identified app sources' >&2; exit 1; }
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] && [ -z "$(git status --porcelain --untracked-files=normal)" ] || {
  echo 'release requires a clean app checkout' >&2; exit 1;
}
version=$(cargo pkgid -p pipkin-app | sed 's/.*[#@]//')
arch=$(uname -m)
"$here/scripts/package.sh" "$@"
out=$here/dist/release
rm -rf "$out"; mkdir -p "$out"
# Collect only this build, not older versions/architectures left in dist.
cp "dist/native/pipkin-native-$version-$arch.tar.zst" "$out/"
(cd "$out" && sha256sum pipkin-*.tar.zst > SHA256SUMS)
if [ -n "${PIPKIN_SIGN_KEY:-}" ]; then
  gpg --batch --yes --local-user "$PIPKIN_SIGN_KEY" --armor --detach-sign --output "$out/SHA256SUMS.asc" "$out/SHA256SUMS"
  echo "signed SHA256SUMS with $PIPKIN_SIGN_KEY"
else
  echo "not signed (set PIPKIN_SIGN_KEY to a gpg key id to sign)"
fi
echo "release in $out:"; ls -1 "$out"
