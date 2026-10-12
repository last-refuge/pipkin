# Native engine extensions

This page covers extensions for Pipkin's built-in Rust engine (`crates/pipkin-engine`, module `extensions`). For what the legacy Pi engine supports, see [extensions.md](extensions.md).

## What an extension can do

The Rust engine's extensions are a port of what the reference engine's **session facets** can reach. Session facets are the chord facet plugin packages that `pi server -e <path>` loads, and that Pipkin's `--pi-extension` passes through. An extension can:

- **Ask the person things** and **show notices and status lines** (the reference `UiRequestHost`):
  - a choice (`select`), a yes/no (`confirm`) or a line of text (`input`);
  - notices at `info`, `warning` or `error` level;
  - keyed status lines.
- **Drive the session's agent** (the reference `AgentController`): prompt, steer, follow up, cancel a queued input, look up a request key, abort and compact.
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
- Agent control: `prompt`, `steer`, `follow_up` (text plus an optional request key) `-> Admission`, `cancel_queued`, `lookup -> LookupResult`, `abort`, `compact`.
- History: `history_page(before, limit)`.

`CallHost` implements the trait. It sends agent control through the same protocol calls a client uses (`Prompt`, `Enqueue`, `Lookup`, `Stop`, `Compact`, `HistoryPage`, `CancelQueued`). As a result, an extension's input is admitted, journaled and made idempotent like any other.

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
| `prompt`, `steer`, `followUp` | `text`, `requestKey`? | `{"run": n\|null, "entry": n\|null}` |
| `cancelQueued` | `entry` | `{"outcome": "cancelled"\|"alreadyConsumed"\|"notFound"}` |
| `lookup` | `requestKey` | the protocol `LookupResult` |
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

1. **Port it** to the native contract, either as a Rust `Extension` or as a process extension in any language. The examples above show both. For facets that only use `UiRequestHost`, `AgentController` and `History`, the port is mechanical.
2. **Keep running it on the legacy Pi engine** (`--engine pi` with `--pi-extension <package>`), unchanged.

The native engine does **not** run arbitrary TypeScript facet packages. The parity row for TypeScript facet compatibility stays **blocked**: it is served only by the legacy Pi bridge.

## Not done here

- **Runtime wiring.** The engine runtime still has to do the following:
  - create a `QuestionHost` per session and publish its views as `SessionChange::UiChanged`;
  - map `AnswerQuestion`, `CancelQuestion` and `DismissNotices` to it;
  - put its view in `SessionSnapshot.ui`;
  - build a `CallHost` over its dispatch;
  - start the configured extensions with an `ExtensionRunner`;
  - dispose the host when the session closes.
- **Configuration.** Nothing yet decides how a person configures process extensions (a flag or a settings key).
- **Missing reference features:**
  - `AgentController.waitForPrompt` (waiting for a prompt's answer text);
  - facet hot reload (`SessionPlugins.reload`);
  - services that facets publish to presentations.
- **No sandbox.** A process extension runs with the person's privileges, like the reference's facets do inside the Node worker.
