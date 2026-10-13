# Live-provider checks (owner-run)

Every network-facing slice shipped against local fixtures: a mock Brave and SearXNG, the
`mcp_fixture` stdio server, a scripted model, a fake `secret-tool`, an in-process OTLP
collector. These checks run the real thing with the owner's credentials. Each is
**unverified** until a result is recorded below; a fixture pass is never evidence for it.
Keep credentials out of this file, the settings file you commit, and any pasted log.

Candidate: the commit named in [native-gate-testing.md](native-gate-testing.md) (v1
candidate section). Build with `cargo build --release -p pipkin-app`; the binary is
`target/release/pipkin`. Use a scratch data directory (`--data-dir /tmp/pipkin-live`) so a
check never touches daily-use history.

| # | Check | Setup | Steps | Pass when | Result |
|---|-------|-------|-------|-----------|--------|
| L1 | Brave Search | A Brave Search API key in the environment the engine inherits (`export BRAVE_SEARCH_API_KEY=…`); settings `{"web": {"search": {"provider": "brave", "apiKeyEnv": "BRAVE_SEARCH_API_KEY"}}}`. | Ask the model to search for something dated this week and cite a source. | The reply cites `[s<n>]`, the source list names `brave`, clicking the citation opens the page, and nothing in the transcript or `--diagnose` output shows the key. | unverified |
| L2 | SearXNG, and the fallback chain | A reachable SearXNG with JSON output enabled; settings `{"web": {"search": {"provider": "brave", "apiKeyEnv": "UNSET_VAR", "fallback": [{"provider": "searxng", "url": "http://<host>:8080"}]}}}`. | Search as in L1. | The first search reports Brave as unavailable and SearXNG as the provider that answered; a second search goes straight to SearXNG. | unverified |
| L3 | A real MCP server over stdio | A published server with no secrets (for example a filesystem server) in `mcp.servers` with `command`/`args`. | `mcp_search`, `mcp_describe` and one `mcp_call` through a prompt; then open the Servers page, Test and Reconnect. | The catalog lists the server's tools, the call returns the server's content, Test names the server and version, the idle reaper stops it after `idleSeconds`. | unverified |
| L4 | A real MCP server over Streamable HTTP with OAuth | A hosted MCP endpoint that uses OAuth (`url` plus `oauth.clientId` or dynamic registration). | Sign in from the Servers page, call a tool, sign out. | The browser flow completes on the loopback redirect (or the paste box), the token is stored in the selected credential store, the call succeeds, sign-out makes the next call ask for sign-in again. | unverified |
| L5 | Keychain credential store | gnome-keyring or KeePassXC providing the Secret Service, `/usr/bin/secret-tool` installed; settings `{"mcp": {"credentialStore": "keychain"}}`. | Repeat L4's sign-in, then `secret-tool search --all pipkin-store pipkin-mcp`; store a plain value with `secret-tool store --label=test pipkin-store pipkin-mcp-secrets pipkin-provider demo` and reference it as `secret:demo` in a server's `env`. | The item appears under `pipkin-store=pipkin-mcp` with the server's name as `pipkin-provider`; the `secret:demo` reference reaches the server; `mcp-auth.json` gains nothing; with the keyring locked the servers page reports a store error rather than silently using the file. | unverified |
| L6 | A live model as the goal auditor | A configured model with a key; settings `{"goals": {"auditorModel": "<provider>/<model>"}}`. | Set a small goal with one file criterion, `/goal audit on`, let the executor propose completion. | The audit child runs on the pinned model (the Subagents inspector shows the model), its verdict completes or pauses the goal with the stated reason, and `/goal cost` shows dollars accumulating when the model has catalog prices. | unverified |
| L7 | Trace export to Langfuse | A Langfuse project; Trace export settings page: kind OTLP, the project's OTLP endpoint, the Basic credential, content level `metadata` first. | Run two prompts, then look at the project. | Two traces with run, provider and tool spans, no prompt text at `metadata`, prompt text present only after switching to `content`; the page shows `healthy` with a non-zero sent count. | unverified |
| L8 | Trace export to a self-hosted collector | An OpenTelemetry Collector with the OTLP/HTTP receiver and a logging exporter. | As L7 with the collector's endpoint and no credential. | The collector logs the spans; stopping the collector turns the page `failed` with retries, restarting it drains the backlog with no duplicates. | unverified |

Record results here with the date, provider, model id, and the commit.
