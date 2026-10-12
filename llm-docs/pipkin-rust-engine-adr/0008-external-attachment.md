# ADR 0008: Explicit owner-authenticated local attachment

**Status:** Implemented on top of `e738037`; native evidence recorded in
[external attachment](../pipkin-rust-engine-evidence/external-attachment.md). Full parity
qualification and non-Linux runs remain open. Protocol 6 / schema 4 remain unchanged.

## Decision

Preserve explicit external attachment through an opt-in Unix socket host, separate from the
default private pipe host. `pipkin --engine-server <endpoint>` runs the same built-in engine
before GPUI starts, with explicit data/config roots. `--engine native --engine-attach <endpoint>`
connects a desktop without owning that server. There is no global discovery or public TCP port.

The endpoint directory is owner-only (created 0700), the socket is 0600, and both peers verify
kernel credentials against the effective UID (`SO_PEERCRED` on Linux, `getpeereid` on macOS).
This authenticates the local OS user, not an individual application. Same-user processes have
the engine user's privileges. No transport token or copied provider credential is required.

A locked owner file permits one endpoint owner. Reclamation removes only a stale socket;
live listeners and other file types are refused. Cleanup verifies the socket inode and never
unlinks the lock file. The owner releases the lock explicitly, so a CLOEXEC descriptor briefly
inherited by a concurrently forked tool cannot delay a restart. The store lock uses the same
owner-release rule; SQLite closes before its lock is released.

Every connection must send a bounded/version-matched Hello before any engine work. The engine
assigns a distinct client ID, auth scope and attachment ownership. Existing durable catch-up,
lookup, queue/Stop and first-answer-wins question behavior stay in the shared runtime.
External Shutdown is explicitly refused. Client quit closes only its socket; SIGINT/SIGTERM
from the server's owner stop the foreground server. Restart resumes durable state; it does
not emulate a separate Pi coordinator retaining per-session worker processes.

There are at most 32 connections, including pending Hello, with bounded input/output queues.
Hello has a 4096-byte / three-second total bound; socket writes time out after five seconds.
A client uses two bounded I/O threads; the engine retains one async runtime and one store
writer, independently of conversation count. Event bodies are now shared and encoded once across clients (ADR 0002); connection-specific
envelopes and replies are written per client. Broader queue/performance qualification remains open.

The desktop reconnects to the selected endpoint without launching a replacement server and
without consuming the owned-worker crash budget. Its local data namespace includes the
endpoint identity. `--diagnose --probe` targets that endpoint and performs only Hello and
ListSessions. Legacy Pi flags remain available during migration; the explicit native endpoint
is their native attachment equivalent, with documentation naming the different flags.

## Evidence boundary

Linux tests use real local sockets, two NativeBackend/core/storage harnesses, a foreground
`pipkin` binary without display variables, SIGKILL/restart, and owner SIGTERM. Permission and
symlink refusal, stale/live ownership, peer-UID decision, bounded admission/handshake, shared
runs/questions, and inherited lock descriptors have direct tests. Cross-user OS-account
integration, macOS execution, installed/live provider use, and full reference differentials
have not been performed. Windows keeps the platform-neutral private host; an external Windows
endpoint is not added here.
