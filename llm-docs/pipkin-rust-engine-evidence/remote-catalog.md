# Native public model-catalog refresh

2026-10-11, following `a98904f`. Protocol 9 / schema 8 unchanged. PCFG-11 and
ENV-OFFLINE are implemented; native remains opt-in and full release qualification is open.

## Behavior

`RefreshModels` rereads proxy/settings and local provider configuration, restores the
public metadata cache, and refreshes configured built-in providers. Radius retains its
credential-dependent gateway catalog. The existing credential resolver gates refresh
and can refresh an expired login; its returned credentials/headers never enter a catalog
request. Public metadata requests use the engine proxy and native `pipkin/<version>`
identity. This does not change Claude/Codex provider authentication or billing paths.

The engine restores `models-store.json` at startup without network. Overlays apply only
when Last-Modified is newer than the embedded manifest's generation timestamp, merge by
provider/type/id, and precede local `models.json` configuration. Chat lookup ignores image
or classifier models with the same id. Metadata for all three known types is preserved;
this does not implement image/classifier generation APIs.

Refresh uses a four-hour freshness window, ETag only when backed by a cached body, and
304 timestamp-only updates. A 404/501 retains the currently restored overlay but clears
the validator and invalidates the stored overlay for the next restore, matching the
reference. Transient status errors preserve cached models/validator and report a warning.
Malformed response/cache data never overwrites the last valid catalog. Corrupt cache
files are preserved and persistence failure prevents publication of the new overlay.

Management requests allow two immediate retries for transport failures or
408/425/429/500/502/503/504. Each attempt has a four-second timeout; that timeout also
bounds response-body stalls. There are at most eight active provider-refresh futures
per current refresh; each body is limited to 8 MiB. These component bounds do not qualify
a global memory budget or arbitrarily large preexisting cache files.

Newer refreshes cancel older ones. A generation check guards both durable and in-memory
publication. The cache reuses locked, private atomic JSON storage and preserves unrelated
entries. Waiting for another process's file lock checks cancellation, and the publication
lock is acquired only after the file lock; cancelled/shut-down work cannot publish late.
Existing credential-file lock behavior is unchanged. Engine shutdown cancels catalog work.

Presence of `PI_OFFLINE`, including an empty string or `0`, prevents catalog network
traffic; cached and embedded models remain available and normal coding still works.
Explicit configuration import includes `models-store.json`, without overwriting a native
file already present, and configuration reload makes the imported cache usable immediately.
No live service is required to build or start the native engine.

## Measured evidence

`auth::remote_catalog::tests` covers cache restoration, user overrides, provider-id
normalization, ETags/304, freshness, old/undated responses, 404/501, three-attempt 503,
header timeouts/body stalls, corrupt and malformed data, unrelated-entry preservation,
empty-body validators, offline presence, no-credential gating, proxy routing, no provider
Authorization/API-key header on metadata requests, supersession/cancellation, cache-lock
cancellation, imported-cache reload, and type-safe chat lookup.

`runtime::settings_tests::refreshed_catalog_models_are_selectable_and_resume_offline_after_restart`
uses the public engine protocol: RefreshModels invalidates the catalog, a newly discovered
model is selected, and a coding turn completes through a real local HTTP provider. The
provider receives its key while the catalog endpoint does not. After engine restart with
empty-valued PI_OFFLINE, selection restores and another coding turn completes without
another catalog request. Configuration discovery/copy-list tests include the metadata cache.

`scripts/capture-catalog-reference.mjs` runs original overlay and retry functions from
pinned Pi `d2a311097cbcf669e699479587332ae3988a49d0`. Import paths are rewritten in a
temporary directory; VERSION is a fixture constant and fetch/persistence are recorders.
Source hashes and eleven cases are in `fixtures/native-catalog/reference.json`.
Native tests compare typed overlays, request counts/validators, persisted freshness/
Last-Modified/ETag and errors across all eleven cases. This is a function differential,
not a live-service/auth/scheduler qualification. No public catalog request was made by
the test/capture process.

Final workspace, Clippy and formatting results are recorded in the progress record.
Logs: `/tmp/pipkin-catalog-verified-{workspace,clippy,fmt}.log`.
Earlier checks are retained: an unnecessary-unwrap Clippy warning was corrected; an
intermediate Clippy build overlapped the raw-store method rename and failed to compile;
an earlier formatting check observed in-progress edits. Final checks run after those
changes. No failing runtime assertion was waived.

## Remaining qualification

Production catalog/service reachability, live-provider account refresh, wider sustained
workloads, desktop/native-window acceptance and packaging/platform gates remain separate.
The general provider identity/attribution decision remains ADP-UA/ADP-ATTRIBUTION; the
public catalog's native UA does not settle those model-wire contracts. Native/Pi default
and dependency removal have not changed.
