#!/usr/bin/env python3
"""Disposable cleanup/launch fixtures; never cleans the real checkout or user profile."""
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('clean_builds', SCRIPTS / 'clean-builds.py')
clean = importlib.util.module_from_spec(spec)
spec.loader.exec_module(clean)
from build_storage import directory_lock


@unittest.skipUnless(sys.platform == 'linux', 'cleanup process guards are Linux-only')
class CleanupTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.base = self.root / 'dist/source-setup'
        self.base.mkdir(parents=True)
        (self.base / clean.MARKER).write_text(clean.IDENTITY)
        self.builds = []
        for index in range(1, 7):
            p = self.base / 'builds' / f'{index:032x}'
            (p / 'bin').mkdir(parents=True)
            (p / clean.MARKER).write_text(clean.IDENTITY)
            (p / 'bin/pipkin').write_text('fixture binary')
            os.utime(p, (index, index))
            self.builds.append(p)
        (self.base / 'current').symlink_to(self.builds[0])
        self.active = {self.builds[1] / 'bin/pipkin'}

    def run_clean(self, **options):
        with patch.object(clean, 'process_paths', return_value=self.active), contextlib.redirect_stdout(io.StringIO()) as output:
            clean.cleanup(self.root, keep=1, **options)
            return output.getvalue()

    def test_preview_is_read_only_and_apply_keeps_current_running_and_newest(self):
        installed = self.root / 'unrelated-installed-app'
        installed.write_text('untouched')
        unknown = self.base / 'builds' / 'not-managed'
        unknown.symlink_to(installed)
        preview = self.run_clean()
        self.assertIn('Source builds selected: 3', preview)
        self.assertTrue(all(p.exists() for p in self.builds))
        result = self.run_clean(apply=True)
        self.assertIn('Removed 3 unused source builds', result)
        self.assertEqual([p.exists() for p in self.builds], [True, True, False, False, False, True])
        self.assertEqual(installed.read_text(), 'untouched')
        self.assertTrue(unknown.is_symlink())
        self.assertEqual((self.base / 'current').resolve(), self.builds[0])

    def test_current_pointer_and_markers_fail_closed(self):
        marker = self.builds[3] / clean.MARKER
        marker.write_text('unrelated data')
        with self.assertRaises(RuntimeError):
            self.run_clean(apply=True)
        self.assertTrue(all(p.exists() for p in self.builds))
        marker.write_text(clean.IDENTITY)
        (self.base / 'current').unlink()
        (self.base / 'current').symlink_to(self.root)
        with self.assertRaises(RuntimeError):
            self.run_clean(apply=True)
        self.assertTrue(all(p.exists() for p in self.builds))

    def test_symlinked_managed_path_is_refused(self):
        p = self.builds[3]
        moved = self.root / 'outside-builds'
        p.rename(moved)
        p.symlink_to(moved, target_is_directory=True)
        with self.assertRaises(RuntimeError):
            self.run_clean(apply=True)
        self.assertTrue((moved / 'bin/pipkin').exists())

    def test_setup_and_launch_locks_are_respected(self):
        with directory_lock(self.root, exclusive=False):
            with self.assertRaisesRegex(RuntimeError, 'in use'):
                self.run_clean(apply=True)
        with directory_lock(self.builds[3], exclusive=False):
            result = self.run_clean(apply=True)
        self.assertIn('running build lock', result)
        self.assertTrue(self.builds[3].exists())
        self.assertFalse(self.builds[2].exists())

    def test_debug_cleanup_is_opt_in_and_delegated_to_cargo(self):
        with patch.object(clean.subprocess, 'run') as run:
            self.run_clean()
            run.assert_not_called()
            self.run_clean(debug=True)
            self.assertEqual(run.call_args.args[0], ['cargo', 'clean', '--profile', 'dev', '--target-dir', str(self.root / 'target'), '--dry-run'])
            self.run_clean(debug=True, apply=True)
            self.assertNotIn('--dry-run', run.call_args.args[0])
        self.assertTrue(self.builds[0].exists())

    def test_debug_running_binary_and_external_target_are_refused(self):
        self.active.add(self.root / 'target/debug/deps/test-binary')
        with patch.object(clean.subprocess, 'run') as run:
            with self.assertRaisesRegex(RuntimeError, 'using target/debug'):
                self.run_clean(debug=True, apply=True)
            run.assert_not_called()
        self.active.clear()
        with patch.dict(os.environ, {'CARGO_TARGET_DIR': '/some/other/project'}):
            with self.assertRaisesRegex(RuntimeError, 'Unset CARGO'):
                self.run_clean(debug=True, apply=True)
        self.assertTrue(all(p.exists() for p in self.builds))

    def test_new_process_is_rechecked_before_removal(self):
        def inspection(root):
            if inspection.calls:
                self.active.add(self.builds[3] / 'bin/pipkin')
            inspection.calls += 1
            return set(self.active)
        inspection.calls = 0
        with patch.object(clean, 'process_paths', side_effect=inspection), contextlib.redirect_stdout(io.StringIO()):
            clean.cleanup(self.root, keep=1, apply=True)
        self.assertTrue(self.builds[3].exists())
        self.assertFalse(self.builds[2].exists())

    def test_changed_build_aborts_before_removing_any_source_builds(self):
        def inspection(root):
            if inspection.calls:
                os.utime(self.builds[3], (123, 123))
            inspection.calls += 1
            return set(self.active)
        inspection.calls = 0
        with patch.object(clean, 'process_paths', side_effect=inspection), contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, 'Build changed'):
                clean.cleanup(self.root, keep=1, apply=True)
        self.assertTrue(all(p.exists() for p in self.builds))

    def test_process_inspection_detects_cargo_and_running_build_paths(self):
        proc = self.root / 'proc'
        p = proc / '99999999'
        p.mkdir(parents=True)
        (p / 'comm').write_text('cargo\n')
        (p / 'cmdline').write_bytes(b'cargo\x00test\x00')
        (p / 'exe').symlink_to('/usr/bin/cargo')
        (p / 'cwd').symlink_to(self.root)
        with self.assertRaisesRegex(RuntimeError, 'active'):
            clean.process_paths(self.root, proc)
        (p / 'comm').write_text('pipkin\n')
        (p / 'exe').unlink()
        (p / 'exe').symlink_to(self.builds[3] / 'bin/pipkin')
        self.assertIn(self.builds[3] / 'bin/pipkin', clean.process_paths(self.root, proc))
        with patch.object(clean.os, 'readlink', side_effect=PermissionError):
            with self.assertRaisesRegex(RuntimeError, 'Cannot inspect'):
                clean.process_paths(self.root, proc)

    def test_only_systemd_user_manager_and_its_pam_helper_may_be_opaque(self):
        proc = self.root / 'proc'
        manager = proc / '99999998'
        pam = proc / '99999999'
        manager.mkdir(parents=True)
        pam.mkdir()
        (manager / 'comm').write_text('systemd\n')
        (manager / 'cmdline').write_bytes(b'/usr/lib/systemd/systemd\0--user\0')
        (pam / 'comm').write_text('(sd-pam)\n')
        (pam / 'cmdline').write_bytes(b'(sd-pam)\0')
        (pam / 'status').write_text('PPid: 99999998\n')
        with patch.object(clean.os, 'readlink', side_effect=PermissionError):
            self.assertEqual(clean.process_paths(self.root, proc), set())
            (manager / 'cmdline').write_bytes(b'/tmp/not-systemd\0--user\0')
            with self.assertRaisesRegex(RuntimeError, 'Cannot inspect'):
                clean.process_paths(self.root, proc)

    def test_source_launcher_keeps_only_its_build_locked_across_exec(self):
        binary = self.builds[0] / 'bin/pipkin'
        binary.write_text('#!/usr/bin/env python3\nimport time\nprint("ready", flush=True)\ntime.sleep(30)\n')
        binary.chmod(0o755)
        code = 'import sys; from pathlib import Path; sys.path.insert(0, sys.argv[1]); import build_storage as b; b.ROOT=Path(sys.argv[2]); b.launch([])'
        child = subprocess.Popen([sys.executable, '-B', '-c', code, str(SCRIPTS), str(self.root)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            import select
            self.assertTrue(select.select([child.stdout], [], [], 5)[0], 'launcher did not reach fixture')
            self.assertEqual(child.stdout.readline().strip(), 'ready')
            with directory_lock(self.root, exclusive=True):
                pass  # Other unused builds can still be cleaned while the app runs.
            with self.assertRaises(RuntimeError):
                with directory_lock(self.builds[0], exclusive=True):
                    pass
        finally:
            child.terminate()
            child.communicate(timeout=5)
        with directory_lock(self.builds[0], exclusive=True):
            pass

    def test_compact_profile_keeps_release_settings_and_test_inheritance(self):
        profile = tomllib.loads((SCRIPTS.parent / 'Cargo.toml').read_text())['profile']
        self.assertEqual(profile['dev'], {'debug': 'line-tables-only', 'incremental': False})
        self.assertNotIn('test', profile)
        self.assertEqual(profile['release']['debug'], 'line-tables-only')


if __name__ == '__main__':
    unittest.main()
