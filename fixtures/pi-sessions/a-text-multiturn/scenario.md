# (a) Plain text replies, multi-turn

Session id `6afe7d07-f735-4357-8767-42fe2da63850` (from `meta.json`; created 1791750696257 ms epoch).

## Steps (driver)

1. `prompt("Hello, who are you?")`, `waitForPrompt`, wait until idle (no `pi.live.run`, no compactions).
2. Same for `"Tell me a second thing."` and `"And a third, please."`.

## Stub provider replies

Every request: text `Answer <n+1>: plain text from the scripted model.`

## Expected visible transcript (active view, `view.json`)

| id | kind | content |
|---|---|---|
| 7 | `pi.user` | user: Hello, who are you? |
| 10 | `pi.system` | system prompt marker |
| 11 | `pi.assistant` | assistant: Answer 1: plain text from the scripted model. |
| 12 | `pi.user` | user: Tell me a second thing. |
| 15 | `pi.assistant` | assistant: Answer 2: plain text from the scripted model. |
| 16 | `pi.user` | user: And a third, please. |
| 19 | `pi.assistant` | assistant: Answer 3: plain text from the scripted model. |

## Provider requests (3 in `requests.jsonl`)

### Request 0 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3044 chars)
1. **user**: Hello, who are you?

### Request 1 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3044 chars)
1. **user**: Hello, who are you?
2. **assistant**: Answer 1: plain text from the scripted model.
3. **user**: Tell me a second thing.

### Request 2 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3044 chars)
1. **user**: Hello, who are you?
2. **assistant**: Answer 1: plain text from the scripted model.
3. **user**: Tell me a second thing.
4. **assistant**: Answer 2: plain text from the scripted model.
5. **user**: And a third, please.

The full system prompt and tool schemas are in `requests.jsonl`.

## Run log (driver notes)

```
session 6afe7d07-f735-4357-8767-42fe2da63850; server ready 710 ms, attached 1215 ms after spawn
prompt "Hello, who are you?" -> {"status":"done","text":"Answer 1: plain text from the scripted model.","reason":null}
prompt "Tell me a second thing." -> {"status":"done","text":"Answer 2: plain text from the scripted model.","reason":null}
prompt "And a third, please." -> {"status":"done","text":"Answer 3: plain text from the scripted model.","reason":null}
server exit code 0; leftover profile processes after stop: []
session dir after stop: meta.json 172, session.sqlite 122880
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
