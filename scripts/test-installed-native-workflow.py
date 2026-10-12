#!/usr/bin/env python3
"""Installed worker migration/coding/restart/archive check against a local HTTP provider.

Uses synthetic credentials and a scratch HOME/project. Optional checkpoints allow the
two-artifact driver to upgrade/roll back between worker runs. No GUI/live provider is used.
Usage: python3 scripts/test-installed-native-workflow.py INSTALLED_PIPKIN FIXTURE_DIR
"""
import hashlib
import http.server
import json
import os
from pathlib import Path
import queue
import re
import shutil
import sqlite3
import stat
import struct
import subprocess
import sys
import tempfile
import tarfile
import threading


def message_text(message):
    content = message.get("content", "")
    return content if isinstance(content, str) else "".join(part.get("text", "") for part in content)


class Worker:
    def __init__(self, binary, root, protocol=None):
        env = {"HOME": str(root / "home"), "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
               "TMPDIR": str(root / "tool-tmp")}
        (root / "tool-tmp").mkdir(exist_ok=True)
        if protocol is None:
            version = subprocess.check_output([str(binary), "--version", "--engine", "native"],
                text=True, env=env, timeout=10)
            protocol = int(re.search(r"engine protocol (\d+)", version).group(1))
        self.log = (root / "worker.log").open("ab")
        self.process = subprocess.Popen(
            [str(binary), "--engine-worker", "--engine-data-dir", str(root / "data"),
             "--engine-config-dir", str(root / "config")],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
            env=env,
        )
        self.frames = queue.Queue()
        self.next_id = 0
        threading.Thread(target=self.read, daemon=True).start()
        try:
            assert self.call("hello", protocol=protocol, client="installed-workflow")["protocol"] == protocol
        except BaseException:
            self.close()
            raise

    def read(self):
        def exact(size):
            data = bytearray()
            while len(data) < size:
                part = self.process.stdout.read(size - len(data))
                if not part:
                    raise EOFError("worker closed its output")
                data.extend(part)
            return data
        try:
            while True:
                size, = struct.unpack(">I", exact(4))
                assert size <= 32 * 1024 * 1024, "oversized worker frame"
                self.frames.put(json.loads(exact(size)))
        except BaseException as error:
            self.frames.put(error)

    def call(self, name, **fields):
        self.next_id += 1
        body = json.dumps({"id": self.next_id, "call": {"call": name, **fields}}).encode()
        self.process.stdin.write(struct.pack(">I", len(body)) + body)
        self.process.stdin.flush()
        # One absolute deadline, including all interleaved events.
        import time
        deadline = time.monotonic() + 30
        while True:
            frame = self.frames.get(timeout=max(0, deadline - time.monotonic()))
            if isinstance(frame, BaseException):
                raise frame
            if frame["frame"] == "reply" and frame["id"] == self.next_id:
                assert "Ok" in frame["result"], frame
                return frame["result"]["Ok"]

    def close(self):
        if self.process.stdin and not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            raise AssertionError("installed worker did not exit after stdin closed")
        finally:
            self.process.stdout.close()
            self.log.close()
        assert self.process.returncode == 0, self.process.returncode


def tree_manifest(root):
    result = {}
    for folder in ["data", "config"]:
        for path in [root / folder, *sorted((root / folder).rglob("*"))]:
            assert not path.is_symlink(), path
            result[str(path.relative_to(root))] = {
                "mode": stat.S_IMODE(path.stat().st_mode),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None,
            }
    return result


def archive_and_restore(root):
    """Closed-profile procedure: retain originals and verify every restored byte and mode."""
    before = tree_manifest(root)
    archive = root / "native-profile.tar"
    with archive.open("xb") as stream:
        os.fchmod(stream.fileno(), 0o600)
        with tarfile.open(fileobj=stream, mode="w") as tar:
            for folder in ["data", "config"]:
                tar.add(root / folder, arcname=folder)
    for folder in ["data", "config"]:
        (root / folder).rename(root / ("saved-" + folder))
    # This archive was just made from our own closed scratch profile. GNU tar -p preserves
    # directory privacy too; Python's data extraction filter intentionally drops dir modes.
    subprocess.run(["tar", "--extract", "--preserve-permissions", "--file", str(archive),
                    "--directory", str(root)], check=True, timeout=30)
    assert tree_manifest(root) == before, "restored profile bytes or permissions differ"
    with sqlite3.connect(root / "data/engine.sqlite") as db:
        assert db.execute("PRAGMA integrity_check").fetchone() == ("ok",)
    # Temp spill paths are not a durable archive. Full retained output must survive their loss.
    shutil.rmtree(root / "tool-tmp")
    print("closed native data/config archive restores every file and permission; SQLite integrity passes")


def main(binary, fixture, checkpoint=lambda phase, root: None):
    with tempfile.TemporaryDirectory(prefix="pipkin-installed-workflow-") as temp:
        root = Path(temp)
        for directory in ["home", "config", "project"]:
            (root / directory).mkdir()
        source = root / "agent/experimental/sessions/forks"
        shutil.copytree(fixture, source)
        (source / "meta.json").write_text(json.dumps({"createdAt": 1700000000000, "cwd": str(root / "project")}))
        original = {p.name: p.read_bytes() for p in source.iterdir() if p.is_file()}
        reference = json.loads((fixture / "reference.json").read_text())
        assert hashlib.sha256(original["session.sqlite"]).hexdigest() == reference["sqliteSha256"]
        requests = []
        tools = [
            ("write", {"path": "hello.rs", "content": 'fn main() { println!("1"); }\n'}),
            ("edit", {"path": "hello.rs", "oldText": 'println!("1")', "newText": 'println!("2")'}),
            ("bash", {"command": "test -f hello.rs && printf 'checked\\n' >> tool-runs.txt; head -c 1048576 /dev/zero | tr '\\0' x; printf '\\nretained-output-end\\n'"}),
            ("read", {"path": "hello.rs"}),
        ]

        class Provider(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                index = len(requests)
                requests.append(body)
                delta = {"role": "assistant", "content": "installed coding complete"}
                finish = "stop"
                if index < len(tools):
                    name, arguments = tools[index]
                    delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": f"installed-{index}", "type": "function", "function": {"name": name, "arguments": json.dumps(arguments)}}]}
                    finish = "tool_calls"
                chunks = []
                for data, reason in [(delta, None), ({}, finish)]:
                    chunks.append("data: " + json.dumps({"id": "installed-test", "object": "chat.completion.chunk", "model": "coding", "choices": [{"index": 0, "delta": data, "finish_reason": reason}]}) + "\n\n")
                payload = ("".join(chunks) + "data: [DONE]\n\n").encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        worker = None
        try:
            (root / "config/models.json").write_text(json.dumps({"providers": {"installed-test": {
                "baseUrl": f"http://127.0.0.1:{server.server_port}/v1", "api": "openai-completions", "apiKey": "synthetic",
                "models": [{"id": "coding", "name": "Installed test", "input": ["text"]}],
            }}}))
            (root / "config").chmod(0o700)
            (root / "config/models.json").chmod(0o600)
            worker = Worker(binary, root)
            imported = worker.call("importSessions", agentDir=str(root / "agent"), serverDir=str(root / "server"), sessions=["forks"])
            assert imported["sessions"][0]["outcome"]["outcome"] == "imported", imported
            assert len(worker.call("listSessions")["sessions"]) == 7
            session = "forks:c20"
            worker.call("openSession", session=session)
            worker.call("setModel", session=session, model="installed-test/coding")
            prompt = {"session": session, "request": "installed-code", "text": "Create, edit and check hello.rs"}
            worker.call("prompt", **prompt)
            assert worker.call("waitForPrompt", session=session, request=prompt["request"])["status"] == "done"
            assert len(requests) == 5, "four real tool executions, followed by the answer"
            assert (root / "project/hello.rs").read_text() == 'fn main() { println!("2"); }\n'
            assert (root / "project/tool-runs.txt").read_text() == "checked\n"
            first = requests[0]["messages"]
            users = [message_text(m) for m in first if m["role"] == "user"]
            assert users == ["shared start", "nested rewind to ancestor", prompt["text"]], users
            results = [m for m in requests[4]["messages"] if m["role"] == "tool"]
            assert len(results) == 4 and 'println!("2")' in message_text(results[-1]), results
            expected_output = "x" * 1048576 + "\nretained-output-end\n"
            def check_output():
                output = worker.call("toolOutput", session=session, call_id="installed-2")
                assert output["text"] == expected_output
                assert output["droppedBytes"] == 0
            check_output()
            worker.close()
            worker = None
            checkpoint("upgrade", root)
            worker = Worker(binary, root)
            assert worker.call("lookup", session=session, request=prompt["request"])["outcome"]["reason"] == "completed"
            worker.call("prompt", **prompt)  # Durable replay must never execute tools again.
            assert worker.call("waitForPrompt", session=session, request=prompt["request"])["status"] == "done"
            assert len(requests) == 5
            assert (root / "project/tool-runs.txt").read_text() == "checked\n"
            worker.call("prompt", session=session, request="after-restart", text="Continue after restart")
            assert worker.call("waitForPrompt", session=session, request="after-restart")["status"] == "done"
            assert len(requests) == 6
            assert len([m for m in requests[-1]["messages"] if m["role"] == "tool"]) == 4
            check_output()
            before_rollback = worker.call("openSession", session=session)["entries"]
            worker.close()
            worker = None
            checkpoint("rollback", root)
            worker = Worker(binary, root)
            assert worker.call("openSession", session=session)["entries"] == before_rollback
            worker.call("prompt", session=session, request="after-rollback", text="Continue after rollback")
            assert worker.call("waitForPrompt", session=session, request="after-rollback")["status"] == "done"
            assert len(requests) == 7
            before_restore = worker.call("openSession", session=session)["entries"]
            worker.close()
            worker = None
            archive_and_restore(root)
            checkpoint("restore", root)
            worker = Worker(binary, root)
            assert worker.call("openSession", session=session)["entries"] == before_restore
            check_output()
            for key in ["installed-code", "after-restart", "after-rollback"]:
                assert worker.call("lookup", session=session, request=key)["outcome"]["reason"] == "completed"
            worker.call("prompt", **prompt)
            assert worker.call("waitForPrompt", session=session, request=prompt["request"])["status"] == "done"
            assert len(requests) == 7, "archive restore must preserve durable request deduplication"
            worker.call("prompt", session=session, request="after-restore", text="Continue after archive restore")
            assert worker.call("waitForPrompt", session=session, request="after-restore")["status"] == "done"
            assert len(requests) == 8
            assert len([m for m in requests[-1]["messages"] if m["role"] == "tool"]) == 4
            assert (root / "project/hello.rs").read_text() == 'fn main() { println!("2"); }\n'
            assert (root / "project/tool-runs.txt").read_text() == "checked\n"
            again = worker.call("importSessions", agentDir=str(root / "agent"), serverDir=str(root / "server"), sessions=["forks"])
            assert again["sessions"][0]["outcome"]["outcome"] == "alreadyImported", again
            assert all((source / name).read_bytes() == content for name, content in original.items())
            print("installed native migration, coding, restart, complete archive/restore, retained output and durable replay passed")
        finally:
            if worker:
                worker.close()
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve())
