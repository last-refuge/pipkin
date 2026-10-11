#!/usr/bin/env bash
# Run one reference scenario or benchmark.
# Layout: <ref-run>/pi = a copy of the staged Pi tree with `npm ci` done; <ref-run>/driver = these driver files;
# this script at <ref-run>/run.sh. Outputs land in <ref-run>/out/<scenario>/ and <ref-run>/bench/.
#   ./run.sh <scenario>                      # a-text-multiturn ... h-worker-killed-mid-tool
#   DRIVER=bench.ts ./run.sh startup 10      # startup/idle baseline
#   DRIVER=bench.ts ./run.sh soak 200        # 200-turn memory soak
#   PI_TREE=<other pi tree> ...              # run the server/worker from another tree (e.g. a pruned copy)
HERE="$(cd "$(dirname "$0")" && pwd)"
NODE="${NODE:-$(mise which node)}"
exec "$NODE" --import "file://$HERE/pi/packages/coding-agent/src/experimental/source-resolver.ts" "$HERE/driver/${DRIVER:-scenarios.ts}" "$@"
