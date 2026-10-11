# Native Claude subscription implementation

## Status and scope

Codex follow-up on 2026-10-11, based on `native-engine` at `852a5df`, with the native integration implemented. The Claude subscription path is implemented. Browser sign-in and a real coding turn passed in the native release binary on 2026-10-11; installed interactive acceptance and the remaining live paths are still unverified. Pi remains the default. This does not close the unrelated engine-parity gaps in the progress record.

The user confirmed approval in response to the integration question and explicitly requested a port of [Pi](https://github.com/earendil-works/pi). This records the user's statement, not independent verification of a provider agreement. The initial RE-03B review checked [Anthropic's developer guidance](https://code.claude.com/docs/en/legal-and-compliance#authentication-and-credential-use), [Agent SDK guidance](https://code.claude.com/docs/en/agent-sdk/overview), and the [subscription usage update](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan). The developer pages distinguish approved app integration from users signing into an unmodified Claude Code binary; the usage update alone does not document a native OAuth contract. Work proceeded following the user's confirmation and reference choice.

## Reference identity and attribution

Ported behavior is pinned to `d2a311097cbcf669e699479587332ae3988a49d0`, matching the existing engine baseline. The OAuth source fetched from `earendil-works/pi` is byte-identical to the pinned `jasona/pi` reference used by the build scripts.

| Input | SHA-256 |
| --- | --- |
| `packages/ai/src/auth/oauth/anthropic.ts` | `d28083819f29cd1b04f7527d3e3f9220c4fcc4e90f2fc8327aab6e1564684286` |
| `packages/ai/src/api/anthropic-messages.ts` | `9eced188fdef0ebbc39e3b3edb187a31940e269057168af05771edcb8ae4a632` |
| Upstream MIT license | `0457f5bcec3b3b211605dfb5d1a49042fd638f3686a410fe099c24a25af13c48` |

Source attribution appears in the Rust modules. The full notice is retained in `packaging/upstream-notices/pi/LICENSE` and installed by native packaging as `usr/share/licenses/pipkin/pi-MIT-LICENSE`. Package tests compare its contents with the source notice. No Pi/Node runtime dependency was introduced.

## Implemented behavior

- `auth/oauth/anthropic.rs`: browser login with a loopback callback, manual redirect paste if the callback is unavailable, copy-code login, PKCE/state checks, token exchange and refresh. Uses the existing per-client attempt/challenge lifecycle, cancellation and credential-store locking.
- `auth/{mod,providers,signin}.rs`: advertise Claude sign-in, reuse saved OAuth credentials, honor `ANTHROPIC_OAUTH_TOKEN` before API-key billing, serialize refresh, persist rotated tokens, preserve credential extras, remove saved credentials and reject implicit replacement by an API key.
- `provider/api/anthropic_messages.rs`: subscription Bearer authentication, pinned Pi request headers/betas/system blocks, cached instruction blocks, canonical wire tool names and conversion back to local tool names. Legacy API-key entries containing a Pi-recognized subscription token use the same path. API keys and Copilot retain their separate behavior.
- `import/mod.rs`: explicit credential import now includes Anthropic OAuth entries. The existing desktop confirmation and non-overwrite/source-preservation rules still apply. Imported credentials make their models available through the ordinary auth catalog; no import itself starts provider work.
- Existing desktop auth mapping enables the Claude option from engine capability metadata. Unavailable-provider presentation remains supported for other capability failures.

Two deliberate refinements: new credentials store actual expiry because the shared auth layer already applies Pi's five-minute refresh margin; token-endpoint errors exclude response bodies, URLs and token values. Existing imported expiry values are preserved. These are not claims of live-provider equivalence.

## Automated evidence

All new tests use isolated profiles, invented credentials and local mock servers. They do not contact a live provider. Assertions are derived from pinned source; a running TypeScript-to-Rust differential harness was not executed for this flow.

| Area | Tests |
| --- | --- |
| Browser callback, state rejection, PKCE exchange, client isolation and listener cleanup | `oauth::anthropic_browser_login_validates_state_and_exchanges_pkce` |
| Copy-code and URL/query/code paste forms | `oauth::anthropic_copy_code_accepts_reference_paste_forms` |
| Invalid state, cancellation, retry and stale answers | `oauth::anthropic_wrong_state_cancel_and_retry_never_save_stale_credentials` |
| Concurrent refresh across separate auth instances, rotation and metadata | `oauth::anthropic_refresh_is_serialized_persists_rotation_and_preserves_metadata` |
| Failed/invalid refresh, secret redaction and no API-key fallback | `oauth::anthropic_refresh_failure_is_redacted_and_never_uses_api_key_billing` |
| Reuse, model availability, removal and API-key replacement guard | `auth::subscription_credentials_are_reused_removed_and_never_swapped_for_api_keys` |
| Environment precedence | `aprov::anthropic_env_rules_and_subscription_token_precedence`, `auth::ordinary_anthropic_api_keys_work` |
| Subscription headers, betas, separate system instructions and caching | `anth::claude_subscription_uses_bearer_identity_betas_and_separate_cached_system` |
| Fragmented tool-call stream, local/wire names, historical calls and usage | `anth::subscription_tool_names_round_trip_with_history_and_cache_usage` |
| Header overrides, unauthorized response and cancellation | `anth::subscription_honors_header_overrides_and_cancellation_without_api_fallback` |
| Credential discovery/import, source preservation and desktop consent | `imp::credentials_and_config_are_listed_without_values`, `rti::credentials_merge_including_claude_oauth_without_overwriting_or_changing_the_source`, `ne2e::pi_credentials_are_imported_only_after_their_own_confirmation` |

Final verification of the uncommitted implementation on 2026-10-11:

| Check | Result |
| --- | --- |
| `cargo test --workspace` | **Passed:** 1,303 passed, 0 failed, 53 ignored. Includes 547 engine unit tests plus integration suites. |
| `cargo clippy --workspace --all-targets` | **Passed** |
| `cargo fmt --all --check` | **Passed** |
| `scripts/test-native-package.sh` | **Passed:** release build, scratch install/uninstall, bare-environment native probe, no Node/Pi invocation, exact MIT notice retained. No interactive GUI or provider request. |
| `git diff --check` | **Passed** |

Local diagnostic logs (temporary, not committed): `/tmp/pipkin-claude-workspace-tests-final.log`,
`/tmp/pipkin-claude-clippy-final.log`, `/tmp/pipkin-claude-format-final.log`, and
`/tmp/pipkin-claude-native-package.log`.

 Initial baseline workspace tests failed under the sandbox because local socket writes returned `Operation not permitted`; the same baseline passed when rerun outside that restriction. The first changed workspace run exposed an incorrect test expectation about credential display order; the test now preserves the source order (`openrouter`, then `anthropic`) instead of imposing a new order. The initial new-test compile also found an unnecessary `unwrap()` on the existing unit-returning cancel API, corrected before execution.

## Live native smoke test — 2026-10-11

**Observed / measured:** launched the current release binary with `--engine native`, an isolated
`--data-dir` and a scratch project. The user completed browser authorization. The isolated store
saved an Anthropic OAuth credential with mode `0600`; no token values were displayed. Anthropic
API-key, auth-token and OAuth-token environment variables were removed from the launched process.
The user also connected ChatGPT; the coding turn's persisted assistant messages specifically identify
`anthropic` / `claude-opus-5-5` / `anthropic-messages`.

Submitted through the pinned window using `scripts/guard.sh`: read the scratch README, fix
`slugify.py`, run `python3 -m unittest -v`, preserve the tests and Pipkin repository. The conversation
was rooted at the Pipkin repository selected by the user; the prompt and all file-changing tool
arguments explicitly targeted `/tmp/pipkin-claude-live-9hdfh3lz/project`.

The native engine completed three tool calls (`bash`, `write`, `bash`) and returned a final reply.
The scratch tests moved from **1 passed / 3 failed** to **4 passed / 0 failed**, confirmed by an
independent rerun. The request settled as `completed`; the final provider stop reason was `stop`.
Provider usage included cache-read and cache-write counters. This is not a billing assertion or a
full differential qualification. Sanitized machine-readable evidence: [live test](claude-subscription-live.json).

Pipkin was left running with the isolated profile at `/tmp/pipkin-claude-live-9hdfh3lz/profile`.
This profile contains real saved sign-ins and must not be committed or bundled as evidence.

## Live lifecycle checks — 2026-10-11

Using the same release binary in native worker mode and the isolated account profile:

- **Measured:** saved OAuth reused after a worker restart; a real request completed.
- **Measured:** forced the isolated credential expiry into the past under its file lock, then sent
  a request through the ordinary auth path. Both access and refresh tokens rotated, future expiry
  persisted at mode `0600`, and the real request completed. Token values were never displayed.
  This checks forced expiry, not passage of the natural token lifetime.
- **Measured:** copied the frozen `a-text-multiturn` Pi fixture, remapped its scratch cwd, imported
  it through `ImportSessions`, selected Claude Opus 5.5, and continued its earlier context.
  This uses public test history, not a private production Claude conversation.
- **Measured:** stopped an observed running `bash` call (`sleep 30`) in that imported conversation.
  It settled `aborted`; after restarting the worker, the stop outcome and retired instruction
  remained persisted, and a new real Claude reply completed.

Sanitized evidence: [lifecycle results](claude-subscription-lifecycle.json). The first refresh
harness assertion ran after metadata-only `AuthReuse`, before a provider request, and therefore
saw the old expiry. Corrected the harness to trigger resolution before checking token rotation;
no provider refresh failure was observed. A later sandboxed process check hid the running desktop,
leading the harness to open a duplicate test instance. Its worker was refused by the existing
engine lock and hit the restart cap. Host-visible process/lock inspection confirmed the original
worker was healthy; the duplicate was closed. This was a harness error, not an OAuth crash.
The user then found a real desktop bug: copy-code instructions appeared without an input.
`App/adapters/native/mapping.rs` accepted `code` but discarded the engine's `manual_code` challenge.
The mapping now accepts both. Regression test
`nmap::the_engine_manual_code_prompt_reaches_the_desktop_input` uses the engine's actual prompt
constructor and confirms the challenge, attempt, provider and placeholder survive mapping.
It passed before rebuilding and reopening the native UI for a fresh live attempt.

**Observed:** the user completed a fresh copy-code login, and the native UI displayed Connected.
Its completion message incorrectly directed the user to a model picker absent from the management
screen; that screen now directs the user back to the workspace. **Measured:** after restarting the
desktop with the saved copy-code credential, a guarded prompt returned `COPY_CODE_RESTART_OK` from
Anthropic `claude-opus-5-5` and settled completed.

Final integration checks: **1,304 tests passed, 0 failed, 53 ignored**; Clippy passed; formatting
passed after applying rustfmt to the new UI branch. Logs: `/tmp/pipkin-claude-final-tests.log`,
`/tmp/pipkin-claude-final-clippy.log`. The earlier 1,303-test result predates the new regression test.

## Remaining acceptance

- Model entitlement coverage beyond the tested Claude Opus 5.5 model; natural expiry over time (forced-expiry refresh passed). Browser and copy-code sign-in both passed live.
- Installed interactive onboarding and continuation of private production Claude history. The copied Pi fixture passed real Claude continuation and Stop/reopen through the release worker.
- Provider-specific differential execution against the pinned reference; full platform/native and performance qualification.
- The main conversion's independent gaps, including extension compatibility and other unqualified parity rows.

The earlier automated implementation checks used only invented credentials. The subsequent user-authorized live test used the sign-in saved by Pipkin; token values were not exposed. No default-engine change, publication or Pi retirement was performed.
