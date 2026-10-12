# Final focused parity assertions

2026-10-12, following `252938a`. This batch closes specific assertion gaps rather than
repeating the sustained-use or live-account workloads already recorded.

- `runtime::settings_tests::models_view_keeps_an_unavailable_selection_without_inventing_unknown_models`
  calls the engine Models protocol: a selected known model lacking credentials remains in
  the list once, unavailable with an explanation. An unknown model keeps its selection
  identity without inventing metadata. This matches `models-provider.ts::readCatalog` at
  pinned Pi `d2a311097cbcf669e699479587332ae3988a49d0` (source comparison).
- `runtime::fault_tests::configured_retry_cap_bounds_the_durable_runtime_deadline` verifies
  a 30,000 ms base delay is capped to configured 1,500 ms in the durable retry checkpoint;
  Stop then aborts without another provider request.
- `auth_redaction::auth_secrets_never_reach_worker_frames_or_debug_logs` runs the real
  `pipkin-engine` binary with an isolated environment and captures its complete outbound
  byte stream and debug log. Fixture API-key storage/removal, saved OAuth access/refresh
  reuse/removal, and a local OAuth provider failure all execute. No fixture API/access/
  refresh/provider-body secret appears in output or logs; the expected failure log proves
  logging was active. This does not claim every provider's live-account acceptance.
- `import::tests::imported_lookup_preserves_all_reference_failure_reasons_and_raw_records`
  imports reference-schema submissions for aborted, model_error, no_model, stale, reset,
  faulted, missing_task, task_too_old and migration_failed. Native lookup is terminal,
  prompt results preserve exact reasons, and raw submissions/source bytes remain unchanged.
  These table fixtures are source-derived; existing captured orphan/reference fixtures
  remain the separate executed-reference evidence.
- `adapters::native::e2e::vanished_attachment_rejects_native_submission_and_preserves_the_draft_for_retry`
  deletes a file after attaching it. Native submission refuses before any provider request,
  keeps text/chip, and an explicit retry after restoring the file succeeds once.

The corrected macOS hosted run `38260978574` passed all 850 engine library tests, protocol
and process tests, and all five app worker/diagnostic tests. It passed 94 native adapter
checks and failed only the external reconnect test. That test accepted a stale UI Ready
state before consuming the old socket disconnect. It now waits for the disconnect before
restarting the server and waiting for a new Ready; its complete local workflow passes.
The earlier production fixes are retained. Hosted rerun is still required; macOS stays
experimental. No physical-keyboard X11, signing, IME or suspend acceptance is implied.
