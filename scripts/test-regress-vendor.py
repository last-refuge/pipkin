#!/usr/bin/env python3
"""Ensure the vendored regex verifier detects unrecorded changes."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parent.parent
verifier = root / 'scripts/verify-regress-vendor.py'
with tempfile.TemporaryDirectory() as temporary:
    copy = Path(temporary)
    shutil.copytree(root / 'vendor', copy / 'vendor')
    def check(success):
        result = subprocess.run([sys.executable, str(verifier), '--root', str(copy)], capture_output=True, text=True)
        assert (result.returncode == 0) == success, result.stdout + result.stderr
    check(True)
    for relative in ['regress-0.12.0/src/lib.rs', 'regress-0.12.0/src/unicode.rs',
                     'regress-0.12.0/LICENSE-MIT', 'regress-ecmascript.patch']:
        path = copy / 'vendor' / relative
        original = path.read_bytes()
        path.write_bytes(original + b'\nunrecorded change\n')
        check(False)
        path.write_bytes(original)
    added = copy / 'vendor/regress-0.12.0/extra.rs'
    added.write_text('// unrecorded source')
    check(False)
    added.unlink()
    check(True)
print('vendor verifier rejects changed upstream/patch/license files, changed diff and extra files')
