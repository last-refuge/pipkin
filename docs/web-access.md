# Web access: search, fetch and retained sources

The native engine offers every session three tools: `web_search`, `web_fetch` and
`web_read`. Searches go through a configured provider; pages are fetched inside a network
boundary and retained as engine-owned sources; the model reads retained text in bounded
slices and cites sources by id. This is WEB-1 of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md) (§5).

Status: implemented and covered by automated tests against a local fixture server
(2026-10-12). No live provider was called in those tests; see [Boundaries](#boundaries).

## Configuration

Search is off until a provider is configured in `settings.json` (user level, or a project's
`.pi/settings.json`), under `web.search`:

```json
{ "web": { "search": { "provider": "brave", "apiKeyEnv": "BRAVE_SEARCH_API_KEY" } } }
```

```json
{ "web": { "search": { "provider": "searxng", "url": "http://search.lan:8080" } } }
```

| Key | Meaning |
| --- | --- |
| `provider` | `brave` (API key) or `searxng` (self-hosted) |
| `apiKey` | The Brave key itself |
| `apiKeyEnv` | The name of an environment variable holding the key (preferred) |
| `url` | The SearXNG instance's base URL; for Brave, an optional endpoint override |
| `fallback` | An array of up to three further providers (same keys), tried in order when the one before fails |

Keys never appear in results, snapshots, history or logs. Without a provider,
`web_search` returns a typed `not_configured` error that names the setting; `web_fetch` and
`web_read` work regardless. Settings are read per call, so a change applies to the next
search.

Provider contracts, as pinned on 2026-10-12:

- **Brave Search API**: `GET https://api.search.brave.com/res/v1/web/search?q=…&count=…`
  with the `X-Subscription-Token` header; results under `web.results[]` with `title`, `url`
  and `description`, per Brave's "get started" documentation. `page_age`/`age` are carried
  verbatim when present; that documentation page does not describe them.
- **SearXNG**: `GET {url}/search?q=…&format=json`. JSON output must be enabled in the
  instance's `settings.yml` (otherwise it answers 403, reported as an `auth` error). Result
  fields `results[].{title,url,content,publishedDate}` come from SearXNG's own JSON output;
  its API documentation does not list them.

## Tools

| Tool | Arguments | Result |
| --- | --- | --- |
| `web_search` | `query`, `count` (1–10, default 5) | One line per result: `[s<n>] Title — URL (published …)` and the provider's excerpt; duplicates by URL dropped; an empty answer is a success |
| `web_fetch` | `url` | The source citation, final URL and redirect count, content type, bytes, retained characters, then the first 8 KiB of text with the `web_read` continuation |
| `web_read` | `source`, `offset`, `limit` (≤ 32 KiB), or `find` | A bounded slice by character offset, or up to 20 occurrences of `find` with context and offsets |

`details.web` (version 1) carries the structured outcome: `search`, `fetch`, `read`, or
`error` with a `kind` from `not_configured`, `auth`, `rate_limited`, `network`,
`invalid_response`, `blocked`, `status`, `unsupported_content`, `too_large`,
`too_many_redirects`, `timeout`, `not_found`, `not_fetched`, `expired`, `invalid`.

Excerpts are provider snippets, not page text; dates are only ever what the provider or the
page supplied (`article:published_time`, `<time datetime>`), verbatim. Summaries the model
writes are its own.

## The network boundary

Every fetch and every redirect hop is checked before any connection: only `http` and
`https`; no credentials in the URL; no loopback, private, link-local, carrier-grade NAT,
multicast, unspecified or documentation addresses, and no `localhost`, `*.local`,
`*.internal`, `*.home.arpa` or dotless names. Host names are resolved first and every
address must be public; the connection is pinned to the addresses that were checked, so a
name cannot resolve differently for the check and the connect. A cross-origin redirect that
carries credentials is refused, not followed with them stripped. At most 5 redirects.

A configured SearXNG instance may be on the local network: the search request goes to it,
but fetched URLs may never reach it or any other private target. The engine configuration
flag `web_allow_private_hosts` exists for the test fixture server only and defaults to off.

Bounds: 30 s per fetch, 20 s per search, 8 MiB download, `Accept-Encoding: identity`
(a compressed response is an explicit `unsupported_content` error rather than a decode),
only text, HTML, JSON and XML types are read. Decoding honours the response charset or a
`<meta charset>`, with UTF-8 and the Latin/Windows-1252 family supported and anything else
reported as lossy.

## Retained sources

Each search result and each fetched page is a source row (`web_sources`, schema 12) with
provenance: kind, provider, requested and final URL, title, excerpt, published date,
retrieval time, content type, byte count, SHA-256 and the originating call id. Fetched
pages keep their extracted text (up to 1 MiB of characters) and, for text and markup, the
original bytes (up to 2 MiB). A session retains content for at most 200 fetched sources and
64 MiB; beyond that the oldest content expires but the row stays, so a citation remains
meaningful and reading it says `expired` and asks for a new fetch. A call id that already
recorded sources returns them again instead of adding rows.

Sources live outside the transcript, so compaction never touches them and they survive a
restart. Clients can list a session's sources with the `WebSources` call (protocol 14,
capability `web`); content is read only through `web_read`. Removing a session removes its
sources.

Extraction is a dependency-free HTML pass: scripts, styles, templates, SVG, iframes and
comments are dropped, blocks become lines, list items get a dash, entities are decoded and
whitespace is collapsed. Nothing in a page is executed and no markup survives, so page
content is evidence the model reads, never an instruction the engine acts on.

## Boundaries

- No live provider was exercised; the Brave and SearXNG contracts are pinned from
  documentation and source as noted above and tested against a local mock.
- No PDF, OCR, screenshots, authenticated pages or browser automation (WEB-2).
- Citations: a session snapshot carries its retained sources (`webSources`, protocol 29)
  and the engine publishes `WebSourcesChanged` after each search or fetch, so a `[s<n>]`
  citation in a reply or a tool result opens its page in the desktop. A citation whose
  source the engine no longer lists stays plain text. The list carried is the newest 500.
- `web_read` is replay-safe; a search or fetch interrupted by a restart is recorded as
  interrupted (the model may call again), while a completed one keeps its sources under its
  call id.
- Fallback chain: `web.search.fallback` lists further providers (same shape) tried in order
  when the one before is down, refusing, rate-limited or answering nonsense; a cancellation
  stops the chain, and the last failure is the outcome. At most three fallbacks; a
  misconfigured fallback is skipped with a warning, a misconfigured primary is the error.
  Results name the provider that answered.
- Batch fetches: `web_fetch` takes `urls` (up to 5), fetched one after another, each its
  own source under a call-derived id (so a replay finds the same sources); a page that
  fails is reported in place and the rest go on; the call is an error only when every page
  failed.
