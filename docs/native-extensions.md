# Native engine extensions

This page covers extensions for Pipkin's built-in Rust engine (`crates/pipkin-engine`, module `extensions`). For what the legacy Pi engine supports, see [extensions.md](extensions.md).

Pi extension compatibility is **not a Pipkin requirement**, per the owner's 2026-10-11 scope
decision. This page documents the optional native facilities already implemented. Existing
TypeScript packages are unsupported; building a loader, porting packages, remote extension
services and hot reload are not required for the native-engine release.

## What an extension can do

The Rust engine's extensions are a port of what the reference engine's **session facets** can reach. Session facets are the chord facet plugin packages that `pi server -e <path>` loads, and that Pipkin's `--pi-extension` passes through. An extension can:

- **Ask the person things** and **show notices and status lines** (the reference `UiRequestHost`):
  - a choice (`select`), a yes/no (`confirm`) or a line of text (`input`);
  - notices at `info`, `warning` or `error` level;
  - keyed status lines.
- **Drive the session's agent** (the reference `AgentController`): prompt, steer, follow up, cancel a queued input, look up a request key, wait for an input’s answer, abort and compact.
- **Read history** a page at a time (the reference `History.page`).

The reference facets cannot add any of the following, and native extensions cannot either:

- model-visible tools;
- prompt sections or hooks;
- model providers;
- terminal widgets or slash commands.

The desktop only shows the questions, notices and status lines.

## The question rules

`QuestionHost` follows the question and refusal behavior of `ui-requests-provider.ts`. Runtime identities are scoped to a session lifetime as described below.

- **Ids.** Treat IDs as opaque strings. Runtime questions and notices include a fresh session-lifetime scope plus a shared counter; delayed answers from a retired lifetime cannot resolve new questions. Standalone `QuestionHost` fixtures use the reference `q<n>` / `n<n>` format.
- **Timeout.**
  - A question waits two minutes by default. A timeout of `0` waits indefinitely.
  - When the time runs out, an info notice `No answer to "<title>"; it was cancelled.` is posted (the title is quoted up to 80 characters). The question then resolves as unanswered.
- **Open questions.**
  - At most 20 questions are open at once.
  - A further question is refused at once with the warning `Too many open questions; "<title>" was not shown.`
  - A `select` with no choices is never shown.
- **Notices.** The host keeps at most 20 notices and drops the oldest first. A presentation can dismiss them all.
- **Answers.** The first accepted answer wins. These refusals keep the question open:
  - `A confirmation needs yes or no.`
  - `An answer must be text.`
  - `That is not one of the choices.`
  - `An answer can be at most 10000 characters.`
  - `That question is no longer open.`
- **Length limits.**

  | Text | Limit (characters) |
  | --- | --- |
  | Title | 300 |
  | Message | 2000 |
  | Notice | 500 |
  | Status key | 80 |
  | Status text | 300 |

  Like the reference, lengths are counted in UTF-16 units, but a cut never splits a character.
- **Ending a question early.** Each of these resolves the question as unanswered:
  - cancelling it by id;
  - the asker's cancellation token;
  - dropping the asking future;
  - disposing the session.

  After disposal, new questions are refused.

## In-process extensions (Rust)

```rust
pub const EXTENSION_API_VERSION: u32 = 1;

pub trait Extension: Send + Sync {
    fn id(&self) -> &str;
    fn api_version(&self) -> u32 { EXTENSION_API_VERSION }
    fn activate(&self, host: Arc<dyn ExtensionHost>) -> BoxFuture<'static, ()>;
    fn deactivate(&self) {}
}
```

`ExtensionHost` provides these methods. Agent calls return protocol types and `ProtoError`s.

- Questions and notices: `ask(Question, timeout, CancellationToken) -> Option<Answer>`, `notify`, `set_status`.
- Agent control: `prompt`, `steer`, `follow_up` (text plus an optional request key) `-> Admission`, `cancel_queued`, `lookup -> LookupResult`, `wait_for_prompt -> PromptResult`, `abort`, `compact`.
- History: `history_page(before, limit)`.

`CallHost` implements the trait. It sends agent control through the same protocol calls a client uses (`Prompt`, `Enqueue`, `Lookup`, `WaitForPrompt`, `Stop`, `Compact`, `HistoryPage`, `CancelQueued`). As a result, an extension's input is admitted, journaled and made idempotent like any other.

`ExtensionRunner` runs each extension's activation as its own task:

- It refuses an extension that declares another API version, or a duplicate id. Each refusal comes with a warning notice.
- It reports a panic once, as an error notice, and does not restart the extension.
- On session close it calls `deactivate`, gives the tasks a grace period, and then aborts them. Aborting withdraws any question an extension was still asking.

The protocol's `OpenSession` attaches a client; `ReleaseSession` releases that client's
attachment without cancelling work. The desktop releases the prior attachment when switching
conversations. Once the last attachment and active operation are gone, the runtime disposes
questions and shuts down extensions. A disconnect allows a thirty-second reconnect grace;
a newly loaded unattached session has a ten-second initial grace. Reopening starts a fresh
extension lifetime after previous shutdown completes. Old hosts cannot submit work or UI
updates into the replacement lifetime. History and admitted work remain durable.

`extensions::testing::TestHost` (under `cfg(test)` or the `test-support` feature) is a test double. It pairs a real `QuestionHost` with a recording agent.

## Waiting for an input

Rust `Admission.operation_id` and process replies' `operationId` identify an admitted prompt,
steer or follow-up. Treat them as opaque strings. Native identities use the session's durable
request key, including an automatically generated key when none was supplied. Engine clients
use `Call::WaitForPrompt { session, request }` (protocol 8, capability `waitForPrompt`).

Waiting returns only the final answer's text blocks, concatenated without a separator; thinking
and intermediate tool-round text are excluded. A completed empty answer is an empty string.
Unanswered results include `aborted`, `model_error` or `no_model`; native recovery can also report
`interrupted` or `failed`. Unknown keys return `Unknown prompt: <key>`. Reads use durable
settlement, so a later turn or restart cannot change an earlier answer. Imported keyed inputs
use their preserved Pi submission's exact answer identity and reason.

Dropping a Rust wait future, cancelling a process `waitForPrompt` request, or disconnecting the
waiting client abandons the wait without cancelling the input. Use `abort`/`cancelQueued` to
cancel work. Storage failure ends pending waits with an error, retaining the unsettled input
for explicit recovery. Engine shutdown ends waits. There are at most 64 pending waits across
the engine; each connection also retains at most 64 wait tasks through reply delivery. Excess
waits return `busy`, and normal/control calls retain separate capacity.

Native compaction currently returns synchronously without a durable operation handle. Waiting
for historical Pi write/compaction submissions or unkeyed imported submissions is not exposed
by this input-key API; general durable submission identity remains part of the remaining
service/history work.

## Out-of-process extensions (any language)

An extension can be any program that speaks the JSON-lines protocol of `extensions::process`. The doc comment there is normative; this is the same contract.

### Launch and ownership

- The engine runs the configured argv directly, with no shell.
- The program runs in its own process group, with stdin, stdout and stderr piped and `PIPKIN_EXTENSION_API=1` set.
- The engine owns the group. When the extension stops for any reason, every process left in the group is killed.
- A crashed extension is reported once and never restarted.

### Framing

- Each message is one JSON object per `\n`-terminated UTF-8 line, in both directions.
- A line from the extension longer than the limit (1 MiB by default) is a protocol violation.
- So is a line that is not UTF-8 or not a JSON object, or an object that is neither a request nor a known message. The extension is then stopped with an error notice saying why.
- Blank lines are ignored.
- Stderr is not part of the protocol. Its last line is quoted when the extension crashes.

### Lifecycle

```text
engine -> {"type":"hello","api":1,"extension":"<id>","session":"<session id>"}
ext    -> {"type":"hello","api":1}                  (within 10 s by default)
engine -> {"type":"activate"}
          ... requests and replies ...
engine -> {"type":"deactivate"}                     (the session is closing)
          stdin is closed; after 2 s by default the whole group is killed
```

- An extension that answers with another `api` is stopped with a notice.
- Exit status 0 means the extension finished. Any other status, or a signal, is reported as a crash.

### Requests from the extension

A request looks like `{"id": <string|number>, "method": "...", "params": {...}}`. The engine answers `{"id": <same>, "result": ...}`, or `{"id": <same>, "error": {"code": "...", "message": "..."}}`.

- Requests run concurrently, at most 64 in flight. Replies come in completion order.
- An id must not be reused while its request is in flight.
- `notify` and `setStatus` take effect in the order they are sent.
- `{"type":"cancel","id":<id>}` withdraws an in-flight request. An `ask` then answers `null`.
- Messages with an unknown `type` are ignored.

| Method | Params | Result |
| --- | --- | --- |
| `ask` | `kind` (`select`/`confirm`/`input`), `title`, `message`?, `options`? (`[{value,label,description?}]`), `placeholder`?, `defaultValue`?, `timeoutMs`? (default 120000; 0 = indefinite) | a string (a boolean for `confirm`), or `null` when unanswered |
| `notify` | `level` (`info`/`warning`/`error`), `message` | `null` |
| `setStatus` | `key`, `text` (`null` clears) | `null` |
| `prompt`, `steer`, `followUp` | `text`, `requestKey`? | `{"operationId": "opaque-key", "run": n\|null, "entry": n\|null}` |
| `cancelQueued` | `entry` | `{"outcome": "cancelled"\|"alreadyConsumed"\|"notFound"}` |
| `lookup` | `requestKey` | the protocol `LookupResult` |
| `waitForPrompt` | `operationId` (from a prompt/steer/followUp reply) | `{"status":"done","text":"...","reason":null}` or `{"status":"unanswered","text":null,"reason":"..."}` |
| `abort` | (none) | `{"run": n\|null}` |
| `compact` | `instructions`? | `null` |
| `historyPage` | `before`?, `limit` (at most 200) | `{"entries": [...], "hasOlder": bool}`, oldest first |

Error codes are the protocol's `ErrorCode` names: `invalid` (bad params or a reused id), `unsupported` (unknown method), `busy`, `tooLarge` (a reply over the line limit), `notFound`, `conflict`, and so on.

## Examples

- `extensions::examples::QuestionsExample` ports `pi-example-questions`. It asks the same three questions with the same titles, choices and messages. It reports each answer with the same notices and sets the same `example-questions` status line.
- `extensions::examples::GreetingExample` ports the Session facet of `pi-example-plugin`:
  - It counts activations and greets with the same text.
  - Its `hello` method runs the session side of the reference `/hello` command (greet, then prompt `pong: <name>`).
  - The reference publishes the greeting as a remote service for its TUI facet. Native extensions cannot publish services to presentations, so here `greet` is a plain method.
- `crates/pipkin-engine/tests/fixtures/extensions/questions.sh` is the same questions example as a process extension in plain POSIX sh. The engine's tests drive it through `extensions::process`.

## Compatibility with existing Pi extensions

- **Existing TypeScript facet packages do not run in the native engine.** Their code is TypeScript, bundled by chord, and loaded into the Node session worker; the native engine has no JavaScript runtime and does not embed or launch Node. So an extension written for the native engine does not run on the legacy engine, and an existing facet package does not run on the native engine.
- **Classic Pi extensions** (`ExtensionAPI`, `pi.registerTool`, ...) are not supported either. The legacy durable path did not load them anyway.

An existing facet package has two options:

1. **Port it** to the native contract, either as a Rust `Extension` or as a process extension in any language. The examples above show both. Ports must account for the missing reference features listed below, especially custom remote services and reload.
2. **Keep running it on the legacy Pi engine** (`--engine pi` with `--pi-extension <package>`), unchanged.

The native engine does **not** run arbitrary TypeScript facet packages. TypeScript facet compatibility is **out of scope by owner decision**. The legacy Pi option remains available only during the broader engine transition.

## Configuration and persistent selections

Launch an owned native engine with `--engine native --engine-extension builtin:questions`
(or `builtin:greeting`, or an executable path). Repeat `--engine-extension` for an ordered
selection. The engine persists this default in its data directory. Later launches without
extension flags restore it. `--engine-clear-extensions` explicitly clears the default; it
cannot be combined with `--engine-extension` or used with `--engine-attach`.

Each root session pins its selection on first acquisition, including an empty selection.
Changing or clearing the default affects only sessions without a pin. An imported plugin-free
Pi session pins an empty selection atomically with its history, so unrelated native defaults
cannot activate in it. Imports with unsupported Pi facet selections are still refused.
Subagent sessions do not load extension profiles.

Protocol 7 adds `PrepareExtensions` and the `extensionProfiles` capability. Preparation
validates and persists selections without activating an extension. The call's optional
`session` selects a root session; omitting it selects the engine default. Omitting
`extensions` reads/restores the selection (and pins an unpinned session); an explicit empty
array clears the default or pins an empty session selection. For example, call payloads are:

```json
{"call":"prepareExtensions","extensions":[{"builtin":"questions"}]}
{"call":"prepareExtensions","session":"<session-id>","extensions":[]}
```

An active session accepts only its current ordered selection. A different selection returns
`Session <id> is active with a different plugin selection`. Release its attachments and let
its runtime retire before selecting a replacement. Reopening waits for the previous extension
lifetime to shut down; old questions cannot answer new ones.

Process selections use `{"process":{"id":"example","argv":["/path/to/program","arg"]}}`.
Preparation resolves the executable to an absolute path, checks file/executable access and
unique runtime IDs, and preserves argument boundaries. It does not run a handshake or build
TypeScript packages. Invalid or future profile formats fail closed. Explicit replacement can
repair an inactive session's bad profile; startup `--engine-clear-extensions` can repair a bad
default without editing SQLite. A failed first session pin prevents work until explicit reopen
successfully persists the originally chosen selection.

Schema 5 marks the new persistent semantics and uses the store's existing backup/upgrade and
newer-schema refusal rules. Older engines must not silently ignore session pins.

Runtime wiring is implemented: session snapshots and changes carry the question host's view,
answer/cancel/dismiss calls reach it, and opening/retiring sessions starts/stops their selected
native extensions.

## Optional reference features not implemented

- Facet hot reload (`SessionPlugins.reload`).
- Services that facets publish to presentations.
- Execution or conversion of existing TypeScript facet packages; profile persistence does
  not provide that compatibility.

A process extension runs with the person's privileges, like the reference's facets do inside
the Node worker. It is not sandboxed.
