#!/bin/sh
# Record a native-engine candidate's identity and public build inputs, without Node.
#
#   scripts/write-native-build-info.sh STAGE      -> STAGE/usr/share/doc/pipkin/build-info.json
#
# POSIX sh plus coreutils. The identity comes from the staged binary itself
# (`pipkin --version --engine native`, stamped by crates/pipkin-app/build.rs and the linked
# engine crates' constants). Arbitrary environment values are never recorded: custom Rust
# flags are reported only as present/absent. The Pi package keeps scripts/write-build-info.mjs.
set -eu
[ $# -eq 1 ] || { echo 'usage: write-native-build-info.sh STAGE' >&2; exit 2; }
here=$(cd "$(dirname "$0")/.." && pwd)
stage=$(cd "$1" && pwd)
binary=$stage/usr/bin/pipkin
[ -x "$binary" ] || { echo "no executable at $binary" >&2; exit 1; }

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}
# A JSON string literal: escape backslashes and quotes, turn tabs into spaces, drop other
# control characters, and join lines with \n.
json() {
  printf '%s' "$1" | tr '\011' ' ' | tr -d '\001-\010\013-\037' |
    sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' |
    awk 'BEGIN { ORS = ""; printf "\"" } NR > 1 { printf "\\n" } { print } END { printf "\"" }'
}
# A tool's version output, or null when the tool is absent (never fatal).
tool() {
  if command -v "$1" >/dev/null 2>&1 && out=$("$@" 2>/dev/null); then json "$out"; else printf null; fi
}

line=$("$binary" --version --engine native)
pattern='^pipkin ([^ ]+) \(source ([^,]+), dirty: (true|false|unknown), engine native ([^,]+), engine protocol ([0-9]+), engine schema ([0-9]+), schema ([0-9]+)\)$'
printf '%s\n' "$line" | grep -Eq "$pattern" || {
  echo "binary has no identifiable native build stamp: $line" >&2; exit 1;
}
field() { printf '%s\n' "$line" | sed -E "s/$pattern/\\$1/"; }
version=$(field 1); revision=$(field 2); dirty=$(field 3)
engine_version=$(field 4); engine_protocol=$(field 5); engine_schema=$(field 6); desktop_schema=$(field 7)

custom_flags=false
if [ -n "${RUSTFLAGS:-}" ] || [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then custom_flags=true; fi
platform=$(uname -s | tr '[:upper:]' '[:lower:]')
architecture=$(uname -m)

out=$stage/usr/share/doc/pipkin/build-info.json
mkdir -p "$(dirname "$out")"
cat > "$out.tmp" <<JSON
{
  "formatVersion": 1,
  "kind": "native-engine-candidate",
  "app": {
    "version": $(json "$version"),
    "revision": $(json "$revision"),
    "dirty": $(json "$dirty"),
    "binarySha256": $(json "$(sha256 "$binary")")
  },
  "compatibility": {
    "engine": "native",
    "engineSelector": "--engine native",
    "engineProtocol": $engine_protocol,
    "engineSchema": $engine_schema,
    "desktopSchema": $desktop_schema,
    "nodeMinimum": null
  },
  "engine": {
    "kind": "native",
    "builtIntoBinary": true,
    "version": $(json "$engine_version"),
    "sourceRevision": $(json "$revision"),
    "bundledNode": false,
    "piEngine": false
  },
  "inputs": {
    "cargoLockSha256": $(json "$(sha256 "$here/Cargo.lock")"),
    "rustToolchainSha256": $(json "$(sha256 "$here/rust-toolchain.toml")"),
    "rustc": $(tool rustc -vV),
    "cargo": $(tool cargo --version),
    "python": $(tool python3 --version),
    "node": null,
    "npm": null,
    "platform": $(json "$platform"),
    "architecture": $(json "$architecture"),
    "customRustFlags": $custom_flags
  }
}
JSON
mv "$out.tmp" "$out"
echo "build info: $version ($revision, dirty: $dirty), native engine $engine_version protocol $engine_protocol schema $engine_schema"
