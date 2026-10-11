# (d) A steer and a follow-up while a run is in flight

Session id `1282425c-ecb1-4bc4-a8a4-10734908e632` (from `meta.json`; created 1791750718568 ms epoch).

## Steps (driver)

1. `prompt("Edit the notes.")`; the stub holds request 0.
2. While held: `steer("Use tabs.")` -> entryId 11, then `followUp("Afterwards, summarize what changed.")` -> entryId 12.
   `inbox-while-running.json` is the `pi.inbox` doc at that moment (both items queued, steer first).
3. Release request 0 (a `write` tool call). After the tool result the same run's next turn carries the steer;
   then the follow-up runs as its own turn.

## Stub provider replies

0. (held) lead `Working. ` + `write {path:"notes.txt", content:"one\ntwo\n"}`
1. text `Done, and I used tabs as you asked.`
2. text `Follow-up answered (turn 3).`

## Expected visible transcript (active view, `view.json`)

| id | kind | content |
|---|---|---|
| 7 | `pi.user` | user: Edit the notes. |
| 10 | `pi.system` | system prompt marker |
| 13 | `pi.assistant` | assistant: Working. [toolCall call_0 write {"path": "notes.txt", "content": "one\ntwo\n"}] |
| 15 | `pi.tool-result` | toolResult: Successfully wrote to notes.txt |
| 16 | `pi.user` | user: Use tabs. |
| 18 | `pi.assistant` | assistant: Done, and I used tabs as you asked. |
| 19 | `pi.user` | user: Afterwards, summarize what changed. |
| 21 | `pi.assistant` | assistant: Follow-up answered (turn 3). |

## Provider requests (3 in `requests.jsonl`)

### Request 0 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3044 chars)
1. **user**: Edit the notes.

### Request 1 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3044 chars)
1. **user**: Edit the notes.
2. **assistant**: Working.  tool_calls=call_0 write({"path":"notes.txt","content":"one\ntwo\n"})
3. **tool**: Successfully wrote to notes.txt (tool_call_id call_0)
4. **user**: Use tabs.

### Request 2 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3044 chars)
1. **user**: Edit the notes.
2. **assistant**: Working.  tool_calls=call_0 write({"path":"notes.txt","content":"one\ntwo\n"})
3. **tool**: Successfully wrote to notes.txt (tool_call_id call_0)
4. **user**: Use tabs.
5. **assistant**: Done, and I used tabs as you asked.
6. **user**: Afterwards, summarize what changed.

The full system prompt and tool schemas are in `requests.jsonl`.

## Extra files

* inbox-while-running.json: `pi.inbox` while the first turn was held.

## Run log (driver notes)

```
session 1282425c-ecb1-4bc4-a8a4-10734908e632; server ready 689 ms, attached 1169 ms after spawn
steer -> {"accepted":true,"entryId":"11","error":null}
followUp -> {"accepted":true,"entryId":"12","error":null}
first prompt -> {"status":"done","text":"Done, and I used tabs as you asked.","reason":null}
server exit code 0; leftover profile processes after stop: []
session dir after stop: meta.json 172, session.sqlite 131072
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
