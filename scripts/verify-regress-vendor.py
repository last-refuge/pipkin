#!/usr/bin/env python3
"""Verify every vendored regress file against original or explicit patch hashes."""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parent.parent)
args = parser.parse_args()
root = args.root.resolve()
provenance = json.loads((root / 'vendor/regress-provenance.json').read_text())
assert provenance['name'] == 'regress' and provenance['version'] == '0.12.0'
assert provenance['archiveSha256'] == '32eef8b209c3c1c15dbad02c1f30f9539f00dc7253e0cbcdaae442a50a09d7c1'
package = root / 'vendor/regress-0.12.0'
expected = provenance['upstreamFiles'] | provenance['patchedFiles']
actual = {str(path.relative_to(package)) for path in package.rglob('*') if path.is_file()}
assert actual == set(expected), f'unexpected/missing files: {actual ^ set(expected)}'
for name, digest in expected.items():
    path = package / name
    assert path.resolve().is_relative_to(package.resolve()) and not path.is_symlink(), name
    assert hashlib.sha256(path.read_bytes()).hexdigest() == digest, f'modified vendor file: {name}'
patch = root / 'vendor/regress-ecmascript.patch'
assert hashlib.sha256(patch.read_bytes()).hexdigest() == provenance['patchSha256'], 'modified patch'
print(f'verified {len(expected)} regress files, pinned archive identity and explicit patch')
