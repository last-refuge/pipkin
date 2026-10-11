# ADR 0007: Credential ownership and migration

**Status:** Accepted (2026-10-11). Configuration paths, the isolated-environment switch and
settings loading are implemented (`crates/pipkin-engine/src/config.rs`). The credential store,
resolution precedence, OAuth flows and import are **in progress** in `E/auth` and `E/import`
(in this tree `auth/mod.rs` is an interface skeleton: sign-in calls return `Unsupported`). The
Claude subscription path is **reserved for Codex — not implemented or qualified**.

## Context

Reference (audit `providers-auth.md` §4.0, §4.11, §4.12, §5; `prompt-settings-layout.md` §7):

- Credentials live in `<agentDir>/auth.json` (`~/.pi/agent/auth.json`, file `0600`, dir
  `0700`), shape `{providerId: {type:"api_key", key?, env?} | {type:"oauth", access, refresh,
  expires, ...extra}}`, cross-process `proper-lockfile` locking, `key` values may be
  `!command` or `$ENV` templates (`coding-agent/core/auth-storage.ts`,
  `resolve-config-value.ts`).
- Resolution order: explicit request key → runtime key → stored credential (owns the
  provider; no env fallback after it) → `models.json` `apiKey` → ambient env/AWS/ADC
  (`auth/resolve.ts:46-93`, `provider-composer.ts:417-481`). OAuth refresh inside
  `store.modify` (double-checked under lock), 15 s, not cancelled by the caller; failure is an
  error, not a fallback (`auth/resolve.ts:102-191`).
- Pipkin's onboarding overlay (`packaging/pi-onboarding/`, `pi.provider-auth`): providers
  `anthropic` and `openai-codex` only; `start/reuse/answer/cancel/remove`; per-connection
  attempts; error text never replicated; `credentialLookupFailed` fails closed; `reuse` refreshes
  with `allowNetwork:false`.
- Pipkin uses Pi's own directories (`docs/support.md`, `docs/platforms.md`): `~/.pi/agent`
  (or `--pi-agent-dir`), `~/.pi/server` (or `--pi-dir`).

## Decision

1. **Own roots.** Native engine state lives under Pipkin's data root (`EngineConfig::data_dir`);
   non-secret configuration (`settings.json`, `models.json`, global `AGENTS.md`, `skills/`,
   `prompts/`) under Pipkin's config root (`EngineConfig::config_dir`). The native engine never
   reads `~/.pi` implicitly (`config.rs` module docs). Platform path conventions are resolved
   by platform code (XDG on Linux; macOS Application Support), not hardcoded.
2. **Credential store:** until a qualified OS secret store (macOS Keychain / Linux Secret
   Service with explicit absent/locked handling) is implemented and approved,
   credentials are kept in an `auth.json`-compatible private file in the config root
   (`EngineConfig::auth_path`, mode `0600`, directory `0700`), same shape as the reference so
   explicit import is a lossless copy and `!cmd`/`$ENV` values keep working. This is a
   documented private-file fallback, not a claim of secret-store security; switching to an OS
   store is a separate, owner-approved step with migration.
3. **Metadata-only lookup:** availability (`Auth::availability`) never refreshes or touches the
   network; secrets never appear in logs, events, `Debug` output or journals (`Secret`,
   redacted `AuthSnapshot`, ADR 0002).
4. **Explicit import, no sync.** Importing credentials from a Pi agent directory is an explicit
   user action per provider, copying entries verbatim (including unknown extra fields);
   unsupported credentials stay untouched in the source; no continuous sync and no write-back
   to `~/.pi`. `models.json` and `settings.json` import is offered separately.
5. **Distinct identities:** provider id, credential kind (`ApiKey | OAuth | Ambient`,
   `view.rs::CredentialKind`) and billing path are distinct. An Anthropic API key never stands
   in for a Claude subscription credential; a failed refresh never falls back to a different
   billing credential; custom endpoints only receive their own configured credential.
6. **Reserved Claude subscription boundary.** The `anthropic` OAuth sign-in and every
   subscription-specific request behavior are reserved for Codex. The native engine:
   - lists the provider with `available: false` and an `unavailable_reason`
     (`AuthProviderView`), refuses `AuthStart{anthropic}` with `ErrorCode::Unsupported`
     (`auth/mod.rs` module docs) — no fake authentication, no dead-end flow, no `todo!()`;
   - treats a stored/imported subscription credential as making affected models
     **unavailable**, never silently using an API key or retrying on Pi;
   - preserves imported subscription credential references and conversations without
     activating them on the native engine (they stay Pi-owned) until RE-13B qualifies.
   Reference locations only: `ai/src/auth/oauth/anthropic.ts` (`loginAnthropic`,
   `loginAnthropicCopyCode`, `refreshAnthropicToken`, `anthropicOAuth`),
   `ai/src/providers/anthropic.ts` (`lazyOAuth`), `ai/src/api/anthropic-messages.ts`
   (`isOAuthToken`, OAuth branch of `createClient`, `claudeCodeVersion`, `claudeCodeTools`,
   `getBetaFeatures`, OAuth `system` branch of `buildParams`), overlay
   `packaging/pi-onboarding/provider-auth.ts:4` (`SignInProvider`).
7. **Generic lifecycle is primary work:** start/challenge/answer/cancel/reuse/remove,
   per-client attempt isolation and stale-attempt guards, refresh serialization per credential
   with atomic persistence, structured errors — tested with fictitious providers and local
   fixtures (no live accounts in automation).
8. **Test isolation:** `Env::isolated` disables ambient environment lookup (no ambient API
   keys or proxies) for qualification runs (`isolated_env_hides_the_process_environment`).

## Consequences

- Users who rely on credentials in `~/.pi/agent/auth.json` (including from classic-CLI logins
  for Copilot, OpenRouter, Kimi, Meta, Radius or xAI) must explicitly import them; nothing
  breaks silently, but the first native run asks.
- Two credential files may exist during migration; the native one is authoritative for native
  conversations only. Rollback does not move native credentials back.
- The private-file fallback is weaker than an OS secret store; the ledger row CRED-STORE stays
  open until a qualified store or an owner decision exists.

## References

- Plan §5 "Provider and credential work", "Boundary Opus must leave ready", §6.
- Audit `providers-auth.md` §4–5; `prompt-settings-layout.md` §7.2, §7.5.
- Native: `crates/pipkin-engine/src/{config.rs,auth/mod.rs,import/mod.rs}`,
  `crates/pipkin-engine-protocol/src/view.rs` (`CredentialKind`, `AuthProviderView`,
  `AuthSnapshot`), `api.rs` (`Secret`, `Call::Auth*`, `Call::SetApiKey`).
- Tests (pass, 2026-10-11): `config.rs` — `project_settings_override_global_and_defaults_apply`,
  `invalid_files_warn_and_fall_back`, `isolated_env_hides_the_process_environment`.
