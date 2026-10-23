#!/usr/bin/env bash
# Launch the last successful native source build; defaults to developing this checkout.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
# Resolve current under the setup/cleanup lock and retain a shared lock on this
# immutable build across exec. Cleanup can still prune other, unused source builds.
exec python3 "$here/scripts/build_storage.py" "$@"
