#!/usr/bin/env python3
"""Real desktop soak against an isolated native server and local scripted provider.

Starts a desktop but sends no synthetic UI input. Create/open its first conversation
using scripts/guard.sh, inspect the window, then create ROOT/go. On completion, inspect
the live window and quit it through the guard; create ROOT/finish to close the server.
All profiles, generated files and raw observations stay in the chosen scratch root.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import struct
import subprocess
import threading
import time


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def text(message):
    value = message.get("content") or ""
    return value if isinstance(value, str) else "".join(b.get("text", "") for b in value)


def reply_text(turn):
    size = 64 * 1024 if turn % 25 == 0 else 8 * 1024
    prefix = f"Local scripted desktop soak — turn {turn:04d}.\n"
    return prefix + ("Rendered stream line.\n" * size)[:size - len(prefix)]


def metrics(pid):
    root = Path(f"/proc/{pid}")
    values = {}
    for line in (root / "status").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in {"VmRSS", "VmHWM", "Threads"}:
            values[key] = int(value.split()[0])
    values["fds"] = len(list((root / "fd").iterdir()))
    fields = (root / "stat").read_text().rsplit(")", 1)[1].split()
    values["cpuTicks"] = int(fields[11]) + int(fields[12])
    return values


class Client:
    def __init__(self, endpoint):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(60)
        self.socket.connect(str(endpoint / "engine.sock"))
        self.next = 0
        self.call("hello", protocol=12, client="desktop-soak-driver")

    def read(self, size):
        data = bytearray()
        while len(data) < size:
            chunk = self.socket.recv(size - len(data))
            if not chunk:
                raise RuntimeError("engine disconnected")
            data.extend(chunk)
        return data

    def call(self, name, **args):
        self.next += 1
        body = json.dumps({"id": self.next, "call": {"call": name, **args}}).encode()
        self.socket.sendall(struct.pack(">I", len(body)) + body)
        while True:
            size = struct.unpack(">I", self.read(4))[0]
            assert size <= 16 * 1024 * 1024
            frame = json.loads(self.read(size))
            if frame["frame"] == "reply" and frame["id"] == self.next:
                assert "Ok" in frame["result"], frame
                return frame["result"]["Ok"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("root", type=Path)
    parser.add_argument("--turns", type=int, default=1500)
    args = parser.parse_args()
    assert args.turns > 0
    # Ensure interrupting the runner also reaps only its own app/server children.
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    binary = args.binary.resolve()
    root = args.root.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    for name in ["home", "config", "project", "profile", "shots", "spills", "shims"]:
        (root / name).mkdir()
    for name in ["node", "npm", "npx", "pi"]:
        shim = root / "shims" / name
        shim.write_text(f'#!/bin/sh\nprintf "%s\\n" "{name}" >> "{root}/forbidden-invocations"\nexit 93\n')
        shim.chmod(0o755)
    workspace = json.loads(subprocess.check_output(["hyprctl", "activeworkspace", "-j"]))["id"]
    subprocess.run(["git", "init", "-q", str(root / "project")], check=True)
    requests = []
    lock = threading.Lock()

    class Provider(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            messages = body["messages"]
            prompt = text(next(m for m in reversed(messages) if m["role"] == "user"))
            assert prompt.startswith("desktop-soak-turn-"), prompt
            turn = int(prompt.removeprefix("desktop-soak-turn-"))
            tool_reply = messages[-1]["role"] == "tool"
            with lock:
                requests.append({"turn": turn, "toolReply": tool_reply,
                                 "bytes": int(self.headers["Content-Length"])})
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True

            def chunk(delta, finish=None):
                value = {"id": "desktop-soak", "object": "chat.completion.chunk", "model": "soak",
                         "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
                if finish:
                    value["usage"] = {"prompt_tokens": 10, "completion_tokens": 10, "total_tokens": 20}
                self.wfile.write(("data: " + json.dumps(value) + "\n\n").encode())
                self.wfile.flush()

            chunk({"role": "assistant", "content": ""})
            if turn % 5 == 0 and not tool_reply:
                command = (f"printf '{turn}\\n' >> effects.log; "
                           "head -c 262144 /dev/zero | tr '\\000' x; "
                           f"printf '\\nfinished-{turn}\\n'")
                chunk({"tool_calls": [{"index": 0, "id": "soak-reused", "type": "function",
                                      "function": {"name": "bash", "arguments": json.dumps({"command": command})}}]})
                chunk({}, "tool_calls")
            else:
                value = reply_text(turn)
                for start in range(0, len(value), 1024):
                    chunk({"content": value[start:start + 1024]})
                    time.sleep(0.01)
                chunk({}, "stop")
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()

    provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    provider.daemon_threads = True
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    save(root / "config/models.json", {"providers": {"desktop-soak": {
        "baseUrl": f"http://127.0.0.1:{provider.server_port}/v1", "api": "openai-completions",
        "apiKey": "synthetic", "models": [{"id": "soak", "name": "Local scripted desktop soak",
            "input": ["text"], "contextWindow": 200000, "maxTokens": 32768}]}}})
    save(root / "config/settings.json", {"retry": {"enabled": False}, "compaction": {"enabled": False}})
    env = {k: os.environ[k] for k in ["WAYLAND_DISPLAY", "DISPLAY", "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS", "XAUTHORITY", "HYPRLAND_INSTANCE_SIGNATURE",
        "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP"] if k in os.environ}
    env.update({"HOME": str(root / "home"), "PATH": str(root / "shims") + ":/usr/bin:/bin",
                "TMPDIR": str(root / "spills"), "PI_OFFLINE": "1", "RUST_LOG": "warn"})
    children = []
    logs = []

    def launch(name, argv):
        log = (root / f"{name}.log").open("wb")
        logs.append(log)
        child = subprocess.Popen([str(binary), *argv], env=env, stdout=log, stderr=log)
        children.append(child)
        return child

    def wait_file(name, timeout=1800):
        until = time.monotonic() + timeout
        while not (root / name).exists():
            assert time.monotonic() < until, f"timed out waiting for {name}"
            assert server.poll() is None, "server stopped"
            time.sleep(0.2)

    try:
        endpoint = root / "endpoint"
        server = launch("server", ["--engine-server", str(endpoint), "--engine-data-dir", str(root / "engine"),
                                   "--engine-config-dir", str(root / "config")])
        deadline = time.monotonic() + 30
        while not (endpoint / "engine.sock").exists():
            assert server.poll() is None and time.monotonic() < deadline
            time.sleep(0.05)
        client = Client(endpoint)
        app = launch("app", ["--engine", "native", "--engine-attach", str(endpoint),
                             "--data-dir", str(root / "profile"), "--project", str(root / "project")])
        identity = {"appPid": app.pid, "enginePid": server.pid, "workspace": workspace,
                    "binary": str(binary), "binarySha256": hashlib.file_digest(binary.open("rb"), "sha256").hexdigest(),
                    "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                    "turns": args.turns, "scope": "Actual desktop and external native server; local scripted provider; no restart during workload"}
        save(root / "identity.json", identity)
        print(json.dumps(identity), flush=True)
        wait_file("go")
        sessions = client.call("listSessions")["sessions"]
        assert len(sessions) == 1, "Create and open exactly one conversation in the test desktop"
        ids = [sessions[0]["id"]]
        for i in range(3):
            ids.append(client.call("createSession", cwd=str(root / "project"), request=f"background-{i}")["summary"]["id"])
        for session in ids:
            client.call("setModel", session=session, model="desktop-soak/soak")
        samples = []
        start = time.monotonic()

        def sample(turn):
            row = {"turn": turn, "elapsed": time.monotonic() - start,
                   "desktop": metrics(app.pid), "engine": metrics(server.pid)}
            windows = json.loads(subprocess.check_output(["hyprctl", "clients", "-j"]))
            window = next(w for w in windows if w["pid"] == app.pid and w["class"] == "pipkin")
            active = json.loads(subprocess.check_output(["hyprctl", "activewindow", "-j"]))
            row["window"] = {"workspace": window["workspace"]["id"],
                             "mapped": window["mapped"], "visible": window.get("visible"),
                             "active": active.get("pid") == app.pid,
                             "size": window["size"], "xwayland": window["xwayland"]}
            samples.append(row)
            with (root / "samples.jsonl").open("a") as stream:
                stream.write(json.dumps(row) + "\n")
            print(json.dumps(row), flush=True)

        sample(0)
        for turn in range(args.turns):
            assert app.poll() is None and server.poll() is None
            session = ids[turn % len(ids)]
            key = f"desktop-soak-turn-{turn}"
            client.call("prompt", session=session, request=key, text=key)
            result = client.call("waitForPrompt", session=session, request=key)
            assert result["status"] == "done", result
            assert result["text"] == reply_text(turn), f"reply mismatch at {turn}"
            time.sleep(0.05)
            if (turn + 1) % 25 == 0:
                sample(turn + 1)
        time.sleep(2)
        sample(args.turns)
        expected_tools = list(range(0, args.turns, 5))
        effects = list(map(int, (root / "project/effects.log").read_text().splitlines()))
        assert effects == expected_tools, "tool effects missing, duplicated or out of order"
        assert len(requests) == args.turns + len(expected_tools)
        assert not (root / "forbidden-invocations").exists()
        with sqlite3.connect(f"file:{root}/engine/engine.sqlite?mode=ro", uri=True) as db:
            assert db.execute("SELECT count(*) FROM runs WHERE state != 'settled'").fetchone()[0] == 0
            assert db.execute("SELECT count(*) FROM entry_chunks").fetchone()[0] == 0
            outputs = []
            for key, output, retained, dropped in db.execute(
                "SELECT r.key,o.text,o.retained,o.dropped FROM requests r JOIN tool_calls t "
                "ON t.session=r.session AND t.run=r.run JOIN tool_outputs o "
                "ON o.session=t.session AND o.call_id=t.call_id"
            ):
                turn = int(key.removeprefix("desktop-soak-turn-"))
                expected = "x" * 262144 + f"\nfinished-{turn}\n"
                assert output == expected and retained == len(expected) and dropped == 0
                outputs.append(turn)
            assert sorted(outputs) == expected_tools
            durable = {"verifiedOutputs": len(outputs),
                       "outboxRows": db.execute("SELECT count(*) FROM event_outbox").fetchone()[0]}
        save(root / "requests.json", requests)
        save(root / "workload-complete.json", {"sessions": ids, "turns": args.turns,
            "effects": len(effects), "requests": len(requests), "duration": time.monotonic() - start,
            "samples": samples, "durable": durable, "noNodePiInvocations": True})
        print("Workload complete; inspect the desktop, quit it through guard.sh, then create ROOT/finish", flush=True)
        wait_file("finish")
        assert app.poll() is not None, "quit the desktop before finishing"
        server.terminate()
        assert server.wait(timeout=20) == 0
        save(root / "closed.json", {"desktopExit": app.returncode, "engineExit": server.returncode})
        client.socket.close()
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
        provider.shutdown()
        for log in logs:
            log.close()


if __name__ == "__main__":
    main()
