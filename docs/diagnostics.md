# Engine diagnostics

The native engine records bounded, structured facts about each session's work, keyed by the
identities it already has, so a person can see where time went and a later exporter can
correlate runs, requests, tool calls and children without scraping logs. This is OBS-1 of
the [native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md) (§12).
OBS-2 (an exporter interface) and OBS-3 (opt-in export configuration) build on it.

Status: implemented and covered by automated tests (2026-10-12). Nothing native was
observed; see [Boundaries](#boundaries). Schema 19, protocol 26, capability `diagnostics`.

## What is recorded

Every record carries the session, a per-session sequence number, the engine's clock, and
the run, request key and tool call id it belongs to. A child session's records carry its
parent session and the parent's `subagent` call id, so parent and child correlate by ids,
never by timing.

| Kind | When | Fields |
| --- | --- | --- |
| `run.start` | a run starts (a prompt, a queued follow-up, a child's task) | `queueWaitMs` (admission to start), `mode`, `inputs`, `parent`, `parentCall` |
| `provider.request` | a model exchange ends | `model`, `attempt`, `durationMs`, `firstTokenMs` (to the first streamed delta; absent when nothing streamed), `outcome` (`completed` / `error` / `cancelled`), `error` (bounded provider message), `tokens` |
| `provider.retry` | an error is classified as retryable | `attempt` that failed, `delayMs` before the next, `overflow` |
| `tool.call` | a tool call ends | `name`, `durationMs`, `outcome` (`ok` / `error` / `cancelled` / `interrupted` / `unavailable`), `retainedBytes`, `droppedBytes`, `replaySafe` |
| `run.end` | a run settles | `outcome`, `detail` (bounded), `durationMs`, `stopRequested`, `stopLatencyMs` (Stop requested to settlement) |
| `run.resumed` | a restart picks an open run up again | `reason: restart` |

Records never carry prompts, tool bodies, credentials, environment or source text; error
texts are provider messages cut at 200 characters. Each session keeps its newest 2,000
records (`MAX_PER_SESSION`), written in the transaction that commits the fact wherever one
exists (`run.start`, `run.end`, `provider.retry`) and otherwise right after it
(`provider.request`, `tool.call`); a record that cannot be written is logged and never
fails the work it describes. Records survive restarts with the session and go with it when
the session is removed.

**Provenance and what the figures mean.** Durations are measured by the engine around the
adapter's whole exchange and around the tool's execution, by the engine's own clock. They
are model and tool latency, not what the person saw: the desktop's input-to-frame figures
(`Log performance stats`) are a separate measurement and are never mixed in.

## Reading and exporting

- `Diagnostics { session, before?, limit }` returns a `DiagnosticsView`: a summary over
  every retained record (runs, cancellations, model requests, errors, retries, p50/p95
  request time, p50 first-token time, tool calls, errors and p50/p95, the longest queue wait
  and the longest Stop) and a page of records newest first, strictly before a sequence
  number, at most 500.
- `ExportDiagnostics { session }` writes every retained record as JSON lines to
  `<data dir>/diagnostics/<session>.jsonl` (each line: `session`, `seq`, `atMs`, `kind`,
  `run`, `request`, `callId`, `data`, `source: pipkin-engine`) and returns the path and count.
- An interrupted exchange (the engine stopped mid-stream) leaves no `provider.request`; the
  resumed run records `run.resumed` and then its own exchanges. Nothing is resent or
  reconstructed.

## In the desktop

The session dialog (`/session`, or **Session usage** in the palette) gains an **Engine
diagnostics** section: the summary rows, the newest records as one line each (bounded at
40), **Refresh** and **Export**. It is fetched when the dialog opens. The demo says it has
no engine diagnostics instead of inventing figures.

## Boundaries

- Nothing native was observed: the section in a window, both themes and narrow widths are
  unverified.
- `firstTokenMs` depends on the adapter streaming deltas; the scripted test provider does
  not, so the automated tests cover its absence, not its value.
- No per-record filtering in the dialog, no live refresh while a run is in progress, and no
  export location choice (the file goes under the engine's data directory).
- No remote export (OBS-2) and no configuration (OBS-3) yet.
