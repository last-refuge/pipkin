#!/usr/bin/env python3
"""Package/verify a self-contained ad-hoc signed macOS bundle; no Developer ID/notarization."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
sys.dont_write_bytecode = True  # Importing the layout helper must not dirty the checkout.
_layout_spec = importlib.util.spec_from_file_location('macos_dmg_layout', ROOT / 'scripts/macos-dmg-layout.py')
dmg_layout = importlib.util.module_from_spec(_layout_spec)
_layout_spec.loader.exec_module(dmg_layout)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check_probe_socket_budget(profile):
    # Pi creates a longer private server socket in addition to the public UUID socket.
    # Reserve seven PID digits so a later runner cannot silently exceed Darwin's
    # 104-byte sun_path field (including its terminator).
    longest = (Path(profile) / 'pipkin-probe-9999999/server' /
               ('server-00000000-0000-4000-8000-000000000000-' + '0' * 12 + '.sock'))
    if len(os.fsencode(longest)) >= 104:
        raise RuntimeError('offline probe profile would exceed the macOS Pi socket-path limit')


def main():
    if sys.platform != 'darwin':
        raise SystemExit('macOS packaging must inspect a native build on macOS')
    target = sys.argv[1]
    if target not in ('aarch64-apple-darwin', 'x86_64-apple-darwin'):
        raise SystemExit('expected a supported native macOS Rust target')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT))
    binary = ROOT / 'target/release/pipkin'
    stage = ROOT / 'dist/macos-notices'
    notices = stage / 'usr/share/doc/pipkin/third-party'
    engine = stage / 'usr/lib/pipkin/engine'
    runtime = stage / 'usr/lib/pipkin/runtime'
    engine_info = json.loads((engine / 'engine.json').read_text())
    runtime_info = json.loads((runtime / 'runtime.json').read_text())
    required_revision = (ROOT / 'packaging/pi-engine-revision').read_text().strip()
    if engine_info.get('sourceRevision') != required_revision or engine_info.get('development') is not False:
        raise SystemExit('Mac package requires the clean pinned production engine')
    if runtime_info['target'] != target or sha256(runtime / 'bin/node') != runtime_info['binarySha256']:
        raise SystemExit('bundled runtime identity/target mismatch')
    if not (notices / 'inventory.json').is_file():
        raise SystemExit('target-specific dependency notices must be generated first')
    out = ROOT / 'dist/macos'
    if out.exists():
        if not (out / 'build-info.json').is_file():
            raise SystemExit('refusing to replace an unidentified output directory')
        shutil.rmtree(out)
    app = out / 'Pipkin.app/Contents'
    executable = app / 'MacOS/pipkin'
    executable.parent.mkdir(parents=True)
    shutil.copy2(binary, executable)
    resources = app / 'Resources'
    resources.mkdir()
    shutil.copy2(ROOT / 'packaging/pipkin.icns', resources / 'pipkin.icns')
    components = app / 'lib/pipkin'
    components.mkdir(parents=True)
    shutil.copytree(engine, components / 'engine', symlinks=True)
    shutil.copytree(runtime, components / 'runtime')
    # Existing install discovery resolves Contents/lib/pipkin/engine relative to MacOS/pipkin.
    bundled_manifest = components / 'engine/engine.json'
    engine_info['requiresBundledNode'] = True
    bundled_manifest.write_text(json.dumps(engine_info, indent=2) + '\n')
    shutil.copytree(notices, resources / 'third-party')
    shutil.copytree(ROOT / 'assets/fonts', resources / 'font-notices', ignore=shutil.ignore_patterns('*.ttf', '*.otf'))
    for source in ['LICENSE', 'SECURITY.md', 'assets/PROVENANCE.md']:
        shutil.copy2(ROOT / source, resources / Path(source).name)
    shutil.copy2(ROOT / 'packaging/upstream-notices/anthropic-sdk/LICENSE', resources / 'anthropic-sdk-MIT-LICENSE')
    shutil.copy2(ROOT / 'docs/macos-first-pass.md', out / 'README.md')
    shutil.copy2(ROOT / 'docs/macos-first-pass.md', resources / 'README.md')
    with (app / 'Info.plist').open('wb') as stream:
        plistlib.dump({'CFBundleExecutable': 'pipkin', 'CFBundleIdentifier': 'org.last-refuge.pipkin',
                       'CFBundleName': 'Pipkin', 'CFBundlePackageType': 'APPL',
                       'CFBundleIconFile': 'pipkin.icns',
                       'CFBundleShortVersionString': version, 'CFBundleVersion': version,
                       'NSHighResolutionCapable': True}, stream)
    dependencies = subprocess.check_output(['otool', '-L', str(executable)], text=True).splitlines()[1:]
    info = {'formatVersion': 1, 'experimental': True,
            'app': {'version': version, 'revision': revision, 'dirty': dirty,
                    'preBundleSigningBinarySha256': sha256(executable), 'target': target},
            'engine': {'bundled': True, 'requiredRevision': required_revision,
                       'productionPruneSucceeded': engine_info.get('productionPruneSucceeded')},
            'nodeRuntime': runtime_info,
            'signing': 'Ad-hoc signed completed app bundle; no Developer ID or notarization. Not publisher authentication or Gatekeeper acceptance.',
            'directMachODependencies': [line.strip() for line in dependencies],
            'limitations': 'Build artifact, not native acceptance. Dependency notices are evidence, not legal clearance.'}
    # The embedded report cannot contain the final signed-binary hash: it is itself sealed
    # into that signature. Embed pre-sign identity, then retain the final hash outside the app.
    (resources / 'build-info.json').write_text(json.dumps(info, indent=2) + '\n')
    bundle = out / 'Pipkin.app'
    subprocess.run(['codesign', '--force', '--sign', '-', '--identifier', 'org.last-refuge.pipkin', str(bundle)], check=True)
    verify = ['codesign', '--verify', '--deep', '--strict', '--verbose=4']
    subprocess.run(verify + [str(bundle)], check=True)
    info['app']['binarySha256'] = sha256(executable)
    (out / 'build-info.json').write_text(json.dumps(info, indent=2) + '\n')
    archive = out / f'pipkin-{version}-{target}-experimental.zip'
    subprocess.run(['ditto', '-c', '-k', '--sequesterRsrc', '--keepParent', str(bundle), str(archive)], check=True)
    # Validate what a user extracts, not just the pre-archive directory. Never modify the app
    # after signing, and reject any extraction that changes its executable or resource seal.
    with tempfile.TemporaryDirectory(prefix='verify-macos-', dir=out) as scratch:
        subprocess.run(['ditto', '-x', '-k', str(archive), scratch], check=True)
        restored = Path(scratch) / 'Pipkin.app'
        subprocess.run(verify + [str(restored)], check=True)
        if sha256(restored / 'Contents/MacOS/pipkin') != info['app']['binarySha256']:
            raise RuntimeError('extracted signed executable checksum mismatch')
        if sha256(restored / 'Contents/lib/pipkin/runtime/bin/node') != runtime_info['binarySha256']:
            raise RuntimeError('extracted Node checksum mismatch')
        if sha256(restored / 'Contents/Resources/pipkin.icns') != sha256(ROOT / 'packaging/pipkin.icns'):
            raise RuntimeError('extracted app icon checksum mismatch')
        # A real packaged CLI, not source-only tests: offline private profile, no provider calls.
        # Finder-like bare PATH excludes setup-node/Homebrew and the source checkout.
        # A private eight-character /tmp directory leaves room for Pi's longest socket.
        # Keep HOME and TMPDIR together; never probe against the runner's real profile.
        with tempfile.TemporaryDirectory(prefix='', dir='/tmp') as profile:
            check_probe_socket_budget(profile)
            environment = {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'HOME': profile,
                           'TMPDIR': profile, 'PI_OFFLINE': '1'}
            subprocess.run([str(restored / 'Contents/MacOS/pipkin'), '--diagnose', '--probe',
                            '--data-dir', str(Path(profile) / 'app-data')], env=environment, check=True)
        subprocess.run(verify + [str(restored)], check=True)  # Engine startup must not mutate sealed resources.
    if '--app-only' in sys.argv[2:]:
        # Local source install should not depend on Finder/DMG creation or mount availability.
        (out / 'SHA256SUMS').write_text(f'{sha256(archive)}  {archive.name}\n')
        print(f'local macOS app: {bundle}; verified ZIP {archive.name}')
        return
    # Build the drag-to-Applications image from the same sealed bundle, before removing it.
    # Include only the app, shortcut, and hidden layout assets: never package build reports
    # or temporary verification data into the volume.
    dmg = out / f'pipkin-{version}-{target}-experimental.dmg'
    layout_path = ROOT / 'packaging/dmg-layout.json'
    background = ROOT / 'packaging/dmg-background.tiff'
    dmg_layout.read_layout(layout_path)
    if not background.is_file():
        raise RuntimeError('DMG instructional background is missing')
    with dmg_layout.private_tools(ROOT / 'packaging/dmg-layout-tools.json') as tools, \
            tempfile.TemporaryDirectory(prefix='dmg-', dir=out) as scratch:
        scratch = Path(scratch)
        payload = scratch / 'payload'
        payload.mkdir()
        shutil.copytree(bundle, payload / 'Pipkin.app', symlinks=True)
        (payload / 'Applications').symlink_to('/Applications', target_is_directory=True)
        # Include artwork in srcfolder sizing before image creation; a minimal image
        # sized only for the app might not leave enough free space for the TIFF.
        (payload / '.background').mkdir()
        shutil.copy2(background, payload / '.background/install.tiff')
        # Finder's mounted-volume icon is a root-level .VolumeIcon.icns plus the volume's
        # custom-icon Finder flag. Set it on a writable image, then convert to the read-only
        # distributable; do not modify the already signed app or rely on upload xattrs.
        editable = scratch / 'editable.dmg'
        subprocess.run(['hdiutil', 'create', '-volname', 'Pipkin', '-srcfolder', str(payload),
                        '-format', 'UDRW', '-fs', 'HFS+', '-ov', str(editable)], check=True)
        writable = scratch / 'writable'
        writable.mkdir()
        try:
            subprocess.run(['hdiutil', 'attach', '-readwrite', '-nobrowse', '-mountpoint',
                            str(writable), str(editable)], check=True)
        except subprocess.CalledProcessError:
            subprocess.run(['hdiutil', 'detach', str(writable)], check=False,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            raise
        try:
            shutil.copy2(ROOT / 'packaging/pipkin.icns', writable / '.VolumeIcon.icns')
            subprocess.run(['xcrun', 'SetFile', '-a', 'C', str(writable)], check=True)
            dmg_layout.run_layout(tools, 'write', writable, layout_path, background)
            dmg_layout.run_layout(tools, 'verify', writable, layout_path, background)
            dmg_layout.enable_auto_open(writable)
        finally:
            subprocess.run(['hdiutil', 'detach', str(writable)], check=True)
        subprocess.run(['hdiutil', 'convert', str(editable), '-format', 'UDZO', '-o',
                        str(dmg)], check=True)
        mount = scratch / 'mount'
        mount.mkdir()
        try:
            subprocess.run(['hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount),
                            str(dmg)], check=True)
        except subprocess.CalledProcessError:
            # An attach can fail after mounting; best-effort detach before deleting scratch.
            subprocess.run(['hdiutil', 'detach', str(mount)], check=False,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            raise
        try:
            dmg_layout.run_layout(tools, 'verify', mount, layout_path, background)
            if sha256(mount / '.VolumeIcon.icns') != sha256(ROOT / 'packaging/pipkin.icns'):
                raise RuntimeError('mounted DMG volume icon does not match Pipkin')
            flags = subprocess.check_output(['xcrun', 'GetFileInfo', '-a', str(mount)], text=True)
            if 'C' not in flags:
                raise RuntimeError('mounted DMG does not have the custom volume icon flag')
            if not (mount / 'Applications').is_symlink() or (mount / 'Applications').readlink() != Path('/Applications'):
                raise RuntimeError('mounted DMG is missing the Applications shortcut')
            copied = scratch / 'installed/Pipkin.app'
            shutil.copytree(mount / 'Pipkin.app', copied, symlinks=True)
            subprocess.run(verify + [str(copied)], check=True)
            if sha256(copied / 'Contents/MacOS/pipkin') != info['app']['binarySha256']:
                raise RuntimeError('DMG-installed executable checksum mismatch')
            if sha256(copied / 'Contents/lib/pipkin/runtime/bin/node') != runtime_info['binarySha256']:
                raise RuntimeError('DMG-installed Node checksum mismatch')
            if sha256(copied / 'Contents/Resources/pipkin.icns') != sha256(ROOT / 'packaging/pipkin.icns'):
                raise RuntimeError('DMG-installed app icon checksum mismatch')
            with tempfile.TemporaryDirectory(prefix='', dir='/tmp') as profile:
                check_probe_socket_budget(profile)
                environment = {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'HOME': profile,
                               'TMPDIR': profile, 'PI_OFFLINE': '1'}
                subprocess.run([str(copied / 'Contents/MacOS/pipkin'), '--diagnose', '--probe',
                                '--data-dir', str(Path(profile) / 'app-data')], env=environment, check=True)
            subprocess.run(verify + [str(copied)], check=True)
        finally:
            subprocess.run(['hdiutil', 'detach', str(mount)], check=True)
    # Avoid uploading the app twice. Both archives retain the same signed bundle.
    shutil.rmtree(bundle)
    (out / 'SHA256SUMS').write_text(
        f'{sha256(archive)}  {archive.name}\n{sha256(dmg)}  {dmg.name}\n')
    print(f'experimental macOS packages: {archive.name}, {dmg.name}; hashes in SHA256SUMS')


if __name__ == '__main__':
    main()
