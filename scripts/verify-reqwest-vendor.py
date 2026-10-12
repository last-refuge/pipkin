#!/usr/bin/env python3
"""Verify every vendored reqwest file against original or explicit patch hashes."""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parent.parent)
args = parser.parse_args()
root = args.root.resolve()
provenance = json.loads((root / 'vendor/reqwest-provenance.json').read_text())
assert provenance['name'] == 'reqwest' and provenance['version'] == '0.12.28'
assert provenance['archiveSha256'] == 'eddd3ca559203180a307f12d114c268abf583f59b03cb906fd0b3ff8646c1147'
package = root / 'vendor/reqwest-0.12.28'
expected = provenance['upstreamFiles'] | provenance['patchedFiles']
actual = {str(path.relative_to(package)) for path in package.rglob('*') if path.is_file()}
assert actual == set(expected), f'unexpected/missing files: {actual ^ set(expected)}'
for name, digest in expected.items():
    path = package / name
    assert path.resolve().is_relative_to(package.resolve()) and not path.is_symlink(), name
    assert hashlib.sha256(path.read_bytes()).hexdigest() == digest, f'modified vendor file: {name}'
patch = root / 'vendor/reqwest-http-tunnel.patch'
assert hashlib.sha256(patch.read_bytes()).hexdigest() == provenance['patchSha256'], 'modified patch'
print(f'verified {len(expected)} reqwest files, pinned archive identity and explicit patch')
