#!/usr/bin/env python3
"""One-command native source build/install. Built-in Rust engine; no sudo."""
import argparse
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parent.parent


def run(args, **kwargs):
    print('+ ' + ' '.join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), cwd=ROOT, check=True, **kwargs)


def target_for(system, machine):
    targets = {('Darwin', 'arm64'): 'aarch64-apple-darwin',
               ('Darwin', 'x86_64'): 'x86_64-apple-darwin',
               ('Linux', 'x86_64'): 'x86_64-unknown-linux-gnu'}
    if (system, machine) not in targets:
        raise RuntimeError('Supported source setup: macOS Apple Silicon/Intel and glibc x86_64 Linux. Windows/musl/Linux ARM are not qualified by this installer.')
    return targets[system, machine]


def owned_directory(path):
    marker = path / '.pipkin-source-setup'
    if any(parent.is_symlink() for parent in [path, *path.parents] if parent != ROOT) or (path.exists() and not marker.is_file()):
        raise RuntimeError(f'Refusing unidentified setup directory: {path}')
    path.mkdir(parents=True, exist_ok=True)
    marker.write_text('Pipkin checkout-local source setup v1\n')


def preflight(system):
    if os.geteuid() == 0:
        raise RuntimeError('Run source setup as your normal user, not root/sudo.')
    if sys.version_info < (3, 11):
        raise RuntimeError('Python 3.11+ is required; see docs/source-setup.md.')
    needed = ['git', 'cargo', 'rustup', 'python3', 'tar', 'sha256sum', 'find', 'zstd', 'clang', 'cmake', 'pkg-config']
    if system == 'Darwin':
        needed += ['codesign', 'ditto', 'otool', 'xcode-select']
    missing = [name for name in needed if not shutil.which(name)]
    if missing:
        raise RuntimeError('Missing build prerequisites: ' + ', '.join(missing) + '. See docs/source-setup.md for your package-manager command; no packages were installed automatically.')
    result = subprocess.run(['tar', '--version'], capture_output=True, text=True, check=True)
    if 'GNU tar' not in result.stdout:
        raise RuntimeError('GNU tar must be on PATH. On Mac: export PATH="$(brew --prefix gnu-tar)/libexec/gnubin:$PATH"; see docs/source-setup.md.')
    run(['find', str(ROOT / 'scripts'), '-maxdepth', '0'])
    if system == 'Darwin':
        run(['xcode-select', '-p'])
    if os.environ.get('CARGO_BUILD_TARGET') or os.environ.get('CARGO_TARGET_DIR'):
        raise RuntimeError('Unset CARGO_BUILD_TARGET/CARGO_TARGET_DIR: source setup supports the native target/release layout only.')


def install_mac(app, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        import plistlib
        identity = plistlib.loads((destination / 'Contents/Info.plist').read_bytes())
        if identity.get('CFBundleIdentifier') != 'org.last-refuge.pipkin' or destination.is_symlink():
            raise RuntimeError('Refusing to replace an unrelated app or symlink')
    with tempfile.TemporaryDirectory(prefix='.pipkin-install-', dir=destination.parent) as scratch:
        staged = Path(scratch) / 'Pipkin.app'
        run(['ditto', app, staged])
        run(['codesign', '--verify', '--deep', '--strict', staged])
        backup = Path(scratch) / 'previous.app'
        if destination.exists():
            destination.rename(backup)
        try:
            staged.rename(destination)
        except OSError:
            if backup.exists():
                backup.rename(destination)
            raise
    print(f'Installed {destination} (ad-hoc signed, not notarized).')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='check build prerequisites only; no download/build/install')
    parser.add_argument('--install', action='store_true', help='also install for this user: Linux ~/.local, Mac ~/Applications/Pipkin.app')
    parser.add_argument('--run', action='store_true', help='launch after building, with this Pipkin checkout as the project')
    options = parser.parse_args()
    system = platform.system()
    target = target_for(system, platform.machine())
    preflight(system)
    if options.check:
        print(f'Prerequisites ready for {target}. GPU/desktop libraries are checked by the native build/runtime, not by this command.')
        return
    base = ROOT / 'dist/source-setup'
    owned_directory(base)
    # Never rebuild in a running engine's tree or overwrite a running executable.
    prefix = base / 'builds' / uuid.uuid4().hex
    owned_directory(prefix)
    env = dict(os.environ)
    run(['python3', ROOT / 'scripts/verify-reqwest-vendor.py'], env=env)
    run(['python3', ROOT / 'scripts/verify-regress-vendor.py'], env=env)
    run(['cargo', 'fetch', '--locked', '--target', target], env=env)
    run(['cargo', 'build', '-p', 'pipkin-app', '--release', '--locked'], env=env)
    (prefix / 'bin').mkdir(exist_ok=True)
    shutil.copy2(ROOT / 'target/release/pipkin', prefix / 'bin/pipkin')
    # Keep the private probe on the build filesystem: /tmp may be a quota-limited
    # tmpfs even when the checkout has plenty of space. No inherited credentials
    # or external-server overrides; TemporaryDirectory removes the scratch profile.
    with tempfile.TemporaryDirectory(prefix='probe-', dir=base) as profile:
        probe_env = {'HOME': profile, 'TMPDIR': profile, 'PATH': '/usr/bin:/bin:/usr/sbin:/sbin'}
        run([prefix / 'bin/pipkin', '--diagnose', '--probe', '--data-dir', Path(profile) / 'app-data'], env=probe_env)
    current = base / 'current'
    if current.exists() and not current.is_symlink():
        raise RuntimeError('Refusing to replace an unidentified current-build pointer')
    pending = base / ('current-' + uuid.uuid4().hex)
    pending.symlink_to(prefix, target_is_directory=True)
    pending.replace(current)
    if options.install:
        run(['cargo', 'fetch', '--locked', '--target', target], env=env)
        if system == 'Darwin':
            stage = ROOT / 'dist/macos-notices'
            components = stage / 'usr/lib/pipkin'
            components.mkdir(parents=True, exist_ok=True)
            # Refuse stale legacy staging rather than inventorying a retired runtime.
            if any((components / name).exists() for name in ['engine', 'runtime']):
                raise RuntimeError('Remove stale dist/macos-notices legacy staging before packaging.')
            run(['python3', ROOT / 'scripts/write-license-inventory.py', stage, target], env=env)
            run(['python3', ROOT / 'scripts/package-macos.py', target, '--app-only'], env=env)
            install_mac(ROOT / 'dist/macos/Pipkin.app', Path.home() / 'Applications/Pipkin.app')
        else:
            run([ROOT / 'scripts/package.sh'], env=env)
            shutil.copy2(ROOT / 'packaging/install.sh', ROOT / 'dist/native/stage/install.sh')
            run(['bash', ROOT / 'dist/native/stage/install.sh'], env=env)
    print('\nReady: scripts/run.sh\nSubagents are included by default. Connect a provider/model in Pipkin; setup does not make paid requests.\nYou can rebuild while Pipkin runs: old build trees are retained. Quit the old app before launching the new one so its worker is not reused.')
    if options.run:
        os.execv(str(ROOT / 'scripts/run.sh'), [str(ROOT / 'scripts/run.sh')])


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError, OSError) as error:
        print(f'Pipkin setup failed: {error}', file=sys.stderr)
        sys.exit(1)
