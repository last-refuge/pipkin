## 1. Registered coding tools

### 1.0 Which tool set is live

The experimental server path (session worker, `durable/main.ts` TUI) does **not** use the classic `coding-agent/src/core/tools/*` implementations. It builds a durable `Registry` with exactly two extensions:

- `CodingTools` (`durable/src/tools/index.ts:19-22`, name `"coding-tools"`): `read`, `write`, `edit`, `bash`, in that order.
- `pi-prompt` (system prompt sections only, `coding-agent/src/experimental/durable/prompt.ts:69-73`).
- plus `Subagent` (`coding-agent/src/experimental/durable/subagent.ts:24-55`) installed after (`session-worker.ts:783-784`, `durable/runtime.ts:133-134`).

So the model sees tools in order **read, write, edit, bash, subagent** (registry install order; `harness/agent.ts` resolution keeps install order, later same-name tool wins). There is **no grep/find/ls** in the durable/server path; those exist only in the classic agent (§1.6). There is no `powershell` either. Images cannot be read by the durable `read`.

Common tool-call machinery (`durable/src/harness/tool.ts`):

- Unknown tool name at call time -> error result, diagnostic code `tool_unavailable`, text `Tool <name> is not available` (`tool.ts:64-66`; also at round start for tools the request did not offer, `generation.ts:569-574`).
- `prepareArguments` (pure repair) then TypeBox validation via pi-ai `validateToolArguments` (coerces) (`tool.ts:68-70,202-208`); failure -> code `invalid_arguments`, message = validator error text.
- `beforeTool` hooks: first `block` wins -> code `blocked`, `Tool call blocked: <reason>`; a throwing hook blocks with its error text; `arguments` replacement re-validated (`tool.ts:73-89`).
- Intent commit then execute (`tool.ts:91-111`), see §3.8.
- Output limits per call: `maxBytes` default 50 KiB (51200), `maxLines` default 2000, `retain` default `head` (`tool.ts:230-234`, `truncate.ts:11-12`). Explicit content is bounded with the same limits (`boundContent`, `tool.ts:534-548`): when joined text exceeds limits, all text items are replaced by one bounded item at the first (head) / last (tail) text item position; images kept.
- A throw from `execute()` -> `isError: true`, content = retained streamed output (if `content` absent), diagnostic `{severity:"error", code:"tool_error", message: err.message}`; task ends `failed` (`tool.ts:310-320`).
- Diagnostics are appended to the tool result content as one final text block (`tool.ts:526-528`):
  ```
  <harness>
  [<severity>] <message>
  ...
  </harness>
  ```
  `data.diagnostics` keeps the structured list (`ToolResultEntry`, `entries.ts:24`).
- Harness truncation diagnostic (`tool.ts:486-496`): `[warn] Output truncated to its beginning|end: <N> lines, <M> bytes dropped` (after recovery, retain unknown: `Output truncated: ...`).
- Result message: `{role:"toolResult", toolCallId, toolName, content, details?, usage?, isError, timestamp}` (`tool.ts:512-523`).
- Result `control` (not used by built-ins): `{terminate?: true, addTools?: string[], handoff?: string}` (`harness/types.ts:129-133`).
- Path normalization for all file tools (`durable/src/tools/path-utils.ts`): Unicode spaces (`U+00A0, U+2000-200A, U+202F, U+205F, U+3000`) -> space; a leading `@` is stripped; then `env.absolutePath` = `~`/`~/` expanded to homedir, `file://` URLs decoded, relative resolved against env cwd (`env/node.ts:72-86`). `read` additionally tries variants if the path does not exist: ` AM.`/` PM.` with U+202F, NFD, `'`->U+2019, NFD+U+2019 (`path-utils.ts:16-30`).
- cwd: one `NodeExecutionEnv` per directory, keyed by the conversation's `pi.agent.cwd` (default: worker cwd) (`experimental/durable/harness-setup.ts:60-82`). `env.id = "node:local"`.
- `write`/`edit` serialize per (env.id, canonical path) inside the process (`tools/file-mutation-queue.ts:33-55`); not a lock against bash or other processes. Non-existent files key on canonical parent + name.
- Missing env -> `No execution environment is configured` (`tools/env.ts:5-8`).
- Abort checks in write/edit throw `Operation aborted`.

### 1.1 `read` (`durable/src/tools/read.ts`)

Schema (`read.ts:26-30`):
```json
{"type":"object","required":["path"],"properties":{
 "path":{"type":"string","description":"Path to the file to read (relative or absolute)"},
 "offset":{"type":"number","description":"Line number to start reading from (1-indexed)"},
 "limit":{"type":"number","description":"Maximum number of lines to read"}}}
```
Description (`read.ts:76`): `Read the contents of a text file. Output is truncated to 2000 lines or 50KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.`

Behavior:
- Opens a binary reader; reads/scans; if size or mtime changed between before/after, retries once, then throws `<path> changed while it was read` (`read.ts:84-91`).
- Image sniff on the first 32 bytes (+PNG chunk walk): JPEG (`FF D8 FF`, not `F7`), PNG (non-animated; APNG `acTL` -> not image), GIF87a/89a, RIFF/WEBP, BMP (`tools/image.ts:16-53`). Image -> `isError:true`, no content, diagnostic `{severity:"error", code:"unsupported_image", message:"<path> is an image (<mime>); reading images is not supported"}` (`read.ts:115-128`).
- Line selection is defined as `text.split("\n").slice(start, start+limit)` with JS slice semantics (fractional truncation, negative end) (`read.ts:130-160`). `startLine = offset ? max(0, offset-1) : 0` (offset 0 = start). UTF-8 BOM skipped at byte 0.
- `startLine >= totalLines` -> throws `Offset <offset> is beyond end of file (<N> lines total)` (`read.ts:146-148`). Total lines = newline count + 1.
- Head truncation (`truncate.ts:121-205`): complete lines only, 2000 lines / 51200 bytes, newline bytes counted.
- Diagnostics (content is only file text; remarks go to `<harness>` block):
  - First selected line > 50KB: shows first 50KB of that line on a char boundary; `[warn]` code `truncated`: `Line <n> is <size>, exceeds the 50.0KB limit; showing its first <size>. Use bash: sed -n '<n>p' <path> | tail -c +<end+1>` (`read.ts:175-188`).
  - Truncated: `[info]` code `truncated`: `Showing lines <a>-<b> of <total>. Use offset=<b+1> to continue.` with ` (50.0KB limit)` appended before the period when bytes hit (`read.ts:189-198`).
  - User limit leaves lines: `[info]` `<remaining> more lines in file. Use offset=<next> to continue.` (`read.ts:199-206`).
- `formatSize`: `<n>B`, `<x.y>KB`, `<x.y>MB` (`truncate.ts:91-99`).
- Empty selection -> empty content array.
- `details.truncation` = TruncationResult minus content.
- Replay: default `unsafe` (no `replay` field).

### 1.2 `write` (`durable/src/tools/write.ts`)

Schema (`write.ts:9-12`): `{path: string "Path to the file to write (relative or absolute)", content: string "Content to write to the file"}`, both required.
Description: `Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.`
Behavior: `env.writeFile` does `mkdir -p` of parent (`env/node.ts:1005-1027`), writes UTF-8 string as-is (no line-ending/BOM handling). Result text: `Successfully wrote to <path>` (original path as given). Replay: default `unsafe`.

### 1.3 `edit` (`durable/src/tools/edit.ts`, `edit-diff.ts`)

Schema (`edit.ts:19-33`):
```json
{"type":"object","required":["path","edits"],"properties":{
 "path":{"type":"string","description":"Path to the file to edit (relative or absolute)"},
 "edits":{"type":"array","description":"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.",
  "items":{"type":"object","required":["oldText","newText"],"properties":{
   "oldText":{"type":"string","description":"Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."},
   "newText":{"type":"string","description":"Replacement text for this targeted edit."}}}}}}
```
Description (`edit.ts:94`): `Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.`

Argument repair (`prepareEditArguments`, `edit.ts:49-71`): `edits` as JSON string (array or single object) is parsed; a single `{oldText,newText}` object is wrapped; top-level legacy `oldText`/`newText` are appended to `edits` and removed.

Algorithm:
1. `edits` empty -> `Edit tool input is invalid. edits must contain at least one replacement.` (`edit.ts:79-84`).
2. `fileInfo`: error -> `Could not edit file: <path>. Error code: <code>.` (codes: `not_found`, `permission_denied`, `not_directory`, `is_directory`, `invalid`, `aborted`); non-file/symlink -> `Could not edit file: <path>. Path is not a file.` (`edit.ts:106-110`).
3. Read text; strip BOM `U+FEFF`; detect line ending (CRLF iff first `\r\n` precedes first lone `\n`); normalize all `\r\n` and `\r` to `\n` (`edit-diff.ts:7-21`). oldText/newText normalized to LF too.
4. Empty oldText -> `oldText must not be empty in <path>.` / `edits[i].oldText must not be empty in <path>.`
5. Matching (`fuzzyFindText`, `edit-diff.ts:203-241`): exact `indexOf` first; else fuzzy in normalized space: NFKC, trimEnd each line, smart single quotes U+2018-201B -> `'`, double U+201C-201F -> `"`, dashes U+2010-2015,U+2212 -> `-`, special spaces U+00A0,U+2002-200A,U+202F,U+205F,U+3000 -> space. If any edit needs fuzzy, ALL edits match against the fuzzy-normalized content and the result is overlaid line-wise onto the original so unchanged lines keep original bytes (`applyReplacementsPreservingUnchangedLines`).
6. Not found -> single: `Could not find the exact text in <path>. The old text must match exactly including all whitespace and newlines.`; multi: `Could not find edits[i] in <path>. The oldText must match exactly including all whitespace and newlines.`
7. Occurrence count (always in fuzzy space) > 1 -> single: `Found <n> occurrences of the text in <path>. The text must be unique. Please provide more context to make it unique.`; multi: `Found <n> occurrences of edits[i] in <path>. Each oldText must be unique. Please provide more context to make it unique.`
8. Overlap after sorting by position -> `edits[a] and edits[b] overlap in <path>. Merge them into one edit or target disjoint regions.`
9. Apply in reverse order. If unchanged -> single: `No changes made to <path>. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected.`; multi: `No changes made to <path>. The replacements produced identical content.`
10. Write `BOM + restoreLineEndings(new, original)`. Result text: `Successfully replaced <n> block(s) in <path>.`; `details = {diff, patch, firstChangedLine?}`: `diff` is a display diff with `+<lineNo> line`/`-<lineNo> line`/context, 4 context lines, padded line numbers; `patch` is a unified patch (`Diff.createTwoFilesPatch(path,path,...,{context:4, FILE_HEADERS_ONLY})`) (`edit-diff.ts:~372-420`).
Replay: default `unsafe`.

### 1.4 `bash` (`durable/src/tools/bash.ts`, `env/node.ts`)

Schema (`bash.ts:10-13`): `{command: string "Bash command to execute", timeout?: number "Timeout in seconds (optional, no default timeout)"}`.
Description (`bash.ts:51`): `Execute a bash command in the current working directory. Returns combined stdout and stderr. Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.`
`outputLimits: {retain: "tail"}` (keeps the END).

Behavior:
- Timeout: none by default. Invalid -> `Invalid timeout: must be a finite number of seconds` (<=0, NaN, inf); > 2147483.647 s -> `Invalid timeout: maximum is 2147483.647 seconds` (`bash.ts:31-39`).
- Shell (`env/node.ts:213-263`): custom `shellPath` if set (not set by Pipkin path), else `/bin/bash` if exists, else `which bash`, else `sh`; args `["-c", command]`. Windows: Git Bash in `%ProgramFiles%`/`%ProgramFiles(x86)%`, `where bash.exe`; legacy WSL `System32\bash.exe` gets command via stdin with `-s`. Not a login/interactive shell; no rc files beyond what bash -c reads (`BASH_ENV`).
- cwd = env.cwd (conversation cwd); missing cwd -> `Working directory does not exist: <cwd>\nCannot execute bash commands.`
- Env: `{...process.env (worker env), ...shellEnv, ...{}}` (`inheritEnv: true`, `env/node.ts:265-276`). No PI-specific vars injected.
- Process: `spawn(shell, args, {detached: true (non-win), stdio: [ignore, pipe, pipe]})`; stdout+stderr interleaved into one stream to `api.output` (`env/node.ts:803-830`). Timeout or abort -> `process.kill(-pid, SIGKILL)` (process group), fallback pid (`env/node.ts:278-303`). After exit waits up to 100 ms (`EXIT_STDIO_GRACE_MS`) for stdio. Signal exit code = 128 + signo.
- Streamed output: harness `OutputBuffer` keeps tail window (50 KiB / 2000 lines); env may skip output outside the window (`outputWindow`, `skipped` counts). Live progress committed into `pi.live.tools[slot].output` at most every `outputIntervalMs` (100 ms) and throttled to ~100 KiB/s by size (`harness/output.ts:251-258`).
- Spill: once seen output exceeds 50 KiB or 2000 lines, all output (including prefix) is written to a temp file `$TMPDIR/tmp-XXXXXX/pi-output-<uuid>.log` (`env/node.ts:776-790,1194-1218`); diagnostic `[info]` code `full_output`: `Full output: <path>` (`bash.ts:79-82`). Spill files are never deleted by the engine.
- Result: `execute` returns `{}` so content = retained tail text. If the run dropped output, harness adds `[warn] Output truncated to its end: <L> lines, <B> bytes dropped`.
- Exit != 0 -> throws `Command exited with code <n>` (error result keeps output + diagnostics, `tool_error`) (`bash.ts:89`).
- Timeout -> `Command timed out after <t> seconds`; abort (not from the caller's signal) -> `Command aborted`; caller abort rethrown (task abort path).
- Options (not used by Pipkin): `commandPrefix` (prepended + `\n`), `prepare(execution)` hook.
- Replay: default `unsafe`; interrupted bash after a crash yields the `interrupted` error result (see §3.8).

### 1.5 `subagent` (`coding-agent/src/experimental/durable/subagent.ts`)

Schema: `{task: string "What the subagent should do"}`. Description: `Delegate a self-contained task to a subagent with the same tools and get its answer back. Give it everything it needs to know; it does not see this conversation.` `replay: "safe"`. Creates/reuses one child conversation per tool task (`scanConversations({ownerTaskId})`), ownership `{kind:"task", taskId}`, removes the `subagent` extension from the child, submits `{type:"input", content: task, requestId: "subagent:<taskId>"}`, waits; non-`done` -> throws `Subagent <childId> failed: <status>`; returns concatenated text blocks of the child's answer, `details: {conversationId}`. Details and patch-specific behavior: see §6.

### 1.6 Classic (non-durable) tools for reference (`coding-agent/src/core/tools/*`)

Not registered in the server path; listed so the port knows what exists upstream. `ToolName = read|bash|powershell|edit|write|grep|find|ls` (`core/tools/index.ts:95`).
- `read` (classic): same schema; supports images jpg/png/gif/webp/bmp with auto-resize to 2000x2000 and 4.5 MB base64 (`utils/image-resize-core.ts:5-27`); notes appended inline as `[Showing lines a-b of N. Use offset=x to continue.]` (`core/tools/read.ts:169-178`); non-vision model text `[Current model does not support images. The image will be omitted from this request.]`.
- `bash` (classic): `command` desc `Shell command to execute`; inline notices `[Showing lines a-b of N. Full output: <path>]`, `Command exited with code <n>`, `Command timed out after <s> seconds` (`core/tools/bash.ts:255-402`).
- `grep`: `{pattern, path?, glob?, ignoreCase?, literal?, context?, limit? (default 100)}`; uses ripgrep `--json --line-number --color=never --hidden`; lines truncated to 500 chars; notices `[100 matches limit reached. Use limit=200 for more, or refine pattern. 50.0KB limit reached. Some lines truncated to 500 chars. Use read tool to see full lines]`; `No matches found` (`core/tools/grep.ts:21-300`).
- `find`: `{pattern (glob), path?, limit? (default 1000)}`; respects .gitignore; notices `[1000 results limit reached ...]` (`core/tools/find.ts:26-160`).
- `ls`: `{path?, limit? (default 500)}`; alphabetical, `/` suffix for dirs, dotfiles included; `(empty directory)`, `Path not found: <p>`, `Not a directory: <p>`, `[500 entries limit reached. Use limit=1000 for more]` (`core/tools/ls.ts:11-155`).

---

## 3. Durable model

Source of truth: `durable/src/*` and the normative spec `durable/docs/spec.md` (4.7k lines; §2 records, §5 tasks, §6 submissions, §8 built-in tasks, §10 storage). README is a good summary (`durable/README.md`).

### 3.1 Concepts

- **Session/Harness**: one storage, one serialized "Session line" of atomic commits; nothing is observable before commit (`README.md:80`). One process owns a storage; no cross-process lock in the engine (the coding-agent adds `proper-lockfile` on the session directory, `experimental/durable/sessions.ts:45-54`, 12 retries x 1 s; stale after 10 s).
- **Conversation**: id (integer), optional `parent {conversationId, at}` (fork) and `owner {conversationId, taskId}` (task-owned) (`types.ts:286-298`). `ROOT_CONVERSATION_ID = 1` (`types.ts:39`).
- **Entry**: immutable `{id, conversationId, kind, model?: Message[], data?: JSON, head?: EntryId, edits?: ContextEdit[], byTaskId?}` (`types.ts:316-331`). IDs come from one global monotonically increasing id space shared by all record types (`record_ids`).
- **Document**: typed JSON state, scope `session|conversation|task`, history `latest|rewindable`, fork policy `initial|current|asOf`, stored as base + Chord deltas (`types.ts:42-130`, `documents.ts`).
- **Task**: durable state machine: `{id, kind, version, conversationId, owner?, background, abortRequested, state, memos?}`; states `pending|running|waiting|completing|terminal` (`types.ts:448-520`).
- **Submission**: `input` or `write`, statuses `queued|placed|done|unanswered` (`types.ts:362-427`).

### 3.2 SQLite schema (`durable/src/storage/sqlite/migrations.ts:9-85`)

All tables `STRICT`; JSON payload in `record TEXT CHECK(json_valid(record))`; indexed columns duplicated out of the JSON.

| Table | Columns | Indexes |
|---|---|---|
| `durable_schema` | `singleton INTEGER PK CHECK(=1)`, `version INTEGER NOT NULL CHECK(>=0)` | created by migrator (`migrations.ts:106-110`) |
| `durable_metadata` | `singleton INTEGER PK CHECK(=1)`, `next_id TEXT NOT NULL` (TEXT: node:sqlite rejects unsafe ints), `next_seq INTEGER NOT NULL`; seeded `(1,'2',1)` | - |
| `record_ids` | `id INTEGER PK`, `record_type TEXT CHECK IN ('conversation','entry','task','submission','document')` | - |
| `conversations` | `id INTEGER PK`, `owner_conversation_id INTEGER`, `owner_task_id INTEGER`, `record TEXT json` | `conversations_by_owner_conversation(owner_conversation_id,id)`, `conversations_by_owner_task(owner_task_id,id)` |
| `entries` | `id INTEGER PK`, `conversation_id INTEGER NOT NULL`, `head INTEGER` (nullable), `commit_seq INTEGER NOT NULL`, `record TEXT json` | `entries_by_conversation(conversation_id, id DESC)`, partial `entry_heads_by_conversation(conversation_id, id DESC) WHERE head IS NOT NULL` |
| `tasks` | `id INTEGER PK`, `conversation_id INTEGER NOT NULL`, `kind TEXT`, `status TEXT CHECK IN ('pending','running','waiting','completing','terminal')`, `abort_requested INTEGER CHECK IN (0,1)`, `background INTEGER CHECK IN (0,1)`, `record TEXT json` | `tasks_by_status(status,id)`, `tasks_by_conversation(conversation_id,id)`, `tasks_by_kind(kind,id)`, `tasks_by_abort_requested(abort_requested,id)`, `tasks_by_background(background,id)` |
| `submissions` | `id INTEGER PK`, `conversation_id INTEGER NOT NULL`, `request_id TEXT`, `status TEXT CHECK IN ('queued','placed','done','unanswered')`, `record TEXT json` | `submissions_by_request(conversation_id,request_id)` (NOT unique; uniqueness enforced in code), `submissions_by_conversation(conversation_id,id)`, `submissions_by_status(status,id)` |
| `documents` | `id INTEGER PK`, `kind TEXT`, `family INTEGER CHECK IN (0,1)`, `key_value TEXT`, `scope_kind TEXT CHECK IN ('session','conversation','task')`, `owner_id INTEGER`, `created_at INTEGER`, `retired_at INTEGER` (nullable), `record TEXT json` | `documents_by_address(kind,scope_kind,owner_id,family,key_value,created_at DESC,retired_at)`, `documents_by_scope(scope_kind,owner_id,id)`, `documents_by_scope_kind(scope_kind,owner_id,kind,id)` |
| `document_revisions` | `document_id INTEGER`, `seq INTEGER` (commit seq), `kind TEXT CHECK IN ('base','delta')`, `version INTEGER`, `content TEXT json`; PK `(document_id, seq)` | `document_revisions_by_kind(document_id, kind, seq DESC)` |

- Migrations: version list must be contiguous from 1; one SQL transaction; refuses a DB whose version is newer (`Durable SQLite schema version X is newer than supported version Y`) (`migrations.ts:95-125`). Current version 1.
- Document read = newest `base` at or before seq + following `delta`s (Chord ops) of the same version (`storage/sqlite/storage.ts:547-577`); a doc definition's `checkpointWhen(value, ops, info)` decides when a commit writes a full base instead of a delta (`session/transaction.ts:850-858`). Built-in policies: `pi.agent`/`pi.usage`/`pi.provider` always base; `pi.inbox` base when empty; `pi.live` base whenever nothing is generating and no tool slot is running (`harness/live.ts:61-71`).
- Entry `head` column marks "head markers" (reset, compaction, and system baselines); `findLatestHeadMarker` uses the partial index.

**Pragmas/connection** (`storage/sqlite/node.ts:16-17,180-198,140-170`): `PRAGMA journal_mode = WAL`; `PRAGMA synchronous = NORMAL`; `PRAGMA wal_autocheckpoint = 1000` (pages); busy timeout 5000 ms (`DatabaseSync(path,{timeout})`); every commit `BEGIN IMMEDIATE ... COMMIT` (rollback on throw; rollback failure -> `AggregateError("SQLite transaction failed and rollback failed")`); close runs `PRAGMA wal_checkpoint(TRUNCATE)`. All DB ops serialized through one queue; prepared statements cached by SQL text. Parent directory `mkdir -p`. Durability: survives process crash; newest commit may be lost on power failure (`README.md:563`).

### 3.3 Entry kinds (`durable/src/entries.ts`, plus overlays)

| kind | model | data | head | writer |
|---|---|---|---|---|
| `pi.user` | `[UserMessage]` (content string or text+image blocks) | - | - | submission admission / boundary (`submissions.ts:213-214`, `inbox.ts:316-318`) |
| `pi.assistant` | `[AssistantMessage]` any stopReason | - | - | generation `appendAssistant` (also records usage) (`generation.ts:649-656`) |
| `pi.system` | `[SystemMessage]` with `content:""`, `sections?: Record<key,string|null>`, `toolsAdded?`, `toolsRemoved?` | - | - | generation prepare (`prompt.ts:144-158`) |
| `pi.tool-result` | `[ToolResultMessage]` (content ends with `<harness>` diagnostics) | `{diagnostics}` | - | tool task / generation |
| `pi.reset` | absent, or `[UserMessage]` handoff text | - | `"self"` | `Conversation.reset()`, tool `control.handoff` (`generation.ts:619-623`) |
| `pi.compaction` | `[UserMessage]` with wrapped summary | `{reason}` | first kept entry | compaction task |
| `pi.cancelled-input` | absent | - | - | edits `[{target: <user entry>, action:"omit"}]`; written by the transaction layer when a placed input settles `unanswered/aborted` (`session/transaction.ts:822-845`) |
| app kinds | any | any | optional | `submit({type:"write"})` |

ContextEdit (`types.ts:301-314`): `{target: EntryId, action: "omit"}` or `{target, action:"replace", messages: Message[]}`. Edits on any entry in the active range apply; newest edit per target wins (`harness/context.ts:68-70`).

### 3.4 Model context derivation (`durable/src/harness/context.ts`)

1. H = newest head marker (entry with non-null `head`) at or before the tail. Range = entries from `H.head` (or start) to tail, fork-aware (a fork sees the parent's entries up to `parent.at`).
2. Active entries = `[H, ...range entries without head]` (older head markers dropped) (`context.ts:121-123`).
3. Per entry: omit -> `[]`; replace -> edit messages; else `entry.model`. Then drop assistant messages with `stopReason` in `{aborted, error, deferred}` (`context.ts:8,73-80`).
4. `orderToolResults`: each assistant's tool results moved directly after it in call order; missing result synthesized `{role:"toolResult", isError:true, content:"Tool result unavailable: history ends before this call completed.", details:{reason:"missing_result"}}`; unmatched results dropped (`context.ts:129-163`).
5. System messages stay positional; the provider layer replays `sections` (set/`null` delete/re-add appends) and tool add/remove (`prompt.ts:9-19`; pi-ai `getCurrentTools`).

### 3.5 Submissions, request keys, conflicts (`harness/submissions.ts`, `harness/inbox.ts`)

`SubmissionDraft` (`harness/types.ts:55-70`): `{requestId?, type:"input", content: UserInput, whenBusy?: "steer"|"followUp"|"reject"}` or `{requestId?, type:"write", entry: EntryDraft}`.

Admission (`admitSubmission`, `submissions.ts:160-219`), all in one commit:
1. `requestId` present and a submission with the same `(conversationId, requestId)` exists: same type -> return existing id, write nothing (**no content fingerprint; the key identifies the request, not its content** — `agent-controller.ts:13-17`); different type -> throw `Request <id> already identifies a submission of type <type>`.
2. Busy = `pi.live.run` present. Busy + `whenBusy:"reject"` -> throw `ConversationBusy` (agent-controller maps to `{code:"busy"}`).
3. Busy, or idle with queued inbox items: create submission `queued`, push inbox item `{id, mode:"steer"|"followUp"|"write", content|entry}`. If idle, apply a `final` boundary immediately (places everything eligible, starts a run).
4. Idle and empty inbox: write -> append entry, submission `done` (or `unanswered/stale` if its `head` < active range start); input -> append `pi.user`, submission `placed` with `entry`, start a run (`pi.live.run = {taskId: new pi.generation, inputs:[id]}`).

Inbox doc `pi.inbox` (`inbox.ts:15-23`): `{items: [{id, mode, content|entry}]}` in id order.

Boundaries (`applyBoundary`, `inbox.ts:284-325`):
- `postTools` (after a tool round): all writes, plus first steer (`steeringMode:"one-at-a-time"`, default) or all steers (`"all"`). Steers join the running run (`live.run.inputs.push`, `generation.ts:637`).
- `final` (run answered, terminate/handoff, or idle admission): writes + steers + first/all follow-ups (`followUpMode`). Placed follow-ups start the next run.
- A queued reset write (`head:"self"`) turns `postTools` into `final`; run then ends `unanswered/reset` (`generation.ts:629-633`).
- Writes placed first, then users, each in id order. A write whose numeric `head` is older than the current active range start is settled `unanswered/stale`.
- Coding-agent maps settings `steeringMode`/`followUpMode` from SettingsManager (`experimental/durable/harness-setup.ts:42-47`).

Withdrawal: `Submission.abort()` / `Harness.abortSubmission(id, ctx, conversationId?)`: queued -> settle `unanswered/aborted` and remove inbox item -> `"aborted"`; placed -> `"already_placed"`; settled -> `"settled"`; unknown/other conversation -> `"not_found"` (`submissions.ts:102-115`). Agent-controller `cancelQueued` maps to `cancelled|already_consumed|not_found`.

Unanswered reasons seen: `aborted`, `stale`, `model_error` (detail = error text), `no_model`, `reset`, `faulted` (detail), `orphaned` reasons (`missing_task|task_too_old|migration_failed`).

Agent-controller mapping (`experimental/services/agent-controller-provider.ts`): `prompt` = submit with `whenBusy:"reject"`; `steer` = `whenBusy:"steer"`; `followUp` = `whenBusy:"followUp"` (both start a run when idle); ids returned as decimal strings; empty `requestId` ignored; `lookup(requestId)` = `submissionByRequest` (never admits).

### 3.6 Stop / abort semantics

`Conversation.abort(ctx, {background?})` (`harness/types.ts:534-538`, scheduler `abortConversation` `scheduler.ts:321+`), one commit then wait:
1. Withdraw every queued **input** of the conversation (settle `unanswered/aborted`, remove from inbox); queued **writes stay** (`inbox.ts:343-351`).
2. Mark `abort_requested` on every live non-background task reachable by ordinary ownership traversal (the run's generation, its tool tasks, child subagent conversations' tasks owned by those tool tasks, manual compactions). Background tasks (threshold compactions; background-anchored work) survive unless `{background:true}`.
3. Running invocations are signalled (AbortSignal); abort handlers run bottom-up (owned work first) (`README.md:476`).
4. Resolve when the scope is idle.

Handlers:
- Tool task abort (`tool.ts:156-160`): result entry from the live slot's partial output/details/diagnostics + `[error] Tool <name> was aborted` (code `aborted`), `isError:true`; task terminal `aborted`.
- Generation abort (`generation.ts:256-287`): cancels a deferred provider handle if polling; converts a committed streaming partial into a `pi.assistant` entry with `stopReason:"aborted"` (excluded from future context, usage still recorded); appends `aborted` results for unstarted sequential calls (`Tool <name> was aborted`); `endRun(... {status:"unanswered", reason:"aborted"})` settles the run's placed inputs.
- **Retire aborted inputs** (latest upstream change, `session/transaction.ts:822-845`): when any `input` submission transitions `placed -> unanswered` with `reason === "aborted"`, the same commit appends a `pi.cancelled-input` entry with `edits:[{target: <that pi.user entry>, action:"omit"}]` (and `byTaskId` when inside a task). Effect: the next model request no longer sees the stopped user instruction; the aborted assistant is excluded by stopReason; completed tool calls/results of the stopped run remain in context ("Stop retires the instruction, not the audit trail or completed tool effects"). Withdrawn queued inputs never had an entry, so nothing to omit. Steers that were placed into the run are also run inputs and are retired too.
- Compaction abort: removes its `pi.live.compactions` status, terminal `aborted` (`compaction.ts:222-227`).
- `failed` tool ending (throw / interrupted) records cancellation intent that aborts conversations the call owns (subagent children).

### 3.7 Tasks, scheduler, recovery

- Task definition: `defineTask({name, version, initial, phases: {phase: handler}, abort, migrate?, hooks?})`; every phase must commit durable progress (new checkpoint) or the task faults `Task <kind> phase <p> returned without durable progress` (`scheduler.ts:893-896`). Uncaught throw -> `faulted`.
- Effect sandwich (spec §5.2): commit intent phase -> perform effect -> commit outcome/next phase. Reopen in an intent phase = effect may have happened.
- States: `pending` (eligible), `running` (reserved by one invocation), `waiting` (`on: TaskId[]`, `policy: "allSettled"|"failFast"`), `completing` (outcome decided, owned work still live), `terminal` (`outcome: completed|failed|aborted|orphaned|faulted`).
- Open (`scheduler.ts:230-256`): scan all live tasks; every `running` becomes `pending` (same checkpoint); re-derive abort cascades and finalize `completing`. Scheduling starts only on `resume()` or any submit/wait call (`harness.resume()` at `session-worker.ts:595`).
- Missing or too-new definition: task blocked; abort-marked ones settle `orphaned`. Older stored version migrates via `migrate`.
- Scheduler-written outcomes (`faulted`/`orphaned`) cleanup (`live.ts:156-187`): generation -> convert partial, `endRun(unanswered, reason faulted|<orphan reason>)`; tool -> slot marked done without entry (context synthesizes the missing result); compaction -> status removed.
- Ownership: tasks owned by a conversation or a task; conversations owned by a task (`ownership:{kind:"task",taskId}`) or ownerless; `background:true` (conversation-owned only) is a boundary for abort and idle.

Recovery per built-in phase:

| Task / phase | On reopen |
|---|---|
| `pi.generation` `prepare` | Re-run (pure until its commit, which appends `pi.system` entries + moves to `request` atomically). |
| `request` | Converts any committed partial (`pi.live.generation.message`) to an aborted assistant entry, then re-sends with the pinned `model`, `thinkingLevel`, `streamOptions`, `cutoff` (`generation.ts:182-206`). |
| `retry` | Sleeps until the durable `until` timestamp, then `prepare` with `attempt+1`. |
| `poll` | Sleeps until `pollAt`, `fetchDeferred(handle)` (deferred provider responses; default poll 5000 ms). |
| `tools` | Waiting on its tool tasks; continues when they settle; sequential rounds start the next pending call. |
| `pi.tool` `call` | Re-run from scratch (resolution, prepareArguments, validation, beforeTool hooks); nothing executed yet. |
| `pi.tool` `execute` (intent committed) | Replay rule (`tool.ts:114-154`): rerun only if stored `replay === "safe"` AND the currently resolved tool's `replay === "safe"` (slot progress cleared first). Otherwise result = durable partial output + `[error] Tool <name> was interrupted and may have partially run` (code `interrupted`), task `failed` (aborts owned children). Pipkin's subagent patch adds: a `subagent` call may resume via the registry's `subagent` extension even when the session disabled subagents, only if `recoveryExtension === "subagent"` and the stored `recoveryContract` (JSON of name, description, parameters, replay) equals the candidate's; a contract mismatch never resumes (`tool.ts:118-140,165-172`). |
| `pi.compaction` `select` | Re-run. |
| `summarize` | Re-request with pinned model/maxTokens/tail/firstKept. |
| `retry` | Sleep until `until`, then `summarize` attempt+1. |

Intent checkpoint (`tool.ts:35-44`): `{phase:"execute", arguments (final validated args), replay, recoveryExtension?, recoveryContract?}`. Recovery uses the stored arguments, not re-validation.

Generation flow (`generation.ts`): prepare -> request (stream; partial committed every `partialIntervalMs`=100 ms into `pi.live.generation.message`) -> classify:
- `deferred` -> `poll`.
- `toolUse` with calls -> `startToolRound`: append assistant, tool task per offered call (parallel by default; any tool with `executionMode:"sequential"` or `settings.toolExecution==="sequential"` makes the round sequential), unoffered calls get immediate `tool_unavailable` results; wait `allSettled`.
- `stop`/`length`/`toolUse` without calls -> `answer`: `onYield` hooks may continue; final boundary; settle run inputs `done` with `answer` entry; start next run from follow-ups.
- `error` + context overflow (pi-ai `isContextOverflow`) and compaction enabled and no prior compaction this attempt and a cut exists -> append the error assistant, create blocking compaction (reason `overflow`), wait, re-prepare; if that compaction did not produce an entry -> fail `model_error` with the overflow text.
- `error` retryable (`isRetryableAssistantError`) and `attempt <= maxRetries` -> append error assistant, `retry` with delay `baseDelayMs * 2^(attempt-1)` capped by `maxAgentDelayMs` (defaults: enabled, maxRetries 3, base 2000 ms, cap 60000 ms; `harness/agent.ts:19-24`). Coding-agent passes `settingsManager.getRetrySettings()`.
- otherwise -> `endRun(unanswered, model_error, detail)`, task `failed`.
- After tools: controls (`terminate` if every call asks, `addTools`, last `handoff`), `afterTools` hooks, `postTools` boundary, hand over to a new generation task (`handOver`).

`pi.live` document shape (`live.ts:43-62`): `run? {taskId, inputs[]}`, `generation? {attempt, message?, retry? {at, error}, deferred? {pollAt}}`, `tools? [ToolSlot{callId, name, taskId?, status pending|running|done, output?, droppedBytes?, droppedLines?, details?, diagnostics?, entry?}]`, `compactions? [{taskId, reason, blocking, attempt, retry?}]`.

Other built-in docs: `pi.agent` (`AgentState {model?, thinkingLevel?, extensions?, tools?, instructions?, cwd?}`, rewindable, fork `asOf`; `harness/agent.ts:41-50`), `pi.provider` (`{sessionId: uuidv7}` forwarded as pi-ai `sessionId` for prompt caching/affinity; fresh per conversation/fork; `harness/provider.ts`), `pi.inbox`, `pi.usage`, `pi.live`.

Stream options per request (`generation.ts:194-199`): `{...settings.stream, signal, sessionId, reasoning: thinkingLevel (omitted when "off")}`. Coding-agent stream settings: `timeoutMs = provider.timeoutMs ?? (httpIdleTimeoutMs===0 ? 2147483647 : idle)`, `maxRetryDelayMs`, `maxRetries` (`harness-setup.ts:25-35`).

### 3.8 System prompt as positional entries (`harness/prompt.ts`)

- Sections render in agent order; tagged sections wrapped `<key>\n...\n</key>` unless `tag:false`; agent `instructions` render last as key `instructions`. A throwing section keeps its previously shown text (`prompt.ts:25-48`).
- `planSystemEntries` (`prompt.ts:66-93`): after a head marker with no later `pi.system` in range -> one complete baseline entry (all sections + all tools) that also `omit`-edits every retained earlier `pi.system`; else minimal patch (`{key: text|null}`), or remove-all/re-add-all when order would differ; tool changes (`toolsRemoved` names / `toolsAdded` declarations, changed declaration = remove+add) ride on the last entry. Only changes are sent, preserving provider prompt caches. Coding-agent sections: `preamble, tools, rules, docs, project_context, skills, cwd` all `tag:false` (see §2).

### 3.9 Compaction (`harness/compaction.ts`)

Policy defaults (`harness/agent.ts:26-31`): `enabled: true`, `reserveTokens: 16384`, `keepRecentTokens: 20000`, `backgroundTokens: 32768` (coding-agent overrides from `settingsManager.getCompactionSettings()`, `harness-setup.ts:36-38`).

Token estimate (`estimateContext`, `compaction.ts:307-322`): usage of the newest assistant after the head marker with non-zero `calculateContextTokens` (= `totalTokens || input+output+cacheRead+cacheWrite`) + chars/4 estimates of later messages (images 4800 chars; tool calls name+JSON args; `ai/src/utils/estimate.ts:15-70`) + planned system entries.

Triggers (`generation.ts:303-322`, in `prepare`): `blocking = contextWindow - reserveTokens`; `background = blocking - backgroundTokens`. tokens > blocking -> create a **blocking** compaction owned by the generation (reason `threshold`), generation waits then re-prepares. tokens > background (and `backgroundTokens > 0`, and no compaction listed) -> **background** conversation-owned compaction; request proceeds. Only if `selectCut` finds a cut. Overflow error -> blocking `overflow` compaction once per attempt. Manual `compact(instructions)` -> conversation-owned, non-background (Esc aborts it).

Cut selection (`selectCut`, `compaction.ts:255-295`): walk back from the tail summing estimated tokens per entry until `>= keepRecentTokens`; cut = first candidate at/after that entry (else the last candidate). Candidate = entry whose first contributed message is an assistant, or a user message not followed (before the next assistant) by a tool result of the preceding assistant's calls. Index 0 (head marker) is never cut-able; returns undefined if nothing non-empty precedes the cut.

Summarization request (`compaction.ts:148-207`): messages `[system: SUMMARIZATION_SYSTEM_PROMPT, user: "<conversation>\n" + serialized + "\n</conversation>\n\n" + SUMMARIZATION_PROMPT + ("\n\nAdditional focus: " + instructions)]`; options `{...stream (minus deferred), cacheRetention:"none", maxTokens: min(floor(0.8*reserveTokens), model.maxTokens), sessionId, reasoning: thinkingLevel unless off}`; uses the conversation's current model. Serialization: `[User]: ...`, `[Assistant thinking]: ...`, `[Assistant]: ...`, `[Assistant tool calls]: name(k=JSON, ...); ...`, `[Tool result]: ...` truncated to 2000 chars + `\n\n[... N more characters truncated]`; system messages omitted; blocks joined `\n\n`.

System prompt text: `You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.` Prompt asks for the exact format with headings `## Goal`, `## Constraints & Preferences`, `## Progress` (`### Done` `- [x]`, `### In Progress` `- [ ]`, `### Blocked`), `## Key Decisions` (`- **[Decision]**: [Brief rationale]`), `## Next Steps` (numbered), `## Critical Context`, ending `Keep each section concise. Preserve exact file paths, function names, and error messages.`; and "If the conversation starts with an earlier summary, preserve its information and fold the newer messages into it." (`compaction.ts:57-96`).

Result handling: valid summary = `stopReason === "stop"`, no tool calls, non-empty trimmed text (text blocks joined `\n`). Retryable error -> durable retry (same retry policy). Else failed: `Summarization failed: <err>` | `Summarization hit the token limit; the summary is incomplete` | `Summarization attempted to call a tool` | `Summarization produced no text`. No model -> `No model is configured` / `Model p/m is not available`. Usage of every attempt recorded under `models["provider/model"]`.

Placement (`compaction.ts:407-436`): entry `{kind:"pi.compaction", head: firstKept, model:[user "The conversation history before this point was compacted into the following summary:\n\n<summary>\n" + summary + "\n</summary>"], data:{reason}}`. Blocking: appended directly. Non-blocking: admitted as a write submission `requestId: "compaction:<taskId>"` (placed at once if idle, at next boundary if busy, `stale` if an earlier cut already passed `firstKept`). What is kept: the summary (as the new head marker) followed by every entry from `firstKept` to the tail; older entries stay in storage but leave model context. `beforeCompact` hooks can decline or supply a summary.

### 3.10 Context edits, reset, fork ("branch")

- Omission edits are produced by: system baseline after a head marker (omit older `pi.system`), and `pi.cancelled-input` (omit aborted user input). `replace` edits are supported by context derivation but not produced by built-ins. Apps may write entries with `edits` via write submissions.
- `reset(handoff?)` (`harness/types.ts:515-519`): admits a write of `pi.reset` with `head:"self"` and optional user handoff message; while busy it is placed at the next boundary and ends the run (`unanswered/reset` at postTools). Tool `control.handoff` does the same inline.
- `fork(at, options)`: new conversation with `parent {conversationId, at}`; sees parent entries up to `at` (inclusive); documents copied per fork policy (`pi.agent` as of `at`, `pi.live`/`pi.inbox`/`pi.usage` initial, `pi.provider` fresh) (`session/forks.ts`). No "branch" API in the server services beyond what §4 exposes.
- `createConversation({ownership})`: ownerless or task-owned; task-owned children start as a copy of the owner conversation's agent.

---

## 8. Usage/cost, models/thinking, attachments, retained output

### 8.1 Usage and cost

- Ledger doc `pi.usage` per conversation: `{models: {"provider/modelId": Usage}, tools: {toolName: Usage}}` (`harness/usage.ts:7-22`). Every assistant append (including errors/aborted partials) and every summarization attempt adds usage in the same commit; tool results with `usage` add to `tools` (`tool.ts:522`). `Harness.usage()` sums all conversations (subagent children included).
- `Usage = {input, output, cacheRead, cacheWrite, cacheWrite1h?, reasoning?, totalTokens, cost:{input, output, cacheRead, cacheWrite, total}}` (`ai/src/types.ts:429-449`). Addition is field-wise; optional counters added once reported (`usage.ts:42-55`).
- Cost is computed by pi-ai per response (`calculateCost`, `ai/src/models.ts:1198-1218`): rates per million tokens from the model catalog (tiered by `inputTokensAbove` on input+cacheRead+cacheWrite); 1h cache writes billed at 2x base input; `total = input+output+cacheRead+cacheWrite`.

### 8.2 Model selection and thinking per session

- Stored per conversation in `pi.agent`: `model {provider, modelId}`, `thinkingLevel` (`harness/types.ts:310-325`). Resolution falls back to `"off"` when unset. Request pins model/thinking at `prepare`; a change applies from the next request.
- Root conversation initial model only at creation: `--provider/--model` CLI or pi default resolution (`settings defaultProvider/defaultModel/defaultThinkingLevel`) (`session-worker.ts:797-812`, `harness-setup.ts:91-123`); later starts keep the durable model.
- Thinking levels: `off|minimal|low|medium|high|xhigh|max` (`ai/src/types.ts:85-86`, order `models.ts:1220`). Supported = `["off"]` for non-reasoning models; otherwise all levels except those mapped to `null` in `thinkingLevelMap`, and `xhigh`/`max` only when explicitly mapped (`models.ts:1222-1231`). `clampThinkingLevel` picks the requested level or the next higher supported, else first supported (`models.ts:1233-1250`). Selecting a model clamps the current level and also persists it as the global default (`settingsManager.setDefaultModelAndProvider`) (`experimental/services/models-provider.ts:96-102`). `reasoning: level` is passed to pi-ai `streamSimple` unless `off`.
- Compaction uses the conversation's current model and thinking level.

### 8.3 Attachments / images

- Client prompt: `AgentPromptRequest {message: string, images: {type:"image", data: base64, mimeType}[] | null, requestId?}` (`experimental/services/agent-controller.ts:3-19`). With images the user content becomes `[{type:"text", text: message}, ...images]`; else a plain string (`agent-controller-provider.ts:113-117`).
- The durable/server path performs **no** image resizing, MIME validation, or count/size limits; images are stored verbatim (base64 in the `pi.user` entry JSON in SQLite). Effective caps: protocol frame `DEFAULT_MAX_FRAME_LENGTH = 16 MiB` (`protocol/src/framing.ts:6`) and worker control line `MAX_CONTROL_LINE_BYTES = 128 MiB` (`experimental/process.ts:100`); provider limits otherwise. Token estimate treats each image as 4800 chars (~1200 tokens).
- Classic path (not server) resizes to <=2000x2000 and <=4.5 MB base64 (`coding-agent/src/utils/image-resize-core.ts:5-27`).
- The durable `read` tool rejects images (§1.1).

### 8.4 Retained tool output

- During a call: the tail (bash) or head (others) window, <=50 KiB/2000 lines, lives in `pi.live.tools[i].output` with `droppedBytes/droppedLines`; committed throttled (100 ms min, ~100 KiB/s) (`tool.ts:339-386`). Cleared when the slot finishes.
- Final: the bounded content is stored in the `pi.tool-result` entry (exactly what the model sees), `details` and structured `diagnostics` in the entry.
- Full output: only bash spills, to `$TMPDIR/tmp-XXXXXX/pi-output-<uuid>.log` once output exceeds 50 KiB or 2000 lines (`env/node.ts:776-790,1194-1218`); path reported as `[info] Full output: <path>` in the `<harness>` block. Never cleaned up by the engine; `NodeExecutionEnv.cleanup()` only kills live child process groups (`env/node.ts:1220-1224`). No engine-managed store of full outputs in SQLite.
- After crash: interrupted/aborted results are rebuilt from the slot's last committed partial output (`fromSlot`, `tool.ts:463-474`).
