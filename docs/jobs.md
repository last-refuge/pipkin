# Background jobs

The native engine can run a command as a **job**: supervised outside the model turn, with a
durable record, an append-only private log, bounded reads, cancellation of the whole process
tree, and a completion that is delivered exactly once. Builds, test runs and dev servers no
longer hold the conversation. This is JOB-1 and JOB-2 of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md) (§9).

Status: implemented and covered by automated tests (2026-10-12). Nothing native was
observed; see [Boundaries](#boundaries).

## Tools

| Tool | Does | Replay after a restart |
| --- | --- | --- |
| `job_start` | Starts `command` through the configured shell in the session's directory (or `cwd`) and returns at once with the job id. Options: `name`, `timeout` (seconds), `wake` (follow up when it finishes), `task` (a plan task id to link) | Never: recorded as interrupted; the job row, if its intent was written, says `unknown` |
| `job_status` | One job by id, or every job in the session, with state, elapsed time and log size | Rerun (pure read) |
| `job_log` | A slice of the log by byte offset (default 8 KiB, at most 64 KiB), or the newest bytes with `tail` | Rerun (pure read) |
| `job_wait` | Waits up to `timeout` seconds (default 60, at most 600) for the job to finish, then reports its state and the end of its log | Rerun (pure read) |
| `job_cancel` | Stops the job's process group. Safe to repeat | Rerun (idempotent) |

Every result carries `details.job` (version 1) with the job's id, state, exit code, log
size and delivery. At most eight jobs run at once per session.

## Lifecycle

States: `starting`, `running`, `cancelling`, `succeeded`, `failed`, `timed_out`,
`cancelled`, `interrupted`, `unknown`. Exit code zero means `succeeded` and nothing more: a
server that is listening has not "succeeded", and a passing build is not a finished task.

A start writes the **intent** (the row, state `starting`) before anything runs, spawns the
process in its own process group with the `PIPKIN_JOB` marker (engine epoch, session, job
id, nonce), then writes the **receipt** (pid, start time). The process belongs to this engine
instance. Timeout and cancellation kill the group with SIGKILL at once, like `bash`, and
stragglers still carrying the marker are killed by marker, never by a pid read from a row.

Logs are private files under the engine's data directory (`jobs/<session>/<id>.log`, mode
0600) holding stdout and stderr in arrival order. Growth is committed and published about
twice a second. Beyond 8 MiB the rest is dropped and the log is marked truncated; the job
keeps running. Reads are by byte offset with a continuation offset.

## Completion delivery

Finishing is a durable event. Every connected client sees `JobsChanged` (the Jobs
inspector shows the job finished and counts it until the person looks). A **model-started
job with `wake: true`** additionally gets one follow-up message under the request key
`job:<id>:done`: the state, exit code, elapsed time, the last 2 KiB of the log and a
pointer to `job_log`. The key makes delivery idempotent: a restart that finds a finished job
with a pending delivery delivers it once, never twice.

Delivery is **suppressed** when the run that started the job was stopped by the person: the
result stays readable, but the model is not woken. User-started jobs (the `JobStart` call)
are notification-only; they never wake the model.

## Restart and shutdown

Jobs live as long as the engine process. An orderly shutdown stops every running job and
records it `cancelled` ("Stopped"). After a crash, rows still marked active are
reconciled on the next start: processes carrying an old marker are killed and the row becomes
`interrupted`; a row with no receipt becomes `unknown`, because the command may or may not
have started, and is never relaunched. Logs survive. Removing a session stops its jobs and
deletes its logs.

## Desktop

The inspector gains a **Jobs** tab when the engine reports jobs: active jobs first, finished
ones counted and listed on request, newest first. The tab label carries the active count and
a dot count of jobs that finished since the person last opened the tab. A job expands to
its command, directory, outcome, the tool call that started it, a linked plan task, and a
log tail fetched through the engine (refreshed when the log grows and on request), with
**Stop** for a running job. The demo backend runs no jobs and says so.

Clients use `Jobs`, `JobLog`, `JobCancel` and `JobStart` (protocol 22, capability `jobs`).

## Boundaries

- **JOB-3 is not built.** A job does not outlive the engine; "keep running after close" has
  no defined behaviour yet. An app-owned engine stopping stops its jobs.
- **No user-started jobs from the desktop yet.** The engine call exists; the inspector only
  lists, reads and stops.
- **Linux process matching.** Marker-based orphan cleanup reads `/proc`; on other systems a
  dead supervisor's processes are not found (the rows are still reconciled).
- **Nothing native was verified.** The Jobs tab renders in GPUI but has not been driven in a
  window.
