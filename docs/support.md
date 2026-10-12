# Support, data ownership and recovery

This page describes the current qualification build. Release acceptance and the native install/recovery gates are
still open. Do not treat automated socket/installer checks as proof of a clean desktop workflow.

## Data and ownership

| Location | Owner / contents | Sharing expectations |
| --- | --- | --- |
| `$XDG_DATA_HOME/pipkin` or `~/.local/share/pipkin` | Pipkin preferences, drafts/attachment references, submission journal, goals and cached conversations in `pipkin.sqlite3`; engine output in `engine.log` | Private: can contain prompt text, tool output, source paths and secrets |
| `<data dir>/pipkin.sqlite3.bak-vN` | Pre-migration backup for schema N | Same sensitivity as the database; does not include later drafts |
| `<data dir>/engine` | Native durable sessions and artifacts | Private; back up the complete directory |
| `~/.config/pipkin/engine` (or explicit data directory) | Native authentication/configuration | `auth.json` may contain API keys/OAuth tokens; never upload it |
| `<agent dir>/experimental/sessions/<session ID>/` | Authoritative experimental Pi sessions: `meta.json` and worker-owned `session.sqlite` | Private; preserve this directory for session recovery, not just the server sockets/profile |
| Installation prefix / version directories | App with built-in engine, public build identity and notices | Not your draft/session storage |
| Project directory | Actual files changed by tools or other programs | Changes survive Stop, quit and uninstall |

Pipkin data-directory precedence is `--data-dir`, `PIPKIN_DATA`, `XDG_DATA_HOME/pipkin`, then
`~/.local/share/pipkin`. Explicit `--data-dir` / `PIPKIN_DATA` also isolates the native engine's
configuration and credentials under that directory. Use a separate project for destructive tests.
Historical Pi directories are import sources only; `--pi-agent-dir` selects that source explicitly.

Draft attachments are saved as file references/metadata, not a promise that the original file will exist forever.
Do not delete originals before checking the restored draft. Keep backups private and include both app and engine
state if you need full recovery; a desktop-cache or server-coordination-profile copy alone is not an engine-session
backup. An externally started server may use an explicit session-directory override; verify its configuration rather
than assuming the defaults.

### Native engine and Pi import

`pipkin --engine native --pi-agent-dir /absolute/pi/profile` selects the source for both
Pi conversation imports and provider settings/credential imports. It overrides the engine's
`PI_CODING_AGENT_DIR` / `~/.pi/agent` discovery default; it does not change native storage or
import anything at startup. Imports still require their normal confirmation. With
`--engine-attach`, this source path is read by the attached engine. Omit the flag to use the
engine's default source. Pi coordination profiles are read only when importing historical plugin metadata.

For an independently owned native server shared by multiple desktops, see
[Native engine attachment](native-attachment.md).

Pipkin runs its built-in engine by default; `--engine native` is an optional alias. Its sessions live in `<data dir>/engine/`
(`engine.sqlite` and artifacts) and its configuration and credentials in `~/.config/pipkin/engine/`
(`auth.json` is `0600`; with `--data-dir`/`PIPKIN_DATA` both move under that directory). It never reads Pi's
directories on its own, and nothing is imported at startup.

Importing from Pi is explicit, from the command palette, and only in native mode:

- **Import conversations from Pi…** reads the selected Pi agent directory (`--pi-agent-dir`, else
  `$PI_CODING_AGENT_DIR` in the engine's environment, else `~/.pi/agent`), shows what can be imported and what cannot (sessions in use by a running Pi,
  sessions that cannot be represented without guessing, with reasons), and imports only after you choose
  **Confirm: import…**. Each Pi session is copied consistently into `<data dir>/engine/imports/<UTC time>/<session>/`
  first and read only from that copy. The desktop drafts, prompt history, unresolved requests and saved transcript
  copies of the imported conversations are copied (not moved) from the Pi namespace of `pipkin.sqlite3`. Running it
  again is safe: already imported sessions are skipped.
- **Import Pi provider settings and credentials…** is a separate confirmation that lists provider ids only (values
  are never shown or logged). It copies `models.json` and `models-store.json` when present and merges the selected `auth.json` entries into the native
  credential store without overwriting anything already set up there. Supported Anthropic subscription (OAuth)
  credentials retain their credential kind; they never become API keys.

`httpIdleTimeoutMs` controls the native shared HTTP header/body idle timeout in global
`settings.json`; zero or `"disabled"` disables it. Invalid values, including explicit `null`,
report a configuration error before affected network requests. Correct the file and retry;
new model requests, model refreshes and sign-in attempts reload network settings. In-flight
operations keep their existing snapshot. Project settings can set the provider request timeout,
and `retry.provider.timeoutMs` takes precedence there; neither overrides an invalid global
shared-HTTP setting.

Native model refresh restores cached public catalogs from `~/.config/pipkin/engine/models-store.json`
and revalidates configured built-in providers against the public catalog service. Local `models.json`
overrides still apply. A refresh failure keeps the usable catalog and reports a warning. Set `PI_OFFLINE`
(even to an empty value) to disable catalog networking while retaining embedded/cached models;
this does not disable ordinary coding requests to a selected provider. Builds and startup do not
require a live catalog service. Radius continues using its credential-dependent gateway catalog.

Anthropic workload identity federation uses `ANTHROPIC_FEDERATION_RULE_ID`,
`ANTHROPIC_ORGANIZATION_ID`, and `ANTHROPIC_IDENTITY_TOKEN_FILE`. Optional
`ANTHROPIC_SERVICE_ACCOUNT_ID` and `ANTHROPIC_WORKSPACE_ID` select the service account and
workspace. The engine rereads the identity-token file when exchanging an expiring token;
projected-token file rotation is supported. Configure an HTTPS Anthropic endpoint (loopback
HTTP is allowed for local development). Explicit API keys, bearer tokens, subscription
credentials and configured auth headers keep their existing precedence. Federation tokens
are cached in memory, independently of Claude subscription sign-in.

Conversation import preserves Pi's plugin selections from `$PI_SERVER_DIR`, or `~/.pi/server` by default.
For a custom coordination directory, launch native Pipkin with `PI_SERVER_DIR=/absolute/path`; protocol clients
can supply `serverDir` on both import discovery and import execution. The agent directory alone does not contain
these selections. Pi extensions are not supported or activated by migration. Profiles from every server identity,
including old paths and unknown formats, are retained byte for byte (base64 plus SHA-256) in the native import
manifest. Conversation history and source database state are preserved separately in the SQLite backup.
Imported conversations start with an empty native extension profile, regardless of native server defaults.
Unreadable or oversized profile files, or profiles changing during the copy, still block import to protect source
state. Close Pi before importing. Pipkin never changes source profiles or executes their package paths.

Pi forked histories import with the inherited history visible at each branch's cutoff.
An ownerless branch becomes an independent native conversation; subagent branches retain
their task ownership. Settings already copied by Pi at the fork point remain branch-specific.
The import manifest retains the original ancestry, source entry counts and materialized
native entry counts. The original database and documents remain in the backup. Import
does not add a command to create new forks in Pipkin.

Ownership: a conversation is written by exactly one engine. When an import finishes, Pipkin records in its own
database (`pipkin.sqlite3`, table `conversation_owners`; never in the Pi profile or the engine's data) that the
built-in engine now owns each conversation it imported, in the same transaction as the desktop-data copy. Sessions
that were reported but not imported (in use by a running Pi, or not representable) are not recorded and stay Pi's.
Conversations imported in an earlier run are not recorded again.

Current Pipkin writes imported conversations with the native engine. The Pi runtime and `--engine pi`
are retired. Ownership records remain for safe rollback to an older Pipkin installation. Those older
versions can display imported conversations read-only and offer their explicit release-back-to-Pi
confirmation. No current command migrates native history back into Pi.

Rollback never changes the preserved Pi source. To use it again, explicitly reactivate a retained older
installation and follow its ownership-release procedure. Keep complete native backups first.
History written in an imported conversation after the import exists only in the native engine; it is not written
back to Pi. Native transcript rows display **Native-only history · not in the Pi copy** for that work;
subagent previews carry the same distinction. Imported rows retain their original text and item IDs.
The labels also survive newly saved offline copies. Older saved copies have no provenance label until
refreshed from the native engine. These labels are presentation metadata, never added to model prompts.

Keep the known-good package and archive the complete `<data dir>/engine/` directory after quitting all
Pipkin processes before rollback or deletion; retain the separate native settings/credentials directory
if you need to restore sign-in. Reopening that archive with a compatible native package preserves access
to the native history. The saved desktop transcript is a bounded reading cache, not a complete archive.
Rolling back does not undo changes made to project files.

Make the archive private (for example, set `umask 077` before creating it), and preserve directory/file
permissions when restoring a trusted archive (`tar --extract --preserve-permissions`). Keep the current
directories aside until the restored profile opens successfully. Restore to the same locations: saved
projects, imported-source references and some artifact links use absolute paths. Project files and Pi
source directories need their own backups. Temporary bash spill paths can expire independently; the
native database retains complete tool output up to the displayed retention limit, available through the
tool-output copy/save action. Archiving the engine does not preserve arbitrary files under `/tmp`.

## Quitting and interrupted work

Save any important draft and wait for save confirmation before quitting. A visible save failure is not a durable
save. Do not delete databases or queue an extra copy of an uncertain prompt as a workaround.

Pipkin shuts down the engine it launched and owns, including same-user processes carrying that profile identity.
It does not own a separately started server selected with `--engine-attach`; that server can continue
work after the window closes. Neither case rolls back file writes, shell commands, commits or remote actions that
already happened. A detached/external process may require separate inspection.

On reopen, saved goals are paused rather than automatically resubmitted. The client reconciles its journal with the
engine's authoritative settlement; a lost acknowledgement does not mean the input never ran. Engine recovery can
repeat a partially executed tool even when Pipkin does **not** resend the prompt. Prompt deduplication is not a
universal exactly-once guarantee for external side effects.

The native engine saves whole-turn retry deadlines and the number of failed attempts. Restarting keeps the remaining
wait and retry limit; an expired deadline proceeds without another full delay. Stop cancels a restored retry wait
and retires the instruction. Native engine schema 2 upgrades schema 1 only after writing a private
`engine.sqlite.v1.bak` beside the database. Older binaries refuse the newer schema; keep the backup if rollback is
needed. Schema 1 did not persist retry deadlines, so an upgrade cannot restore a prior deadline that was never saved.

Schema 3 adds a durable event catch-up log and first saves `engine.sqlite.v2.bak` when upgrading
schema 2. Slow native clients coalesce streamed presentation, reread saved state, and reconcile
pending requests; they do not resend prompts to recover missing updates. This preserves loaded
history and the selected child conversation. Schema 4 adds positional system-message bodies and
saves `engine.sqlite.v3.bak` before upgrading schema 3. It preserves ordered instruction and tool
updates across restart/import. Protocol 5 rejects mismatched older workers.
Native schema 10 distinguishes later tool invocations that reuse a provider's tool-call ID.
The engine retains separate durable output/recovery identities and restores the provider's
original IDs when sending context. Upgrading schema 9 first writes `engine.sqlite.v9.bak`;
older engines refuse schema 10. To return to an older native engine, quit all profile users,
retain a complete current archive, and restore a compatible pre-upgrade profile. Switching
binaries alone does not downgrade data or transfer later work into the backup.

On Linux/glibc, the private worker limits allocator arenas and releases unused pages at run
boundaries to avoid retaining large temporary context buffers. This does not shorten saved
history or change the machine's allocator environment/configuration.


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
