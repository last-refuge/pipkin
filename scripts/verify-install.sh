#!/usr/bin/env bash
# Verify the current installed native package, including migration, tools and restart.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
if [[ ${1:-} == --full ]]; then shift; fi
[[ $# == 1 ]] || { echo 'Pass one native .tar.zst archive.' >&2; exit 2; }
exec "$here/scripts/test-native-package.sh" --no-build "$1"
