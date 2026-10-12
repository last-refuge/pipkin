# Native extension profiles — 2026-10-11

## Identity and boundary

Native base `9047b57`, branch `native-engine`; this change adds protocol **7**, schema **5**.
Measured on Linux through real engine/store/protocol tests with scripted providers. No GUI,
production profile, live provider or TypeScript facet package was exercised in this batch.
Native remains opt-in; Pi default/retirement qualification is not claimed.

## Behavior implemented

- Engine defaults persist in SQLite meta. Omitted launch selections restore them; explicit
  selection replaces them; `--engine-clear-extensions` deletes the default. Existing session
  pins survive a default change or clear.
- Root sessions pin their ordered selection on first acquisition, including empty. Explicit
  `PrepareExtensions` validates/persists without loading a runtime or activating a package.
  It accepts an identical active selection and refuses a different one with Pi's conflict text.
- Preparation and first acquisition serialize with runtime retirement. After retirement a new
  selection can activate; questions from the old generation cannot answer the replacement.
- Process profiles keep argv boundaries and canonical executable identity, including PATH
  resolution at preparation. Missing/nonexecutable files, unknown builtins, duplicate runtime
  IDs, invalid/future profiles and invalid clear/select combinations refuse without replacing
  prior valid selections. Preparation does not verify a process handshake or compile packages.
- First-pin storage failure retains the chosen selection and fails the session closed. Explicit
  reopen retries that pin before allowing work, even if the default changed in the meantime.
- Plugin-free imported roots pin empty in the same transaction as imported history. Opaque Pi
  state stays preserved; nonempty unsupported Pi selections still refuse import.
- Schema 4 upgrades receive the existing pre-upgrade backup, retain opaque kv values, and move
  to schema 5. The version change prevents older engines from silently ignoring profile pins.

## Executed reference comparison

Reference revision `d2a311097cbcf669e699479587332ae3988a49d0`.
`packages/coding-agent/src/experimental/plugins/package.ts` SHA-256:
`ea626c8a56aa2bf81b2b67646e60a9f1f0467633e4f8cf9836ba948dd2b0702f`.
The available `ref-run/pi` source matches that pinned file. Capture with Node v26.8.1:

```sh
node scripts/capture-extension-profile-reference.mjs "$PI_SOURCE" fixtures/pi-extension-profiles/profiles.json
```

The capture script checks the source hash, loads the original source resolver/profile helpers,
uses only a fresh temporary directory, and deletes it afterwards. It starts no provider,
server or facet package. Thirteen observations cover absent/explicit/restored/changed/cleared
server selections, pinned and empty sessions surviving changes/clear, duplicate refusal and
future profile refusal. Temporary package paths map to builtin labels. The native test
`native_profile_results_match_the_pinned_pi_profile_helper_trace` compares every observation
through native protocol calls. This is a profile-helper comparison, not a differential of Pi
worker conflicts, package execution, reload or a full desktop workflow.

## Native checks

`runtime/plugins/tests.rs` contains nine tests covering the reference trace, persistent defaults
and session pins, active conflicts/retirement/stale questions, injected write failure,
concurrent first acquisition/replacement (12 races), malformed/future profiles, executable
identity/argv, failed-first-pin recovery and startup clear recovery. Additional tests cover
plugin-free import isolation, schema-4 backup/upgrade, wire omission versus explicit empty,
and worker flag forwarding. Controller parsing checks incompatible flag combinations.

Required final checks all completed successfully:

```text
cargo test --workspace                 1417 passed; 0 failed; 53 ignored
cargo clippy --workspace --all-targets  clean
cargo fmt --all --check                clean
```

Logs: `/tmp/pipkin-profiles-{workspace,clippy,fmt}-final.log`.
Initial workspace failures are retained in `/tmp/pipkin-profiles-workspace-initial.log`:
first-pin write failure initially escaped before the runtime could retain fail-closed state,
breaking two existing admission/storage-failure regressions. The implementation was corrected;
the original tests were not weakened. `/tmp/pipkin-profiles-workspace-protocol-update.log`
records an obsolete protocol-6 assertion corrected to 7. An intermediate collapsible-if Clippy
warning is retained in `/tmp/pipkin-profiles-clippy.log`; the final log is clean.

## Remaining

Existing TypeScript facets still need a supported native port/conversion path. Remote service
publishing, hot reload, `waitForPrompt`, desktop extension workflows and full qualification
remain open. Restored executable validation uses filesystem metadata while resolving session
profiles; this batch establishes correctness, not a hard latency bound for remote filesystems.
