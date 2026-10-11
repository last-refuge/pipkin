# Support, data ownership and recovery

This page describes the current qualification build. Release acceptance and the native install/recovery gates are
still open. Do not treat automated socket/installer checks as proof of a clean desktop workflow.

## Data and ownership

| Location | Owner / contents | Sharing expectations |
| --- | --- | --- |
| `$XDG_DATA_HOME/pipkin` or `~/.local/share/pipkin` | Pipkin preferences, drafts/attachment references, submission journal, goals and cached conversations in `pipkin.sqlite3`; engine output in `engine.log` | Private: can contain prompt text, tool output, source paths and secrets |
| `<data dir>/pipkin.sqlite3.bak-vN` | Pre-migration backup for schema N | Same sensitivity as the database; does not include later drafts |
| `~/.pi/server` (or explicit `--pi-dir`) | Pi server coordination profile/identity, sockets and process/launcher state | Private; not the durable session database location |
| `~/.pi/agent` (or `--pi-agent-dir`) | Pi authentication/configuration and other agent-managed state | `auth.json` may contain API keys/OAuth tokens; never upload it |
| `<agent dir>/experimental/sessions/<session ID>/` | Authoritative experimental Pi sessions: `meta.json` and worker-owned `session.sqlite` | Private; preserve this directory for session recovery, not just the server sockets/profile |
| Installation prefix / version directories | App, paired engine, public build identity and third-party notices | Not your draft/session storage |
| Project directory | Actual files changed by tools or other programs | Changes survive Stop, quit and uninstall |

Pipkin data-directory precedence is `--data-dir`, `PIPKIN_DATA`, `XDG_DATA_HOME/pipkin`, then
`~/.local/share/pipkin`. **Changing only `--data-dir` does not isolate the engine profile or credentials.**
Disposable destructive tests must also use a separate server profile, agent directory and project. With a source
or installed engine explicitly passed as `--pi-repo`, `--pi-dir` selects that owned engine's profile; `--pi-dir`
without an engine-to-launch selects an already running external server instead.

Draft attachments are saved as file references/metadata, not a promise that the original file will exist forever.
Do not delete originals before checking the restored draft. Keep backups private and include both app and engine
state if you need full recovery; a desktop-cache or server-coordination-profile copy alone is not an engine-session
backup. An externally started server may use an explicit session-directory override; verify its configuration rather
than assuming the defaults.

### Native engine (development)

`--engine native` runs Pipkin's built-in engine instead of Pi. Its sessions live in `<data dir>/engine/`
(`engine.sqlite` and artifacts) and its configuration and credentials in `~/.config/pipkin/engine/`
(`auth.json` is `0600`; with `--data-dir`/`PIPKIN_DATA` both move under that directory). It never reads Pi's
directories on its own, and nothing is imported at startup.

Importing from Pi is explicit, from the command palette, and only in native mode:

- **Import conversations from Pi…** reads the Pi agent directory (`$PI_CODING_AGENT_DIR` in the engine's
  environment, else `~/.pi/agent`), shows what can be imported and what cannot (sessions in use by a running Pi,
  sessions that cannot be represented without guessing, with reasons), and imports only after you choose
  **Confirm: import…**. Each Pi session is copied consistently into `<data dir>/engine/imports/<UTC time>/<session>/`
  first and read only from that copy. The desktop drafts, prompt history, unresolved requests and saved transcript
  copies of the imported conversations are copied (not moved) from the Pi namespace of `pipkin.sqlite3`. Running it
  again is safe: already imported sessions are skipped.
- **Import Pi provider settings and credentials…** is a separate confirmation that lists provider ids only (values
  are never shown or logged). It copies `models.json` and merges the selected `auth.json` entries into the native
  credential store without overwriting anything already set up there. Anthropic subscription (OAuth) sign-ins stay
  in the Pi profile, unimported, for a later integration.

Rollback and ownership: an import never changes the Pi source. The Pi sessions, settings and credentials keep
working with `--engine pi` (the default) exactly as before. History written in an imported conversation after the
import exists only in the native engine; it is not written back to Pi. To roll back, run with `--engine pi`; to
discard the native copies, remove `<data dir>/engine/` (and `~/.config/pipkin/engine/` for its settings and
credentials) after making sure nothing there is needed.

## Quitting and interrupted work

Save any important draft and wait for save confirmation before quitting. A visible save failure is not a durable
save. Do not delete databases or queue an extra copy of an uncertain prompt as a workaround.

Pipkin shuts down the engine it launched and owns, including same-user processes carrying that profile identity.
It does not own a separately started server selected with `--pi-dir` / `--pi-server-id`; that server can continue
work after the window closes. Neither case rolls back file writes, shell commands, commits or remote actions that
already happened. A detached/external process may require separate inspection.

On reopen, saved goals are paused rather than automatically resubmitted. The client reconciles its journal with the
engine's authoritative settlement; a lost acknowledgement does not mean the input never ran. Engine recovery can
repeat a partially executed tool even when Pipkin does **not** resend the prompt. Prompt deduplication is not a
universal exactly-once guarantee for external side effects.

For prolonged **Stopping**, an unresolved outcome, or a lost connection:

1. Keep the saved history and submission journal. Wait for reconnect/reconciliation; do not infer settlement from
   a timeout or from a spinner disappearing.
2. Inspect the actual project and any external effect (for example Git history or a deployment) before repeating
   work. Completed effects remain even when the instruction is retired.
3. Use supported reconnect/recovery controls when available. If none is offered or the state remains unresolved,
   collect build identity and the visible status; do not force-edit a journal row to manufacture success.
4. Quit/reopen only with the above interruption semantics understood. Restart is not an undo or proof of cancellation.

The previous-package procedure is in [packaging](packaging.md#upgrade-and-rollback). Quit all processes using the
profile before manually restoring data. Keep a private copy of current data first; restoring an older backup can
lose drafts/history written after it. A newer-schema refusal is a compatibility safeguard, not permission to delete
schema metadata. Test manual restore in a disposable profile before applying it to your working data.

## Diagnostics privacy

```sh
pipkin --version
pipkin --diagnose
pipkin --diagnose --probe
```

`--version` identifies the app revision and compatibility. `--diagnose` also reports paths, display variables,
database health, engine discovery and **a tail of engine output**. Home-directory substitution is not comprehensive
secret redaction: logs and arbitrary error messages may contain credentials, private endpoints, prompts or source
names. **Review and redact the report before sharing it.** Never upload `auth.json`, the database, full logs, process
environments or provider headers merely because someone requests “diagnostics”.

`--probe` starts an offline throwaway profile and checks a trusted handshake; it does not send a paid model request,
validate provider authentication, test every tool, or qualify the native desktop.

For an intentionally limited report, use **Ctrl K → Copy diagnostics**. The palette labels this as metadata only;
it copies app version/revision/dirty state, supported protocol/schema, build platform, real-versus-demo mode and the
configured launch-source manifest revision/protocol. It does not collect logs, arbitrary errors, paths, session IDs,
conversation/attachment text, credentials or environment values. This allowlist has automated hostile-manifest and
headless clipboard tests; native clipboard observation on the installed candidate remains separate.

The report is a launch-source snapshot, **not an attestation of the running server**. A development checkout without
an identified manifest, an external server, a missing source or a rejected source is marked unidentified rather than
guessed. It does not test database health, Node installation or provider authentication. Review even this minimal
report before sharing; use reviewed detailed CLI output only when the metadata report is insufficient.

## Reporting a bug or security concern

Use the [Pipkin issue tracker](https://github.com/last-refuge/pipkin/issues) for non-sensitive bugs. Include app/engine
identity, whether the artifact is dirty/unsigned, desktop/environment, exact reproduction, expected versus observed
behavior, and whether data/effects are uncertain. State whether you used a demo, scripted provider or real provider.
Include only minimal reviewed evidence. Never include secrets or a whole private session by default.

Do not publish an exploitable security issue or credential leak in a public issue. GitHub private vulnerability
reporting is enabled: use **[Report a vulnerability](https://github.com/last-refuge/pipkin/security/advisories/new)**.
See the [security policy](../SECURITY.md) for reporting, privacy and current fix/support scope. If the route is
unavailable, request a confidential route without disclosing exploit details. No response SLA is promised. Rotate
an exposed credential at its provider; deleting a posted log does not revoke the key.

An installed qualification build is not an automatic update channel. Retain the previous known-working package;
verify downloaded artifacts and follow the documented manual install/rollback procedure. Only an owner-approved,
signed and qualified candidate may become v1.
