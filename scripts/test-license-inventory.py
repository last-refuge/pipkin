#!/usr/bin/env python3
"""Inventory fixture tests; no owner cache/profile or live dependency fetches."""
import hashlib
import json
from pathlib import Path
import runpy
import sys
import tempfile
from unittest.mock import patch

script = Path(__file__).resolve().parent / 'write-license-inventory.py'
catalog = script.parent.parent / 'packaging/upstream-notices.json'
if catalog.exists():
    for entry in json.loads(catalog.read_text())['notices']:
        source = catalog.parent / entry['file']
        assert source.resolve().is_relative_to((catalog.parent / 'upstream-notices').resolve())
        assert hashlib.sha256(source.read_bytes()).hexdigest() == entry['sha256']
        assert len(entry['revision']) == 40 and entry['revision'] in entry['url']
with tempfile.TemporaryDirectory() as temporary:
    root = Path(temporary)
    stage = root / 'stage'
    engine = stage / 'usr/lib/pipkin/engine'
    workspace = engine / 'packages/example'
    workspace.mkdir(parents=True)
    (engine / 'LICENSE').write_text('fixture MIT repository notice')
    (workspace / 'package.json').write_text(json.dumps({'name': 'example', 'version': '1', 'license': 'MIT'}))
    modules = engine / 'node_modules'
    modules.mkdir()
    (modules / 'example').symlink_to('../packages/example')
    dependency = modules / 'dependency'
    dependency.mkdir()
    (dependency / 'package.json').write_text(json.dumps({'name': 'dependency', 'version': '1', 'license': 'MIT'}))
    (dependency / 'LICENSE').write_text('fixture MIT dependency notice')
    # Embedded test metadata must not be mistaken for an installed package root.
    (dependency / 'fixtures').mkdir()
    (dependency / 'fixtures/package.json').write_text('{"name":"not-installed","version":"1"}')
    rust = root / 'rust'
    rust.mkdir()
    (rust / 'LICENSE.txt').write_text('fixture MPL notice')
    # Published yaml-rust2 uses these names instead of LICENSE-MIT/LICENSE-APACHE.
    (rust / 'MIT-LICENSE').write_text('fixture MIT notice with original copyright')
    (rust / 'Apache-LICENSE').write_text('fixture full Apache notice')
    (rust / 'NOT-A-LICENSE').write_text('not a notice')
    (rust / 'src').mkdir()
    (rust / 'src/lib.rs').write_text('// fixture source')
    metadata = {'packages': [{'id': 'app', 'name': 'pipkin-app', 'version': '1', 'license': 'MPL-2.0',
                              'manifest_path': str(rust / 'Cargo.toml'), 'source': None}],
                'resolve': {'nodes': [{'id': 'app', 'deps': []}]}}
    with patch.object(sys, 'argv', [str(script), str(stage)]), patch('subprocess.check_output', return_value=json.dumps(metadata).encode()):
        runpy.run_path(str(script), run_name='__main__')
    output = stage / 'usr/share/doc/pipkin/third-party'
    report = json.loads((output / 'inventory.json').read_text())
    assert len(report['packages']) == 3
    assert len([r for r in report['packages'] if r['name'] == 'example']) == 1
    assert (output / 'sources/pipkin-app-1/src/lib.rs').exists()
    assert all(r['notices'] for r in report['packages'])
    assert any(r['reviewRequired'] for r in report['packages'])
    assert temporary not in (output / 'inventory.json').read_text()
    assert list((output / 'notices').rglob('*.txt'))
    rust_record = next(r for r in report['packages'] if r['ecosystem'] == 'rust')
    copied = {(output / n['file']).read_text() for n in rust_record['notices']}
    assert copied == {'fixture MPL notice', 'fixture MIT notice with original copyright',
                      'fixture full Apache notice'}, copied
    (output / 'stale.txt').write_text('old generated artifact')
    with patch.object(sys, 'argv', [str(script), str(stage)]), patch('subprocess.check_output', return_value=json.dumps(metadata).encode()):
        runpy.run_path(str(script), run_name='__main__')
    assert not (output / 'stale.txt').exists()
    # An unbundled macOS app still needs its target's Rust notices, without a fake npm tree.
    mac_stage = root / 'mac-stage'
    with patch.object(sys, 'argv', [str(script), str(mac_stage), 'aarch64-apple-darwin']), patch('subprocess.check_output', return_value=json.dumps(metadata).encode()) as cargo:
        runpy.run_path(str(script), run_name='__main__')
        assert cargo.call_args.args[0][-1] == 'aarch64-apple-darwin'
    mac_report = json.loads((mac_stage / 'usr/share/doc/pipkin/third-party/inventory.json').read_text())
    assert len(mac_report['packages']) == 1
    assert mac_report['scope'].startswith('aarch64-apple-darwin ')
    assert (mac_stage / 'usr/share/doc/pipkin/third-party/sources/pipkin-app-1/src/lib.rs').exists()
    (modules / 'escape').symlink_to(rust)
    with patch.object(sys, 'argv', [str(script), str(stage)]), patch('subprocess.check_output', return_value=json.dumps(metadata).encode()):
        try:
            runpy.run_path(str(script), run_name='__main__')
            raise AssertionError('external staged dependency link was accepted')
        except RuntimeError as error:
            assert 'outside the bundled engine' in str(error)
print('notice retention, workspace links, fixture exclusion, source provision and host-path omission verified')
