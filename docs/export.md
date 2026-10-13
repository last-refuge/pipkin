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
collector; see [Boundaries](#boundaries). Protocol 28, capabilities `export` and
`exportConfig`.

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

## Configuration (OBS-3)

Settings → **Trace export** writes the configuration through the engine
(`ExportConfigure`), which keeps it under `export` in the user settings file:

```json
{
  "export": {
    "enabled": true,
    "kind": "otlp",
    "endpoint": "http://127.0.0.1:4318/v1/traces",
    "content": "metadata",
    "serviceName": "pipkin-engine"
  }
}
```

- `enabled` defaults to false: a disabled exporter makes no requests at all. Enabling needs
  an `http(s)` endpoint. **Turn off** keeps the rest of the configuration.
- `content` is `metadata` (the first enabled level) or `content` (below). `kind` must be
  `otlp`. Anything else is a configuration error shown in the health view; nothing is sent.
- The collector credential is one header stored through the engine's credential store in
  its private `export-auth.json` (0600, locked, atomic writes) under the id `export`
  (`ExportSetCredential { header, value }`, `ExportClearCredential`). It is added to every
  post and never reported back: the health view names the header, not the value. A
  settings file that still carries `export.headers` values is honoured but flagged
  (`headersInSettings`), and the page suggests moving the value.
- The settings file is re-read on every exporter cycle; a change takes effect without a
  restart, and a credential change reaches the very next post.
- **Preview** (`ExportPreview { session?, content }`) returns a pretty-printed payload at
  either level, built from the open conversation's newest records (or a sample when it has
  none), already redacted, and sends nothing.

### Content levels

| Level | Adds | Never |
| --- | --- | --- |
| `metadata` | ids, names, timings, token counts, outcomes, bounded error texts | prompts, replies, tool bodies, environment, credentials, images, web page bodies |
| `content` | on the run span `pipkin.input` (the run's first prompt) and `pipkin.output` (its final reply); on each generation `gen_ai.completion`; on each tool span `pipkin.tool.arguments` and `pipkin.tool.output`; each cut at 4,096 characters | images, web page bodies, environment, credentials |

Every exported string passes through a redactor before it is enqueued: the stored
credential value and any header values from the settings file are replaced by
`[redacted]`, as are `Bearer …` / `Basic …` tokens, `sk-…` keys and `api_key|secret|token|
password = …` pairs. Provider API keys the engine resolves for models are not in its
inputs, so they cannot appear in a payload; nothing else is scanned for.

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

**Metadata by default.** At the `metadata` level payloads contain ids, names, timings,
counts, outcomes and bounded error texts; the `content` level adds the texts listed above.

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
`lastSuccessMs`, plus the configuration the page edits: `kind`, `content`, `serviceName`,
`endpointUrl` (without its query string), `credential` (the header name) and
`headersInSettings`. Header values are never reported back. The session dialog's Engine
diagnostics section shows it as one line (**Export**); the settings page shows the rest.

## Boundaries

- Nothing was sent to a real collector or a Langfuse project; the mapping is verified
  against a recording mock collector only.
- The settings page shows the credential value field as plain text while it is typed; the
  value is cleared from the field once stored and never shown back.
- No OS keychain: the credential file is the engine's private 0600 file (the same store as
  model credentials).
- No gzip, no OTLP/gRPC, no logs or metrics signals; traces only.
- Export state is per engine data directory; a different data directory starts from no
  cursor.
