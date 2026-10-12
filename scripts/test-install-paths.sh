#!/usr/bin/env bash
# Fixture test for packaging/install.sh's two archive shapes, without building anything: a
# Pi-shaped archive (private Node + paired engine) and a native-engine candidate (no Node). Fake
# binaries stand in for pipkin/node, so this checks the installer's branching, desktop entry,
# probe arguments, mixed-version rollback and refusals, not a real engine (test-install.sh and
# test-native-package.sh do that with real archives).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(mktemp -d /tmp/pk-paths-XXXXXX)
trap 'rm -rf "$root"' EXIT
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
sha() { sha256sum "$1" | awk '{print $1}'; }

fake_pipkin() { # $1 = archive root, $2 = build tag (changes the binary hash)
  install -d "$1/usr/bin" "$1/usr/share/applications" "$1/usr/share/doc/pipkin"
  cat > "$1/usr/bin/pipkin" <<SH
#!/bin/sh
# build $2
lib=\$(dirname "\$0")/../lib/pipkin
case "\$*" in
  '--version') echo 'pipkin 0.0.1 (source fixture, dirty: false, protocol 8, schema 7)' ;;
  '--version --engine native') echo 'pipkin 0.0.1 (source fixture, dirty: false, engine native 0.0.1, engine protocol 1, engine schema 1, schema 7)' ;;
  '--diagnose --probe') [ -d "\$lib/engine" ] && echo 'engine probe: OK pi' ;;
  '--diagnose --probe --engine native') [ -f "\$lib/native-engine.json" ] && echo 'engine probe: OK native' ;;
  *) exit 2 ;;
esac
SH
  chmod +x "$1/usr/bin/pipkin"
  printf '{\n  "app": { "binarySha256": "%s" }\n}\n' "$(sha "$1/usr/bin/pipkin")" > "$1/usr/share/doc/pipkin/build-info.json"
  cp "$here/packaging/install.sh" "$1/install.sh"
}

pi=$root/pi
fake_pipkin "$pi" pi
cp "$here/packaging/pipkin.desktop" "$pi/usr/share/applications/pipkin.desktop"
install -d "$pi/usr/lib/pipkin/runtime/bin" "$pi/usr/lib/pipkin/engine/node_modules"
printf '#!/bin/sh\necho v22.23.3\n' > "$pi/usr/lib/pipkin/runtime/bin/node"
chmod +x "$pi/usr/lib/pipkin/runtime/bin/node"
printf '{\n  "version": "22.23.3",\n  "target": "x86_64-unknown-linux-gnu",\n  "binarySha256": "%s"\n}\n' \
  "$(sha "$pi/usr/lib/pipkin/runtime/bin/node")" > "$pi/usr/lib/pipkin/runtime/runtime.json"
printf '#!/bin/sh\n' > "$pi/usr/lib/pipkin/engine/pi-test.sh"
printf '{\n  "version": "pinfixture",\n  "requiresBundledNode": true,\n  "pipkinAuthBridgeSha256": "%064d"\n}\n' 0 \
  > "$pi/usr/lib/pipkin/engine/engine.json"

native=$root/native
fake_pipkin "$native" native
sed 's/^Exec=pipkin$/Exec=pipkin --engine native/' "$here/packaging/pipkin.desktop" > "$native/usr/share/applications/pipkin.desktop"
install -d "$native/usr/lib/pipkin"
printf '{"engine": "native"}\n' > "$native/usr/lib/pipkin/native-engine.json"

prefix="$root/prefix with spaces"
run() { env -i HOME="$root/home" PATH=/usr/bin:/bin "$@"; }
desktop=$prefix/share/applications/pipkin.desktop

run "$pi/install.sh" --prefix "$prefix" > "$root/pi.log" 2>&1 || { cat "$root/pi.log"; fail=1; }
check "Pi-shaped archive installs through the Node path" "grep -q 'Private Node 22.23.3 verified' '$root/pi.log' && grep -q 'paired Pi engine verified' '$root/pi.log'"
check "Pi desktop entry is unchanged: quoted binary, no flags" "grep -Fqx 'Exec=\"$prefix/bin/pipkin\"' '$desktop'"
pi_version=$(run "$pi/install.sh" --prefix "$prefix" --list | awk '/current/ {print $2}')
check "Pi version name carries the engine version" "[[ '$pi_version' == 0.0.1-pinfixture-* ]]"
sleep 1
run "$native/install.sh" --prefix "$prefix" > "$root/native.log" 2>&1 || { cat "$root/native.log"; fail=1; }
check "native archive installs without the Node path" "grep -q 'no Node runtime needed' '$root/native.log' && ! grep -q 'Private Node' '$root/native.log'"
check "the native probe ran with --engine native" "grep -q 'passed an offline handshake' '$root/native.log'"
check "native desktop entry keeps --engine native" "grep -Fqx 'Exec=\"$prefix/bin/pipkin\" --engine native' '$desktop'"
check "native version is current, Pi version retained" \
  "run '$native/install.sh' --prefix '$prefix' --list | grep -q -- '-native-.*(current)' && run '$native/install.sh' --prefix '$prefix' --list | grep -q '$pi_version'"
native_version=$(run "$native/install.sh" --prefix "$prefix" --list | awk '/current/ {print $2}')
prior="$prefix/lib/pipkin/versions/$pi_version"
cp "$prior/usr/bin/pipkin" "$root/prior-binary"
printf '\n# tampered rollback target\n' >> "$prior/usr/bin/pipkin"
if run "$native/install.sh" --prefix "$prefix" --rollback > "$root/rollback-tampered.log" 2>&1; then
  echo 'FAIL rollback activated a tampered prior binary'; fail=1
else
  check "rejected rollback leaves native current" \
    "run '$native/install.sh' --prefix '$prefix' --list | grep -q '$native_version (current)'"
fi
cp "$root/prior-binary" "$prior/usr/bin/pipkin"
# Restore native if the pre-fix installer activated the bad prior version, so the rest still runs.
run "$native/install.sh" --prefix "$prefix" > /dev/null
cp "$prior/usr/lib/pipkin/runtime/bin/node" "$root/prior-node"
printf '\n# tampered runtime\n' >> "$prior/usr/lib/pipkin/runtime/bin/node"
if run "$native/install.sh" --prefix "$prefix" --rollback > "$root/rollback-node.log" 2>&1; then
  echo 'FAIL rollback activated a tampered Node runtime'; fail=1
else
  check "rollback validates the retained Node runtime" "grep -q 'Node checksum mismatch' '$root/rollback-node.log'"
fi
cp "$root/prior-node" "$prior/usr/lib/pipkin/runtime/bin/node"
cp "$prior/usr/share/doc/pipkin/build-info.json" "$root/prior-info"
sed "s/echo 'engine probe: OK pi'/exit 91/" "$root/prior-binary" > "$prior/usr/bin/pipkin"
printf '{\n  "app": { "binarySha256": "%s" }\n}\n' "$(sha "$prior/usr/bin/pipkin")" > "$prior/usr/share/doc/pipkin/build-info.json"
if run "$native/install.sh" --prefix "$prefix" --rollback > "$root/rollback-probe.log" 2>&1; then
  echo 'FAIL rollback activated a target whose probe fails'; fail=1
else
  check "rollback requires the retained engine to answer" "grep -q 'failed its offline handshake' '$root/rollback-probe.log'"
  check "failed rollback probe preserves binary selection and native launcher" \
    "run '$native/install.sh' --prefix '$prefix' --list | grep -q '$native_version (current)' && grep -Fqx 'Exec=\"$prefix/bin/pipkin\" --engine native' '$desktop'"
fi
cp "$root/prior-binary" "$prior/usr/bin/pipkin"
cp "$root/prior-info" "$prior/usr/share/doc/pipkin/build-info.json"
run "$native/install.sh" --prefix "$prefix" --rollback > /dev/null
check "rollback returns to the Pi version" "run '$native/install.sh' --prefix '$prefix' --list | grep -q '$pi_version (current)'"
check "rollback restores the Pi desktop entry" "grep -Fqx 'Exec=\"$prefix/bin/pipkin\"' '$desktop'"

# A native archive must not smuggle a Pi engine/Node tree.
cp -a "$native" "$root/mixed"
install -d "$root/mixed/usr/lib/pipkin/engine"
if run "$root/mixed/install.sh" --prefix "$prefix" > "$root/mixed.log" 2>&1; then
  echo 'FAIL a native archive with a Pi engine directory was installed'; fail=1
else
  check "native archive carrying a Pi engine is refused" "grep -q 'must not also carry' '$root/mixed.log'"
fi
# A tampered native binary fails closed (checksum), before anything is copied.
cp -a "$native" "$root/tampered"
printf '\n# tampered\n' >> "$root/tampered/usr/bin/pipkin"
if run "$root/tampered/install.sh" --prefix "$prefix" > "$root/tampered.log" 2>&1; then
  echo 'FAIL a tampered native binary was installed'; fail=1
else
  check "tampered native binary fails closed" "grep -q 'executable checksum mismatch' '$root/tampered.log'"
fi
run "$pi/install.sh" --prefix "$prefix" --uninstall > /dev/null
check "uninstall removes both versions" "[ ! -e '$prefix/bin/pipkin' ] && [ ! -d '$prefix/lib/pipkin' ]"
[ $fail = 0 ] && echo "installer paths verified" || { echo "installer paths NOT verified"; exit 1; }
