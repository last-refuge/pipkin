# Claude subscription handoff: native engine (Opus → Codex)

## Codex implementation update on 2026-10-11

The reserved native path is now implemented and undergoing qualification. See
[implementation and evidence](pipkin-rust-engine-evidence/claude-subscription.md) for the
pinned source, OAuth/request/import changes, tests and remaining live-account acceptance.
The user confirmed approval and directed the Pi port. The production default remains Pi.

**The original Opus handoff below is a historical snapshot.** Its absent-adapter descriptions,
reserved checks and credential-exclusion instructions are superseded by this implementation.
Current entry point: `crates/pipkin-engine/src/auth/oauth/anthropic.rs`, registered by
`auth/oauth/mod.rs`; the Anthropic adapter handles subscription requests and explicit import
includes OAuth credentials. Follow the evidence record for the remaining work, rather than
removing the old checks again. A subsequent native release smoke test passed browser sign-in and a real Claude Opus 5.5 coding turn (four scratch tests passed). Saved-sign-in restart, forced-expiry live refresh, copy-code login, and copied-Pi-fixture continuation with Stop/reopen subsequently passed. The copy-code desktop mapping and completion copy were fixed during validation. Installed interactive acceptance and full parity remain pending; see the evidence record.

---

This is the section 13 handoff from `llm-docs/pipkin-rust-engine-plan.md`. It is for Codex to finish the reserved Claude subscription integration (RE-03B / RE-13B) on top of the native Rust engine. It contains no credentials, account material, callback codes or private session data.

**Status of the reserved work:** the Claude subscription integration is **reserved for Codex — not implemented or qualified.** No subscription request, sign-in, refresh, import activation or model availability has been implemented, researched or tested by Opus. Everything below is the generic boundary it plugs into.

**Status of the main implementation:** see §2. Most non-reserved work is implemented and tested against generic fixtures; a list of remaining non-reserved gaps is given there, separately from the reservation. This handoff does **not** claim full parity, does not change the production default (Pi stays the default; native is opt-in with `--engine native`), retires nothing and publishes nothing.

## 1. Revision, working tree, build and run

| Item | Value |
| --- | --- |
| Repository | `/home/jason/coding/pipkin` |
| Branch | `native-engine` (local, not pushed) |
| Base | `main` at `75f5c35` (planning inspection revision) |
| Handoff head | see `git log -1 native-engine` (the commit that adds this file; earlier integrated heads: `e4fe919`, `9b64a99`) |
| Uncommitted | `llm-docs/pipkin-rust-engine-plan.md` — the owner's own edit, deliberately left unstaged and untouched |
| Worker branches | all merged into `native-engine`; their worktrees under `.claude/worktrees/agent-*` are disposable |
| Engine protocol | `PROTOCOL_VERSION = 2` in `crates/pipkin-engine-protocol/src/lib.rs` (2 since the explicit `Import*` calls merged) |
| Engine store schema | `SCHEMA_VERSION = 1` (`crates/pipkin-engine/src/store/schema.rs`) |
| Toolchain | Rust 1.99.0 (`rust-toolchain.toml`), installed user-locally with rustup (owner-approved) |

Another chat does not see this checkout's state automatically. Read `git status` and `git log --oneline main..native-engine` first.

Build and check:

```sh
export PATH=$HOME/.cargo/bin:$PATH
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

Run the desktop on an isolated native profile (the engine data and config go under the data dir; nothing touches `~/.pi`):

```sh
cargo run -p pipkin-app --release -- --engine native --data-dir /tmp/pipkin-native-try
```

Other native options: `--engine-host inprocess` (engine in the desktop process; for tests and benchmarking), `--engine-extension builtin:questions|<executable>` (repeatable). The headless engine: `cargo run -p pipkin-engine --release --bin pipkin-engine -- --data-dir D --config-dir C` (framed JSON over stdio; see `crates/pipkin-engine/tests/worker_process.rs`). The desktop runs the same engine as a private worker mode of its own executable: `pipkin --engine-worker --engine-data-dir D --engine-config-dir C`.

## 2. Readiness for the reserved integration, and other unfinished work

The native candidate builds and runs with the Claude subscription adapter absent. Reserved paths fail explicitly (§4.8). Independent native conversations (other providers, scripted tests) work.

Other work that is **not** finished, independent of the reservation (also tracked in the progress record and parity ledger):

- Parity ledger rows are not yet marked from the integrated evidence: most rows still say "not started (in progress)" from when the ledger was drafted, although the modules now exist and have tests. A ledger refresh is needed before beta.
- Remaining items reported by implementers as not ported or not verified are listed per module in their sections of the progress record. The notable ones: reasoning-token and 1-hour-cache-write usage counters are dropped because `Usage` has no fields for them; grammar/constrained-sampling tools and mid-conversation system messages are not expressible; payload/response hooks are not ported; some AWS/Vertex credential sources (SSO, MFA, external accounts) are not ported; Codex WebSocket sessions are not closed when a conversation ends and do not use proxies.
- Arbitrary TypeScript facet extensions (`--pi-extension`) cannot run natively (they need Node). That parity row stays `blocked` / `legacy bridge`; see `docs/native-extensions.md`.
- Performance: measured results and misses are in `llm-docs/pipkin-rust-engine-evidence/performance.md`. Two engine bugs it found were fixed afterwards (history scan on open; flush timing); single-session RSS growth over very long sessions remains open.
- Not done: guarded native GUI walkthrough with `scripts/guard.sh`; installed-package qualification on macOS; the diagnostics-correlation advantage; IME/screen reader/scale/suspend/cold-boot gates (unchanged: unverified).
- No live provider has been called. All provider evidence is from local mocks and from goldens generated by running the reference code.

## 3. Claude subscription parity rows (all reserved, unverified)

From `llm-docs/pipkin-rust-engine-parity.md` (section "Credentials/OAuth"). None is passed; generic fixtures and API-key tests do not count for any of them.

| Row | What must be preserved | Native status |
| --- | --- | --- |
| CLS-ONBOARD-BROWSER | Claude Pro/Max browser sign-in started from onboarding (`pi.provider-auth` `start("anthropic")`) | reserved — not implemented or qualified |
| CLS-ONBOARD-COPYCODE | Headless copy-code variant and manual code answer | reserved |
| CLS-CANCEL | Cancel / retry of a Claude sign-in attempt | reserved |
| CLS-REUSE | `reuse("anthropic")`: offline check of a saved subscription credential and model check | reserved |
| CLS-IMPORT | Explicit import of a stored `anthropic` OAuth entry | reserved (importer preserves and excludes it; §4.6) |
| CLS-REMOVE | `remove("anthropic")` | generic removal works; subscription-specific qualification reserved |
| CLS-REFRESH | Refresh under the store lock, concurrent refresh | reserved (generic lock exists; §4.4) |
| CLS-ENV-TOKEN | `ANTHROPIC_OAUTH_TOKEN` as a credential source | reserved (currently resolves to "reserved/unavailable", never to `ANTHROPIC_API_KEY`) |
| CLS-REQUEST | Subscription-specific request, identity, system, tool-name and beta behavior | reserved (adapter refuses before sending) |
| CLS-MODELS | Models available under a subscription | reserved |
| CLS-CONTEXT-CACHE-USAGE | Prompt caching, usage/cost and context behavior under a subscription | reserved |
| CLS-MIGRATION-ACTIVATION | Activating imported Claude-subscription conversations natively | reserved (they stay readable but blocked; §4.6) |
| CLS-DESKTOP | Onboarding Claude option and status on the native backend | reserved (UI shows it disabled with the reason) |
| CLS-BOUNDARY (Opus) | Explicit unavailable behavior | implemented; tests in §6 — proves the boundary only, not parity |

Also affected: P03/P04 provider rows `PRV-anthropic` (API-key paths are Opus's and implemented; the subscription half is reserved), onboarding rows, and the migration rows for anthropic credentials.

## 4. Where the adapter plugs in (exact modules and symbols)

All paths are relative to the repository root.

### 4.1 Provider registration and request/stream conversion

- `crates/pipkin-engine/src/provider/mod.rs`: `ApiAdapter` (one wire protocol; `fn api()` and `fn stream(StreamCtx) -> BoxFuture<Message>`), `Registry` (`builtin()`, `install()`, `get(api)`), `ProviderRequest` (resolved model, flat `system`, context `messages`, `tools: Vec<ToolSpec>`, `thinking`, `session_id` for cache affinity, `max_tokens`, `cache_retention`, `credential`, `headers` where `None` deletes, `base_url`, `timeout`, `env`, `options` for provider-specific knobs), `Credential` (`None | ApiKey | Bearer | OAuth { access, metadata } | Ambient`; `Debug` is redacted), `Emitter` (builds the final message and emits committed deltas; text/thinking/signature/redacted/tool-call helpers; `block_extra`/`message_extra` for opaque replay metadata), `StreamCtx` (`req`, shared `http` client, `events`, `cancel`, and `failed()` which maps a cancelled exchange to `Aborted`).
- `crates/pipkin-engine/src/provider/api/mod.rs`: `all()` lists every built-in adapter.
- `crates/pipkin-engine/src/provider/api/anthropic_messages.rs`: the Anthropic Messages adapter for API keys, bearer tokens and compatible gateways. The reserved branch is `RESERVED_SUBSCRIPTION` and its check in `stream()`: `Credential::OAuth` for provider `anthropic` returns an error message **before any request is built or sent** (test: `claude_subscription_credentials_are_reserved_and_send_nothing`). This is the extension point: replace the refusal with the subscription request path, keeping the API-key path unchanged. The module header lists the request options it reads.
- Shared helpers: `crates/pipkin-engine/src/provider/http.rs` (`send` with request-level retry and `retry-after`, `read_sse` with idle timeout and cancellation, `format_error` producing the reference `"<status>: <body>"` text), `provider/sse.rs` (fragmentation-safe SSE decoder), `provider/errors.rs` (whole-turn retry and context-overflow classifiers, ported verbatim; they match error *text*, so keep reference formats). Shared adapter helpers live in `crates/pipkin-engine/src/provider/api/common/` (`transform.rs` cross-provider replay normalization, `headers.rs`, `http.rs` with `post_json`/retry/idle timeouts, `thinking.rs` levels/budgets/`clampMaxTokensToContext`, `json.rs` tool-argument repair, `schema.rs` strict tool schemas); the cache-retention contract is documented at the top of `common/mod.rs`. The Anthropic adapter already uses them; a subscription request path should too.

### 4.2 Model capabilities

- `crates/pipkin-engine/src/provider/model.rs`: `Model` (reference model JSON shape: `api`, `provider`, `baseUrl`, `reasoning`, `input`, `cost` with tiers, `contextWindow`, `maxTokens`, `thinkingLevelMap`, `headers`, `compat` kept as raw JSON), `thinking_levels()`, `clamp_thinking()`, `thinking_value()`, `cost_of()` (1-hour cache writes at 2× input). Pinned catalog: `crates/pipkin-engine/model-data/` (hash-checked against the pinned manifest by `embedded_files_match_the_pinned_manifest`).
- `crates/pipkin-engine/src/runtime/models.rs`: initial model selection (reference default table), `models_view`, `thinking_view`.
- `crates/pipkin-engine/src/auth/models_json.rs`: `models.json` layering (custom providers, overrides).

### 4.3 Auth lifecycle and challenges

- `crates/pipkin-engine/src/auth/mod.rs`: `Auth` facade. Metadata-only `availability(&Model)`; per-request `resolve(&Model, &http)`, which may refresh; `AuthError::{NotConfigured, Unavailable, Failed}`; and the per-client lifecycle: `snapshot`, `start`, `reuse`, `answer`, `cancel`, `remove`, `set_api_key`, `client_gone`, `set_listener` (publishes `Event::AuthChanged` to that client only).
- `crates/pipkin-engine/src/auth/signin.rs`: attempt and challenge state, stale-attempt and expired-question guards, the reference user-facing texts. Failure snapshots never carry provider error text.
- `crates/pipkin-engine/src/auth/oauth/mod.rs`: `OAuthFlow` (`name`, `login(ctx, interaction)`, `refresh(ctx, credential)`, `to_auth(credential) -> ModelAuth`), `FlowCtx`, `Interaction`/`InteractionSink` (auth URL, device code, text/secret/select/manual-code prompts), `Endpoints::from_env` (test overrides), `BUILTIN_OAUTH_PROVIDERS`, `flow_for(provider, ...)`. Shared pieces: `oauth/pkce.rs`, `oauth/callback.rs` (loopback server, host override `PI_OAUTH_CALLBACK_HOST`), `oauth/device.rs`, `oauth/http.rs`.
- Non-secret generic example to follow: `crates/pipkin-engine/src/auth/oauth/openai_codex.rs` (ChatGPT subscription; browser PKCE + device flow) and its MockServer tests in `oauth/tests.rs`. A Claude flow would be a new `OAuthFlow` registered in `flow_for` and `BUILTIN_OAUTH_PROVIDERS`, plus removing the `anthropic` special-casing in `signin.rs` and `providers.rs` (§4.8).
- Protocol: `Call::{AuthState, AuthStart, AuthReuse, AuthAnswer, AuthCancel, AuthRemove, SetApiKey}`, `AuthSnapshot`, `AuthProviderView { available, unavailable_reason, kind, has_credentials }`, `AuthChallenge` (`crates/pipkin-engine-protocol/src/{api.rs,view.rs}`). The engine dispatches them in `crates/pipkin-engine/src/runtime/mod.rs` (`dispatch`).

### 4.4 Credential storage and refresh locking

- `crates/pipkin-engine/src/auth/store.rs`: `CredentialStore` / `StoreTxn` traits and `FileStore` (`auth.json`-compatible format under the native config dir, 0600 file in a 0700 dir, temp + fsync + rename, `flock` on `auth.json.lock` with a bounded wait, unknown and untouched entries preserved byte-for-byte). `OAuthCredential` keeps provider extras.
- Refresh: `Auth`'s per-provider `refresh_lock` (in-process mutex) plus the file lock; refreshed tokens are persisted atomically; a 5-minute validity margin and a bounded refresh timeout apply. A failed refresh returns `AuthError::Failed` and never falls back to another credential (tested with `XAI_API_KEY`).
- Secret Service/Keychain are not qualified; the private file is the current store (as in the reference).

### 4.5 Cancellation

`StreamCtx.cancel` (a `CancellationToken`): a cancelled exchange must end as `StopReason::Aborted` via `StreamCtx::failed`. Stop is durable before cancellation (`crates/pipkin-engine/src/runtime/session.rs` `Shared::stop`), and settlement retires the stopped inputs (`settle`).

### 4.6 Importer and migration hooks

- `crates/pipkin-engine/src/import/mod.rs`: `discover` lists stored credentials by provider and kind only, marking `anthropic` `oauth` entries as reserved (`is_reserved_credential`). `credential_entries` **excludes** them. Imported sessions keep their model selection (`anthropic/...`) unchanged.
- Explicit import calls (protocol v2): `Call::{ImportDiscover, ImportSessions, ImportConfig, ImportCredentials}` handled in `crates/pipkin-engine/src/runtime/import.rs`; the desktop flow is in `crates/pipkin-core/src/import.rs` and `crates/pipkin-app/src/adapters/native/import.rs` (palette commands with confirmation, native mode only, never at startup). Reserved Anthropic subscription credentials are reported as "kept for a later integration" and are **not copied** into the native store at all (not even as inactive entries); they remain only in the legacy `auth.json`. When the subscription adapter exists, extend `ImportCredentials` to copy them under explicit consent.
- Native readers mark such sessions instead of running them: `crates/pipkin-engine/src/runtime/mod.rs` `summaries()` and `runtime/views.rs` set `SessionSummary.blocked` when the selected model's credential is `AuthError::Unavailable`; a run then fails with that message (no API-key substitution, no retry on Pi).
- Desktop data for imported conversations: `crates/pipkin-app/src/storage.rs` `copy_conversations_from` (copies drafts, goals, journal, cache from the Pi namespace without changing the source).
- To activate: implement the subscription credential, then allow `credential_entries` to include it under explicit consent, and remove the reserved checks in §4.8. Legacy sources stay untouched in every case.

### 4.7 Desktop availability

- `crates/pipkin-core/src/onboarding.rs`: `SignInSnapshot.unavailable` (provider + reason), `unavailable_reason()`, `can_sign_in()`.
- `crates/pipkin-ui/src/shell/onboarding.rs`: `provider_choices()` disables an unavailable provider and shows its reason (test `a_reserved_claude_sign_in_is_shown_unavailable_with_its_reason_and_chatgpt_works`).
- `crates/pipkin-app/src/adapters/native/mapping.rs`: maps `AuthSnapshot` to `SignInSnapshot`; a reserved provider is never offered for reuse (test `a_reserved_claude_sign_in_is_reported_unavailable_and_never_offered_for_reuse`). `adapters/native/mod.rs` reports reserved errors without falling back.

### 4.8 Every reserved check to revisit

| Location | Current behavior |
| --- | --- |
| `crates/pipkin-engine/src/auth/providers.rs` `RESERVED_MESSAGE`, `KeyAuth::Reserved` (incl. `ANTHROPIC_OAUTH_TOKEN`) | Subscription credentials resolve to `Unavailable` |
| `crates/pipkin-engine/src/auth/signin.rs` (anthropic entry; never replaced implicitly by `set_api_key`) | Listed `available: false`; `start`/`reuse` → `Unsupported` |
| `crates/pipkin-engine/src/auth/mod.rs` (module docs; availability) | Stored anthropic `oauth` → affected models unavailable |
| `crates/pipkin-engine/src/provider/api/anthropic_messages.rs` `RESERVED_SUBSCRIPTION` | Refuses before sending |
| `crates/pipkin-engine/src/import/mod.rs` `is_reserved_credential` | Excluded from credential import |
| Desktop mapping/UI (§4.7) | Disabled with reason |

## 5. Reference source and overlays (location only)

- Reference engine: pin `d2a311097cbcf669e699479587332ae3988a49d0` (`packaging/pi-engine-revision`), from the pinned source repository named in `scripts/setup.py`. Stage it exactly as `scripts/build-engine.sh` does (`git archive` + `packaging/pi-onboarding.patch` + `packaging/pi-onboarding/*.ts` + `packaging/pi-subagents.patch` + `packaging/pi-subagents/*.ts` + the model-data archive). Overlay hashes: auth bridge `370dbef417d1cdeb68239f0d283d4dceb36f5ed539e500e0ba041238aad94e3a`, subagents `9fb846e55a931d829e2ca25bde08ae98d2f128c14ceb6f1bc033b0a2e6fb3664`.
- Model data: `packaging/pi-model-data-d2a311097cbcf669e699479587332ae3988a49d0.tar.zst` (sha256 `2be5f68d90695e71372c01ccbd08d5728b93691e07c45918846997c8949bcabb`, manifest `b92d631bb8bb2cac6f824a03a814baad9452164c518426b8bc4bbc5c2142bb05`), embedded unchanged in `crates/pipkin-engine/model-data/`.
- Claude subscription in the reference, **location only** (from `llm-docs/pipkin-rust-engine-audit/providers-auth.md` §1.1 and §4.1): `packages/ai/src/auth/oauth/anthropic.ts` (`anthropicOAuth` and its login/refresh functions); in `packages/ai/src/api/anthropic-messages.ts`: `isOAuthToken`, the OAuth branch of `createClient`, `claudeCodeVersion`, `claudeCodeTools` / `toClaudeCodeName` / `fromClaudeCodeName`, the OAuth entries of `getBetaFeatures`, the OAuth `system` branch of `buildParams`, and the `isOAuth` flag through `convertMessages`/`convertTools`. Pipkin's onboarding overlay: `packaging/pi-onboarding/provider-auth*.ts` (`SignInProvider = "anthropic" | "openai-codex"`). Opus did not read these for implementation.
- Applicable baseline tests: `packaging/pi-onboarding/provider-auth-provider.test.ts`; the desktop's Pi onboarding tests in `crates/pipkin-app/src/adapters/pi/` and `crates/pipkin-core/src/onboarding.rs`.
- Codex must recheck current provider requirements and the supported integration path from official sources. Nothing here pre-claims feasibility.

## 6. Fixtures, commands and results

Results at the handoff head (fill in by rerunning; the numbers below are from the last full run):

| Check | Result |
| --- | --- |
| `cargo test --workspace` | see `llm-docs/pipkin-rust-engine-progress.md` § "Handoff verification" |
| `cargo clippy --workspace --all-targets` | same |
| `cargo fmt --all --check` | same |

Generic boundary tests (not parity evidence): `claude_subscription_credentials_are_reserved_and_send_nothing` (adapter), the reserved-provider tests in `crates/pipkin-engine/src/auth/tests.rs` (`start`/`reuse` refused, stored subscription credential → unavailable with no API-key fallback, `remove("anthropic")` works, entry preserved), the importer's credential-exclusion test, and the two desktop tests in §4.7.

Fixtures and evidence:

- Reference session fixtures and next-request goldens: `fixtures/pi-sessions/` (8 scenarios; `scenario.md` in each; driver in `_driver/`); baseline numbers: `fixtures/pi-sessions/BASELINE.md`.
- Native performance: `llm-docs/pipkin-rust-engine-evidence/performance.md`, raw data `fixtures/native-bench/`, harness `crates/pipkin-engine/tests/perf.rs` (`cargo test --release -p pipkin-engine --test perf -- --ignored --nocapture`), `scripts/bench-native.sh`.
- Packaging: `llm-docs/pipkin-rust-engine-evidence/packaging.md`; `scripts/package-native.sh` (or `scripts/package.sh --engine native`) builds a no-Node candidate; `scripts/test-native-package.sh` installs it to a temp prefix and runs `pipkin --diagnose --probe --engine native` with a bare environment.
- Guarded native desktop walkthrough: `llm-docs/pipkin-rust-engine-evidence/native-walkthrough.md` (observed: Claude sign-in disabled with the reserved reason in the real window).
- Import equivalence: `crates/pipkin-engine/src/runtime/import.rs` tests import every `fixtures/pi-sessions/` scenario and check the next provider request against the reference `requests.jsonl`.
- Codex (ChatGPT) differential goldens: `crates/pipkin-engine/src/provider/api/openai_codex/fixtures/` (`regenerate.sh` reruns the reference code).

Rerun after adding the adapter: the full workspace checks; `crates/pipkin-engine/src/auth/**` and `provider/api/anthropic_messages.rs` tests; the importer tests (`crates/pipkin-engine/src/import/tests.rs`) including the real-fixture test with `PIPKIN_PI_SESSION_FIXTURES`; the desktop native e2e suite (`cargo test -p pipkin-app -- native::e2e`); onboarding UI tests; the performance harness if the request path changes; then packaging and installed-workflow checks.

## 7. Data ownership and rollback state

- Legacy profiles (`~/.pi/agent`, `~/.pi/server`) are never written by the native engine or importer. Import reads a SQLite backup copy and refuses locked or in-flight sessions.
- Native state lives in its own directories (desktop data dir `/engine` for sessions; `~/.config/pipkin/engine` or `<data-dir>/engine-config` for settings, `models.json` and `auth.json`). Desktop data is namespaced `native:<hash>` and never overwrites the Pi namespace.
- No Claude subscription conversation has been activated on the native engine; such sessions are shown as blocked with the reserved reason.
- There is no silent account or billing fallback: subscription credentials never become API-key requests, failed refresh never switches credentials, and a native failure is never retried on Pi.
- Rollback = run without `--engine native`. Pi remains the default and is unchanged. History written natively after an import is native-only.

## 8. Final sequence for Codex

1. **RE-03B feasibility review:** check the current supported Claude subscription integration requirements from official sources. Record the approach, or a specific blocker. If it is blocked, keep Pi for that path and report it; do not claim completion.
2. **RE-13B implementation:** a subscription `OAuthFlow` (§4.3); credential handling, refresh and removal on the existing store and locks (§4.4); request behavior in `anthropic_messages.rs` replacing the refusal (§4.1); model availability (§4.2/§4.3); usage, cache and context behavior; then removal of the reserved checks (§4.8).
3. **Affected migration and desktop integration:** credential import under explicit consent and activation of imported subscription conversations (§4.6); onboarding on the native backend (§4.7).
4. **Complete provider and fault suite:** golden request/stream tests, refresh concurrency, cancelled and retried login, expired token, removal, and the full workspace checks.
5. **Rebuilt package and native/account acceptance:** owner-authorized live sign-in and first reply, kept separate from fixture evidence.
6. **Beta, default and retirement gates** (plan §8 and the default-cutover checklist) only after every required parity row passes, including the remaining non-reserved items in §2.
