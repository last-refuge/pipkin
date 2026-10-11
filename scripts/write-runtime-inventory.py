#!/usr/bin/env python3
"""Inspect staged native artifacts without executing them; not a complete binary SBOM.

    write-runtime-inventory.py STAGE                    the Pi package (bundled Node engine)
    write-runtime-inventory.py STAGE --native-engine    a native-engine candidate (no Node)
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

stage = Path(sys.argv[1]).resolve()
native = sys.argv[2:] == ['--native-engine']
if sys.argv[2:] and not native:
    raise SystemExit('usage: write-runtime-inventory.py STAGE [--native-engine]')
readelf_env = {**os.environ, 'LC_ALL': 'C'}
records = []
for path in sorted(stage.rglob('*')):
    if path.is_symlink() or not path.is_file():
        continue
    with path.open('rb') as stream:
        magic = stream.read(4)
    relative = str(path.relative_to(stage))
    if magic != b'\x7fELF':
        if path.suffix == '.node':
            records.append({'file': relative, 'format': 'non-ELF native addon',
                            'sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'reviewRequired': True})
        continue
    header = subprocess.check_output(['readelf', '-hW', str(path)], text=True, env=readelf_env)
    dynamic = subprocess.check_output(['readelf', '-dW', str(path)], text=True, env=readelf_env)
    versions = subprocess.check_output(['readelf', '-VW', str(path)], text=True, env=readelf_env)
    machine = re.search(r'^\s*Machine:\s*(.+)$', header, re.M).group(1).strip()
    needed = sorted(set(re.findall(r'\(NEEDED\).*\[([^]]+)\]', dynamic)))
    abi = sorted(set(re.findall(r'\b(?:GLIBC|GLIBCXX|CXXABI)_[0-9.]+', versions)))
    records.append({'file': relative, 'format': 'ELF', 'machine': machine,
                    'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                    'neededSharedLibraries': needed, 'referencedAbiVersions': abi,
                    'reviewRequired': machine != 'Advanced Micro Devices X86-64'})
report = {'formatVersion': 1, 'inspection': 'readelf only; native artifacts are never executed',
          'scope': 'actual staged ELF files and non-ELF .node artifacts; direct NEEDED/ABI references only',
          'limitations': 'Not a complete linked/static/dlopen dependency or license audit. Foreign artifacts are shipped source-tree contents, not supported runtimes. System Node and desktop/GPU libraries remain external requirements.',
          'requiredNodeMinimum': '22.19', 'artifacts': records}
if native:
    report['limitations'] = ('Not a complete linked/static/dlopen dependency or license audit. The built-in engine is '
                             'statically linked Rust inside usr/bin/pipkin (run as `pipkin --engine-worker`); '
                             'desktop/GPU libraries remain external requirements.')
    report['engine'] = 'native (built into usr/bin/pipkin; no Node runtime, npm tree or Pi engine)'
    report['requiredNodeMinimum'] = None
output = stage / 'usr/share/doc/pipkin/runtime-inventory.json'
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text(json.dumps(report, indent=2) + '\n')
print(f'runtime inventory: {len(records)} native artifacts, {sum(r["reviewRequired"] for r in records)} foreign-format/architecture review flags')
