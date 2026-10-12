# Radius gateway catalog discovery and recovery

Follow-up to `73979ae`, 2026-10-11. Protocol 11 / schema 9 unchanged.

The native auth service now refreshes authenticated `/v1/config` catalogs for the default
Radius provider and `models.json` custom Radius providers, including providers with no
models yet. The account credential goes only to its configured gateway. Public catalog
requests remain credential-free. Custom Radius providers also use the reference
`RADIUS_API_KEY` fallback when no stored or configured key takes precedence.

Gateway metadata is stored independently in `models-store.json`, without requiring a public
Last-Modified timestamp or applying the public four-hour freshness interval. Refresh restores
the previous catalog first and migrates legacy OAuth `gatewayConfig` even with `PI_OFFLINE`.
A direct coding request can rotate a token before catalog migration; the native credential
store retains the legacy metadata so this ordering cannot lose models. Cached metadata takes
precedence once present and survives credential removal/restart.

Gateway models overlay the appropriate Radius baseline, then local model definitions and
overrides apply. Custom gateways do not advertise the default gateway's static catalog. The
gateway response supplies the model execution URL; the configured gateway URL selects the
discovery/auth service. Publication shares the existing locked atomic catalog store and
generation/cancellation guards. Failures retain the previous catalog. Requests have a four-
second deadline and an 8 MiB response bound; error diagnostics omit remote response bodies.

## Evidence

Eight new tests in `auth/remote_catalog/radius/tests.rs` cover:

- Default/custom discovery with distinct credentials, empty initial custom catalog, local
  override precedence, metadata-only cache, restart and malformed model filtering.
- The Radius environment key for custom providers and repeated explicit refreshes.
- Direct token rotation before legacy migration, offline migration and restoration with no
  credential remaining.
- Denied/malformed/oversized responses, preserving cache bytes and redacting diagnostics.
- Superseding an in-flight refresh and cancelling a subsequent refresh without stale writes.
- Empty replacement catalogs and provider filtering for imported cache entries.
- A stalled response body bounded by the deadline and failed cache writes preserving both
  corrupt source bytes and the last usable in-memory catalog.
- Six executed pinned-reference sanitizer/converter cases, including custom provider remapping,
  preserved additional metadata and valid thinking-level mappings, filtering and bad envelopes.

`runtime::settings_tests::radius_discovery_supports_a_real_tool_turn_and_cached_resume_without_catalog_network`
uses the actual engine client protocol, local HTTP Radius adapter and built-in write tool.
It discovers/selects a model, completes a two-generation tool turn, verifies the written file,
restarts, then completes another provider turn using the durable catalog/history. Only one
catalog request occurs; no scripted provider runs. This is local provider simulation with
real native execution, not a live Radius account qualification or desktop walkthrough.

Reference capture:

```sh
node scripts/capture-radius-catalog-reference.mjs PI_SOURCE fixtures/native-radius-catalog/reference.json
```

The script verifies pin `d2a311097cbcf669e699479587332ae3988a49d0` and clean source, then executes
the original `packages/ai/src/providers/radius-config.ts` helpers. The fixture records source
SHA-256 and scope. Provider refresh/merging semantics were separately audited in original
`providers/radius.ts` and `models.ts`; these lifecycle paths are not an executed differential.

Required checks: **1518 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-radius-workspace.log`, `/tmp/pipkin-radius-clippy.log`,
`/tmp/pipkin-radius-fmt.log`. Initial test iterations corrected a nonexistent mock helper,
fixtures that wrongly assumed default models for a custom gateway or discovery URL as the
execution URL, and an invalid numeric thinking-level fixture (reference type is string/null).
No failed behavior was waived. Two prior token-refresh tests now define explicit test models
instead of depending on the incorrect default-gateway baseline for a custom gateway.

PRV-radius returns PA → NC with measured catalog/runtime evidence. This closes the discovered
implementation gap, not full provider or release qualification. Native remains opt-in. Copilot
login Retry-After parsing, remaining runtime assertions, sustained/installed/platform/package
qualification and native-default/Pi retirement remain separate work.
