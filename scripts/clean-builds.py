#!/usr/bin/env python3
"""Preview/prune unused checkout-local source builds. Linux only; no app data or installed versions."""
import argparse
from contextlib import ExitStack
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys

sys.dont_write_bytecode = True
from build_storage import IDENTITY, MARKER, ROOT, directory_lock


def under(path, parent):
    return path == parent or parent in path.parents


def checked(path, root, *, directory=True):
    """All deletion ancestors must be real, user-owned paths inside this checkout."""
    if not under(path, root) or path == root:
        raise RuntimeError(f'Refusing path outside build storage: {path}')
    for part in [path, *path.parents]:
        if part == root:
            break
        info = part.lstat()
        if stat.S_ISLNK(info.st_mode) or info.st_uid != os.getuid():
            raise RuntimeError(f'Refusing symlink or foreign-owned build path: {part}')
        if (part != path or directory) and not stat.S_ISDIR(info.st_mode):
            raise RuntimeError(f'Expected a build directory: {part}')
    return path.stat()


def owned_build(path, root):
    info = checked(path, root)
    marker = path / MARKER
    checked(marker, root, directory=False)
    if not marker.is_file() or marker.read_text() != IDENTITY:
        raise RuntimeError(f'Refusing unidentified source build: {path}')
    return info


def session_service(entry, proc, comm, argv):
    """systemd's non-dumpable user manager/PAM helper cannot run checkout binaries.

    Their exe/cwd links are intentionally unreadable even to the same UID. This
    narrow exception does not allow skipping an opaque app, shell or build tool.
    """
    def user_manager(args):
        return bool(args) and args[0] in ('/usr/lib/systemd/systemd', '/lib/systemd/systemd') and '--user' in args
    if comm == 'systemd' and user_manager(argv):
        return True
    if comm != '(sd-pam)' or argv != ['(sd-pam)']:
        return False
    status = (entry / 'status').read_text().splitlines()
    parent = next(line.split()[1] for line in status if line.startswith('PPid:'))
    manager = proc / parent
    args = [os.fsdecode(a) for a in (manager / 'cmdline').read_bytes().split(b'\0') if a]
    return manager.stat().st_uid == os.getuid() and (manager / 'comm').read_text().strip() == 'systemd' and user_manager(args)


def process_paths(root, proc=Path('/proc')):
    """Inspect same-user executables/cwds/absolute arguments; refuse incomplete visibility."""
    active = set()
    busy = []
    tools = {'cargo', 'rustc', 'rustdoc', 'clippy-driver', 'rustfmt'}
    setup_names = {'setup.py', 'setup.sh', 'package.sh', 'package-native.sh', 'package-macos.py'}
    for entry in proc.iterdir():
        if not entry.name.isdecimal() or int(entry.name) == os.getpid():
            continue
        try:
            if entry.stat().st_uid != os.getuid():
                continue
            args = (entry / 'cmdline').read_bytes().split(b'\0')
            # Zombies have no executable/cwd and cannot start more work.
            if not any(args):
                continue
            comm = (entry / 'comm').read_text().strip()
            argv = [os.fsdecode(arg) for arg in args if arg]
            try:
                exe = Path(os.readlink(entry / 'exe').removesuffix(' (deleted)'))
                cwd = Path(os.readlink(entry / 'cwd').removesuffix(' (deleted)'))
            except PermissionError:
                if session_service(entry, proc, comm, argv):
                    continue
                raise
            paths = {exe, cwd}
            paths.update(Path(arg) for arg in argv if arg.startswith('/'))
            active.update(paths)
            related = under(cwd, root) or any(under(p, root) for p in paths)
            if related and (comm in tools or any(Path(a).name in setup_names for a in argv[:3])):
                busy.append(f'{comm} (PID {entry.name})')
        except (FileNotFoundError, ProcessLookupError):
            continue  # The process exited during inspection.
        except PermissionError as error:
            raise RuntimeError(f'Cannot inspect same-user process {entry.name}; refusing cleanup.') from error
    if busy:
        raise RuntimeError('Build/test/cleanup operation is active: ' + ', '.join(busy) + '. Try again when it finishes.')
    return active


def catalog(root, keep, active):
    base = root / 'dist/source-setup'
    if not base.exists() and not base.is_symlink():
        return [], None
    owned_build(base, root)
    builds = base / 'builds'
    if not builds.exists() and not builds.is_symlink():
        return [], None
    checked(builds, root)
    current_link = base / 'current'
    current = None
    if current_link.exists() or current_link.is_symlink():
        if not current_link.is_symlink():
            raise RuntimeError(f'Refusing non-symlink current pointer: {current_link}')
        current = current_link.resolve(strict=True)
        if current.parent != builds:
            raise RuntimeError('Current source build is outside the managed builds directory.')
        owned_build(current, root)
    entries = []
    for path in sorted(builds.iterdir()):
        if not re.fullmatch(r'[0-9a-f]{32}', path.name):
            entries.append((path, 'unrecognized name; leave untouched', None))
            continue
        info = owned_build(path, root)
        entries.append((path, '', info))
    newest = {p for p, _, _ in sorted((e for e in entries if e[2]), key=lambda e: e[2].st_mtime_ns, reverse=True)[:keep]}
    plan = []
    for path, reason, info in entries:
        if not reason:
            if path == current:
                reason = 'current'
            elif any(under(p, path) for p in active):
                reason = 'running process'
            elif path in newest:
                reason = f'newest {keep}'
        plan.append((path, reason, info))
    return plan, current


def allocated(path):
    """Allocated-block estimate; never follow links or count hard links twice."""
    seen = set()
    total = 0
    for folder, dirs, files in os.walk(path, followlinks=False):
        for name in ['.', *dirs, *files]:
            info = (Path(folder) / name).lstat()
            identity = info.st_dev, info.st_ino
            if identity not in seen:
                total += info.st_blocks * 512
                seen.add(identity)
    return total


def gib(size):
    return f'{size / 1024**3:.2f} GiB'


def cleanup(root, *, apply=False, debug=False, keep=3):
    if sys.platform != 'linux':
        raise RuntimeError('Cleanup currently supports Linux /proc process checks only.')
    if os.geteuid() == 0:
        raise RuntimeError('Run cleanup as your normal user, not root/sudo.')
    if keep < 1:
        raise RuntimeError('--keep must be at least 1; current/running builds are always additional protections.')
    if not shutil.rmtree.avoids_symlink_attacks:
        raise RuntimeError('This Python lacks safe directory-tree removal.')
    # Setup and the source launcher coordinate on this inode. No lock file is created.
    with directory_lock(root, exclusive=True), ExitStack() as locks:
        active = process_paths(root)
        plan, current = catalog(root, keep, active)
        candidates = []
        total = 0
        for path, reason, info in plan:
            if not reason:
                try:
                    locks.enter_context(directory_lock(path, exclusive=True))
                except RuntimeError:
                    reason = 'running build lock'
            if reason:
                print(f'KEEP {path.relative_to(root)} — {reason}')
                continue
            size = allocated(path)
            total += size
            candidates.append((path, info))
            print(f'{"REMOVE" if apply else "WOULD REMOVE"} {path.relative_to(root)} — {gib(size)}')
        print(f'Source builds selected: {len(candidates)}; allocated estimate: {gib(total)}')
        if debug:
            if os.environ.get('CARGO_TARGET_DIR') or os.environ.get('CARGO_BUILD_TARGET'):
                raise RuntimeError('Unset CARGO_TARGET_DIR/CARGO_BUILD_TARGET; cleanup only manages this checkout’s native target/debug.')
            target = root / 'target'
            if target.exists() or target.is_symlink():
                checked(target, root)
                if (target / 'debug').exists() or (target / 'debug').is_symlink():
                    checked(target / 'debug', root)
            if any(under(p, target / 'debug') for p in active):
                raise RuntimeError('A process is using target/debug; quit that test/debug app before cleaning it.')
            print('Debug artifacts: cargo clean --profile dev (release artifacts retained)', flush=True)
            command = ['cargo', 'clean', '--profile', 'dev', '--target-dir', str(target)]
            if not apply:
                command.append('--dry-run')
            # Cargo itself serializes this operation against raw cargo invocations.
            subprocess.run(command, cwd=root, check=True)
        else:
            print('Debug artifacts: untouched (add --debug to include them).')
        if not apply:
            print('Preview only. Re-run with --apply to remove the selected artifacts.')
            return
        # Check again after the potentially long Cargo cleanup. Official launch/setup
        # cannot start under our root lock; also catch already-running/older launchers.
        active = process_paths(root)
        _, latest_current = catalog(root, keep, active)
        if latest_current != current:
            raise RuntimeError('Current build changed during cleanup; no source builds were removed.')
        for path, before in candidates:
            now = owned_build(path, root)
            if (now.st_dev, now.st_ino, now.st_mtime_ns) != (before.st_dev, before.st_ino, before.st_mtime_ns):
                raise RuntimeError(f'Build changed during cleanup: {path}')
        removed = 0
        for path, _ in candidates:
            if any(under(p, path) for p in active):
                print(f'KEEP {path.relative_to(root)} — process started since preview')
                continue
            shutil.rmtree(path)
            removed += 1
        print(f'Removed {removed} unused source builds. Installed versions and app data were not touched.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--apply', action='store_true', help='delete eligible artifacts; default is preview only')
    parser.add_argument('--debug', action='store_true', help='also clean target/debug through Cargo; preserves target/release')
    parser.add_argument('--keep', type=int, default=3, metavar='N', help='retain the N newest source builds (default 3), plus current/running builds')
    options = parser.parse_args()
    cleanup(ROOT, apply=options.apply, debug=options.debug, keep=options.keep)


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'Pipkin cleanup refused/failed: {error}', file=sys.stderr)
        sys.exit(1)
