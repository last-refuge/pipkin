#!/usr/bin/env python3
"""Ensure the vendored transport verifier detects unrecorded changes."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parent.parent
verifier = root / 'scripts/verify-reqwest-vendor.py'
with tempfile.TemporaryDirectory() as temporary:
    copy = Path(temporary)
    shutil.copytree(root / 'vendor', copy / 'vendor')
    def check(success):
        result = subprocess.run([sys.executable, str(verifier), '--root', str(copy)], capture_output=True, text=True)
        assert (result.returncode == 0) == success, result.stdout + result.stderr
    check(True)
    for relative in ['reqwest-0.12.28/src/lib.rs', 'reqwest-0.12.28/src/connect.rs',
                     'reqwest-0.12.28/LICENSE-MIT', 'reqwest-http-tunnel.patch']:
        path = copy / 'vendor' / relative
        original = path.read_bytes()
        path.write_bytes(original + b'\nunrecorded change\n')
        check(False)
        path.write_bytes(original)
    added = copy / 'vendor/reqwest-0.12.28/extra.rs'
    added.write_text('// unrecorded source')
    check(False)
    added.unlink()
    check(True)
print('vendor verifier rejects changed upstream/patch/license files, changed diff and extra files')
