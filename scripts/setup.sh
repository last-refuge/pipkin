#!/usr/bin/env bash
# Supported source setup: native Rust application and built-in engine.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
command -v python3 >/dev/null || { echo 'Python 3.11+ is required. See docs/source-setup.md.' >&2; exit 1; }
exec python3 "$here/scripts/setup.py" "$@"
