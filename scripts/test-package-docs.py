#!/usr/bin/env python3
"""Verify critical installed help is the current source, not absent/stale documentation."""
import hashlib
from pathlib import Path
import sys

root = Path(__file__).resolve().parent.parent
# test-package-docs.py STAGE [--native]   (--native: the native-engine candidate's installed help)
help_dir = Path(sys.argv[1]) / 'usr/share/doc/pipkin'
native = sys.argv[2:] == ['--native']
if native:
    files = {'README.md': 'packaging/native-README.md', 'SECURITY.md': 'SECURITY.md',
             'assets/PROVENANCE.md': 'assets/PROVENANCE.md'}
else:
    files = {'README.md': 'packaging/README.md', 'SECURITY.md': 'SECURITY.md',
             'assets/PROVENANCE.md': 'assets/PROVENANCE.md', 'packaging/PKGBUILD': 'packaging/PKGBUILD',
             'llm-docs/pipkin-v1-release-plan.md': 'llm-docs/pipkin-v1-release-plan.md'}
for guide in (root / 'docs').glob('*.md'):
    relative = str(guide.relative_to(root))
    files[relative] = relative
for installed, source in files.items():
    expected = hashlib.sha256((root / source).read_bytes()).digest()
    assert hashlib.sha256((help_dir / installed).read_bytes()).digest() == expected, installed
assert 'private' in (help_dir / 'SECURITY.md').read_text().lower()
assert (help_dir / 'docs/support.md').is_file()
assert (help_dir / 'docs/getting-started.md').is_file()
print(f'installed documentation verified: {len(files)} current guides/inputs; no source checkout needed to read help')
