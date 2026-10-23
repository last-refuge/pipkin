"""Coordination for immutable source builds, launch and explicit cleanup."""
from contextlib import contextmanager
import fcntl
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent.parent
MARKER = '.pipkin-source-setup'
IDENTITY = 'Pipkin checkout-local source setup v1\n'


@contextmanager
def directory_lock(path, *, exclusive):
    """Lock the directory inode, without creating a file or following a symlink."""
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        try:
            fcntl.flock(fd, (fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH) | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError(f'Build storage is in use: {path}. Try again after the other operation finishes.') from error
        yield fd
    finally:
        os.close(fd)


def launch(arguments):
    if sys.platform != 'linux':
        # Cleanup is Linux-only; retain the existing canonical-path launch on macOS.
        binary = (ROOT / 'dist/source-setup/current/bin/pipkin').resolve(strict=True)
        os.execv(binary, [str(binary), '--project', str(ROOT), *arguments])
        return
    # Protect selection of current until the executable's own immutable build is
    # locked. Keep only that build lock across exec, allowing other trees to be pruned.
    with directory_lock(ROOT, exclusive=False):
        build = (ROOT / 'dist/source-setup/current').resolve(strict=True)
        binary = build / 'bin/pipkin'
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise RuntimeError('No successful source build yet. Run scripts/setup.sh first.')
        fd = os.open(build, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
            os.set_inheritable(fd, True)
        except BaseException:
            os.close(fd)
            raise
    try:
        os.execv(binary, [str(binary), '--project', str(ROOT), *arguments])
    finally:
        os.close(fd)


if __name__ == '__main__':
    try:
        launch(sys.argv[1:])
    except (OSError, RuntimeError) as error:
        print(f'Pipkin launch failed: {error}', file=sys.stderr)
        sys.exit(1)
