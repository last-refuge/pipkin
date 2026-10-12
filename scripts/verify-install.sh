#!/usr/bin/env bash
# Check that a built package works on its own, away from every source checkout.
#
#   scripts/verify-install.sh [TARBALL]        (default: the newest dist/pipkin-*.tar.zst)
#   scripts/verify-install.sh --full [TARBALL] also runs the real-engine workflow tests against the
#                                              unpacked engine (needs the repository to run them)
#
# It unpacks into a scratch prefix, then runs the installed binary with a bare environment: an empty
# HOME, a minimal PATH, no PIPKIN_ENGINE_DIR, and a working directory that is not a checkout. It then
# checks the desktop entry and icon, that no link escapes the prefix, and that `--diagnose --probe`
# finds the bundled engine through the binary's own location and gets a handshake from it.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
full=0; [ "${1:-}" = "--full" ] && { full=1; shift; }
tarball=${1:-$(ls -t "$here"/dist/pipkin-*.tar.zst | head -1)}
root=$(mktemp -d /tmp/pk-verify-XXXXXX)
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/prefix" "$root/home" "$root/cwd"
tar --zstd -C "$root/prefix" -xf "$tarball"
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }

check "binary is executable" "[ -x '$root/prefix/usr/bin/pipkin' ]"
check "engine manifest present" "[ -f '$root/prefix/usr/lib/pipkin/engine/engine.json' ]"
check "engine requires private Node" "grep -q '\"requiresBundledNode\"[[:space:]]*:[[:space:]]*true' '$root/prefix/usr/lib/pipkin/engine/engine.json'"
check "private Node and notices are present" "[ -x '$root/prefix/usr/lib/pipkin/runtime/bin/node' ] && [ -s '$root/prefix/usr/lib/pipkin/runtime/LICENSE' ] && [ -f '$root/prefix/usr/lib/pipkin/runtime/runtime.json' ]"
check "desktop entry present" "grep -q '^Exec=pipkin' '$root/prefix/usr/share/applications/pipkin.desktop'"
check "the MIT licence is installed" "grep -q '^MIT License' '$root/prefix/usr/share/licenses/pipkin/LICENSE'"
check "icon present" "[ -f '$root/prefix/usr/share/icons/hicolor/256x256/apps/pipkin.png' ]"
if command -v desktop-file-validate >/dev/null; then
  check "desktop entry validates" "desktop-file-validate '$root/prefix/usr/share/applications/pipkin.desktop'"
fi
check "no link points outside the prefix" \
  "! find '$root/prefix' -type l -lname '/*' | grep -q ."
check "no escaping relative link" \
  "! find '$root/prefix' -xtype l | grep -q ."
check "no reference to a source checkout in the binary" \
  "! strings '$root/prefix/usr/bin/pipkin' | grep -q '/coding/pi-fork'"

mkdir -p "$root/shims"
printf '#!/bin/sh\nexit 93\n' > "$root/shims/node"
chmod +x "$root/shims/node"
# An unusable system Node cannot accidentally satisfy the handshake.
run() { (cd "$root/cwd" && env -i HOME="$root/home" PATH="$root/shims:/usr/bin:/bin" XDG_DATA_HOME="$root/home/.local/share" \
  WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-1}" "$root/prefix/usr/bin/pipkin" "$@"); }
check "--version prints the version" "run --version | grep -q '^pipkin '"
out=$(run --diagnose --probe 2>&1) || true
echo "$out" | sed 's/^/     | /'
check "engine found next to the binary" "echo \"\$out\" | grep -q 'engine: pi .* at $root/prefix/usr/lib/pipkin/engine'"
check "engine probe answered a handshake" "echo \"\$out\" | grep -q 'engine probe: OK'"
check "no problems reported" "! echo \"\$out\" | grep -qE 'PROBLEM|NOT FOUND|FAILED'"

if [ $full = 1 ]; then
  echo "running the real-engine workflow tests against the unpacked engine..."
  # The cross-package rollback test requires both archives and runs in its own CI step.
  (cd "$here" && PIPKIN_PI_REPO="$root/prefix/usr/lib/pipkin/engine" \
    cargo test -p pipkin-app --release --locked e2e -- --ignored --test-threads=1 \
      --skip installed_pi_native_rollback_preserves_history_and_requires_ownership_release --skip packaged_engines_matched_performance 2>&1)
fi
[ $fail = 0 ] && echo "install verified" || { echo "install NOT verified"; exit 1; }
