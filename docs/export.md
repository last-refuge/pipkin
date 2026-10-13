# Trace export

The native engine can post its [diagnostics records](diagnostics.md) to an OpenTelemetry
collector as OTLP/HTTP JSON traces. This is OBS-2 of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md) (§12):
an exporter interface fed from the engine's own durable records, with parent/child
correlation and usage from the committed ledger, bounded buffering, retry with backoff, a
drop policy and a visible health state. Ordinary execution never depends on it. OBS-3 adds
the settings page, content levels and credential handling; until then export is configured
by hand in `settings.json`.

Status: implemented and covered by automated tests (2026-10-12). Nothing was sent to a real
collector; see [Boundaries](#boundaries). Protocol 27, capability `export`.

## OTLP, and Langfuse through OTLP

The plan asked for OTLP to be evaluated alongside the documented Langfuse mapping. The
outcome: one exporter, OTLP/HTTP JSON (`POST <endpoint>` with a `resourceSpans` body), and
Langfuse reached through its own OTLP endpoint rather than a second protocol. Spans carry
both the OpenTelemetry GenAI attributes and Langfuse's observation attributes, so a Langfuse
project shows runs as agents, model exchanges as generations and tool calls as tools without
any Langfuse-specific code in the engine. Importing the TypeScript plugin was not needed.

| Collector | `export.endpoint` | `export.headers` |
| --- | --- | --- |
| Any OTLP/HTTP collector | `http://host:4318/v1/traces` | as the collector requires |
| Langfuse | `https://cloud.langfuse.com/api/public/otel/v1/traces` (or the self-hosted host) | `Authorization: Basic <base64 of public-key:secret-key>` |

## Settings (hand-edited until OBS-3)

```json
{
  "export": {
    "enabled": true,
    "kind": "otlp",
    "endpoint": "http://127.0.0.1:4318/v1/traces",
    "headers": { "Authorization": "Basic …" },
    "content": "metadata",
    "serviceName": "pipkin-engine"
  }
}
```

`enabled` defaults to false: a disabled exporter makes no requests at all. `kind` must be
`otlp` and `content` must be `metadata` (the only level in OBS-2); anything else is a
configuration error shown in the health view, and nothing is sent. The settings file is
re-read on every exporter cycle, so a change takes effect without a restart.

## What is exported

Each diagnostics record becomes at most one span. The trace id is derived from the root
session, the span ids from the engine's own identities, so ids are stable across restarts and
retries, and a collector that deduplicates by id sees one span per fact:

| Record | Span | Parent | Attributes |
| --- | --- | --- | --- |
| `run.end` | `run` (`langfuse.observation.type = agent`) | the parent session's `subagent` tool span for a child; none for a root run | `pipkin.session`, `pipkin.run`, `pipkin.request`, `pipkin.outcome`, `pipkin.queue_wait_ms`, `pipkin.stop_requested`, `pipkin.stop_latency_ms`, `pipkin.parent_session`, `pipkin.parent_call` |
| `provider.request` | `generation` | the run | `gen_ai.request.model`, `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`, `pipkin.cache_read_tokens`, `pipkin.cache_write_tokens`, `pipkin.attempt`, `pipkin.first_token_ms`, `pipkin.outcome`; status ERROR with the bounded provider message on an error |
| `tool.call` | `tool <name>` | the run | `pipkin.tool`, `pipkin.call_id`, `pipkin.outcome`, `pipkin.retained_bytes`, `pipkin.dropped_bytes` |
| `provider.retry` | `retry` (zero length) | the run | `pipkin.attempt`, `pipkin.delay_ms` |
| `run.resumed` | `resumed` (zero length) | the run | `pipkin.reason` |
| `run.start` | none (its facts are copied onto `run.end`) | | |

Span times come from the record's clock and duration. Token counts are the ones committed
to the usage ledger for that exchange. The resource carries `service.name` (default
`pipkin-engine`) and the engine version.

**Metadata only.** Payloads contain ids, names, timings, counts, outcomes and bounded error
texts. No prompt, message, tool argument or output, environment, credential or source text
is in them; the exporter has no access to anything but the diagnostics table.

## Delivery

- The spool is the diagnostics table (2,000 records per session). The exporter keeps a
  durable cursor per session in `<data dir>/export/state.json` and only considers records
  written after the moment export was enabled (`since`); enabling export never pushes
  history, and a restart resends nothing already sent.
- One batch of at most 200 records is in flight at a time, posted with a 10-second timeout.
  A failure is retried after 1, 2, 4, 8 seconds (never more than 60); after five attempts
  the batch is dropped and counted. Records that aged out of the spool before they were sent
  are counted as dropped too.
- The exporter runs on its own task. Model and tool work writes its records and pokes the
  exporter; nothing waits for the collector, and a collector that is slow, down or rejecting
  only changes the health view. Stop, streaming and commits are unaffected.
- Shutdown lets a post in flight finish within the engine's grace period and starts no
  other.

## Health

`ExportHealth` (and `ExportChanged` whenever it changes) reports `enabled`, `state` (`off`,
`idle`, `sending`, `backoff`, `failed`), the collector's host, `content`, `queued`, `sent`,
`dropped`, `failures` (consecutive attempts of the current batch), `lastError` (bounded) and
`lastSuccessMs`. Headers and the endpoint path are never reported back. The session dialog's
Engine diagnostics section shows it as one line (**Export**).

## Boundaries

- Nothing was sent to a real collector or a Langfuse project; the mapping is verified
  against a recording mock collector only.
- Metadata only. Content levels, the settings page, a payload preview, a disable control
  and credential storage are OBS-3.
- No gzip, no OTLP/gRPC, no logs or metrics signals; traces only.
- Export state is per engine data directory; a different data directory starts from no
  cursor.
