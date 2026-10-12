#!/usr/bin/env python3
"""Check a trusted local native makepkg artifact without installing it system-wide.

Usage: verify-native-arch-package.py ARCHIVE.pkg.tar.zst [REPORT.json]
Checks package metadata, actual payload identity and the extracted binary's isolated
probe/migration/coding/restart/archive workflow. Does not claim a pacman installation or GUI.
"""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

REPO = Path(__file__).resolve().parent.parent


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def fields(path):
    result = {}
    for line in path.read_text().splitlines():
        if line.startswith("#") or " = " not in line:
            continue
        key, value = line.split(" = ", 1)
        result.setdefault(key, []).append(value)
    return result


def main(archive, report_path=None):
    with tempfile.TemporaryDirectory(prefix="pipkin-arch-check-") as temp:
        root = Path(temp)
        payload = root / "payload"
        payload.mkdir()
        subprocess.run(["tar", "--zstd", "-xf", str(archive), "-C", str(payload)], check=True)
        package = fields(payload / ".PKGINFO")
        build = fields(payload / ".BUILDINFO")
        assert package["pkgname"] == ["pipkin"] and package["arch"] == ["x86_64"]
        assert build["pkgname"] == package["pkgname"] and build["pkgver"] == package["pkgver"]
        assert build["pkgbuild_sha256sum"] == [digest(REPO / "packaging/PKGBUILD")]
        assert (payload / ".MTREE").is_file()
        for key in ["depend", "makedepend"]:
            dependencies = {re.split(r"[<>=]", value)[0] for value in package.get(key, [])}
            assert not dependencies.intersection({"nodejs", "npm", "pi"}), dependencies
        forbidden = re.compile(r"(^|/)(node|npm|npx|node_modules|pi-test\.sh|package(-lock)?\.json)(/|$)|\.(m?js|cjs|ts|node)$")
        for path in payload.rglob("*"):
            assert not forbidden.search(path.relative_to(payload).as_posix()), path
        assert not (payload / "usr/lib/pipkin/engine").exists()
        assert not (payload / "usr/lib/pipkin/runtime").exists()
        assert {p.name for p in payload.iterdir()} == {"usr", ".PKGINFO", ".BUILDINFO", ".MTREE"}
        binary = payload / "usr/bin/pipkin"
        docs = payload / "usr/share/doc/pipkin"
        info = json.loads((docs / "build-info.json").read_text())
        marker = json.loads((payload / "usr/lib/pipkin/native-engine.json").read_text())
        assert info["kind"] == "native-engine-candidate"
        assert digest(binary) == info["app"]["binarySha256"], "makepkg changed the recorded binary"
        assert marker["engine"] == "native" and marker["selector"] == "--engine native"
        assert package["pkgver"][0].rsplit("-", 1)[0] == marker["appVersion"]
        desktop = payload / "usr/share/applications/pipkin.desktop"
        assert "Exec=pipkin --engine native" in desktop.read_text().splitlines()
        inventory = json.loads((docs / "third-party/inventory.json").read_text())
        assert {p["ecosystem"] for p in inventory["packages"]} == {"rust"}
        for dependency in inventory["packages"]:
            for notice in dependency["notices"]:
                assert digest(docs / "third-party" / notice["file"]) == notice["sha256"]
            if source := dependency.get("providedSource"):
                assert (docs / "third-party" / source / "Cargo.toml").is_file()
                assert (docs / "third-party" / source / "src/lib.rs").is_file()
        runtime = json.loads((docs / "runtime-inventory.json").read_text())
        assert runtime["requiredNodeMinimum"] is None
        assert [a["file"] for a in runtime["artifacts"]] == ["usr/bin/pipkin"]
        subprocess.run([sys.executable, str(REPO / "scripts/test-package-docs.py"), str(payload), "--native"], check=True)
        validator = shutil.which("desktop-file-validate")
        if validator:
            subprocess.run([validator, str(desktop)], check=True)
        (root / "home").mkdir()
        shims = root / "shims"
        shims.mkdir()
        for tool in ["node", "npm", "npx", "pi"]:
            shim = shims / tool
            shim.write_text(f'#!/bin/sh\nprintf "invoked\\n" >> "{root}/used-{tool}"\nexit 93\n')
            shim.chmod(0o755)
        env = {"HOME": str(root / "home"), "PATH": f"{shims}:/usr/bin:/bin"}
        probe = subprocess.run([str(binary), "--diagnose", "--probe", "--engine", "native"],
            cwd=root, env=env, text=True, capture_output=True, timeout=30)
        assert probe.returncode == 0, probe.stdout + probe.stderr
        assert "engine probe: OK" in probe.stdout, probe.stdout
        workflow = subprocess.run([sys.executable, "-I", str(REPO / "scripts/test-installed-native-workflow.py"),
            str(binary), str(REPO / "fixtures/native-fork-import")], env=env, text=True,
            capture_output=True, timeout=180)
        print(workflow.stdout, end="")
        assert workflow.returncode == 0, workflow.stdout + workflow.stderr
        assert not list(root.glob("used-*")), "Node/Pi was invoked"
        report = {"result": "passed", "archiveSha256": digest(archive),
            "package": package, "marker": marker, "app": info["app"],
            "metadataSha256": {name: digest(payload / name) for name in [".PKGINFO", ".BUILDINFO", ".MTREE"]},
            "rustPackages": len(inventory["packages"]),
            "noticeHashesVerified": sum(len(p["notices"]) for p in inventory["packages"]),
            "licenseReviewFlags": sum(p["reviewRequired"] for p in inventory["packages"]),
            "testInputsSha256": {name: digest(REPO / name) for name in [
                "scripts/verify-native-arch-package.py", "scripts/test-installed-native-workflow.py",
                "scripts/test-package-docs.py", "packaging/PKGBUILD"]},
            "probe": probe.stdout, "workflow": workflow.stdout,
            "scope": "actual makepkg metadata/payload, extracted binary with isolated probe/import/coding/restart/archive restoration and no Node/Pi invocation; no pacman system installation, GUI or live provider"}
        if report_path:
            report_path.parent.mkdir(parents=True, exist_ok=True)
            report_path.write_text(json.dumps(report, indent=2) + "\n")
        print("native Arch package verified; no system installation performed")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve() if len(sys.argv) > 2 else None)
