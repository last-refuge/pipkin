#!/usr/bin/env bash
# Native build-info manifest and privacy checks, without Node, a real profile or credentials.
# The counterpart of scripts/test-build-info.sh for scripts/write-native-build-info.sh.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
stage=$root/stage
mkdir -p "$stage/usr/bin" "$root/shims"
rev=0123456789abcdef0123456789abcdef01234567
cat > "$stage/usr/bin/pipkin" <<SH
#!/bin/sh
[ "\$*" = '--version --engine native' ] || exit 64
echo 'pipkin 0.0.1 (source $rev, dirty: true, engine native 0.0.1, engine protocol 1, engine schema 1, schema 9)'
SH
chmod +x "$stage/usr/bin/pipkin"
# Any attempt to use Node/npm is recorded and fails.
for tool in node npm npx; do
  printf '#!/bin/sh\ntouch "%s/used-%s"\nexit 93\n' "$root" "$tool" > "$root/shims/$tool"
  chmod +x "$root/shims/$tool"
done
PATH="$root/shims:$PATH" RUSTFLAGS='private-secret-not-for-reports' \
  sh "$here/scripts/write-native-build-info.sh" "$stage" >/dev/null
for tool in node npm npx; do
  [ ! -e "$root/used-$tool" ] || { echo "FAIL the native writer ran $tool"; exit 1; }
done
python3 -I - "$stage/usr/share/doc/pipkin/build-info.json" "$rev" <<'PY'
import json, re, sys
text = open(sys.argv[1]).read()
info = json.loads(text)
assert info['kind'] == 'native-engine-candidate'
assert info['app']['revision'] == sys.argv[2] and info['app']['dirty'] == 'true'
assert re.fullmatch(r'[0-9a-f]{64}', info['app']['binarySha256'])
assert info['compatibility']['engineProtocol'] == 1 and info['compatibility']['engineSchema'] == 1
assert info['compatibility']['desktopSchema'] == 9 and info['compatibility']['nodeMinimum'] is None
assert info['engine']['builtIntoBinary'] is True and info['engine']['bundledNode'] is False
assert info['engine']['sourceRevision'] == sys.argv[2]
assert info['inputs']['node'] is None and info['inputs']['npm'] is None
assert info['inputs']['customRustFlags'] is True
assert re.fullmatch(r'[0-9a-f]{64}', info['inputs']['cargoLockSha256'])
assert 'private-secret-not-for-reports' not in text
PY
# Without custom flags, and with a bare PATH (rustc/cargo may be absent: recorded as null).
env -i PATH=/usr/bin:/bin sh "$here/scripts/write-native-build-info.sh" "$stage" >/dev/null
python3 -I -c 'import json,sys; i=json.load(open(sys.argv[1])); assert i["inputs"]["customRustFlags"] is False' \
  "$stage/usr/share/doc/pipkin/build-info.json"
# An unidentifiable binary is refused and leaves no partial manifest behind.
printf '#!/bin/sh\necho pipkin 0.0.1\n' > "$stage/usr/bin/pipkin"
rm "$stage/usr/share/doc/pipkin/build-info.json"
if sh "$here/scripts/write-native-build-info.sh" "$stage" 2>/dev/null; then
  echo 'FAIL an unidentified binary was accepted'; exit 1
fi
[ ! -e "$stage/usr/share/doc/pipkin/build-info.json" ] || { echo 'FAIL partial manifest written'; exit 1; }
echo 'native build identity, no-Node generation and flag redaction verified'
