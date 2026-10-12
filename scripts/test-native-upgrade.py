#!/usr/bin/env python3
"""Qualify two real native archives through install, coding, upgrade, rollback and restore.

Usage: test-native-upgrade.py PREVIOUS.tar.zst CURRENT.tar.zst [REPORT.json]
Both artifacts must be Linux native candidates with different binary hashes and compatible
engine schemas. Archives are trusted local build inputs. Nothing touches the user's install.
This does not claim Pi rollback, GUI acceptance or a cross-schema downgrade.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("installed_workflow", HERE / "test-installed-native-workflow.py")
workflow = importlib.util.module_from_spec(spec)
spec.loader.exec_module(workflow)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main(previous, current, report_path=None):
    with tempfile.TemporaryDirectory(prefix="pipkin-native-upgrade-") as scratch:
        root = Path(scratch)
        home = root / "home"
        home.mkdir()
        prefix = root / "prefix with spaces"
        shims = root / "shims"
        shims.mkdir()
        for name in ["node", "npm", "npx", "pi"]:
            path = shims / name
            path.write_text(f'#!/bin/sh\ntouch "{root}/used-{name}"\nexit 93\n')
            path.chmod(0o755)
        env = {"HOME": str(home), "PATH": f"{shims}:/usr/bin:/bin"}
        artifacts = []
        for name, archive in [("previous", previous), ("current", current)]:
            directory = root / name
            directory.mkdir()
            subprocess.run(["tar", "--zstd", "-xf", str(archive), "-C", str(directory)], check=True, timeout=60)
            marker = json.loads((directory / "usr/lib/pipkin/native-engine.json").read_text())
            info = json.loads((directory / "usr/share/doc/pipkin/build-info.json").read_text())
            binary = directory / "usr/bin/pipkin"
            assert marker["engine"] == "native"
            assert digest(binary) == info["app"]["binarySha256"], "archive binary identity mismatch"
            artifacts.append({"role": name, "archiveSha256": digest(archive), "marker": marker, "app": info["app"]})
        assert artifacts[0]["app"]["binarySha256"] != artifacts[1]["app"]["binarySha256"], "use two different real builds"
        assert artifacts[0]["marker"]["engineSchema"] == artifacts[1]["marker"]["engineSchema"], "this workflow tests a schema-compatible rollback"
        assert digest(root / "current/install.sh") == digest(REPO / "packaging/install.sh"), "rebuild the current archive with this installer"
        events = []
        def install(which, *args, succeeds=True):
            command = [str(root / which / "install.sh"), "--prefix", str(prefix), *args]
            result = subprocess.run(command, env=env, text=True, capture_output=True, timeout=60)
            if succeeds:
                assert result.returncode == 0, result.stdout + result.stderr
            else:
                assert result.returncode != 0, "damaged rollback unexpectedly succeeded"
            events.append({"action": [which, *args], "exitCode": result.returncode,
                           "stdout": result.stdout, "stderr": result.stderr})
            return result
        def selected():
            return (prefix / "lib/pipkin/current").resolve()
        def verify_selected(index):
            assert digest(prefix / "bin/pipkin") == artifacts[index]["app"]["binarySha256"]
            launcher = (prefix / "share/applications/pipkin.desktop").read_text()
            assert f'Exec="{prefix}/bin/pipkin" --engine native' in launcher
        install("previous")
        verify_selected(0)
        older = selected()
        def checkpoint(phase, profile):
            if phase == "upgrade":
                install("current")
                verify_selected(1)
                current_target = selected()
                prior_binary = older / "usr/bin/pipkin"
                saved = root / "prior-binary"
                shutil.copy2(prior_binary, saved)
                try:
                    with prior_binary.open("ab") as stream:
                        stream.write(b"damaged rollback fixture")
                    result = install("current", "--rollback", succeeds=False)
                    assert "checksum mismatch" in result.stdout + result.stderr
                    assert selected() == current_target
                    verify_selected(1)
                finally:
                    shutil.copy2(saved, prior_binary)
            elif phase == "rollback":
                install("current", "--rollback")
                assert selected() == older
                verify_selected(0)
                future = root / "future-schema"
                future.mkdir()
                database = future / "engine.sqlite"
                shutil.copy2(profile / "data/engine.sqlite", database)
                with sqlite3.connect(database) as db:
                    db.execute(f"PRAGMA user_version = {artifacts[0]['marker']['engineSchema'] + 1}")
                db.close()
                before = digest(database)
                refused = subprocess.run([str(prefix / "bin/pipkin"), "--engine-worker",
                    "--engine-data-dir", str(future), "--engine-config-dir", str(profile / "config")],
                    input=b"", env=env, capture_output=True, timeout=10)
                assert refused.returncode != 0 and b"newer" in refused.stderr, refused.stderr
                assert digest(database) == before, "older installed worker changed a newer-schema database"
                print("older installed package refuses newer engine schema without changing database bytes")
            elif phase == "restore":
                install("current", "--rollback")
                verify_selected(1)
            print(f"installed package checkpoint passed: {phase}")
        inherited_path = os.environ.get("PATH")
        os.environ["PATH"] = env["PATH"]
        try:
            # Keep the symlink: the package checkpoints deliberately change its target.
            workflow.main(prefix / "bin/pipkin", REPO / "fixtures/native-fork-import", checkpoint)
        finally:
            if inherited_path is None:
                os.environ.pop("PATH", None)
            else:
                os.environ["PATH"] = inherited_path
        assert not list(root.glob("used-*")), "native workflow attempted Node/npm/Pi"
        install("current", "--uninstall")
        assert not (prefix / "bin/pipkin").exists()
        report = {"artifacts": artifacts, "installerSha256": digest(REPO / "packaging/install.sh"),
                  "result": "passed", "events": events,
                  "scope": "real native package upgrade/rollback with equal engine schemas, migration/coding/retained output/replay/archive restore; no GUI, Pi rollback, live auth or schema downgrade"}
        if report_path:
            report_path.parent.mkdir(parents=True, exist_ok=True)
            report_path.write_text(json.dumps(report, indent=2) + "\n")
        print("real native upgrade, guarded rollback, full archive restore and continuation passed")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve(), Path(sys.argv[3]) if len(sys.argv) > 3 else None)
