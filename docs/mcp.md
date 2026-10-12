# MCP: connecting servers and executing their tools

The native engine supervises Model Context Protocol servers configured in settings, discovers
their tools lazily, keeps the catalog cached, and executes calls with bounded timeouts. The
model reaches every server through three compact built-in tools: `mcp_search`,
`mcp_describe` and `mcp_call`. This is MCP-1 of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md) (§4).

Status: implemented for the stdio transport and covered by automated tests against a
scripted fixture server (2026-10-12). No third-party server was exercised; see
[Boundaries](#boundaries).

## Configuration

Servers live under `mcp.servers` in `settings.json` (user level) or a project's
`.pi/settings.json`. The merged configuration applies; a project entry with the same name
replaces the user entry, and the `McpServers` view reports each server's scope.

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
        "keepAlive": false
      }
    }
  }
}
```

| Key | Meaning |
| --- | --- |
| `command`, `args`, `cwd` | The server process (stdio transport) |
| `env` | Variables for the process only: a literal, `$NAME` (the engine's environment) or `secret:<key>` (the private secrets file) |
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
working directory. Changing any of them stops the running process and discards its catalog;
the next use starts the new configuration. There is no previewed import of other tools'
`mcpServers` files yet and no automatic launch of executables found in a repository.

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
(protocol 19, capability `mcp`): name, scope, enabled, state (`stopped`, `connecting`,
`ready`, `failed`), last error, cached tool count, revision, command line, environment
references, idle policy and calls in flight, plus configuration warnings.

Children launched with the `worker` profile keep the three tools; `reviewer`, `scout` and
`auditor` do not (their frozen launch policies list only reads).

## Boundaries

- **Transports.** Stdio only. Streamable HTTP is next: the SDK's reqwest transport would
  add a second reqwest beside the engine's patched one and bypass its proxy/CA policy, so
  the plan is to implement the SDK's client trait over the engine's own HTTP client.
- **Authentication.** No OAuth; credentials are environment references and the private
  secrets file (MCP-2 adds a Connections page and token flows).
- **Protocol surface.** Tools only. Resources, prompts, elicitation, sampling and apps are
  MCP-3.
- **Not exercised.** Any real third-party server, pagination beyond what the SDK's
  `list_all_tools` does, and the per-session versus shared process question: processes are
  engine-wide and shared by every session whose settings name them.
- **Nothing native was verified.** There is no inspector for servers yet; the `McpServers`
  call is the only presentation.
