#!/usr/bin/env bash
# Build (release), package and install the native-engine candidate into a scratch prefix, then run
# the installed binary with a bare environment. Asserts the package carries no Node/npm/Pi payload,
# that installed migration/coding/restart works without node/npm/pi, and that Pi stays default.
#
#   scripts/test-native-package.sh                 build + package + install + checks
#   scripts/test-native-package.sh --no-build TAR  reuse an existing native archive
#
# Nothing outside the scratch directory is touched: every run uses env -i with a scratch HOME.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(mktemp -d /tmp/pk-native-XXXXXX)
trap 'rm -rf "$root"' EXIT
if [ "${1:-}" = --no-build ]; then
  tarball=${2:?--no-build needs an archive}
else
  PIPKIN_NATIVE_OUT="$root/dist" "$here/scripts/package-native.sh"
  tarball=$(ls "$root"/dist/pipkin-native-*.tar.zst)
fi
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }

tar --zstd -tf "$tarball" > "$root/listing"
check "archive has the binary, marker, desktop entry, notices and inventories" \
  "grep -qx './usr/bin/pipkin' '$root/listing' && grep -qx './usr/lib/pipkin/native-engine.json' '$root/listing' &&
   grep -qx './usr/share/applications/pipkin.desktop' '$root/listing' && grep -qx './usr/share/licenses/pipkin/LICENSE' '$root/listing' &&
   grep -qx './usr/share/doc/pipkin/third-party/inventory.json' '$root/listing' &&
   grep -qx './usr/share/doc/pipkin/runtime-inventory.json' '$root/listing' &&
   grep -qx './usr/share/doc/pipkin/build-info.json' '$root/listing' && grep -Eqx '(\./)?install\.sh' '$root/listing'"
pattern='(^|/)(node|npm|npx|node_modules|pi-test\.sh|package(-lock)?\.json)(/|$)|\.(m?js|cjs|ts|node)$|^\./usr/lib/pipkin/(engine|runtime)(/|$)'
check "archive has no Node, npm or Pi engine files" "! grep -Eq '$pattern' '$root/listing'"

mkdir -p "$root/pkg" "$root/home" "$root/shims"
tar --zstd -C "$root/pkg" -xf "$tarball"
check "installed help is the current source documentation" \
  "python3 '$here/scripts/test-package-docs.py' '$root/pkg' --native >/dev/null"
check "native Pi source port includes the exact upstream MIT notice" \
  "cmp '$here/packaging/upstream-notices/pi/LICENSE' '$root/pkg/usr/share/licenses/pipkin/pi-MIT-LICENSE'"
check "native federation source port includes the exact Anthropic SDK notice" \
  "cmp '$here/packaging/upstream-notices/anthropic-sdk/LICENSE' '$root/pkg/usr/share/licenses/pipkin/anthropic-sdk-MIT-LICENSE'"
check "license inventory lists Rust packages only" \
  "python3 -I -c 'import json,sys; r=json.load(open(sys.argv[1])); e={p[\"ecosystem\"] for p in r[\"packages\"]}; assert e=={\"rust\"}, e' '$root/pkg/usr/share/doc/pipkin/third-party/inventory.json'"
check "runtime inventory names the built-in engine and no Node" \
  "python3 -I -c 'import json,sys; r=json.load(open(sys.argv[1])); assert r[\"requiredNodeMinimum\"] is None; assert [a[\"file\"] for a in r[\"artifacts\"]]==[\"usr/bin/pipkin\"]' '$root/pkg/usr/share/doc/pipkin/runtime-inventory.json'"
check "build info identifies the binary and a native engine" \
  "python3 -I -c 'import json,sys,hashlib; i=json.load(open(sys.argv[1])); assert i[\"kind\"]==\"native-engine-candidate\" and i[\"inputs\"][\"node\"] is None; assert i[\"app\"][\"binarySha256\"]==hashlib.sha256(open(sys.argv[2],\"rb\").read()).hexdigest()' '$root/pkg/usr/share/doc/pipkin/build-info.json' '$root/pkg/usr/bin/pipkin'"

# Any node/npm/npx/pi invocation is recorded and fails.
for tool in node npm npx pi; do
  printf '#!/bin/sh\ntouch "%s/used-%s"\nexit 93\n' "$root" "$tool" > "$root/shims/$tool"
  chmod +x "$root/shims/$tool"
done
prefix="$root/prefix with spaces"
run() { env -i HOME="$root/home" PATH="$root/shims:/usr/bin:/bin" "$@"; }
bare() { env -i HOME="$root/home" PATH=/usr/bin:/bin "$@"; }
mkdir -p "$root/home/.local/share/pipkin"
echo keep > "$root/home/.local/share/pipkin/draft-marker"
run "$root/pkg/install.sh" --prefix "$prefix" > "$root/install.log" 2>&1 || { cat "$root/install.log"; fail=1; }
sed 's/^/     | /' "$root/install.log"
check "installer used the native path" "grep -q 'no Node runtime needed' '$root/install.log' && grep -q 'built-in native engine verified' '$root/install.log'"
check "installed version is named as native" "run '$root/pkg/install.sh' --prefix '$prefix' --list | grep -q -- '-native-.*(current)'"
installed="$prefix/lib/pipkin/current"
check "installed tree has no Node, npm or Pi engine files" \
  "! (cd '$installed/' && find . -print) | grep -Eq '$pattern'"
check "desktop entry launches the native engine explicitly" \
  "grep -Fqx 'Exec=\"$prefix/bin/pipkin\" --engine native' '$prefix/share/applications/pipkin.desktop'"
if command -v desktop-file-validate >/dev/null; then
  check "desktop entry validates" "desktop-file-validate '$prefix/share/applications/pipkin.desktop'"
fi

version=$(bare "$prefix/bin/pipkin" --version)
echo "     | $version"
check "installed --version (Pi-default line) runs with a bare PATH" "echo \"\$version\" | grep -q '^pipkin .*protocol [0-9]*, schema [0-9]*)$'"
native_version=$(bare "$prefix/bin/pipkin" --version --engine native)
echo "     | $native_version"
check "installed --version --engine native names the engine" "echo \"\$native_version\" | grep -q 'engine native .*engine protocol [0-9]*, engine schema [0-9]*'"
set +e
diag=$(cd "$root" && bare "$prefix/bin/pipkin" --diagnose --probe --engine native 2>&1); code=$?
set -e
echo "$diag" | sed 's/^/     | /'
check "native --diagnose --probe exits 0 with a bare PATH" "[ $code = 0 ]"
check "the probe shook hands and listed sessions" "echo \"\$diag\" | grep -q 'engine probe: OK (hello: protocol .*sessions in a fresh profile: 0'"
check "the report identifies the installed native candidate" "echo \"\$diag\" | grep -q 'installed as: native candidate'"
check "native engine directories are separate from Pi's" "echo \"\$diag\" | grep -q 'engine data directory: ~/.local/share/pipkin/engine'"
check "the probe left no engine data in the real profile" "[ ! -e '$root/home/.local/share/pipkin/engine' ]"
diag_shim=$(cd "$root" && run "$prefix/bin/pipkin" --diagnose --probe --engine native 2>&1) || true
check "with node/npm/pi shims first on PATH the probe still passes" "echo \"\$diag_shim\" | grep -q 'engine probe: OK'"
check "installed native migration, coding tools and restart work" \
  "run python3 '$here/scripts/test-installed-native-workflow.py' '$prefix/bin/pipkin' '$here/fixtures/native-fork-import'"
check "nothing invoked node, npm, npx or pi" "[ -z \"\$(ls '$root' | grep '^used-')\" ]"
set +e
default=$(cd "$root" && bare "$prefix/bin/pipkin" --diagnose 2>&1); default_code=$?
set -e
check "Pi stays the default: plain --diagnose looks for a Pi engine and points at --engine native" \
  "[ $default_code != 0 ] && echo \"\$default\" | grep -q 'engine: NOT FOUND' && echo \"\$default\" | grep -q 'diagnose --engine native'"

echo "sizes (bytes):"
echo "     | binary            $(stat -c %s "$installed/usr/bin/pipkin")"
echo "     | installed version $(du -sb "$(readlink -f "$installed")" | awk '{print $1}')"
echo "     | archive           $(stat -c %s "$tarball")"

run "$root/pkg/install.sh" --prefix "$prefix" --uninstall >/dev/null
check "uninstall removes the links and versions" "[ ! -e '$prefix/bin/pipkin' ] && [ ! -d '$prefix/lib/pipkin' ]"
check "app data survives install and uninstall" "[ -f '$root/home/.local/share/pipkin/draft-marker' ]"
[ $fail = 0 ] && echo "native package verified" || { echo "native package NOT verified"; exit 1; }
