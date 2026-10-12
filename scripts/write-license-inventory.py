#!/usr/bin/env python3
"""Conservative target Rust graph + actual staged-engine notices. No legal-clearance claim."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
STAGE = Path(sys.argv[1]).resolve()
RUST_TARGET = sys.argv[2] if len(sys.argv) > 2 else 'x86_64-unknown-linux-gnu'
if not re.fullmatch(r'[a-zA-Z0-9_-]+', RUST_TARGET):
    raise RuntimeError('invalid Rust target triple')
ENGINE = STAGE / 'usr/lib/pipkin/engine'
OUT = STAGE / 'usr/share/doc/pipkin/third-party'
if OUT.exists():
    if not (OUT / 'inventory.json').is_file():
        raise RuntimeError('refusing to replace an unidentified third-party directory')
    shutil.rmtree(OUT)
OUT.mkdir(parents=True, exist_ok=True)
records = []
supplemental_path = ROOT / 'packaging/upstream-notices.json'
supplemental = json.loads(supplemental_path.read_text())['notices'] if supplemental_path.exists() else []


def notice_files(directory):
    result = []
    for base, dirs, files in os.walk(directory):
        dirs[:] = [d for d in dirs if d not in ('node_modules', '.git', 'target')]
        for name in files:
            # Some published crates (e.g. yaml-rust2) put the offered license first.
            if re.match(r'^(?:(?:mit|apache)[._-])?(license|licence|copying|copyright|notice)([._-].*)?$', name, re.I):
                path = Path(base) / name
                if path.is_file():
                    result.append(path)
    return sorted(result)


def collect(ecosystem, name, version, declared, source, directory, inherited=None):
    files = notice_files(directory)
    provenance = 'package files'
    recovered = []
    if not files:
        recovered = [n for n in supplemental if (n['ecosystem'], n['name'], n['version']) == (ecosystem, name, version)]
        for notice in recovered:
            path = ROOT / 'packaging' / notice['file']
            if not path.resolve().is_relative_to((ROOT / 'packaging/upstream-notices').resolve()):
                raise RuntimeError('supplemental notice path escapes its input directory')
            if hashlib.sha256(path.read_bytes()).hexdigest() != notice['sha256']:
                raise RuntimeError('supplemental notice checksum mismatch')
            files.append(path)
        if files:
            provenance = 'immutable upstream notices; applicability requires review'
    if not files and inherited:
        files = inherited
        provenance = 'ancestor/repository notices; applicability requires review'
    notice_digest = hashlib.sha256(source.encode() + json.dumps(declared).encode())
    for path in files:
        notice_digest.update(path.read_bytes())
    key = re.sub(r'[^a-zA-Z0-9_.-]', '_', f'{ecosystem}-{name}-{version}') + '-' + notice_digest.hexdigest()[:16]
    entries = []
    for index, path in enumerate(files):
        # Keep full notices, including vendored-component notices, without publishing host paths.
        destination = OUT / 'notices' / key / f'{index:03d}-{path.name}'
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, destination)
        entries.append({'file': str(destination.relative_to(OUT)),
                        'sha256': hashlib.sha256(destination.read_bytes()).hexdigest()})
    selected = None
    if ecosystem == 'rust' and name == 'self_cell' and declared == 'Apache-2.0 OR GPL-2.0-only':
        selected = 'Apache-2.0'
    if (ecosystem, name, version, declared) == ('rust', 'zune-inflate', '0.2.54', 'MIT OR Apache-2.0 OR Zlib'):
        # The exact published source offers this alternative. Its full original notice is
        # retained alongside the repository attribution; see the source comparison record.
        selected = 'Zlib'
    if ecosystem == 'npm' and name == 'node-forge' and declared == '(BSD-3-Clause OR GPL-2.0)':
        selected = 'BSD-3-Clause'
    records.append({'ecosystem': ecosystem, 'name': name, 'version': version,
                    'selectedAlternative': selected,
                    'declaredLicense': declared, 'source': source,
                    'noticeProvenance': provenance, 'notices': entries,
                    'supplementalProvenance': recovered,
                    'reviewRequired': not declared or not entries or provenance != 'package files'})


metadata = json.loads(subprocess.check_output(
    ['cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
     '--filter-platform', RUST_TARGET], cwd=ROOT))
packages = {p['id']: p for p in metadata['packages']}
nodes = {n['id']: n for n in metadata['resolve']['nodes']}
app = next(p['id'] for p in metadata['packages'] if p['name'] == 'pipkin-app')
seen, pending = set(), [app]
while pending:
    identity = pending.pop()
    if identity in seen:
        continue
    seen.add(identity)
    for dependency in nodes[identity]['deps']:
        if any(kind['kind'] != 'dev' for kind in dependency['dep_kinds']):
            pending.append(dependency['pkg'])
for identity in sorted(seen):
    package = packages[identity]
    directory = Path(package['manifest_path']).parent
    inherited = []
    for ancestor in list(directory.parents)[:3]:
        candidates = [p for p in ancestor.glob('LICENSE*') if p.is_file()]
        # Zed's GPL root license does not override its explicitly Apache-licensed GPUI crates.
        candidates = [p for p in candidates if 'GPL' not in p.name.upper()
                      or 'GPL' in (package['license'] or '')]
        if candidates:
            inherited = candidates
            break
    if package.get('license_file'):
        inherited = [Path(package['license_file'])]
    source = package['source'] or 'Pipkin workspace'
    vendor_patch = None
    if package['source'] is None and directory.resolve().is_relative_to((ROOT / 'vendor').resolve()):
        source = 'Vendored dependency; see ' + str(directory.relative_to(ROOT)) + '/PIPKIN-PATCH.md'
        if package['name'] in ('reqwest', 'regress'):
            vendor_patch = OUT / 'patches' / f"{package['name']}-{package['version']}"
            vendor_patch.mkdir(parents=True, exist_ok=True)
            stem = package['name']
            patch_name = 'reqwest-http-tunnel.patch' if stem == 'reqwest' else 'regress-ecmascript.patch'
            for path in [directory / 'PIPKIN-PATCH.md', ROOT / f'vendor/{stem}-provenance.json', ROOT / 'vendor' / patch_name]:
                shutil.copyfile(path, vendor_patch / path.name)
            source = 'Vendored dependency; see ' + str((vendor_patch / 'PIPKIN-PATCH.md').relative_to(OUT))
    collect('rust', package['name'], package['version'], package['license'],
            source, directory, inherited)
    if vendor_patch is not None:
        records[-1]['patchProvenance'] = str(vendor_patch.relative_to(OUT))
    if package['license'] == 'MPL-2.0':
        # Ship the exact unmodified registry source, not just a vague source offer.
        target = OUT / 'sources' / f"{package['name']}-{package['version']}"
        shutil.copytree(directory, target, dirs_exist_ok=True)
        records[-1]['providedSource'] = str(target.relative_to(OUT))

# Enumerate package roots, not embedded package.json fixtures inside a dependency.
seen_paths = set()


def npm_package(directory, workspace=False):
    actual = directory.resolve()
    if not actual.is_relative_to(ENGINE):
        raise RuntimeError('staged npm package links outside the bundled engine')
    if actual in seen_paths:
        return
    seen_paths.add(actual)
    manifest = actual / 'package.json'
    if not manifest.is_file():
        return
    package = json.loads(manifest.read_text())
    collect('npm', package.get('name', directory.name), package.get('version', 'unknown'),
            package.get('license'), 'pinned engine workspace' if workspace else 'staged npm package; see package-lock.json',
            actual, [ENGINE / 'LICENSE'] if workspace else None)
    npm_modules(actual / 'node_modules')


def npm_modules(directory):
    if not directory.is_dir():
        return
    for entry in sorted(directory.iterdir()):
        if entry.name.startswith('.'):
            continue
        if entry.name.startswith('@') and entry.is_dir():
            for package in sorted(entry.iterdir()):
                npm_package(package, package.is_symlink())
        elif entry.is_dir():
            npm_package(entry, entry.is_symlink())


npm_modules(ENGINE / 'node_modules')
for workspace in sorted((ENGINE / 'packages').iterdir()) if (ENGINE / 'packages').is_dir() else []:
    if workspace.is_dir():
        npm_package(workspace, True)
records.sort(key=lambda r: (r['ecosystem'], r['name'], r['version']))
report = {'formatVersion': 1,
          'scope': f'{RUST_TARGET} normal/build reachable Rust graph (workspace feature unification may overinclude); actual staged npm/workspace roots',
          'limitations': 'Declared licenses and copied notices are evidence, not legal clearance. Ancestor notices and missing declarations need review; data/assets and external runtime libraries are separate.',
          'packages': records}
(OUT / 'inventory.json').write_text(json.dumps(report, indent=2) + '\n')
counts = {ecosystem: sum(r['ecosystem'] == ecosystem for r in records) for ecosystem in ('rust', 'npm')}
print(f"license inventory: {counts}; review required: {sum(r['reviewRequired'] for r in records)}")
