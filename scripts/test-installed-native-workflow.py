#!/usr/bin/env python3
"""Installed worker migration/coding/restart check against a local scripted HTTP provider.

Uses synthetic credentials and a scratch HOME/project. No GUI or live provider is exercised.
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
import struct
import subprocess
import sys
import tempfile
import threading


def message_text(message):
    content = message.get("content", "")
    return content if isinstance(content, str) else "".join(part.get("text", "") for part in content)


class Worker:
    def __init__(self, binary, root, protocol):
        self.log = (root / "worker.log").open("ab")
        self.process = subprocess.Popen(
            [str(binary), "--engine-worker", "--engine-data-dir", str(root / "data"),
             "--engine-config-dir", str(root / "config")],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
            env={"HOME": str(root / "home"), "PATH": os.environ.get("PATH", "/usr/bin:/bin")},
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


def main(binary, fixture):
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
            ("bash", {"command": "test -f hello.rs && printf 'checked\\n' >> tool-runs.txt"}),
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
            version = subprocess.check_output([str(binary), "--version", "--engine", "native"], text=True,
                env={"HOME": str(root / "home"), "PATH": os.environ.get("PATH", "/usr/bin:/bin")}, timeout=10)
            protocol = int(re.search(r"engine protocol (\d+)", version).group(1))
            worker = Worker(binary, root, protocol)
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
            worker.close()
            worker = Worker(binary, root, protocol)
            assert worker.call("lookup", session=session, request=prompt["request"])["outcome"]["reason"] == "completed"
            worker.call("prompt", **prompt)  # Durable replay must never execute tools again.
            assert worker.call("waitForPrompt", session=session, request=prompt["request"])["status"] == "done"
            assert len(requests) == 5
            assert (root / "project/tool-runs.txt").read_text() == "checked\n"
            worker.call("prompt", session=session, request="after-restart", text="Continue after restart")
            assert worker.call("waitForPrompt", session=session, request="after-restart")["status"] == "done"
            assert len(requests) == 6
            assert len([m for m in requests[-1]["messages"] if m["role"] == "tool"]) == 4
            again = worker.call("importSessions", agentDir=str(root / "agent"), serverDir=str(root / "server"), sessions=["forks"])
            assert again["sessions"][0]["outcome"]["outcome"] == "alreadyImported", again
            assert all((source / name).read_bytes() == content for name, content in original.items())
            print("installed native migration, write/edit/bash/read, restart, durable replay and continuation passed")
        finally:
            if worker:
                worker.close()
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve())
