#!/bin/sh
# Regenerate the differential goldens from the reference engine.
# Usage: sh regenerate.sh <pi-staged root>   (needs python3 and Node >= 23 for type stripping)
# Writes <name>.spec.json and <name>.golden.json next to this script. The reference is only
# imported (never modified); its npm packages are replaced by an inert stub module.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
export PI_STAGED="$1"
test -d "$PI_STAGED/packages/ai/src" || { echo "usage: regenerate.sh <pi-staged>" >&2; exit 2; }
python3 "$HERE/gen_specs.py" "$HERE" > /dev/null
for spec in "$HERE"/*.spec.json; do
  name=$(basename "$spec" .spec.json)
  node --no-warnings --import "$HERE/register.mjs" "$HERE/ref-run.mts" < "$spec" > "$HERE/$name.golden.json"
  echo "$name"
done
