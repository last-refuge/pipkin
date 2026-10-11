# (h) Session worker killed mid-tool (bash sleep), then reopened

Session id `48f6783c-9088-4456-bb0c-59067d5fcd2c` (from `meta.json`; created 1791750743917 ms epoch).

## Steps (driver)

1. `prompt("Run the slow command.")` (operationId 8).
2. Wait until `pi.live.tools[]` has a `running` slot, +500 ms; `view-before-kill.json` dumped.
3. SIGKILL the session worker (the profile process whose initial environ has `__PI_INTERNAL_SPAWN=session-worker`).
   The tool's `bash -c "sleep 15; echo finished"` and `sleep 15` survived their worker; the driver SIGKILLed them too.
4. Dispose the client, connect a new client and `attach` the same Session id. (The server had already started a
   replacement worker right after the kill.)
5. The new worker resumes the interrupted run by itself: the bash call is closed as interrupted and the model is
   asked again; `waitForPrompt(8)` settles done.

## Stub provider replies

* Last message is a tool result: text `Recovered after the interruption (turn <n+1>).`
* Otherwise: lead `Running the slow command. ` + `bash {command:"sleep 15; echo finished"}`

## What this fixture pins

The bash tool result is synthesized by the harness: entry `pi.tool-result` with
`data.diagnostics = [{severity:"error", code:"interrupted", message:"Tool bash was interrupted and may have partially run"}]`
and model content `<harness>\n[error] Tool bash was interrupted and may have partially run\n</harness>`. Request 1 is
the recovery turn with that result. The prompt is not re-sent.

## Expected visible transcript (active view, `view.json`)

| id | kind | content |
|---|---|---|
| 7 | `pi.user` | user: Run the slow command. |
| 10 | `pi.system` | system prompt marker |
| 11 | `pi.assistant` | assistant: Running the slow command. [toolCall call_0 bash {"command": "sleep 15; echo finished"}] |
| 13 | `pi.tool-result` | toolResult: <harness>\n[error] Tool bash was interrupted and may have partially run\n</harness> / data={"diagnostics": [{"severity": "error", "code": "interrupted", "message": "Tool bash was interrupted and may have partially run"}]} |
| 15 | `pi.assistant` | assistant: Recovered after the interruption (turn 2). |

## Provider requests (2 in `requests.jsonl`)

### Request 0 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3052 chars)
1. **user**: Run the slow command.

### Request 1 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3052 chars)
1. **user**: Run the slow command.
2. **assistant**: Running the slow command.  tool_calls=call_0 bash({"command":"sleep 15; echo finished"})
3. **tool**: <harness>\n[error] Tool bash was interrupted and may have partially run\n</harness> (tool_call_id call_0)

The full system prompt and tool schemas are in `requests.jsonl`.

## Extra files

* view-before-kill.json: the view while the bash tool was running (live tool slot `running`).

## Run log (driver notes)

```
session 48f6783c-9088-4456-bb0c-59067d5fcd2c; server ready 690 ms, attached 1168 ms after spawn
prompt operation 8
processes before kill: [{"pid":304129,"role":"launcher/server","cmd":"pi"},{"pid":304141,"role":"coordinator","cmd":"<node> --import file:///tmp/claude-1000/-home-jason-coding-pipkin/a5fc1b2a-1fd2-4186-b7b8-f58155abe540/scra"},{"pid":304156,"role":"session-worker","cmd":"<node> --import file:///tmp/claude-1000/-home-jason-coding-pipkin/a5fc1b2a-1fd2-4186-b7b8-f58155abe540/scra"},{"pid":304208,"role":"launcher/server","cmd":"/bin/bash -c sleep 15; echo finished"},{"pid":304209,"role":"launcher/server","cmd":"sleep 15"}]
SIGKILLed session worker 304156
killed orphaned tool process 304208: /bin/bash -c sleep 15; echo finished
killed orphaned tool process 304209: sleep 15
reattached 48f6783c-9088-4456-bb0c-59067d5fcd2c
original prompt settled: {"status":"done","text":"Recovered after the interruption (turn 2).","reason":null}
processes after reopen: [{"pid":304129,"role":"launcher/server"},{"pid":304141,"role":"coordinator"},{"pid":304210,"role":"session-worker"}]
server exit code 0; leftover profile processes after stop: []
session dir after stop: meta.json 180, session.sqlite 122880
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
