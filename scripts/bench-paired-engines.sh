#!/usr/bin/env bash
# Matched headless execution of trusted local Pi/native release archives, without live accounts.
# Usage: scripts/bench-paired-engines.sh PI.tar.zst NATIVE.tar.zst OUTPUT_DIRECTORY
# PERF_PAIRED_RUNS=20 pairs; PERF_PAIRED_TURNS=6 coding turns per profile.
# Scratch stores must stay on disk, not tmpfs; default is the short workspace target/pb path.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
[ "$#" = 3 ] || { echo 'usage: bench-paired-engines.sh PI.tar.zst NATIVE.tar.zst OUTPUT_DIRECTORY' >&2; exit 2; }
pi_archive=$(realpath "$1")
native_archive=$(realpath "$2")
mkdir -p "$3"
out=$(realpath "$3")
scratch=${PIPKIN_BENCH_SCRATCH:-$here/target/pb}
mkdir -p "$scratch"
scratch=$(realpath "$scratch")
case $(stat -f -c %T "$scratch") in tmpfs|ramfs) echo 'use on-disk scratch for durable-store measurements' >&2; exit 2;; esac
root=$(mktemp -d /tmp/pk-paired-XXXXXX)
trap 'rm -rf "$root"' EXIT
mkdir "$root/pi" "$root/native" "$root/home"
tar --zstd -xf "$pi_archive" -C "$root/pi"
tar --zstd -xf "$native_archive" -C "$root/native"
python3 - "$root" "$pi_archive" "$native_archive" "$out" "$scratch" <<'PY'
import hashlib,json,os,platform,subprocess,sys
from pathlib import Path
root,pi_archive,native_archive,out,scratch=map(Path,sys.argv[1:])
def sha(path):
    with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def command(*args):return subprocess.check_output(args,text=True).strip()
artifacts={}
for kind,archive in [('pi',pi_archive),('native',native_archive)]:
    stage=root/kind
    info=json.loads((stage/'usr/share/doc/pipkin/build-info.json').read_text())
    assert sha(stage/'usr/bin/pipkin')==info['app']['binarySha256']
    artifacts[kind]={'archiveSha256':sha(archive),'buildInfo':info}
assert artifacts['pi']['buildInfo']['app']==artifacts['native']['buildInfo']['app'], 'use matched app binaries and revision stamps'
engine=json.loads((root/'pi/usr/lib/pipkin/engine/engine.json').read_text())
assert not engine['development'] and engine['productionPruneSucceeded']
artifacts['pi']['engine']=engine
artifacts['pi']['runtime']=json.loads((root/'pi/usr/lib/pipkin/runtime/runtime.json').read_text())
assert not (root/'native/usr/lib/pipkin/engine').exists()
assert not (root/'native/usr/lib/pipkin/runtime').exists()
(out/'artifacts.json').write_text(json.dumps(artifacts,indent=2)+'\n')
cpu=next(l.split(':',1)[1].strip() for l in Path('/proc/cpuinfo').read_text().splitlines() if l.startswith('model name'))
paths=['scripts/bench-paired-engines.sh','crates/pipkin-app/src/adapters/pi/paired_perf.rs',
       'crates/pipkin-app/src/adapters/pi/e2e.rs','crates/pipkin-app/src/adapters/pi/testsupport.rs',
       'crates/pipkin-app/src/adapters/native/e2e.rs']
machine={'cpu':cpu,'logicalCpus':os.cpu_count(),'kernel':platform.release(),
 'gitRevision':command('git','rev-parse','HEAD'),'dirty':bool(command('git','status','--porcelain')),
 'rustc':command('rustc','-V'),'loadavg':Path('/proc/loadavg').read_text().strip(),
 'scratchFilesystem':command('stat','-f','-c','%T',str(scratch)),
 'clockTicksPerSecond':int(command('getconf','CLK_TCK')),
 'measurement':'alternating order; fresh profiles; warm executable/filesystem cache; local HTTP scripted provider',
 'releaseProfile':'workspace release opt-level=3, debug=line-tables-only',
 'inputSha256':{p:sha(Path(p)) for p in paths}}
(out/'machine.json').write_text(json.dumps(machine,indent=2)+'\n')
PY
# The test build uses explicit local caches; all provider/proxy/account environment is absent.
env -i HOME="$root/home" PATH="$HOME/.cargo/bin:/usr/bin:/bin" \
  CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
  PIPKIN_PI_REPO="$root/pi/usr/lib/pipkin/engine" \
  PIPKIN_NATIVE_BINARY="$root/native/usr/bin/pipkin" \
  PIPKIN_BENCH_SCRATCH="$scratch" PIPKIN_BENCH_OUT="$out" \
  PERF_PAIRED_RUNS="${PERF_PAIRED_RUNS:-20}" PERF_PAIRED_TURNS="${PERF_PAIRED_TURNS:-6}" \
  cargo test --release --locked -p pipkin-app packaged_engines_matched_performance -- \
    --ignored --nocapture --test-threads=1 2>&1 | tee "$out/run.log"
