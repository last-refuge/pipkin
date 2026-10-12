#!/usr/bin/env python3
"""Compare trusted Pi/native Linux archives carrying an identical application binary.
Usage: compare-native-package.py PI.tar.zst NATIVE.tar.zst REPORT.json
No payload is executed. GNU tar extracts local build artifacts into a private scratch tree.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def inspect(archive, root, native):
    root.mkdir()
    subprocess.run(['tar', '--zstd', '-xf', str(archive), '-C', str(root)], check=True, timeout=60)
    usr = root / 'usr'
    info = json.loads((usr / 'share/doc/pipkin/build-info.json').read_text())
    actual = sha(usr / 'bin/pipkin')
    assert actual == info['app']['binarySha256']
    inventory = json.loads((usr / 'share/doc/pipkin/third-party/inventory.json').read_text())
    npm = [p for p in inventory['packages'] if p['ecosystem'] == 'npm']
    marker = usr / 'lib/pipkin/native-engine.json'
    engine = usr / 'lib/pipkin/engine'
    runtime = usr / 'lib/pipkin/runtime'
    if native:
        assert marker.is_file() and not engine.exists() and not runtime.exists()
        assert not npm
        for path in usr.rglob('*'):
            assert path.name not in ['node', 'npm', 'npx', 'node_modules'], path
            assert path.suffix not in ['.js', '.mjs', '.cjs', '.ts', '.node'], path
    else:
        assert not marker.exists()
        assert (runtime / 'bin/node').is_file() and (engine / 'engine.json').is_file()
        assert npm
    installed = int(subprocess.check_output(['du', '-sb', str(usr)], text=True).split()[0])
    counts = {'files': 0, 'symlinks': 0, 'fileAndSymlinkBytes': 0}
    for path in usr.rglob('*'):
        if path.is_symlink():
            counts['symlinks'] += 1
            counts['fileAndSymlinkBytes'] += path.lstat().st_size
        elif path.is_file():
            counts['files'] += 1
            counts['fileAndSymlinkBytes'] += path.stat().st_size
    row = {'archiveSha256': sha(archive), 'archiveBytes': archive.stat().st_size,
           'installedApparentBytes': installed, 'app': info['app'], 'buildInfo': info,
           'npmPackages': len(npm), **counts}
    if not native:
        row['engine'] = json.loads((engine / 'engine.json').read_text())
        row['runtime'] = json.loads((runtime / 'runtime.json').read_text())
    return row


def main(pi_archive, native_archive, output):
    with tempfile.TemporaryDirectory(prefix='pipkin-package-comparison-') as tmp:
        root = Path(tmp)
        pi = inspect(pi_archive, root / 'pi', False)
        native = inspect(native_archive, root / 'native', True)
        assert pi['app']['binarySha256'] == native['app']['binarySha256'], 'build both forms with the same app binary and revision stamp'
        installed = 1 - native['installedApparentBytes'] / pi['installedApparentBytes']
        archive = 1 - native['archiveBytes'] / pi['archiveBytes']
        report = {'method': 'GNU du -sb of extracted usr/ on the same filesystem; original tar.zst byte lengths; identical unstripped app binary',
                  'platform': os.uname().sysname + ' ' + os.uname().machine,
                  'pi': pi, 'native': native, 'installedReductionPercent': installed * 100,
                  'archiveReductionPercent': archive * 100,
                  'targets': {'installedReductionPercent': 40, 'archiveReductionPercent': 25},
                  'targetsMet': installed >= 0.40 and archive >= 0.25,
                  'limits': 'Current transitional app includes both adapters/engines in both packages. This compares runtime payload removal, not historical app revisions or final post-retirement binary size. Not distribution portability or complete licensing qualification.'}
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2) + '\n')
        print(f'Identical app SHA-256: {pi["app"]["binarySha256"]}')
        print(f'Installed apparent bytes: Pi {pi["installedApparentBytes"]}; native {native["installedApparentBytes"]}; reduction {installed:.2%}')
        print(f'Archive bytes: Pi {pi["archiveBytes"]}; native {native["archiveBytes"]}; reduction {archive:.2%}')
        print(f'Provisional package targets met: {report["targetsMet"]}')
        if not report['targetsMet']:
            raise SystemExit(1)


if __name__ == '__main__':
    main(*(Path(arg).resolve() for arg in sys.argv[1:4]))
