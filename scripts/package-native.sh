#!/usr/bin/env bash
# Build a native-engine candidate package: the pipkin binary (which hosts the built-in engine via
# `pipkin --engine-worker`), desktop entry, icons, notices, Rust license inventory, runtime
# inventory and build identity. No Node, npm or Pi checkout is used to build it or shipped in it.
# The Pi package (scripts/package.sh with no --engine flag) and its outputs are unchanged.
#
#   scripts/package-native.sh            -> dist/native/pipkin-native-<version>-<arch>.tar.zst
#   scripts/package.sh --engine native      (the same)                and dist/native/stage/
#
# PIPKIN_NATIVE_OUT=DIR puts the stage and archive in DIR instead of dist/native.
# Development candidate only: Pi remains the default engine; this is not a release.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
python3 scripts/verify-reqwest-vendor.py
python3 scripts/verify-regress-vendor.py
version=$(cargo pkgid -p pipkin-app | sed 's/.*[#@]//')
arch=$(uname -m)
target=$(rustc -vV | awk '/^host:/ {print $2}')
[ "$arch" = x86_64 ] && [ "$target" = x86_64-unknown-linux-gnu ] &&
  { [ -z "${CARGO_BUILD_TARGET:-}" ] || [ "$CARGO_BUILD_TARGET" = "$target" ]; } || {
  echo "the native Linux package supports native x86_64-unknown-linux-gnu builds only (got $arch/$target/${CARGO_BUILD_TARGET:-native})" >&2; exit 1;
}
out=${PIPKIN_NATIVE_OUT:-$here/dist/native}
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$out/stage
archive=$out/pipkin-native-$version-$arch.tar.zst

# The engine's Welcome reports PIPKIN_REVISION (compile-time); give it the app's source identity,
# marking a dirty tree, so a probe of the installed worker names the source it was built from.
revision=$(git rev-parse --verify HEAD 2>/dev/null || echo unknown)
if [ "$revision" != unknown ] && [ -n "$(git status --porcelain --untracked-files=normal)" ]; then
  revision=$revision-dirty
fi
# The offline inventory resolves the workspace graph, including target dev dependencies
# a release-only build may not download on a fresh machine. Respect CARGO_NET_OFFLINE.
cargo fetch --locked --target "$target"
PIPKIN_REVISION=$revision cargo build -p pipkin-app --release --locked

rm -rf "$stage"
install -Dm755 target/release/pipkin "$stage/usr/bin/pipkin"
# Identify the candidate; the marker never changes which engine runs.
"$stage/usr/bin/pipkin" --version --engine native | sed -E \
  's/^pipkin ([^ ]+) \(.*engine native ([^,]+), engine protocol ([0-9]+), engine schema ([0-9]+), schema ([0-9]+)\)$/{\n  "formatVersion": 1,\n  "engine": "native",\n  "selector": "--engine native",\n  "appVersion": "\1",\n  "engineVersion": "\2",\n  "engineProtocol": \3,\n  "engineSchema": \4,\n  "desktopSchema": \5\n}/' \
  > "$out/native-engine.json"
grep -q '"engineProtocol": [0-9]' "$out/native-engine.json" || { echo 'cannot identify the native engine' >&2; exit 1; }
install -Dm644 "$out/native-engine.json" "$stage/usr/lib/pipkin/native-engine.json"
rm "$out/native-engine.json"
install -d "$stage/usr/share/applications"
sed -e 's/^Exec=pipkin$/Exec=pipkin --engine native/' \
  -e 's/^Comment=.*/Comment=Native desktop coding agent client (built-in engine candidate)/' \
  packaging/pipkin.desktop > "$stage/usr/share/applications/pipkin.desktop"
grep -qx 'Exec=pipkin --engine native' "$stage/usr/share/applications/pipkin.desktop"
for size in 128 256 512; do
  install -Dm644 "packaging/pipkin-$size.png" "$stage/usr/share/icons/hicolor/${size}x${size}/apps/pipkin.png"
done
install -Dm644 LICENSE "$stage/usr/share/licenses/pipkin/LICENSE"
install -Dm644 packaging/upstream-notices/pi/LICENSE "$stage/usr/share/licenses/pipkin/pi-MIT-LICENSE"
install -Dm644 packaging/upstream-notices/anthropic-sdk/LICENSE "$stage/usr/share/licenses/pipkin/anthropic-sdk-MIT-LICENSE"
install -Dm644 packaging/upstream-notices/minimatch-10.2.6/LICENSE.md "$stage/usr/share/licenses/pipkin/minimatch-BlueOak-LICENSE"
install -Dm644 packaging/upstream-notices/brace-expansion-5.0.12/LICENSE "$stage/usr/share/licenses/pipkin/brace-expansion-MIT-LICENSE"
install -Dm644 assets/icons/LICENSE "$stage/usr/share/licenses/pipkin/icons-LICENSE"
for f in Poppins-OFL.txt Lilex-OFL.txt IBMPlexSans-LICENSE.txt; do
  install -Dm644 "assets/fonts/$f" "$stage/usr/share/licenses/pipkin/$f"
done
install -Dm644 assets/PROVENANCE.md "$stage/usr/share/doc/pipkin/PROVENANCE.md"
install -Dm644 packaging/native-README.md "$stage/usr/share/doc/pipkin/README.md"
install -Dm644 SECURITY.md "$stage/usr/share/doc/pipkin/SECURITY.md"
for guide in docs/*.md; do
  install -Dm644 "$guide" "$stage/usr/share/doc/pipkin/$guide"
done
install -Dm644 assets/PROVENANCE.md "$stage/usr/share/doc/pipkin/assets/PROVENANCE.md"
sh "$here/scripts/write-native-build-info.sh" "$stage"
python3 "$here/scripts/write-license-inventory.py" "$stage"
python3 "$here/scripts/write-runtime-inventory.py" "$stage" --native-engine

# Fail closed if anything from the Node/Pi package found its way in.
forbidden=$(cd "$stage" && find . \( -path ./usr/lib/pipkin/engine -o -path ./usr/lib/pipkin/runtime \
  -o -name node -o -name npm -o -name npx -o -name node_modules -o -name pi-test.sh \
  -o -name package.json -o -name package-lock.json -o -name '*.js' -o -name '*.mjs' -o -name '*.cjs' \
  -o -name '*.ts' -o -name '*.node' \) -print)
[ -z "$forbidden" ] || { echo "native package contains Node/Pi payload:" >&2; echo "$forbidden" >&2; exit 1; }
grep -q '"ecosystem": "npm"' "$stage/usr/share/doc/pipkin/third-party/inventory.json" &&
  { echo 'native license inventory lists npm packages' >&2; exit 1; }

# The archive carries the installer at its root, like the Pi archive.
tar --zstd -cf "$archive" -C "$stage" . -C "$here/packaging" install.sh
echo "built $archive"
echo "installed tree: $(du -sb "$stage" | awk '{print $1}') bytes; archive: $(stat -c %s "$archive") bytes"
