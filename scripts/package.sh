#!/usr/bin/env bash
# Build the native Rust package; no Pi checkout or Node/npm is required.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
if [ "${1:-}" = --engine ] && [ "${2:-}" = native ]; then shift 2; fi
[ $# -eq 0 ] || { echo 'Usage: scripts/package.sh [--engine native]. Legacy runtime packaging is retired.' >&2; exit 2; }
exec "$here/scripts/package-native.sh"
