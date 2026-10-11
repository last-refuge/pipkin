#!/usr/bin/env python3
"""Copy driver outputs (ref-run/out/<scenario>) into Pipkin's fixtures/pi-sessions/<scenario>/
and render scenario.md (how it was produced, stub replies, expected transcript, next request)."""
import json, os, re, shutil, sys

HERE = os.path.dirname(os.path.abspath(__file__))
REF = os.path.dirname(HERE)
OUT = os.path.join(REF, "out")
DEST = sys.argv[1]
NODE_RE = re.compile(r"/home/[^ \"]*/bin/node")

COMMON = """\
## How it was produced

* Reference engine: Pi monorepo at rev `d2a311097cbcf669e699479587332ae3988a49d0` plus Pipkin overlays (the
  "staged" tree, local commit `747e8da staged`), copied to a scratch dir, `npm ci` there, run from source
  (`node --import packages/coding-agent/src/experimental/source-resolver.ts packages/coding-agent/src/experimental/cli.ts server --provider stub --model scripted`).
  Node v26.8.1 (mise), npm 11.19.0.
* Environment of the server (nothing else inherited): `PI_EXPERIMENTAL=1 PI_OFFLINE=1 PI_SERVER_DIR=/tmp/claude-1000/pkr/<scenario>
  PI_SERVER_ID=5f0c7b1e-2d4a-4f6b-9a3e-1c8d7e6f5a40 PI_CODING_AGENT_DIR=<scratch>/runs/<scenario>/agent HOME=<scratch>/runs/<scenario>/home
  LANG=C.UTF-8 TZ=UTC GIT_CONFIG_NOSYSTEM=1 PATH=<node bin>:/usr/local/bin:/usr/bin:/bin`.
* `<agentDir>/models.json`: one custom provider `stub` (`api: openai-completions`, `apiKey: "stub"`,
  `baseUrl: http://127.0.0.1:<ephemeral port>/v1`) with models `scripted` (input text+image, no reasoning) and
  `scripted-think` (`reasoning: true`, input text). No real credentials, no network providers.
* Driver: `../_driver/scenarios.ts <scenario>` (`../_driver/run.sh`), a Node script using the reference's own client
  runtime (`openClientRuntime` from `coding-agent/src/experimental/client-runtime.ts`) over the Unix socket. It opens
  the server services `SessionDirectory, SessionManagement, PresentationPlugins` and the session services
  `Models, AgentController, Transcript, Subagents, History`; creates the Session with `cwd` = a fresh git project
  (`git init`, one commit "initial" by "Pipkin Fixture"), `prepareSession({packagePaths:null})`, `attach`.
* Stub provider (`StubProvider` in `_driver/lib.ts`): OpenAI chat-completions SSE. Each reply streams
  `{role:assistant,content:""}`, optional `reasoning_content`, then either the text, or optional lead text + one
  `tool_calls[0]` (`id: call_<request index>`), then a finish chunk (`stop` / `tool_calls`) carrying
  `usage {prompt_tokens:10, completion_tokens:5, total_tokens:15}`, then `data: [DONE]`.
  Every received request (headers + JSON body, index `n` from 0) was appended to `requests.jsonl`.
* Afterwards the driver dumped the `Transcript` replicated value (`view.json`), the whole `History`
  (`history.json`, oldest first), then disposed the client and stopped the server with SIGTERM (exit code 0, no
  profile process left, no `-wal`/`-shm` left), and copied `meta.json` + `session.sqlite`.

## Files

* `meta.json`, `session.sqlite`: the Session directory exactly as the reference left it
  (`<agentDir>/experimental/sessions/<id>/`). The database is in WAL journal mode (persisted in its header):
  copy it before opening it writable, or open it with `?immutable=1` / read-only, otherwise SQLite creates
  `-wal`/`-shm` next to it. `PRAGMA integrity_check` = ok.
* `requests.jsonl`: every provider request the reference made, in order: `{n, method, path, headers, body}`.
  These are the "next provider request" goldens. The only credential is the fake `authorization: Bearer stub`;
  `host` carries the ephemeral stub port, `user-agent`/`x-stainless-*` carry this machine's OS/arch/Node version.
* `view.json`: the root conversation's `ConversationView` (active entries + `pi.agent`, `pi.live`, `pi.inbox`,
  `pi.provider`, `pi.usage` docs) at the end. `history.json`: every root entry, oldest first.
* `models.json`: the `Models` service state at the end.

Paths: the project cwd recorded in `meta.json`, the `pi.agent` doc, the system prompt (`<cwd>`, `<docs>`) and tool
output is the scratch path `/tmp/claude-1000/-home-jason-coding-pipkin/a5fc1b2a-1fd2-4186-b7b8-f58155abe540/scratchpad/ref-run/...`.
It was left as is (rewriting it would change the sqlite records); it is not a home directory and holds no secrets.
"""

SCEN = {
    "a-text-multiturn": dict(
        title="(a) Plain text replies, multi-turn",
        steps="""\
1. `prompt("Hello, who are you?")`, `waitForPrompt`, wait until idle (no `pi.live.run`, no compactions).
2. Same for `"Tell me a second thing."` and `"And a third, please."`.""",
        stub="""\
Every request: text `Answer <n+1>: plain text from the scripted model.`""",
        extra=[],
    ),
    "b-tools-diff": dict(
        title="(b) Tool use: write, read, edit, bash, with a git diff",
        steps="""\
Project starts with a committed `README.md` = `# Fixture project\\n\\nA small project for reference fixtures.\\n`.
1. `prompt("Create notes.txt, look at the README, edit its title and show me the diff.")`, wait for it.
The run makes five model turns (four tool rounds).""",
        stub="""\
In request order (not content-dependent):
0. lead `I'll create the notes file. ` + `write {path:"notes.txt", content:"alpha\\nbeta\\ngamma\\n"}`
1. `read {path:"README.md"}`
2. lead `Now editing the README. ` + `edit {path:"README.md", edits:[{oldText:"Fixture project", newText:"Fixture project (edited)"}]}`
3. `bash {command:"git status --short && git diff"}`
4. text `Done: wrote notes.txt, edited README.md, and showed the diff.`""",
        extra=["project-diff.txt: `git diff` of the project after the run (README.md title edited; notes.txt untracked)."],
    ),
    "c-stop-then-prompt": dict(
        title="(c) Stop during a held model reply, then a new prompt",
        steps="""\
1. `prompt("Take your time on this one.")`; the stub holds request 0 open.
2. Once the request arrived and `pi.live.run` is present (+200 ms): `AgentController.abort()`.
3. `waitForPrompt` settles `{"status":"unanswered","reason":"aborted"}`; wait until idle.
4. `prompt("Next question after stopping.")`, wait for it.""",
        stub="""\
0. held until the client disconnects (the engine aborts the HTTP request; nothing is streamed).
1. text `Fresh answer after the stop.`""",
        extra=[],
        expect="""\
The stopped input is recorded as a `pi.cancelled-input` entry and is **not** sent to the model again: request 1
contains only the new user message (see `requests.jsonl`).""",
    ),
    "d-steer-followup": dict(
        title="(d) A steer and a follow-up while a run is in flight",
        steps="""\
1. `prompt("Edit the notes.")`; the stub holds request 0.
2. While held: `steer("Use tabs.")` -> entryId 11, then `followUp("Afterwards, summarize what changed.")` -> entryId 12.
   `inbox-while-running.json` is the `pi.inbox` doc at that moment (both items queued, steer first).
3. Release request 0 (a `write` tool call). After the tool result the same run's next turn carries the steer;
   then the follow-up runs as its own turn.""",
        stub="""\
0. (held) lead `Working. ` + `write {path:"notes.txt", content:"one\\ntwo\\n"}`
1. text `Done, and I used tabs as you asked.`
2. text `Follow-up answered (turn 3).`""",
        extra=["inbox-while-running.json: `pi.inbox` while the first turn was held."],
    ),
    "e-compaction": dict(
        title="(e) Manual compaction after several turns, then a prompt",
        steps="""\
`<agentDir>/settings.json` = `{"compaction":{"keepRecentTokens":60}}` (so a short conversation has something to
cut; defaults otherwise).
1. Four prompts `Topic <i>: ` + `"some words about this topic " x 8`, each waited.
2. `AgentController.compact({customInstructions:null})` -> accepted, operationId 24; wait until the summary request
   arrived and the conversation is idle again. Views dumped as `view-after-compaction.json` /
   `history-after-compaction.json`.
3. `prompt("What were we talking about?")`, wait.""",
        stub="""\
* A request whose messages include a system message containing "context summarization assistant" (the
  compaction request, n=4) gets text
  `## Goal\\nScripted summary of the earlier turns.\\n\\n## Progress\\n- Turns 1-3 discussed topics one to three.\\n\\n## Next Steps\\n- Continue.`
* Any other request: text `Answer <n+1> about <first line of the last user message>.`""",
        extra=[
            "view-after-compaction.json, history-after-compaction.json, models-after-compaction.json: state right after the compaction, before the last prompt.",
        ],
        expect="""\
Request 4 is the summarization request (`max_completion_tokens` 13107, its own system prompt, the serialized
conversation in one user message). Request 5 is the post-compaction context: the compaction summary user message,
then the kept tail (from `Answer 4 ...`), then the new prompt. The live `view.json` starts at the `pi.compaction`
entry (head 23); `history.json` still has everything.""",
    ),
    "f-subagent": dict(
        title="(f) A subagent call that completes (child conversation)",
        steps="""\
1. `Subagents.setEnabled(true)` (the `subagent` tool was already offered by default; the call is idempotent).
2. `prompt("Ask a subagent to count the files.")`, wait.""",
        stub="""\
* A request that does not offer the `subagent` tool is the child conversation: text
  `Child answer: the subagent counted three files.`
* A root request whose last message is a tool result: text `The subagent reports three files. Done.`
* Otherwise (root, first turn): lead `Delegating. ` + `subagent {task:"Count the files in the project and report the number."}`""",
        extra=[
            "subagents.json: `Subagents.list()` (child conversation 13, task 12, status done).",
            "subagent-13-view.json: the child's `ConversationView` (`conversation.owner = {conversationId:1, taskId:12}`).",
        ],
        expect="""\
Requests: 0 = root first turn; 1 = child conversation (its own system prompt, tools without `subagent`, the task
as the only user message); 2 = root turn with the subagent tool result.""",
    ),
    "g-model-thinking": dict(
        title="(g) Model and thinking-level changes between prompts",
        steps="""\
1. `prompt("First, on the default model.")` on `stub/scripted` (server default `--provider stub --model scripted`).
2. `Models.select({provider:"stub", modelId:"scripted-think"})` -> configuration thinkingLevel `off`;
   `getThinkingLevels()` = off, minimal, low, medium, high.
3. `prompt("Second, on the reasoning model.")`.
4. `Models.selectThinking("high")`.
5. `prompt("Third, with high thinking.")`.""",
        stub="""\
* Requests for model `scripted-think`: `reasoning_content` `Thinking about request <n+1>.` then text
  `Answer <n+1> from scripted-think (reasoning_effort=<body.reasoning_effort or none>).`
* Otherwise: text `Answer <n+1> from <model>.`""",
        extra=[],
        expect="""\
Request 0: model `scripted`, `system` role. Request 1: model `scripted-think`, the system prompt is sent with role
`developer`, no `reasoning_effort` (level off). Request 2: `reasoning_effort: "high"`, and the previous assistant
turn is replayed with `reasoning_content`. Model/thinking changes are recorded in the `pi.agent` document, not as
transcript entries.""",
    ),
    "h-worker-killed-mid-tool": dict(
        title="(h) Session worker killed mid-tool (bash sleep), then reopened",
        steps="""\
1. `prompt("Run the slow command.")` (operationId 8).
2. Wait until `pi.live.tools[]` has a `running` slot, +500 ms; `view-before-kill.json` dumped.
3. SIGKILL the session worker (the profile process whose initial environ has `__PI_INTERNAL_SPAWN=session-worker`).
   The tool's `bash -c "sleep 15; echo finished"` and `sleep 15` survived their worker; the driver SIGKILLed them too.
4. Dispose the client, connect a new client and `attach` the same Session id. (The server had already started a
   replacement worker right after the kill.)
5. The new worker resumes the interrupted run by itself: the bash call is closed as interrupted and the model is
   asked again; `waitForPrompt(8)` settles done.""",
        stub="""\
* Last message is a tool result: text `Recovered after the interruption (turn <n+1>).`
* Otherwise: lead `Running the slow command. ` + `bash {command:"sleep 15; echo finished"}`""",
        extra=["view-before-kill.json: the view while the bash tool was running (live tool slot `running`)."],
        expect="""\
The bash tool result is synthesized by the harness: entry `pi.tool-result` with
`data.diagnostics = [{severity:"error", code:"interrupted", message:"Tool bash was interrupted and may have partially run"}]`
and model content `<harness>\\n[error] Tool bash was interrupted and may have partially run\\n</harness>`. Request 1 is
the recovery turn with that result. The prompt is not re-sent.""",
    ),
}


def text_of(content):
    if isinstance(content, str):
        return content
    out = []
    for p in content or []:
        t = p.get("type")
        if t == "text":
            out.append(p["text"])
        elif t == "thinking":
            out.append(f"[thinking: {p.get('thinking')}]")
        elif t == "toolCall":
            out.append(f"[toolCall {p.get('id')} {p.get('name')} {json.dumps(p.get('arguments'))}]")
        elif t == "image":
            out.append("[image]")
        else:
            out.append(f"[{t}]")
    return "".join(out)


def clip(s, n=240):
    s = s.replace("\n", "\\n")
    return s if len(s) <= n else s[:n] + f"... ({len(s)} chars)"


def render_entries(entries):
    rows = ["| id | kind | content |", "|---|---|---|"]
    for e in entries:
        parts = []
        for m in e.get("model", []) or []:
            role = m.get("role")
            if role == "system":
                parts.append("system prompt marker")
                continue
            parts.append(f"{role}: {clip(text_of(m.get('content')))}")
        if "data" in e and e["data"] not in (None, {"diagnostics": []}):
            parts.append(f"data={clip(json.dumps(e['data']), 200)}")
        if "head" in e:
            parts.append(f"head={e['head']}")
        rows.append(f"| {e['id']} | `{e['kind']}` | {' / '.join(parts).replace('|', '\\|')} |")
    return "\n".join(rows)


def render_request(body):
    rows = []
    other = {k: v for k, v in body.items() if k not in ("messages", "tools")}
    rows.append(f"Parameters: `{json.dumps(other)}`; tools: {', '.join(t['function']['name'] for t in body.get('tools', []))}")
    rows.append("")
    for i, m in enumerate(body.get("messages", [])):
        c = text_of(m.get("content"))
        if m["role"] in ("system", "developer"):
            c = clip(c, 80)
        else:
            c = clip(c)
        extra = ""
        if m.get("tool_calls"):
            extra += " tool_calls=" + ", ".join(f"{t['id']} {t['function']['name']}({t['function']['arguments']})" for t in m["tool_calls"])
        if m.get("tool_call_id"):
            extra += f" (tool_call_id {m['tool_call_id']})"
        if m.get("reasoning_content"):
            extra += f" reasoning_content={json.dumps(m['reasoning_content'])}"
        rows.append(f"{i}. **{m['role']}**: {c}{extra}")
    return "\n".join(rows)


def main():
    for name, d in SCEN.items():
        src = os.path.join(OUT, name)
        dst = os.path.join(DEST, name)
        if os.path.exists(dst):
            shutil.rmtree(dst)
        os.makedirs(dst)
        for f in ("meta.json", "session.sqlite"):
            shutil.copy2(os.path.join(src, "session", f), os.path.join(dst, f))
        for f in sorted(os.listdir(src)):
            if f in ("session", "notes.txt", "run.json"):
                continue
            shutil.copy2(os.path.join(src, f), os.path.join(dst, f))
        run = json.load(open(os.path.join(src, "run.json")))
        meta = json.load(open(os.path.join(src, "session", "meta.json")))
        notes = NODE_RE.sub("<node>", open(os.path.join(src, "notes.txt")).read())
        reqs = [json.loads(l) for l in open(os.path.join(src, "requests.jsonl"))]
        view = json.load(open(os.path.join(src, "view.json")))
        history = json.load(open(os.path.join(src, "history.json")))
        md = [f"# {d['title']}", "", f"Session id `{run['sessionId']}` (from `meta.json`; created {meta['createdAt']} ms epoch).", "",
              "## Steps (driver)", "", d["steps"], "", "## Stub provider replies", "", d["stub"], ""]
        if d.get("expect"):
            md += ["## What this fixture pins", "", d["expect"], ""]
        md += ["## Expected visible transcript (active view, `view.json`)", "", render_entries(view["entries"]), ""]
        if len(history) != len(view["entries"]):
            md += ["## Whole history (`history.json`)", "", render_entries(history), ""]
        md += [f"## Provider requests ({len(reqs)} in `requests.jsonl`)", ""]
        for r in reqs:
            md += [f"### Request {r['n']} `{r['method']} {r['path']}`", "", render_request(r["body"] or {}), ""]
        md += ["The full system prompt and tool schemas are in `requests.jsonl`.", ""]
        if d["extra"]:
            md += ["## Extra files", ""] + [f"* {x}" for x in d["extra"]] + [""]
        md += ["## Run log (driver notes)", "", "```", notes.strip(), "```", "", COMMON]
        open(os.path.join(dst, "scenario.md"), "w").write("\n".join(md))
        print("wrote", dst)


main()
