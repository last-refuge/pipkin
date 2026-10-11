# ADR 0001: Execution topology

**Status:** Accepted (2026-10-11). Engine host and stdio worker loop implemented; desktop
supervisor, worker entry in the `pipkin` executable and the in-process vs child benchmark are
not yet implemented or measured.

## Context

The reference baseline runs four roles of one Node/Bun executable (audit `service-api.md` §4.1):
a client, a **coordinator** that owns the public and control Unix sockets
(`ce/coordinator.ts:301-328`), a **server** with the session catalog and services
(`ce/server.ts:768-809`), and one detached **session worker per open session**
(`ce/session-worker.ts:833-843`, spawned `detached: true`, `stdio: "ignore"`,
`ce/process.ts:41-71`). Pipkin launches `pi server` with `PI_EXPERIMENTAL=1`, `PI_SERVER_DIR`,
`PI_SERVER_ID` and finds its processes by profile identity
(`crates/pipkin-app/src/adapters/pi/engine.rs:174-199`, `macos_processes.rs`). Historical
measurements (not current baselines, plan §1 step 7) attributed 625–702 MiB aggregate engine
RSS to this tree, and the coordinator/worker tree requires `/proc` or kernel process inspection
to own and reap.

The plan (§3 "Execution topology") requires: no Node/Pi at runtime, crash isolation from the
UI, no global server discovery in native mode, no per-conversation processes, nothing on the
render thread, and the same engine API for an in-process host.

## Decision

1. **Production host:** a private worker mode of the same shipped `pipkin` executable, started
   by the desktop as an **owned child** before GPUI initialization. The desktop holds the
   child handle; the channel is the child's inherited stdin/stdout (ADR 0002). One engine
   child serves every conversation; switching conversations never starts a process.
2. **Engine as a library:** `pipkin-engine` is GPUI-free and transport-free
   (`crates/pipkin-engine/src/lib.rs`). Hosts drive it only through
   `pipkin-engine-protocol` frames via `host::Connection`
   (`crates/pipkin-engine/src/host.rs`). `host::run_stdio` is the worker loop: it serves one
   client until the channel closes or `Shutdown`, then shuts the engine down with a 10 s grace
   (`host.rs`, `run_stdio`).
3. **In-process host:** tests and the headless harness use `host::start` +
   `EngineHandle::connect` with direct channels — the identical contract. An optional
   in-process production host is a benchmark candidate only; switching requires a new ADR
   with same-fixture measurements against the child host.
4. **One async runtime** inside the engine with bounded concurrency
   (`EngineConfig::max_active_generations` = 8, `max_tool_processes` = 16,
   `crates/pipkin-engine/src/config.rs`); blocking SQLite/filesystem/process work is kept off
   the scheduler.
5. **Not a sandbox.** Tools run with the user's permissions, as in the reference
   (`docs/platforms.md` "Isolation"). No isolation claim is made.
6. External attachment and multi-client (P19, `--pi-dir`/`--pi-server-id`) are **not** served
   by the pipe; they stay on the Pi adapter until an opt-in owner-authenticated endpoint or
   shim is designed and tested (plan §3 "Contract and transport"). The P19 rows stay open.

## Consequences

- Crash isolation is kept (a worker crash does not take down the UI), with one child instead
  of a coordinator + server + N workers; owned-child reaping replaces environment/`/proc`
  sweeps in native mode.
- One engine process is a larger blast radius than per-session workers (plan §12 risk
  "Single native engine has excessive blast radius"). Mitigation: per-session fail-closed state
  (ADR 0003), bounded per-session work and supervisor restart. If measured isolation needs
  require it, revisit the topology.
- The reference's worker survival across a server restart (`discover_workers`,
  `ce/session-worker-manager.ts:151-173`) has no native analogue; engine restart resumes from
  durable state instead (ADR 0003). This is recorded in the parity ledger (row SVC-PROC-*).
- Performance targets (plan §7: ≤40 MiB idle, ≤250 ms warm readiness) are **not measured**.

## References

- Plan §3 "Execution topology", §7, §12.
- Audit `service-api.md` §4.1, §4.7.
- Native: `crates/pipkin-engine/src/host.rs`, `crates/pipkin-engine/src/runtime/mod.rs`
  (`Shared`, `EngineHandle`), `crates/pipkin-engine/src/config.rs`.
- Pi adapter (continuity path): `crates/pipkin-app/src/adapters/pi/engine.rs`,
  `crates/pipkin-app/src/adapters/pi/e2e.rs`
  (`the_app_owns_the_engine_restarts_it_after_a_crash_and_stops_it_on_quit`,
  `an_engine_the_app_did_not_start_is_never_stopped_by_it`).
