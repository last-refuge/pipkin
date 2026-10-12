# MCP: connecting servers and executing their tools

The native engine supervises Model Context Protocol servers configured in settings (stdio
processes and Streamable HTTP endpoints), discovers their tools lazily, keeps the catalog
cached, executes calls with bounded timeouts, and signs in to HTTP servers with OAuth. The
model reaches every server through three compact built-in tools: `mcp_search`,
`mcp_describe` and `mcp_call`. The desktop manages servers in Settings → MCP servers. This
is MCP-1 and MCP-2 of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md) (§4).

Status: implemented and covered by automated tests against scripted fixture servers (a stdio
process and an HTTP server with a scripted authorization server, 2026-10-12). No
third-party server was exercised and nothing native was verified; see
[Boundaries](#boundaries).

## Configuration

Servers live under `mcp.servers` in `settings.json` (user level) or a project's
`.pi/settings.json`. The merged configuration applies; a project entry with the same name
replaces the user entry, and the `McpServers` view reports each server's scope. A server is
a process over stdio (`command`) or a Streamable HTTP endpoint (`url`).

```json
{
  "mcp": {
    "callTimeoutSeconds": 120,
    "startTimeoutSeconds": 30,
    "servers": {
      "docs": {
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "/srv/docs"],
        "env": { "API_TOKEN": "$DOCS_TOKEN", "OTHER": "secret:docs-other" },
        "cwd": "/srv",
        "enabled": true,
        "idleSeconds": 300,
        "keepAlive": false,
        "disabledTools": ["delete_file"]
      },
      "remote": {
        "url": "https://mcp.example.com/mcp",
        "headers": { "X-Team": "docs" },
        "oauth": { "clientId": "$REMOTE_CLIENT_ID", "scopes": ["mcp:read"] }
      }
    }
  }
}
```

| Key | Meaning |
| --- | --- |
| `command`, `args`, `cwd` | The server process (stdio transport) |
| `url`, `headers` | A Streamable HTTP endpoint and headers sent with every request |
| `env` | Variables for the process only: a literal, `$NAME` (the engine's environment) or `secret:<key>` (the private secrets file); header values use the same forms |
| `disabledTools` | Tools switched off: never offered to the model or callable |
| `oauth.clientId`, `oauth.clientSecret`, `oauth.scopes` | A pre-registered OAuth client and the scopes to request; without a client id the engine registers one dynamically |
| `enabled` | `false` keeps the entry visible but never starts it |
| `idleSeconds` | Stop the process after this long without use (default 300) |
| `keepAlive` | Never stop it for idleness (stateful servers) |
| `callTimeoutSeconds` | Per-call reply timeout (default 120) |
| `startTimeoutSeconds` | Spawn-and-initialize timeout (default 30) |

The secrets file is `mcp-secrets.json` next to `settings.json` in the engine's
configuration directory: a flat `{"key": "value"}` object read only at launch. Resolved
values reach the child process and nothing else: views, logs, tool results and the catalog
cache show `$NAME` or `secret:key`, and an inline literal appears as `…`. A missing reference
is a start failure naming the reference. Server names are letters, digits, `-`, `_` and `.`,
at most 64 characters; at most 32 servers are managed. Invalid entries are reported as
warnings in the view and left out; the rest keep working. The process gets a minimal
environment (`PATH`, `HOME`, `LANG`, `TMPDIR`, `SHELL`, `USER` from the engine, then the
entry's `env`), never the engine's full environment.

Each entry has a **revision**: a hash of command, arguments, environment references and
working directory (or URL, header references and OAuth client). Changing any of them stops
the running server and discards its catalog; the next use starts the new configuration.
Switching tools off is not a revision change: it applies without a restart.

## Managing servers

Settings → MCP servers (desktop) and the engine's management calls (`McpUpsertServer`,
`McpRemoveServer`, `McpSetDisabledTools`, `McpTestServer`, `McpReconnectServer`,
`McpAuthStart` / `McpAuthAnswer` / `McpAuthCancel` / `McpAuthRevoke`,
`McpImportPreview` / `McpImportApply`; protocol 20, capability `mcpManagement`) edit the
settings files in place:

- **Add** writes an entry to the user's `settings.json` or the open project's
  `.pi/settings.json`. Values are given as `NAME=value` lines (environment for commands,
  headers for URLs); literals are stored but shown as `…` afterwards, and an edit that sends
  `…` keeps the stored literal. Invalid entries are refused before anything is written.
- **Test** connects afresh, lists the tools, and reports the server's name, version,
  protocol version and latency, or the failure.
- **Reconnect** stops the server, clears its backoff and starts it again.
- **Tools** lists the cached catalog with an On/Off switch per tool (`disabledTools`).
- **Sign in / Sign out** apply to HTTP servers (below). **Remove** deletes the entry from
  the scope it came from and stops the server.
- **Import** converts a pasted `mcpServers` block (Claude Desktop, Cursor, Pi and similar:
  `command`/`args`/`env`, or `url`/`headers` with `type`), shows each entry with its
  warnings (unknown keys left out, legacy SSE declared, invalid entries), and writes only
  the entries chosen. `${VAR}` and `${env:VAR}` become `$VAR` references. Nothing is
  launched by an import, and no repository is scanned for executables.

The engine publishes `McpChanged` whenever connection, sign-in or catalog state changes, so
the page stays current without polling.

## HTTP servers and sign-in

HTTP servers use the Streamable HTTP transport through the engine's own HTTP client, so the
engine's proxy, CA and idle-timeout settings apply to MCP traffic too. A `405` on the event
stream is accepted (request/response servers); session ids are honoured.

When a server answers `401`, its state becomes **Sign-in needed** and calls are refused with
`auth_required` until the person signs in. Sign-in follows the MCP authorization
specification on the engine's existing OAuth primitives: the `WWW-Authenticate` challenge
may name the protected-resource metadata (RFC 9728), which names the authorization server,
whose metadata (RFC 8414, then OpenID discovery, then the conventional paths) gives the
endpoints; without `oauth.clientId` the engine registers a client dynamically (RFC 7591)
for a loopback redirect URI; the authorization code flow uses PKCE (S256), a random
`state`, the challenge's scopes (or the resource's, or the configured ones), and the
server's canonical URL as `resource` (RFC 8707). The page shows the sign-in URL and a paste
box for the redirect URL when the browser cannot reach the loopback server.

Tokens are stored per server in the engine's private `mcp-auth.json` next to
`settings.json` (the same private-file store as model credentials; no OS keychain yet) and
are refreshed before they expire. A refused refresh makes the server **Sign-in needed**
again; nothing is ever sent with a stale token. **Sign out** forgets the tokens. Tokens,
header literals and environment literals never appear in views, events, logs, tool
arguments or results.

## Tools

| Tool | Does | Replay after a restart |
| --- | --- | --- |
| `mcp_search` | Matches a query against tool names and descriptions across enabled servers (or one `server`); lists `server/name` with a one-line description. Servers whose start fails are reported beside the results. Starts servers with no cached catalog. | Rerun (pure read) |
| `mcp_describe` | One tool's description, JSON schema and the server's annotations (shown as unverified hints) by `server/name` | Rerun (pure read) |
| `mcp_call` | Validates `arguments` against the cached schema, sends one request with the call timeout, forwards cancellation, returns the server's content blocks (text; images as a size note; others as JSON) plus structured content | Never: recorded as interrupted |

Every result carries `details.mcp` (version 1) with the resolved server, tool and catalog
revision, so policy, usage and traces see the actual operation. Error kinds:
`not_configured`, `unknown_server`, `disabled`, `start_failed`, `unknown_tool`,
`invalid_arguments`, `timeout`, `cancelled`, `transport`. A server's own failure
(`isError`) is reported as `tool_error` with its content, separate from transport failures.
Large results are bounded by the tool runner like any other tool output.

Uncertain outcomes stay uncertain. A timeout or lost reply is reported as "its effect is
unknown"; the engine never re-sends the request, and a call interrupted by an engine restart
is recorded as interrupted, never rerun (the model may decide to call again). Tool
annotations are carried as hints and never treated as proof that a tool is read-only or
idempotent.

## Lifecycle and catalog

A server starts on first use (a search without a cached catalog, a describe or a call) and
is stopped by a reaper after `idleSeconds` without use unless `keepAlive` is set. A call
in flight is never reaped. A failed start is remembered with its reason and retried only
after a 30-second backoff. On engine shutdown every running server is stopped.

The catalog (names, descriptions, schemas, annotations) is read once per revision, kept in
memory and persisted in `mcp-catalog.json` in the data directory, so after a restart a
search or describe answers without starting any process. A server's `tools/list_changed`
notification marks the catalog stale; the next search or describe lists again. Schemas over
64 KiB are replaced by a placeholder and descriptions are cut at 2,000 characters; at most
500 tools per server are kept; duplicate names within one server keep the first.

Clients read the configured servers and their state through `Call::McpServers { session }`
(capability `mcp`; `session` is optional and selects the project scope): name, scope, kind,
enabled, state (`stopped`, `connecting`, `ready`, `authRequired`, `failed`), last error,
the cached catalog with enablement, revision, command line or URL, environment or header
references, idle policy, calls in flight, sign-in state and the redacted settings entry,
plus configuration warnings.

Children launched with the `worker` profile keep the three tools; `reviewer`, `scout` and
`auditor` do not (their frozen launch policies list only reads).

## Boundaries

- **Transports.** Stdio and Streamable HTTP. Legacy HTTP+SSE servers are not supported
  (an import declaring `sse` is converted with a warning).
- **Credential storage.** The private file `mcp-auth.json` (mode-restricted like
  `auth.json`), not an OS keychain. Client-credential and enterprise-managed OAuth grants
  are not implemented; `secret` prompts from a flow are refused on the page.
- **Protocol surface.** Tools only. Resources, prompts, elicitation, sampling and apps are
  MCP-3.
- **Not exercised.** Any real third-party server or authorization server; pagination beyond
  what the SDK's `list_all_tools` does; the per-session versus shared process question:
  servers are engine-wide and shared by every session whose settings name them.
- **Nothing native was verified.** The MCP servers section renders in GPUI but has not
  been driven in a window; keyboard operation and project switching on the page are
  unverified.
