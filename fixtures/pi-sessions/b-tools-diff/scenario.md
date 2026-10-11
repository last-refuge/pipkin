# (b) Tool use: write, read, edit, bash, with a git diff

Session id `4f79d20d-06f6-48da-8cac-d99092468026` (from `meta.json`; created 1791750714292 ms epoch).

## Steps (driver)

Project starts with a committed `README.md` = `# Fixture project\n\nA small project for reference fixtures.\n`.
1. `prompt("Create notes.txt, look at the README, edit its title and show me the diff.")`, wait for it.
The run makes five model turns (four tool rounds).

## Stub provider replies

In request order (not content-dependent):
0. lead `I'll create the notes file. ` + `write {path:"notes.txt", content:"alpha\nbeta\ngamma\n"}`
1. `read {path:"README.md"}`
2. lead `Now editing the README. ` + `edit {path:"README.md", edits:[{oldText:"Fixture project", newText:"Fixture project (edited)"}]}`
3. `bash {command:"git status --short && git diff"}`
4. text `Done: wrote notes.txt, edited README.md, and showed the diff.`

## Expected visible transcript (active view, `view.json`)

| id | kind | content |
|---|---|---|
| 7 | `pi.user` | user: Create notes.txt, look at the README, edit its title and show me the diff. |
| 10 | `pi.system` | system prompt marker |
| 11 | `pi.assistant` | assistant: I'll create the notes file. [toolCall call_0 write {"path": "notes.txt", "content": "alpha\nbeta\ngamma\n"}] |
| 13 | `pi.tool-result` | toolResult: Successfully wrote to notes.txt |
| 15 | `pi.assistant` | assistant: [toolCall call_1 read {"path": "README.md"}] |
| 17 | `pi.tool-result` | toolResult: # Fixture project\n\nA small project for reference fixtures.\n |
| 19 | `pi.assistant` | assistant: Now editing the README. [toolCall call_2 edit {"path": "README.md", "edits": [{"oldText": "Fixture project", "newText": "Fixture project (edited)"}]}] |
| 21 | `pi.tool-result` | toolResult: Successfully replaced 1 block(s) in README.md. |
| 23 | `pi.assistant` | assistant: [toolCall call_3 bash {"command": "git status --short && git diff"}] |
| 25 | `pi.tool-result` | toolResult:  M README.md\n?? notes.txt\ndiff --git a/README.md b/README.md\nindex bf52ea1..d285ed9 100644\n--- a/README.md\n+++ b/README.md\n@@ -1,3 +1,3 @@\n-# Fixture project\n+# Fixture project (edited)\n \n A small project for reference fixtures.\n |
| 27 | `pi.assistant` | assistant: Done: wrote notes.txt, edited README.md, and showed the diff. |

## Provider requests (5 in `requests.jsonl`)

### Request 0 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3040 chars)
1. **user**: Create notes.txt, look at the README, edit its title and show me the diff.

### Request 1 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3040 chars)
1. **user**: Create notes.txt, look at the README, edit its title and show me the diff.
2. **assistant**: I'll create the notes file.  tool_calls=call_0 write({"path":"notes.txt","content":"alpha\nbeta\ngamma\n"})
3. **tool**: Successfully wrote to notes.txt (tool_call_id call_0)

### Request 2 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3040 chars)
1. **user**: Create notes.txt, look at the README, edit its title and show me the diff.
2. **assistant**: I'll create the notes file.  tool_calls=call_0 write({"path":"notes.txt","content":"alpha\nbeta\ngamma\n"})
3. **tool**: Successfully wrote to notes.txt (tool_call_id call_0)
4. **assistant**:  tool_calls=call_1 read({"path":"README.md"})
5. **tool**: # Fixture project\n\nA small project for reference fixtures.\n (tool_call_id call_1)

### Request 3 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3040 chars)
1. **user**: Create notes.txt, look at the README, edit its title and show me the diff.
2. **assistant**: I'll create the notes file.  tool_calls=call_0 write({"path":"notes.txt","content":"alpha\nbeta\ngamma\n"})
3. **tool**: Successfully wrote to notes.txt (tool_call_id call_0)
4. **assistant**:  tool_calls=call_1 read({"path":"README.md"})
5. **tool**: # Fixture project\n\nA small project for reference fixtures.\n (tool_call_id call_1)
6. **assistant**: Now editing the README.  tool_calls=call_2 edit({"path":"README.md","edits":[{"oldText":"Fixture project","newText":"Fixture project (edited)"}]})
7. **tool**: Successfully replaced 1 block(s) in README.md. (tool_call_id call_2)

### Request 4 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3040 chars)
1. **user**: Create notes.txt, look at the README, edit its title and show me the diff.
2. **assistant**: I'll create the notes file.  tool_calls=call_0 write({"path":"notes.txt","content":"alpha\nbeta\ngamma\n"})
3. **tool**: Successfully wrote to notes.txt (tool_call_id call_0)
4. **assistant**:  tool_calls=call_1 read({"path":"README.md"})
5. **tool**: # Fixture project\n\nA small project for reference fixtures.\n (tool_call_id call_1)
6. **assistant**: Now editing the README.  tool_calls=call_2 edit({"path":"README.md","edits":[{"oldText":"Fixture project","newText":"Fixture project (edited)"}]})
7. **tool**: Successfully replaced 1 block(s) in README.md. (tool_call_id call_2)
8. **assistant**:  tool_calls=call_3 bash({"command":"git status --short && git diff"})
9. **tool**:  M README.md\n?? notes.txt\ndiff --git a/README.md b/README.md\nindex bf52ea1..d285ed9 100644\n--- a/README.md\n+++ b/README.md\n@@ -1,3 +1,3 @@\n-# Fixture project\n+# Fixture project (edited)\n \n A small project for reference fixtures.\n (tool_call_id call_3)

The full system prompt and tool schemas are in `requests.jsonl`.

## Extra files

* project-diff.txt: `git diff` of the project after the run (README.md title edited; notes.txt untracked).

## Run log (driver notes)

```
session 4f79d20d-06f6-48da-8cac-d99092468026; server ready 689 ms, attached 1170 ms after spawn
prompt -> {"status":"done","text":"Done: wrote notes.txt, edited README.md, and showed the diff.","reason":null}
git status after run:
 M README.md
?? notes.txt

server exit code 0; leftover profile processes after stop: []
session dir after stop: meta.json 168, session.sqlite 131072
```

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
