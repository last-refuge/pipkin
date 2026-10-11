# (e) Manual compaction after several turns, then a prompt

Session id `15107c4b-0ff1-4ce3-b3ac-21293b80525e` (from `meta.json`; created 1791750731011 ms epoch).

## Steps (driver)

`<agentDir>/settings.json` = `{"compaction":{"keepRecentTokens":60}}` (so a short conversation has something to
cut; defaults otherwise).
1. Four prompts `Topic <i>: ` + `"some words about this topic " x 8`, each waited.
2. `AgentController.compact({customInstructions:null})` -> accepted, operationId 24; wait until the summary request
   arrived and the conversation is idle again. Views dumped as `view-after-compaction.json` /
   `history-after-compaction.json`.
3. `prompt("What were we talking about?")`, wait.

## Stub provider replies

* A request whose messages include a system message containing "context summarization assistant" (the
  compaction request, n=4) gets text
  `## Goal\nScripted summary of the earlier turns.\n\n## Progress\n- Turns 1-3 discussed topics one to three.\n\n## Next Steps\n- Continue.`
* Any other request: text `Answer <n+1> about <first line of the last user message>.`

## What this fixture pins

Request 4 is the summarization request (`max_completion_tokens` 13107, its own system prompt, the serialized
conversation in one user message). Request 5 is the post-compaction context: the compaction summary user message,
then the kept tail (from `Answer 4 ...`), then the new prompt. The live `view.json` starts at the `pi.compaction`
entry (head 23); `history.json` still has everything.

## Expected visible transcript (active view, `view.json`)

| id | kind | content |
|---|---|---|
| 25 | `pi.compaction` | user: The conversation history before this point was compacted into the following summary:\n\n<summary>\n## Goal\nScripted summary of the earlier turns.\n\n## Progress\n- Turns 1-3 discussed topics one to three.\n\n## Next Steps\n- Continue.\n</s... (247 chars) / data={"reason": "manual"} / head=23 |
| 23 | `pi.assistant` | assistant: Answer 4 about Topic 4: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars) |
| 27 | `pi.user` | user: What were we talking about? |
| 30 | `pi.system` | system prompt marker |
| 31 | `pi.assistant` | assistant: Answer 6 about What were we talking about?. |

## Whole history (`history.json`)

| id | kind | content |
|---|---|---|
| 7 | `pi.user` | user: Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic  |
| 10 | `pi.system` | system prompt marker |
| 11 | `pi.assistant` | assistant: Answer 1 about Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars) |
| 12 | `pi.user` | user: Topic 2: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic  |
| 15 | `pi.assistant` | assistant: Answer 2 about Topic 2: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars) |
| 16 | `pi.user` | user: Topic 3: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic  |
| 19 | `pi.assistant` | assistant: Answer 3 about Topic 3: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars) |
| 20 | `pi.user` | user: Topic 4: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic  |
| 23 | `pi.assistant` | assistant: Answer 4 about Topic 4: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars) |
| 25 | `pi.compaction` | user: The conversation history before this point was compacted into the following summary:\n\n<summary>\n## Goal\nScripted summary of the earlier turns.\n\n## Progress\n- Turns 1-3 discussed topics one to three.\n\n## Next Steps\n- Continue.\n</s... (247 chars) / data={"reason": "manual"} / head=23 |
| 27 | `pi.user` | user: What were we talking about? |
| 30 | `pi.system` | system prompt marker |
| 31 | `pi.assistant` | assistant: Answer 6 about What were we talking about?. |

## Provider requests (6 in `requests.jsonl`)

### Request 0 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3040 chars)
1. **user**: Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic 

### Request 1 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3040 chars)
1. **user**: Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic 
2. **assistant**: Answer 1 about Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars)
3. **user**: Topic 2: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic 

### Request 2 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3040 chars)
1. **user**: Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic 
2. **assistant**: Answer 1 about Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars)
3. **user**: Topic 2: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic 
4. **assistant**: Answer 2 about Topic 2: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars)
5. **user**: Topic 3: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic 

### Request 3 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3040 chars)
1. **user**: Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic 
2. **assistant**: Answer 1 about Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars)
3. **user**: Topic 2: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic 
4. **assistant**: Answer 2 about Topic 2: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars)
5. **user**: Topic 3: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic 
6. **assistant**: Answer 3 about Topic 3: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars)
7. **user**: Topic 4: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic 

### Request 4 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 13107}`; tools: 

0. **system**: You are a context summarization assistant. Your task is to read a conversation b... (312 chars)
1. **user**: <conversation>\n[User]: Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words ... (2835 chars)

### Request 5 `POST /v1/chat/completions`

Parameters: `{"model": "scripted", "stream": true, "stream_options": {"include_usage": true}, "store": false, "max_completion_tokens": 16384}`; tools: read, write, edit, bash, subagent

0. **system**: You are an expert coding assistant operating inside pi, a coding agent harness. ... (3040 chars)
1. **user**: The conversation history before this point was compacted into the following summary:\n\n<summary>\n## Goal\nScripted summary of the earlier turns.\n\n## Progress\n- Turns 1-3 discussed topics one to three.\n\n## Next Steps\n- Continue.\n</s... (247 chars)
2. **assistant**: Answer 4 about Topic 4: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about thi... (249 chars)
3. **user**: What were we talking about?

The full system prompt and tool schemas are in `requests.jsonl`.

## Extra files

* view-after-compaction.json, history-after-compaction.json, models-after-compaction.json: state right after the compaction, before the last prompt.

## Run log (driver notes)

```
session 15107c4b-0ff1-4ce3-b3ac-21293b80525e; server ready 669 ms, attached 1171 ms after spawn
prompt 1 -> {"status":"done","text":"Answer 1 about Topic 1: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic .","reason":null}
prompt 2 -> {"status":"done","text":"Answer 2 about Topic 2: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic .","reason":null}
prompt 3 -> {"status":"done","text":"Answer 3 about Topic 3: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic .","reason":null}
prompt 4 -> {"status":"done","text":"Answer 4 about Topic 4: some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic some words about this topic .","reason":null}
compact -> {"accepted":true,"operationId":"24","error":null}
post-compaction prompt -> {"status":"done","text":"Answer 6 about What were we talking about?.","reason":null}
server exit code 0; leftover profile processes after stop: []
session dir after stop: meta.json 168, session.sqlite 139264
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
