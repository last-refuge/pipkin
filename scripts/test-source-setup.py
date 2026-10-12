#!/usr/bin/env python3
"""Source setup contracts with disposable files and mocked subprocesses; never desktop/owner installs."""
import importlib.util
from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

sys.dont_write_bytecode = True
scripts = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('setup', scripts / 'setup.py')
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)
assert setup.target_for('Darwin', 'arm64') == 'aarch64-apple-darwin'
assert setup.target_for('Darwin', 'x86_64') == 'x86_64-apple-darwin'
assert setup.target_for('Linux', 'x86_64') == 'x86_64-unknown-linux-gnu'
try:
    setup.target_for('Linux', 'aarch64')
    raise AssertionError('unqualified target accepted')
except RuntimeError:
    pass

with tempfile.TemporaryDirectory() as scratch:
    root = Path(scratch)
    (root / 'scripts').mkdir(); (root / 'packaging').mkdir()
    base = root / 'dist/source-setup'
    with patch.object(setup, 'ROOT', root):
        setup.owned_directory(base)
        unidentified = root / 'unidentified'; unidentified.mkdir()
        try:
            setup.owned_directory(unidentified)
            raise AssertionError('unidentified output accepted')
        except RuntimeError:
            pass
        calls = []
        fail_probe = False

        def command(args, **kwargs):
            global fail_probe
            args = list(map(str, args)); calls.append(args)
            if args[:2] == ['cargo', 'build']:
                binary = root / 'target/release/pipkin'; binary.parent.mkdir(parents=True, exist_ok=True)
                binary.write_text('fixture binary')
            if '--probe' in args:
                assert set(kwargs['env']) == {'HOME', 'TMPDIR', 'PATH'}
                assert kwargs['env']['PATH'] == '/usr/bin:/bin:/usr/sbin:/sbin'
                assert kwargs['env']['HOME'].startswith('/tmp/')
                if fail_probe:
                    raise RuntimeError('injected failed probe')

        with patch.object(setup, 'run', side_effect=command), patch.object(setup, 'preflight'), patch.object(setup.platform, 'system', return_value='Linux'), patch.object(setup.platform, 'machine', return_value='x86_64'), patch.object(sys, 'argv', ['setup.py']):
            setup.main()
            first = (base / 'current').resolve()
            assert (first / 'bin/pipkin').is_file()
            assert not (first / 'lib/pipkin/engine').exists()
            assert not (first / 'lib/pipkin/runtime').exists()
            setup.main()
            second = (base / 'current').resolve()
            assert second != first and (first / 'bin/pipkin').exists()  # Running build not overwritten.
            fail_probe = True
            try:
                setup.main()
                raise AssertionError('failed probe accepted')
            except RuntimeError:
                assert (base / 'current').resolve() == second
        assert not any('--install' in call or 'sudo' in call for call in calls)
assert not any(any(token in str(arg) for token in ['npm', 'nodejs', 'build-engine', 'stage-node', 'pi-fork']) for call in calls for arg in call)
print('source setup: native targets, Rust-only build, isolated probe, immutable rebuilds and failed-build recovery verified')
