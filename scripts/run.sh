#!/usr/bin/env bash
# Launch the last successful native source build; defaults to developing this checkout.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
build=$here/dist/source-setup/current
if [[ ! -x "$build/bin/pipkin" ]]; then
  echo 'No successful source build yet. Run scripts/setup.sh first.' >&2
  exit 1
fi
# Canonicalize before launch: an in-progress self-rebuild can atomically switch current,
# but this process and its engine must keep using the exact immutable build it started with.
build=$(cd "$build" && pwd -P)
exec "$build/bin/pipkin" --project "$here" "$@"
