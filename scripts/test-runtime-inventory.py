#!/usr/bin/env python3
"""Disposable native-metadata fixtures; never execute the fixture binary."""
import json
from pathlib import Path
import subprocess
import tempfile

script = Path(__file__).resolve().parent / 'write-runtime-inventory.py'
with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    stage = root / 'stage'
    binary = stage / 'usr/bin/fixture'
    binary.parent.mkdir(parents=True)
    marker = root / 'must-not-exist'
    source = root / 'fixture.c'
    source.write_text(f'#include <stdio.h>\nint main(void) {{ return fopen("{marker}", "w") == NULL; }}\n')
    subprocess.run(['cc', str(source), '-o', str(binary)], check=True)
    (binary.parent / 'foreign.node').write_bytes(b'foreign native fixture')
    # Change only the metadata machine field; this is not a runnable ARM program.
    foreign_elf = bytearray(binary.read_bytes())
    assert foreign_elf[5] == 1, 'fixture requires little-endian ELF'
    foreign_elf[18:20] = (183).to_bytes(2, 'little')
    (binary.parent / 'foreign-architecture').write_bytes(foreign_elf)
    (binary.parent / 'linked').symlink_to(binary)
    subprocess.run(['python3', str(script), str(stage)], check=True)
    report_path = stage / 'usr/share/doc/pipkin/runtime-inventory.json'
    before = report_path.read_text()
    report = json.loads(before)
    assert len(report['artifacts']) == 3
    elf = next(a for a in report['artifacts'] if a['file'] == 'usr/bin/fixture')
    assert any(a.get('machine') == 'AArch64' and a['reviewRequired'] for a in report['artifacts'])
    assert 'libc.so.6' in elf['neededSharedLibraries']
    assert any(v.startswith('GLIBC_') for v in elf['referencedAbiVersions'])
    assert not marker.exists(), 'inventory executed the fixture binary'
    assert directory not in before
    assert any(a['reviewRequired'] for a in report['artifacts'])
    subprocess.run(['python3', str(script), str(stage)], check=True)
    assert report_path.read_text() == before
    subprocess.run(['python3', str(script), str(stage), '--native-engine'], check=True)
    native = json.loads(report_path.read_text())
    assert native['requiredNodeMinimum'] is None and 'no Node runtime' in native['engine']
    assert native['artifacts'] == report['artifacts']
    assert subprocess.run(['python3', str(script), str(stage), '--bogus'], capture_output=True).returncode != 0
print('native dependencies, ABI references, foreign flags, path omission and non-execution verified')
