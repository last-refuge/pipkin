#!/usr/bin/env bash
# Build an installable tree and tarball: binary, bundled engine, desktop entry, icon, licences.
#
#   scripts/package.sh [PI_CHECKOUT]       -> dist/pipkin-<version>-x86_64.tar.zst and dist/stage/
#
# The tree uses the /usr layout, so it can be unpacked at / (what the PKGBUILD does) or into any
# prefix: the binary finds the engine at ../lib/pipkin/engine relative to itself.
#   scripts/package.sh --engine native   -> the no-Node native-engine candidate (scripts/package-native.sh)
#   scripts/package.sh --engine pi [PI_CHECKOUT]   the same as no flag
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
if [ "${1:-}" = --engine ]; then
  case "${2:-}" in
    native) shift 2; [ $# -eq 0 ] || { echo 'scripts/package.sh --engine native takes no other arguments' >&2; exit 2; }
            exec "$here/scripts/package-native.sh" ;;
    pi) shift 2 ;;
    *) echo "unknown engine ${2:-(none)}; choose pi or native" >&2; exit 2 ;;
  esac
fi
cd "$here"
version=$(cargo pkgid -p pipkin-app | sed 's/.*[#@]//')
arch=$(uname -m)
stage=$here/dist/stage
pi=${1:-$here/../pi-fork/pi}
# Reject invalid sources before clearing prior staging or doing an expensive app build.
"$here/scripts/check-engine-source.sh" "$pi" >/dev/null
cargo build -p pipkin-app --release --locked
rm -rf "$stage"
"$here/scripts/build-engine.sh" "$pi" "$stage/usr/lib/pipkin/engine"
target=$(rustc -vV | awk '/^host:/ {print $2}')
[ "$arch" = x86_64 ] && [ "$target" = x86_64-unknown-linux-gnu ] &&
  { [ -z "${CARGO_BUILD_TARGET:-}" ] || [ "$CARGO_BUILD_TARGET" = "$target" ]; } || {
  echo "Linux installer supports native x86_64-unknown-linux-gnu builds only (got $arch/$target/${CARGO_BUILD_TARGET:-native})" >&2; exit 1;
}
python3 "$here/scripts/stage-node-runtime.py" "$target" "$stage/usr/lib/pipkin/runtime"
python3 - "$stage/usr/lib/pipkin/engine/engine.json" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
manifest = json.loads(path.read_text())
manifest['requiresBundledNode'] = True
path.write_text(json.dumps(manifest, indent=2) + '\n')
PY
install -Dm755 target/release/pipkin "$stage/usr/bin/pipkin"
install -Dm644 packaging/pipkin.desktop "$stage/usr/share/applications/pipkin.desktop"
for size in 128 256 512; do
  install -Dm644 "packaging/pipkin-$size.png" "$stage/usr/share/icons/hicolor/${size}x${size}/apps/pipkin.png"
done
install -Dm644 LICENSE "$stage/usr/share/licenses/pipkin/LICENSE"
install -Dm644 packaging/upstream-notices/anthropic-sdk/LICENSE "$stage/usr/share/licenses/pipkin/anthropic-sdk-MIT-LICENSE"
install -Dm644 assets/icons/LICENSE "$stage/usr/share/licenses/pipkin/icons-LICENSE"
for f in Poppins-OFL.txt Lilex-OFL.txt IBMPlexSans-LICENSE.txt; do
  install -Dm644 "assets/fonts/$f" "$stage/usr/share/licenses/pipkin/$f"
done
install -Dm644 assets/PROVENANCE.md "$stage/usr/share/doc/pipkin/PROVENANCE.md"
install -Dm644 packaging/README.md "$stage/usr/share/doc/pipkin/README.md"
install -Dm644 SECURITY.md "$stage/usr/share/doc/pipkin/SECURITY.md"
for guide in docs/*.md; do
  install -Dm644 "$guide" "$stage/usr/share/doc/pipkin/$guide"
done
install -Dm644 llm-docs/pipkin-v1-release-plan.md "$stage/usr/share/doc/pipkin/llm-docs/pipkin-v1-release-plan.md"
install -Dm644 assets/PROVENANCE.md "$stage/usr/share/doc/pipkin/assets/PROVENANCE.md"
install -Dm644 packaging/PKGBUILD "$stage/usr/share/doc/pipkin/packaging/PKGBUILD"
# Preserve the earlier installed manual location as well.
install -Dm644 docs/native-gate-testing.md "$stage/usr/share/doc/pipkin/native-gate-testing.md"
node "$here/scripts/write-build-info.mjs" "$stage" "$pi"
python3 "$here/scripts/write-license-inventory.py" "$stage"
python3 "$here/scripts/write-runtime-inventory.py" "$stage"
# The tarball also carries the installer at its root; the Arch package (which uses the stage tree
# directly) does not, since a package must not put files in /.
tar --zstd -cf "dist/pipkin-$version-$arch.tar.zst" -C "$stage" . -C "$here/packaging" install.sh
echo "built dist/pipkin-$version-$arch.tar.zst"
