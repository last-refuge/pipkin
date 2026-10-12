#!/usr/bin/env bash
# Native engine performance harness (plan §7). Builds release binaries, runs the ignored
# benchmarks in crates/pipkin-engine/tests/perf.rs one at a time, records the machine and the
# binary sizes, and writes raw JSON to fixtures/native-bench/<date>/.
#
#   scripts/bench-native.sh                 # everything (builds pipkin-app too; slow)
#   SKIP_APP=1 scripts/bench-native.sh      # engine only; package sizes for the engine binary
#   TESTS="readiness stop_latency" scripts/bench-native.sh
#   OUT=fixtures/native-bench/mylabel scripts/bench-native.sh
#
# Knobs passed through to the tests: PERF_RUNS (20), PERF_SOAK_TURNS (200),
# PERF_LONG_SOAK_TURNS (1500), PERF_SCALING_REPS (3), PIPKIN_BENCH_SCRATCH (scratch profiles;
# default target/tmp, on the same disk as the build: keep it off tmpfs, the store fsyncs).
#
# PERF_MIXED_TURNS (720) and PERF_MIXED_RESTART_EVERY (240) tune the mixed workload.
# PERF_MIXED_COMPACT=1 enables byte/4 usage estimates and repeated automatic compaction.
# No network, no credentials: the provider is a local scripted HTTP stub inside the test
# process, reached through a models.json custom provider.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

OUT="${OUT:-fixtures/native-bench/$(date +%F)}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
TESTS="${TESTS:-readiness soak_200 soak_1500 soak_diagnostic stream_latency stream_scaling history stop_latency mixed::mixed_soak}"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
LOG="$OUT/run.log"
: >"$LOG"

say() { echo "== $*" | tee -a "$LOG"; }

built_bins=("$TARGET/release/pipkin-engine")
say "building release engine"
cargo build --release -p pipkin-engine 2>&1 | tail -n 2 | tee -a "$LOG"
if [[ -z "${SKIP_APP:-}" ]]; then
    built_bins+=("$TARGET/release/pipkin")
    say "building release app (pipkin-app)"
    cargo build --release -p pipkin-app 2>&1 | tail -n 2 | tee -a "$LOG"
fi
cargo test --release -p pipkin-engine --test perf --no-run 2>&1 | tail -n 2 | tee -a "$LOG"

# ---- machine ------------------------------------------------------------------------------
scratch="${PIPKIN_BENCH_SCRATCH:-$TARGET/tmp}"
mkdir -p "$scratch"
python3 - "$OUT/machine.json" "$scratch" <<'PY'
import json, os, platform, subprocess, sys
def sh(*a):
    try:
        return subprocess.run(a, capture_output=True, text=True, check=False).stdout.strip()
    except FileNotFoundError:
        return None
cpu = next((l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name")), None)
mem = next((int(l.split()[1]) for l in open("/proc/meminfo") if l.startswith("MemTotal")), None)
gov = None
try:
    gov = open("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").read().strip()
except OSError:
    pass
json.dump({
    "cpu": cpu,
    "logicalCpus": os.cpu_count(),
    "memTotalKiB": mem,
    "kernel": platform.release(),
    "governor": gov,
    "rustc": sh("rustc", "-V"),
    "gitRevision": sh("git", "rev-parse", "HEAD"),
    "gitDirty": bool(sh("git", "status", "--porcelain")),
    "loadavg": open("/proc/loadavg").read().strip(),
    "scratch": sys.argv[2],
    "scratchFilesystem": sh("stat", "-f", "-c", "%T", sys.argv[2]),
    "releaseProfile": "workspace [profile.release]: opt-level 3, debug = line-tables-only",
}, open(sys.argv[1], "w"), indent=2)
PY
say "machine: $(cat "$OUT/machine.json" | tr -d '\n' | tr -s ' ')"

# ---- benchmarks ---------------------------------------------------------------------------
for t in $TESTS; do
    say "test $t (loadavg $(cut -d' ' -f1-3 /proc/loadavg))"
    PIPKIN_BENCH_OUT="$OUT" cargo test --release -p pipkin-engine --test perf -- \
        --ignored --nocapture --test-threads=1 --exact "$t" 2>&1 | tee -a "$LOG"
done

# ---- package bytes ------------------------------------------------------------------------
say "binary sizes"
tmp="$(mktemp -d "$scratch/sizes-XXXXXX")"
python3 - "$OUT/package-sizes.json" "$tmp" "${built_bins[@]}" <<'PY'
import gzip, hashlib, json, lzma, os, shutil, subprocess, sys
out, tmp, bins = sys.argv[1], sys.argv[2], sys.argv[3:]
rows = []
for b in bins:
    if not os.path.exists(b):
        rows.append({"binary": b, "present": False})
        continue
    stripped = os.path.join(tmp, os.path.basename(b) + ".stripped")
    subprocess.run(["strip", "-o", stripped, b], check=True)
    data = open(stripped, "rb").read()
    row = {
        "binary": os.path.basename(b),
        "present": True,
        "unstrippedBytes": os.path.getsize(b),
        "unstrippedSha256": hashlib.file_digest(open(b, "rb"), "sha256").hexdigest(),
        "strippedBytes": len(data),
        "strippedGzip9Bytes": len(gzip.compress(data, 9)),
        "strippedXz6Bytes": len(lzma.compress(data, preset=6)),
    }
    if shutil.which("zstd"):
        z = subprocess.run(["zstd", "-19", "-q", "-c", stripped], capture_output=True, check=True).stdout
        row["strippedZstd19Bytes"] = len(z)
    ldd = subprocess.run(["ldd", b], capture_output=True, text=True).stdout
    row["dynamicLibraries"] = sorted({l.split()[0] for l in ldd.splitlines() if l.strip()})
    rows.append(row)
json.dump({"binaries": rows}, open(out, "w"), indent=2)
print(json.dumps(rows, indent=2))
PY
rm -rf "$tmp"
say "done: $OUT"
