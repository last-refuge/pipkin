# External native attachment evidence

Built on `e738037`, protocol **6** / schema **4**. User contract and commands:
[docs/native-attachment.md](../../docs/native-attachment.md). Architecture:
[ADR 0008](../pipkin-rust-engine-adr/0008-external-attachment.md).

## Implementation

- Explicit private Unix endpoint with same-user kernel peer verification on server/client,
  owner lock, checked stale-socket reclamation and inode-checked cleanup.
- Foreground `pipkin --engine-server` runs without GPUI; the development binary offers
  `--listen`. SIGINT/SIGTERM are registered before the endpoint is advertised.
- `--engine native --engine-attach` and endpoint-specific desktop namespace. The external
  supervisor only disconnects; it never starts or kills the server. Server configuration is
  authoritative; conflicting owned-host/extension flags are refused.
- Mandatory bounded Hello before admission, per-client engine connections, explicit refusal
  of external Shutdown. Normal durable state, auth scoping, attachments, Stop and question
  semantics are shared with private/in-process hosts.
- External diagnosis probes the selected endpoint without model calls or server shutdown.

## Automated coverage

`ep::` = `E/endpoint/unix/tests.rs`:

1. `endpoint_clients_share_work_and_disconnect_without_owning_shutdown`: two socket clients,
   a gated model turn, disconnect, refused Shutdown, durable settlement and listener reopen.
2. `hello_is_mandatory_bounded_and_version_checked_before_any_session_call`: no admission
   before Hello, protocol mismatch, oversized header refusal without reading its body.
3. `unsafe_endpoints_and_foreign_path_types_are_refused_without_overwriting_them`: directory
   permissions, socket/lock/directory symlinks and ordinary-file preservation.
4. `single_owner_reclaims_only_stale_sockets_and_does_not_delete_a_replacement`: live listener
   refusal even without our lock, stale reclaim, exclusive lock and replacement preservation.
5. `two_socket_clients_share_questions_and_only_the_first_answer_wins`.
6. `a_silent_handshake_does_not_prevent_other_clients_or_owner_shutdown`.
7. `attachment_capacity_is_bounded_and_released_clients_allow_new_connections`: 32 admitted
   clients, overflow refusal and slot reuse.
8. `peer_identity_requires_the_current_os_user_and_handshake_deadlines_are_absolute`: UID
   acceptance/refusal decision and a trickled handshake with one absolute deadline. UID
   refusal is a direct helper test, not a run under another OS account.
9. `an_inherited_endpoint_lock_descriptor_cannot_keep_a_closed_owner_alive`.

Additional tests:

- `ne2e::external_desktops_share_runs_and_reconnect_without_owning_the_server`: two real
  NativeBackend/core/storage compositions over sockets. A starts work and quits; B stops it,
  continues, reconnects across an engine restart, then completes another turn. External
  diagnosis succeeds after both desktops quit.
- `ew::external::foreground_server_is_external_to_clients_and_recovers_its_stale_socket_after_sigkill`:
  actual `pipkin` foreground process with cleared environment and no display. Shared catalog,
  client Shutdown refusal, actual diagnostic subprocess, SIGKILL, stale endpoint reclaim,
  epoch advance/history preservation, successful owner SIGTERM and socket cleanup.
- Controller namespace/flag conflict and foreground server argument tests.
- `st::closing_a_store_releases_ownership_even_when_a_child_retains_its_descriptor` reproduces
  and fixes the inherited-descriptor lock lifetime described below.

## Verification record and limits

Initial focused socket tests and desktop/foreground tests passed. The first workspace run
failed a pre-existing store reopen assertion: `usage_optional_subsets_survive_reopen_and_later_legacy_records`
reported `Locked` after dropping its previous Store. A deterministic duplicate-descriptor
regression reproduced the underlying ownership problem: flock persists until every descriptor
sharing the open-file description closes. A concurrent fork can retain that description until
exec. Explicit parent-owner unlock now releases the store and endpoint even when such a
copy survives, while a new owner's lock remains protected. Failed logs are retained at
`/tmp/pipkin-attach-workspace.log` and `/tmp/pipkin-store-lock-repro.log`.

Required checks passed: **1385 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-attach-final-{workspace,clippy,fmt}.log`. All tests
use isolated fixtures/local sockets; no private credentials, live provider calls, desktop input,
OS configuration, or installed package changes were used. The tests do not claim a Pi/native
lifecycle differential, actual cross-user process rejection, macOS execution, an installed/live
GUI acceptance run, remote access, a Windows endpoint, or full fan-out/performance qualification.
