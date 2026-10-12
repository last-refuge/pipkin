# Native engine progress record

Companion to the execution contract `llm-docs/pipkin-rust-engine-plan.md` (plan §13). This file
records decisions, identities, status, commands, evidence, blockers and the next safe batch.
It does not qualify anything by itself: a gate closes only with the execution evidence the plan
requires. Failed runs are recorded here, not replaced by later successes.

**Historical handoff state (2026-10-11, `native-engine` at `4fc4060`):** the main implementation has landed
on `native-engine` and every wave-2 worker branch is merged. The native engine is an opt-in
candidate (`--engine native`); Pi remains the default and only shipped engine. **Nothing is
qualified** and no parity row is passed. The Claude subscription integration is **reserved for
Codex — not implemented or qualified**; the native boundary reports it unavailable.

## Current follow-up — compaction validation and project null replacement

2026-10-12, following **`cf5043a`**. Malformed compaction token settings no longer discard
the whole native settings file. Ordinary settings and the selected model's override are
validated on use in reference order; other models' invalid overrides and unrelated settings
remain intact. Policy resolution happens before credential/provider requests. Live project
corrections restore generation and manual compaction without restarting the engine.

A new capture executes the pinned SettingsManager's actual in-memory storage, migration,
merge and getters with isolated unused dependencies. **46 cases** compare effective policies,
exact errors and preserved provider/model/retry settings. A runtime test verifies failed
coding/manual compaction without provider calls, live project repair and the original summary
budget. Existing crash/recovery compaction checks remain in the focused suite.

The first expanded workspace run found a real project-merge bug: null overrides were ignored.
Native now retains explicit null replacements and applies each nullish-default getter's own
default; invalid compaction token/entry nulls remain errors. Three additional captures cover
retry null/default behavior. Both focused runs passed **49 tests**; the first used 37 capture
cases and the corrected final run uses all 46. The failed first workspace run and initial
Clippy range-check warning are preserved in the evidence. Final required checks passed **1626 tests, 0 failed, 53 ignored**, with clean Clippy and
formatting. Logs: `/tmp/pipkin-compaction-settings-workspace2.log`,
`/tmp/pipkin-compaction-settings-clippy2.log`, `/tmp/pipkin-compaction-settings-fmt2.log`.
See [validation evidence](pipkin-rust-engine-evidence/compaction-settings-validation.md).

Protocol 11 / schema 9 unchanged. Inventory remains **504 rows, 488 non-OS; NC 412,
NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed, 1 unverified,
52 not started; 0 fully qualified**. Native remains opt-in. Invalid HTTP timeout behavior,
other settings/model-scope gaps, credential SDK retries, trust configuration scope, rollback
history labels, sustained workloads and release/CI/platform qualification remain open.
Owner plan/worktree changes are preserved.

## Previous follow-up — explicit Pi import source in native mode

2026-10-12, following **`6b148ee`**. `--engine native --pi-agent-dir DIR` now selects the
source for discovery and confirmed conversation/config/credential imports, including an
attached engine. Relative CLI paths become absolute; an omitted flag retains the engine's
existing default. Native data/configuration roots and process environment are unchanged.
There is no startup import, and Pi server/launch options retain their separate meanings.

Two new tests cover owned/attached CLI options and a real core/adapter/engine/SQLite workflow
with two disposable profiles. The selected profile overrides the engine's default, all import
steps use it, source bytes remain unchanged, and imported history continues natively through
a scripted provider. The first run exposed an incorrect test expectation for no-op rediscovery;
the existing Idle/informational behavior was preserved and the corrected tests passed.
See [source flag evidence](pipkin-rust-engine-evidence/import-source-flag.md).

Required checks passed **1624 tests, 0 failed, 53 ignored**, with clean Clippy and formatting.
Logs: `/tmp/pipkin-import-source-tests{,2}.log`, `/tmp/pipkin-import-source-workspace.log`,
`/tmp/pipkin-import-source-clippy.log`, `/tmp/pipkin-import-source-fmt.log`. This is headless
workflow evidence, not a new desktop/live-provider/platform acceptance run.

Protocol 11 / schema 9 unchanged. Inventory remains **504 rows, 488 non-OS; NC 412,
NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed, 1 unverified,
52 not started; 0 fully qualified**. Native remains opt-in. Invalid settings semantics,
model-scope glob/ordering, credential SDK retries, trust configuration scope, rollback history
labels, sustained workloads and release/CI/platform qualification remain open. Owner changes
are preserved.

## Previous follow-up — invalid proxy diagnostics before dispatch

2026-10-12, following **`367e389`**. The shared client now validates proxy policy before every
HTTP/1 or HTTP/2 dispatch, including redirects and pooled connections. The reserved DNS
failure route is removed. Login and credential requests receive the actual proxy error;
catalog refresh reports it explicitly; shared sending does not retry configuration errors.
NO_PROXY and scheme selection remain intact. Malformed proxy errors omit raw URLs to avoid
exposing embedded credentials. The opt-in vendor hook keeps other consumers unchanged and
explicitly refuses experimental HTTP/3 builds that cannot enforce it.

Eight actual pinned Pi resolver executions compare routing and unsupported-protocol errors.
Five additional native tests cover those cases, redirect/NO_PROXY policy, pooled dispatch,
OAuth code submission and catalog diagnostics; the previous invalid-proxy test now also
asserts exact errors, credential redaction and one attempt. The corrected focused suite
passed **26 tests**; final workspace checks **1622 tests, 0 failed, 53 ignored**, with clean
Clippy and formatting. Vendor integrity/tamper and license inventory fixture checks pass.
The initial private-module test compile error and sandbox capture restriction are recorded in
[validation evidence](pipkin-rust-engine-evidence/proxy-validation.md). Logs:
`/tmp/pipkin-proxy-validation-workspace.log`, `/tmp/pipkin-proxy-validation-clippy.log`,
`/tmp/pipkin-proxy-validation-fmt.log`. No installed package or GitHub CI execution claimed.

Protocol 11 / schema 9 unchanged. Inventory remains **504 rows, 488 non-OS; NC 412,
NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed, 1 unverified,
52 not started; 0 fully qualified**. Native remains opt-in. Credential SDK retries,
OS/Node-specific trust scope, remaining settings/migration, sustained workloads and clean
release/CI/platform qualification remain open. Owner plan/worktree changes are preserved.

## Previous follow-up — plain HTTP CONNECT proxy parity

2026-10-12, following **`bdc7a26`**. Native now tunnels HTTP origins through HTTP/HTTPS
proxies with CONNECT, matching pinned Pi's undici configuration. An additive, default-off
option in a locally vendored reqwest 0.12.28 retains the existing HTTP/TLS implementation,
pooling and timeouts. The archive was verified against the committed Cargo.lock checksum;
all original licenses remain, source modifications are identified, and the package inventory
includes the patch and provenance. Packaging and qualification CI verify the vendor tree.

Eight executed Node 26.8.1 / undici 8.10.2 reference cases cover origin/proxy protocols and
ports. Native comparisons check authority, body, authorization and credential separation.
Additional tests cover IPv6 default ports, pool reuse, stalled CONNECT cleanup, NO_PROXY,
runtime setting changes, catalog requests and unchanged behavior without the opt-in.
See [HTTP CONNECT evidence](pipkin-rust-engine-evidence/http-connect-tunnels.md).

The corrected proxy suite passed **22 tests**, the HTTP suite **8 tests**, and final workspace
checks **1617 tests, 0 failed, 53 ignored**, with clean Clippy and formatting. Initial compile,
fixture and lint corrections remain recorded in the evidence. Vendor verification, tamper
checks and license inventory generation passed. No installed package, GitHub CI run or
additional platform acceptance is claimed. Logs: `/tmp/pipkin-http-tunnel-workspace2.log`,
`/tmp/pipkin-http-tunnel-clippy2.log`, `/tmp/pipkin-http-tunnel-fmt.log`.

Protocol 11 / schema 9 unchanged. Inventory remains **504 rows, 488 non-OS; NC 412,
NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed, 1 unverified,
52 not started; 0 fully qualified**. Native remains opt-in. Unusable-proxy diagnostics,
credential retry comparisons, OS/Node-specific trust scope, remaining settings/migration,
sustained workloads and clean release/CI/platform qualification remain open. Owner plan and
worktree changes are preserved.

## Previous follow-up — configured TLS roots and connection isolation

2026-10-12, following **`d948887`**. Native now honors the engine's
`NODE_EXTRA_CA_CERTS` configuration for shared HTTP, credential/identity clients and Codex
WebSockets, including HTTPS proxies. Bundled roots and peer/hostname verification remain
active. The file loader retains valid certificates preceding a malformed entry, matching the
executed reference, reports invalid files without exposing contents, and bounds file reads.

Unlike Node's startup-only CA loading, native picks up certificate-file changes at the next
network snapshot. Pooled WebSockets now separate endpoint, proxy, handshake identity and trust
configuration, preventing stale connections/continuations after any of those change. The
existing permanent per-session SSE fallback still applies after a WebSocket transport failure.
See [trust evidence](pipkin-rust-engine-evidence/extra-ca-trust.md).

Ten cases were executed through Node 26.8.1 and pinned undici 8.10.2 against local TLS. Five new
native tests cover those outcomes, identity-client propagation, active snapshot retention,
running-engine reload, HTTP/HTTPS proxy WSS, invalidated pool entries and hostname rejection.
The corrected focused suite passed **5 tests**. Earlier fixture mistakes (missing new struct
field/debug accessor mismatch, incorrect fallback expectation and a request outside a Tokio
reactor) are recorded in the evidence. Required checks passed **1615 tests, 0 failed,
53 ignored**, with clean Clippy and formatting. Logs: `/tmp/pipkin-extra-ca-tests4.log`,
`/tmp/pipkin-extra-ca-workspace.log`, `/tmp/pipkin-extra-ca-clippy2.log`,
`/tmp/pipkin-extra-ca-fmt.log`. Earlier failed fixture runs remain recorded.

Protocol 11 / schema 9 unchanged. Inventory remains **504 rows, 488 non-OS; NC 412,
NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed, 1 unverified,
52 not started; 0 fully qualified**. Native remains opt-in. OS/Node-specific trust options,
plain-HTTP proxy tunneling, unusable-proxy diagnostics, credential retries, remaining settings/
migration behavior, sustained workloads and clean release/CI/platform qualification remain
open. Owner plan/worktree changes are preserved.

## Previous follow-up — AWS credential cache and helper isolation

2026-10-12, following **`0c3d742`**. The shared AWS credential cache no longer identifies
process credentials by profile name, assumed roles by profile/role, container credentials by
URL alone, or every EC2 instance by one global key. Identity keys include configuration and
source credentials, rotating web/container token contents, endpoints and helper environments.
The cache is bounded to 64 identities with expiry pruning and least-recently-used eviction.

Bedrock now honors the isolated-environment flag for credentials, profiles and regions.
Shell helpers receive the request's effective environment, retain shell semantics, bound each
output stream to 1 MiB, redact output in failures, reject expired credentials and clean up
Unix descendants on timeout/cancellation. Token files and credential HTTP bodies are bounded.
See [AWS isolation evidence](pipkin-rust-engine-evidence/aws-credential-isolation.md).

Twelve new tests cover identity separation, rotation, limits, cancellation, cleanup and an
actual local Bedrock stream with conflicting ambient identity. Twelve cases captured by
executing Pi's pinned AWS credential-process SDK compare successful credentials and error
categories. The focused provider suite passed **264 tests**. Initial workspace checks passed
**1610 tests, 0 failed, 53 ignored**. Clippy's one collapsible-if warning was corrected. The
final workspace run also passed **1610 / 0 / 53**, including that correction and a helper
wait-error cleanup adjustment; final Clippy and formatting are clean. Logs:
`/tmp/pipkin-aws-isolation-provider-tests.log`, `/tmp/pipkin-aws-isolation-workspace2.log`,
`/tmp/pipkin-aws-isolation-clippy2.log`, `/tmp/pipkin-aws-isolation-fmt.log`. Initial logs
are retained in `...-workspace.log` and `...-clippy.log`.

Protocol 11 / schema 9 unchanged. Inventory remains **504 rows, 488 non-OS; NC 412,
NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed, 1 unverified,
52 not started; 0 fully qualified**. Native remains opt-in. Live AWS acceptance, broader
credential retries, proxy/server-trust settings, migration flags, sustained workloads and
clean release/CI/platform qualification remain open. Owner plan/worktree changes are preserved.

## Previous follow-up — Google certificate federation and transport preservation

2026-10-12, following **`d1809e7`**. Native certificate ADC now loads explicit/env/default
certificate config, validates leaf/trust chains and performs credential exchanges with mutual
TLS. Nineteen executed SDK cases use an actual local server requiring client authentication.
Native comparisons retain STS/IAM bodies and peer fingerprints, including source-client
impersonation. Model streams receive bearer headers without the auth certificate, matching
the audited genai boundary. See [certificate evidence](pipkin-rust-engine-evidence/google-certificate.md).

Ordinary turns and compaction now carry the HTTP client and network settings as one snapshot.
Certificate clients retain its proxy/idle settings; network changes also isolate Google token
cache state. Six certificate tests cover rotation, cancellation, bounds, untrusted servers,
CONNECT proxy routing, idle-timeout recovery and two real local TLS Vertex streams.

An initial fixture compile used the wrong Env::isolated argument type. After correcting it,
the Google suite passed 49 tests but failed an existing Vertex cache-reuse assertion. The
expanded suite exposed arbitrary idle eviction from the bounded Google identity cache.
Least-recently-used idle eviction now preserves recency and active owners, with a seventh
regression test. The corrected focused suite passed **51 tests**.

Required checks: **1598 passed, 0 failed, 53 ignored**, clean Clippy and formatting.
Logs: `/tmp/pipkin-google-certificate-tests.log` (compile failure),
`/tmp/pipkin-google-certificate-tests2.log` (cache failure),
`/tmp/pipkin-google-certificate-tests3.log`, `/tmp/pipkin-google-certificate-workspace.log`,
`/tmp/pipkin-google-certificate-clippy3.log`, `/tmp/pipkin-google-certificate-fmt.log`.
Protocol 11 / schema 9 unchanged. Inventory remains **504 rows, 488 non-OS; NC 412,
NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed, 1 unverified,
52 not started; 0 fully qualified**.

Known Google ADC source implementation gaps are closed with local evidence. Next: broader
credential retry/cache/configuration review (including server trust settings), remaining
proxy/settings/migration flags, sustained workloads and clean release/CI/platform qualification.
Native remains opt-in. Live enterprise/cloud acceptance and native-default release acceptance
remain unverified. Owner plan/worktree changes remain untouched.

## Previous follow-up — Google AWS federation

2026-10-12, following **`d0e949b`**. Native Vertex ADC now supports environment or metadata
AWS federation, including IMDSv2 session headers, cached region discovery and rotating roles.
Twenty exact signed-proof vectors and thirteen executed SDK cases verify discovery, form
encoding, signature canonicalization and source selection. Cancellation preserves the shared
exchange; AWS credential environment changes isolate cached access tokens. See
[AWS federation evidence](pipkin-rust-engine-evidence/google-aws.md).

Five tests were added; the existing Vertex test now also completes two streams using an AWS
proof. The focused Google suite passed **44 tests**. A real thirty-second metadata timeout
settles and a subsequent request succeeds. The reference factory audit also corrected raw
ADC selector handling, including malformed selectors and aliases within selected clients.

The first workspace run failed in the launcher fixture: an identical stale terminal record
could satisfy the second iteration's wait before its new process wrote, allowing recorder
processes to race. Clearing prior terminal/editor records before each iteration fixes that
synchronization; production launch behavior is unchanged. The focused launcher test passed.

Final required checks: **1591 passed, 0 failed, 53 ignored**, clean Clippy and formatting.
Logs: `/tmp/pipkin-google-aws-tests.log`, `/tmp/pipkin-google-aws-workspace.log` (initial failure),
`/tmp/pipkin-google-aws-launcher.log`, `/tmp/pipkin-google-aws-workspace2.log`,
`/tmp/pipkin-google-aws-clippy2.log`, `/tmp/pipkin-google-aws-fmt2.log`.
Protocol 11 / schema 9 unchanged. Inventory remains **504 rows, 488 non-OS; NC 412,
NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed, 1 unverified,
52 not started; 0 fully qualified**.

Next: certificate federation (real mutual TLS while preserving proxy/timeout configuration),
broader credential retry/cache/configuration review, sustained workloads and release/CI/platform
qualification. The certificate reference reads certificate_config.json, builds a leaf/trust
chain subject and applies the identity to STS and subsequent Google HTTP requests. It cannot
be completed with certificate parsing alone. Native remains opt-in; live cloud and release
acceptance remain open. Owner plan/worktree changes remain untouched.

## Previous follow-up — Google executable credential helpers

2026-10-12, following **`d721b3a`**. Native ADC now executes explicitly enabled Google
credential helpers, preserves the SDK's command/environment and output-cache contracts,
and supports OIDC/SAML responses. The per-identity cache includes helper environment;
caller cancellation does not abandon the owned exchange. Configured helper timeouts up to
120 seconds have their own allowance, and Unix timeout/error cleanup kills the helper group.
See [helper evidence](pipkin-rust-engine-evidence/google-executable.md).

Five new tests compare thirteen executed SDK cases and command parsing, check environment
isolation, shared refresh, redacted/bounded output, a real five-second descendant timeout
and recovery, and a real 31-second successful exchange. Vertex's existing external-account
test now also completes two streams using an actual helper and cached bearer/quota headers.
An initial compile visibility error was fixed; an initial filter matched zero tests and was
not counted. The corrected Google suite passed **39 tests**.

Required checks: **1586 passed, 0 failed, 53 ignored**, clean Clippy and formatting.
Logs: `/tmp/pipkin-google-executable-tests2.log`, `/tmp/pipkin-google-executable-workspace.log`,
`/tmp/pipkin-google-executable-clippy2.log`, `/tmp/pipkin-google-executable-fmt.log`.
Protocol 11 / schema 9 unchanged. Inventory remains **504 rows, 488 non-OS; NC 412,
NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed, 1 unverified,
52 not started; 0 fully qualified**. Process checks were measured on Linux, not other OSes.

Next: AWS and certificate federation sources, broader credential retry/cache and configuration
review, sustained workloads and release/CI/platform qualification. Native remains opt-in.
The owner plan, native-capabilities plan and Claude worktrees remain untouched.

## Previous follow-up — Google external file/URL and authorized-user credentials

2026-10-12, following **`28b0e4d`**. Native Vertex now supports external-account file/URL
subject suppliers, workforce STS billing, optional service-account impersonation and external
user refresh grants. Eight executed SDK cases establish request/token contracts. Seven new
tests cover refresh rotation after cancellation, source reload, aliases, bounds, a real
thirty-second timeout/recovery and actual Vertex streams. See
[external ADC evidence](pipkin-rust-engine-evidence/google-external.md).

Required checks: **1581 passed, 0 failed, 53 ignored**, clean Clippy and formatting.
Logs: `/tmp/pipkin-google-external-workspace.log`, `/tmp/pipkin-google-external-clippy2.log`,
`/tmp/pipkin-google-external-fmt.log`. Two initial Clippy warnings were corrected with
behavior-preserving control-flow/formatting cleanup. Protocol 11 / schema 9 unchanged.

Inventory remains **504 rows, 488 non-OS; NC 412, NE 38, NP 37, PA 0, BL 0, LB 1,
OS 16; 444 measured, 7 observed, 1 unverified, 52 not started; 0 fully qualified**.
External user refresh rotation is in memory, matching the SDK; it does not rewrite ADC files.
AWS/executable/certificate federation sources, broader credential retries/configuration,
sustained workloads and release qualification remain open. Native remains opt-in.
Unrelated owner plan/worktree changes remain untouched.

## Previous follow-up — Google metadata and impersonated credentials

2026-10-12, following **`8abc540`**. Native Vertex now supports impersonated-service-account
ADC files and metadata-server credentials. Executed SDK comparisons cover source grants,
IAM delegates/scope/lifetime and source/target billing. Metadata checks verify scope and
flavor headers, detection controls and bounded responses. Two actual local Vertex streams
for each new path verify bearer/quota forwarding and cached reuse. See
[Google ADC evidence](pipkin-rust-engine-evidence/google-adc.md).

The token cache now includes parsed credential contents and configuration, so replacing a
file cannot reuse its previous identity. Independent bounded refresh owners avoid blocking
unrelated identities; cancellation stops only the waiter. Nested impersonation validates
inherited universe domains before exchanging source credentials. No model history is cloned
into an authentication task. Initial new-test failures came from a missing model ID in the
fixture; after correction the checks passed. Eight tests were added.

Final required checks: **1574 passed, 0 failed, 53 ignored**, clean Clippy and formatting.
Logs: `/tmp/pipkin-google-adc-workspace2.log`, `/tmp/pipkin-google-adc-clippy2.log`,
`/tmp/pipkin-google-adc-fmt2.log`. The earlier workspace run also passed; the final rerun
includes inherited-universe validation. Protocol 11 / schema 9 unchanged.

Inventory remains **504 rows, 488 non-OS; NC 412, NE 38, NP 37, PA 0, BL 0, LB 1,
OS 16; 444 measured, 7 observed, 1 unverified, 52 not started; 0 fully qualified**.
Real GCE/Cloud Run acceptance is unverified. Metadata ADC uses the existing configured
ADC marker when no file is present; provider discovery still matches Pi's file/key checks.

Next: external-account federation/source types and external-account authorized-user ADC,
remaining proxy/configuration and credential-cache/retry checks, sustained workloads and
release/CI/platform qualification. Native remains opt-in. Unrelated owner plan/worktree
changes remain untouched.

## Previous follow-up — AWS login-session credentials

2026-10-12, following **`fe4a15a`**. Native Bedrock now resolves the separate AWS CLI
`login_session` cache, generates verified P-256 DPoP refresh proofs, atomically saves rotated
credentials and preserves account/unknown metadata. Concurrent requests share the refresh
owner; canceled callers cannot abandon token rotation. Access denial is final even with a
still-valid old token or malformed denial body. The proof binds to the normalized URL sent
by the HTTP client. See [AWS login evidence](pipkin-rust-engine-evidence/aws-login.md).

Eight new tests cover six executed SDK outcomes, signature verification, unique proof IDs,
cache paths/permissions, cancellation/concurrency, a newer login, malformed/oversized responses,
a real thirty-second timeout and two signed Bedrock turns reusing the committed credentials.
Required checks: **1566 passed, 0 failed, 53 ignored**, clean Clippy and formatting. Final
logs: `/tmp/pipkin-login-workspace2.log`, `/tmp/pipkin-login-clippy2.log`,
`/tmp/pipkin-login-fmt2.log`. The initial workspace run also passed; the final rerun includes
the endpoint-normalization assertion. No failed behavior was waived. Protocol 11 / schema 9
unchanged.

Inventory remains **504 rows, 488 non-OS; NC 412, NE 38, NP 37, PA 0, BL 0, LB 1,
OS 16; 444 measured, 7 observed, 1 unverified, 52 not started; 0 fully qualified**.
The AWS login implementation gap is closed; these local comparisons do not establish live
AWS, SDK credential retry middleware, complete provider or release qualification.

Next: remaining Google ADC/proxy/configuration gaps and provider credential-cache isolation
review, sustained workloads, clean release comparisons and CI/platform acceptance. Native
remains opt-in. Unrelated owner plan/worktree changes remain untouched.

## Previous follow-up — AWS IAM Identity Center credentials

2026-10-12, following **`8a0ac3d`**. Native Bedrock now resolves legacy and session-based
SSO profiles, refreshes eligible cached tokens, preserves rotating credentials after caller
cancellation, and signs model requests with the returned role credentials. Atomic private
cache writes preserve metadata and avoid overwriting a newer login. Ten new tests include
executed pinned-SDK comparisons, concurrent cancellation, role/rotation isolation,
AssumeRole sourcing, malformed/oversized inputs and a real thirty-second stalled exchange.
See [SSO evidence](pipkin-rust-engine-evidence/aws-sso.md).

The first workspace run exposed ETXTBSY in the generated launcher recorder fixture. Using
an existing shell interpreter removes execution of newly written files while retaining
real process/argument assertions; production launcher behavior is unchanged. The corrected
required checks passed: **1558 passed, 0 failed, 53 ignored**, clean Clippy and formatting.
Logs: `/tmp/pipkin-sso-workspace.log` (initial failure), `/tmp/pipkin-sso-workspace2.log`,
`/tmp/pipkin-sso-clippy.log`, `/tmp/pipkin-sso-fmt.log`. Protocol 11 / schema 9 unchanged.

MFA callback refusal matches the executed SDK and Pi's client configuration; it is not a
missing native prompt. A separate required gap was confirmed: the pinned SDK supports
AWS `login_session` credential-cache profiles; native does not yet resolve them. SSO support
therefore does not establish complete AWS chain parity. Live AWS, broader provider/platform
and native-default qualification remain open. Inventory counts are unchanged: **504 rows,
488 non-OS; NC 412, NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed,
1 unverified, 52 not started; 0 fully qualified**. Native remains opt-in.

Next: AWS login-session and remaining Google ADC/proxy/configuration gaps, sustained
workloads, clean release comparisons and CI/platform acceptance. Extension-only behavior
remains out of scope. Unrelated owner plan/worktree changes remain untouched.

## Previous follow-up — fork migration and installed coding

Following **`c508954`**, forked Pi histories now import with their visible ancestor cutoffs,
branch documents, independent native roots and separate subagent ownership preserved. Five
new tests compare executed Pi SQLite/context results, atomic failure/retry, malformed ancestry,
usage grouping and native continuation of seven contexts after restart. All 40 import tests
pass. See [fork/import evidence](pipkin-rust-engine-evidence/fork-import.md).

Required checks: **1548 passed, 0 failed, 53 ignored**; Clippy, formatting and diff checks clean.
Logs: `/tmp/pipkin-fork-workspace2.log`, `/tmp/pipkin-fork-clippy2.log`,
`/tmp/pipkin-fork-fmt2.log`. Protocol 11 / schema 9 unchanged.

The expanded installed-package check imports a fork, executes real write/edit/bash/read tools
through a local HTTP provider, restarts, verifies durable replay without repeating effects,
and continues again. The final native-only candidate passes with Node/npm/Pi trap shims.
Archive SHA-256: `7eabb9dcbe0f9f74da4cf7459d079cc70fcdd4ca45a8a38b873376b0e43d4f58`.
Package logs: `/tmp/pipkin-fork-package-build2.log`, `/tmp/pipkin-fork-package-checks2.log`.
This dirty candidate identifies `c508954`; it is not a clean release or GUI/live-provider
qualification. Stale Claude-unavailable diagnostic text is corrected.

An independent native CI job now builds without Pi provisioning or Node setup and runs these
installed checks. YAML validation passes locally; the GitHub runner result is unverified.
PKG-CI therefore moves from not started to unverified. Inventory: **504 rows, 488 non-OS;
NC 412, NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed,
1 unverified, 52 not started; 0 fully qualified**. FORK keeps its NP branch-creation
disposition while its required import support now has executed-reference evidence.

Next: remaining provider/configuration and unsupported-history gaps, sustained desktop/tool
workloads, clean release comparisons and platform/CI acceptance. Native remains opt-in.
Unrelated owner plan/worktree changes remain untouched.

## Previous follow-up — Anthropic workload identity federation

Following **`d211c03`**, the native Anthropic adapter now exchanges projected identity tokens,
caches and refreshes the resulting bearer credentials, invalidates rejected tokens, and
preserves explicit credential precedence. Nine tests cover local exchange, concurrency,
cancellation, rotation, error handling, native engine restart and comparisons against the
executed pinned SDK. See [federation evidence](pipkin-rust-engine-evidence/anthropic-federation.md).
This is separate from the existing Claude subscription integration.

Required checks: **1543 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
The staged diff check reports only a trailing blank line in the verbatim upstream MIT
license; it is retained to preserve the recorded source bytes and SHA-256.
Logs: `/tmp/pipkin-federation-workspace.log`, `/tmp/pipkin-federation-clippy.log`,
`/tmp/pipkin-federation-fmt.log`. Protocol 11 / schema 9 unchanged.

A fresh Linux native-only package built and passed installation, worker handshake/session
listing, diagnostics, notice retention and uninstall/data-preservation checks with a bare
PATH and failing Node/npm/Pi shims. This is a dirty candidate based on `d211c03`, not a clean
release qualification or installed coding-turn test. Package SHA-256:
`c4396027491f5cd6ee84eb64f7b5878516fd77c8ff9412af460e20dfc3a9a0f6`.
Logs: `/tmp/pipkin-federation-package-build.log`, `/tmp/pipkin-federation-package-checks.log`.
The probe exposed stale diagnostic text claiming native Claude subscription is unavailable
(`crates/pipkin-app/src/install.rs`); correcting that remains follow-up work.

Inventory counts and qualification status are unchanged: **504 rows, 488 non-OS;
NC 412, NE 38, NP 37, PA 0, BL 0, LB 1, OS 16; 444 measured, 7 observed,
0 unverified, 53 not started; 0 fully qualified**. The federation row previously measured
only refusal; it now has working implementation evidence. Remaining partial-row gaps and
sustained/installed/migration/platform qualification still block default cutover and Pi removal.

## Previous follow-up — prompt, WebSocket connection and launcher checks

Following **`af059d5`**, five new tests close the three remaining explicitly unverified row
claims. A real stalled WebSocket handshake exercises configured timeout, SSE fallback and
cancellation with timeout disabled; native settings edits reach provider requests. A native
write-tool change drives core launcher effects with parsed editor/terminal flags and actual
scratch recorder processes. Built-in prompt rendering keeps frozen instructions/skills
after source loss; new roots skip bad files and retain fallback/built-in sections.

PRM-THROWING-SECTION moves NC → NE after auditing the supported fixed prompt registration.
Pipkin has pure rendering from typed snapshots, not arbitrary throwing section callbacks;
extension callbacks remain out of scope. This is a native-equivalence decision with resource
failure tests, not a claim of Rust panic recovery. See
[prompt/timeout/launcher evidence](pipkin-rust-engine-evidence/prompt-timeout-launch-checks.md).

Required checks: **1534 passed, 0 failed, 53 ignored**; Clippy, formatting and diff checks clean.
Logs: `/tmp/pipkin-final-row-checks-workspace.log`, `/tmp/pipkin-final-row-checks-clippy.log`,
`/tmp/pipkin-final-row-checks-fmt.log`. Protocol 11 / schema 9 and production code unchanged.

Inventory: **504 rows, 488 non-OS; NC 412, NE 38, NP 37, PA 0, BL 0, LB 1, OS 16;
444 measured, 7 observed, 0 unverified, 53 not started; 0 fully qualified**.
The zero unverified count does not close partial measured-row gaps or qualify release.
Next: reconcile those remaining gaps against the shipped baseline, fix required differences,
complete sustained/installed/migration/packaging/platform checks, then switch defaults and
remove Pi/Node only after the required gates pass. Native remains opt-in; unrelated owner
plan/worktree changes remain untouched.

## Previous follow-up — durable partial tool output and Stop recovery

Following **`6253ebb`**, tool progress now commits its bounded window and truncation counts
before publication. Interrupted and durable-Stop recovery retains that output in history,
the next model request and retained-output access. Result settlement and checkpoint deletion
are atomic; Stop recovery reaps orphaned processes. A new fault test also exposed and fixed
early reopening clearing a storage failure before the canceled execution finished.

The strengthened old SIGKILL test failed because its published prefix disappeared. Two new
process-crash tests cover commit-before-publication and bounded output with/without durable
Stop. A new runtime test covers failed progress writes and rolled-back final results, both
without repeating effects. Full crash suite: **17 passed**. See
[recovery evidence](pipkin-rust-engine-evidence/tool-output-recovery.md), including failures.

Required checks: **1529 passed, 0 failed, 53 ignored**; Clippy, formatting and diff checks clean.
Logs: `/tmp/pipkin-output-recovery-workspace.log`, `/tmp/pipkin-output-recovery-clippy.log`,
`/tmp/pipkin-output-recovery-fmt.log`. Protocol 11 / schema 9 unchanged.

Inventory: **504 rows, 488 non-OS; NC 413, NE 37, NP 37, PA 0, BL 0, LB 1, OS 16;
441 measured, 7 observed, 3 unverified, 53 not started; 0 fully qualified**.
OUT-RECOVERY now has measured evidence. Next: throwing prompt-section applicability,
WebSocket connection timeout and native editor/terminal flags; then remaining partial-row
gaps and sustained/installed/packaging/platform qualification. Native remains opt-in.
Unrelated owner plan/worktree files remain untouched.

## Previous follow-up — Copilot retry bounds and subagent accounting

Following **`4a0601f`**, Copilot login retries now handle numeric prefixes and HTTP dates,
reject non-finite delays without panics, discard stalled 429 bodies, and honor cancellation
and one total retry budget. Five new tests include 21 executed-reference first-delay cases
and real local HTTP timing/retry checks. Zero-retry token refresh remains unchanged.
See [Copilot retry evidence](pipkin-rust-engine-evidence/copilot-login-retries.md).

Subagent tests reproduced omitted child spend (15 displayed versus 45 committed). Root
usage views now aggregate descendant ledgers by their original model/tool identities. Child
usage and ancestor revisions/invalidation commit atomically, then publish live parent usage;
compaction uses the same update path. Tests cover saturated parent/child scheduling, live
aggregate totals, replay/restart, independent roots, and injected rollback/recovery. Rich
import assertions verify unchanged own ledgers plus correct group totals. The previous
ledger claim that children were already attributed to a subagent tool bucket was false.
See [subagent evidence](pipkin-rust-engine-evidence/subagent-accounting.md).

Required checks: **1526 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-retry-usage-workspace2.log`, `/tmp/pipkin-retry-usage-clippy2.log`,
`/tmp/pipkin-retry-usage-fmt2.log`. The first workspace run exposed the old root-only import
assertion; earlier targeted failures/fixture corrections are recorded in the evidence.
No failed behavior was waived. Both fixes were validated in the same working tree.

Inventory: **504 rows, 488 non-OS; NC 413, NE 37, NP 37, PA 0, BL 0, LB 1, OS 16;
440 measured, 7 observed, 4 unverified, 53 not started; 0 fully qualified**.
SUB-PERMITS and USE-TOTAL now have measured evidence. Next: throwing prompt-section
applicability, interrupted partial tool output recovery, WebSocket connection timeout, and
native desktop editor/terminal flags. Partial measured-row gaps and sustained/installed/
packaging/platform qualification still require work before native-default/Pi retirement.
Unrelated owner plan/worktree files remain untouched; native remains opt-in.

## Prior follow-up — private Radius catalogs

Following **`73979ae`**, default/custom Radius gateways support authenticated discovery and
independent durable catalogs. Empty custom gateways can discover their first models. Local
model overrides apply after gateway metadata; custom gateways do not inherit default models.
Legacy credential catalogs survive token rotation and migrate offline; cached catalogs restore
without credentials. Cancellation, supersession, malformed/oversized/stalled responses and
failed persistence preserve the last usable models. Custom Radius providers use the reference
Radius environment-key fallback. See [Radius evidence](pipkin-rust-engine-evidence/radius-catalog.md).

Eight new auth/catalog tests include six executed pinned-reference helper cases. A ninth
runtime test discovers/selects a model, executes a real write-tool turn through the local
HTTP Radius adapter, restarts and resumes with no further catalog request. This is actual
native execution against a local provider fixture, not a live-account or GUI qualification.

Required checks: **1518 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-radius-workspace.log`, `/tmp/pipkin-radius-clippy.log`,
`/tmp/pipkin-radius-fmt.log`. Initial fixture/compile errors are recorded in the evidence.
PRV-radius returns PA → NC. Inventory: **504 rows, 488 non-OS; NC 413, NE 37, NP 37,
PA 0, BL 0, LB 1, OS 16; 438 measured, 7 observed, 6 unverified, 53 not started;
0 fully qualified**. Native remains opt-in; unrelated owner changes are untouched.

Next: fix Copilot login Retry-After date/non-finite handling, close the six remaining unverified
runtime/desktop row claims, and audit partial measured rows for required gaps. Then complete
sustained reliability/performance and installed/package/platform qualification before cutover.

## Prior follow-up — stored provider refresh and cancellation

Following **`1429251`**, five real local-HTTP/auth-store tests cover Copilot and default/custom
Radius refresh: concurrent deduplication, token rotation, expiry, restart, failures and no
API-key billing fallback. A cancellation regression reproduced stale live Copilot catalogs
after token persistence; catalog publication now belongs to the owned refresh task and
survives caller cancellation. See [refresh evidence](pipkin-rust-engine-evidence/provider-refresh.md).

Required checks: **1509 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
The first targeted run reproduced the bug and exposed a fixture omission; both corrected.
Copilot refresh intentionally does not retry catalog 429s, as the pinned source specifies;
the old ledger claim was corrected. No live account or full qualification is claimed.

Recomputed inventory: **504 rows, 488 non-OS; NC 412, NE 37, NP 37, PA 1, BL 0,
LB 1, OS 16; 438 measured, 7 observed, 6 unverified, 53 not started; 0 fully qualified**.
PRV-radius reopens: authenticated gateway catalog refresh and independent persistence are
missing, and OAuth rotation discards the legacy catalog field. This required gap is next,
along with Copilot login Retry-After date/non-finite handling. Then close remaining runtime
assertions and sustained/installed/package qualification before native-default/Pi retirement.
Unrelated owner plan and worktree files remain untouched; native stays opt-in.

## Prior follow-up — product identity, embedded help and OAuth reachability

Following **`291e854`**, the native system prompt identifies Pipkin and embeds a compact
product/engine reference from `docs/native-engine-reference.md`. It no longer points at
Pi package documentation or silently omits help. Root/child/restart requests contain the
reference without an installed docs tree. Tool, project, skill and positional section
behavior is preserved; eight executed pinned-reference prompt cases assert all sections
with exactly the declared preamble-name/docs transformations.

Audited actual provider identity: the shared HTTP client already sends pipkin/version,
with explicit provider/configuration override precedence. Codex's existing pi UA/originator
and Claude subscription identities remain unchanged. A local HTTP test measures the real
default/override headers and retained auth. OAuth reachability is now resolved: keep existing
native service login flows while desktop retains only Claude/ChatGPT subscription choices.
The mapping regression includes all additional providers and a custom Radius gateway.
See [identity/auth audit](pipkin-rust-engine-evidence/identity-auth-audit.md).

Required checks: **1504 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-identity-workspace.log`, `/tmp/pipkin-identity-clippy.log`,
`/tmp/pipkin-identity-fmt.log`. The reference capture initially lacked sandbox subprocess
permission to check Git; its permitted rerun succeeded. An initial runtime test inspected
the legacy leading-system field instead of resolving durable positional messages; corrected
before the passing full run. No failure was waived and no live-provider login was claimed.

The remaining eleven PA rows move to NE: eight service OAuth login rows, provider UA,
preamble and product docs. Recomputed inventory: **504 rows, 488 non-OS; NC 413, NE 37,
NP 37, PA 0, BL 0, LB 1, OS 16; 436 measured, 7 observed, 8 unverified,
53 not started; 0 fully qualified**. This closes disposition decisions, not release gates.
Native remains opt-in and unrelated owner plan/worktree changes remain untouched.

Next: replace the eight unverified row claims with evidence or concrete fixes. They cover
Copilot/Radius refresh, subagent execution permits, aggregate child usage, throwing prompt
sections, interrupted tool-output recovery, WebSocket connection timeout and desktop editor/
terminal actions. Audit applicability against actual built-in paths without reintroducing
extensions. Then complete sustained reliability/performance, installed coding/migration,
packaging/platform qualification and the final native-default/Pi removal batch. The one LB
row is the transitional `--pi-repo` launch path, to retire at cutover.

## Prior follow-up — durable root and child removal

Following **`dfbca07`**, implemented `RemoveSession` with protocol **11** / schema **9**.
Removal commits an intent before stopping work, rejects new admissions, cancels live root/
child runs and foreground/background compactions, and wakes queued prompt waiters. Cleanup
outlives caller disconnect and atomically drops session-owned records and import mappings
with deletion invalidations. Eight concurrent cleanup jobs are allowed; capacity refusal
commits nothing. Repeated calls succeed without resurrecting old create/import identities.
Startup reaps orphaned Linux tool processes before deleting their recovery records.

Seven runtime tests cover lifecycle/races/faults/capacity. Real engine-process tests exercise
running shell teardown and SIGKILL after intent and after deletion. Headless desktop tests
verify live and dropped notifications, removal of an open running conversation, restart and
subsequent coding work. Rich imported history/profile/source/backup bytes remain unchanged;
schema-8 migration creates a readable pre-migration backup. The core catalog now also drops
missing live conversations, fixing stale selection after deletion.
See [session removal evidence](pipkin-rust-engine-evidence/session-removal.md).

Required checks: **1502 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-removal-workspace2.log`, `/tmp/pipkin-removal-clippy2.log`,
`/tmp/pipkin-removal-fmt.log`. The first workspace run found five historical migration
fixture failures (schema-9 objects left behind in synthetic old databases and hardcoded
schema-8 assertions); corrected fixtures retain their backup/data assertions. Initial
child-wait, sandbox, writer-lock and test-target corrections are recorded in the evidence.
No test failure was waived.

SVC-MGMT-REMOVE moves PA → NC. Recomputed inventory: **504 rows, 488 non-OS;
NC 413, NE 26, NP 37, PA 11, BL 0, LB 1, OS 16; 436 measured, 7 observed,
8 unverified, 53 not started; 0 fully qualified**. Native remains opt-in.

Next: close provider identity/OAuth reachability and prompt identity/documentation audits,
then remaining sustained reliability/performance, installed migration/coding workflows,
packaging/platform and release qualification. Pi/Node removal is still gated on that
acceptance. Pi extensions remain out of scope. Removal does not claim secure erasure or
non-Linux orphan process qualification; source backups and desktop draft/cache data remain.

## Prior follow-up — thinking cycling and atomic model configuration

Following **`5917287`**, implemented the built-in Models service's missing cycle operation.
`CycleThinking` advances/wraps through supported levels and matches the reference for
missing/unknown/disabled current levels, missing/non-reasoning models and empty level sets.
It is advertised in Hello; protocol **10**, schema **8**. The desktop retains its existing
explicit selection controls. Model selection now writes model and thinking together in one
transaction/revision/invalidation; configuration changes and notifications are serialized.

Nine executed pinned-reference service/level-selector cases match the native protocol.
Twenty-three independent clients race cycles without losing an accepted change; observer
events remain ordered, generation uses the resulting level, and restart preserves it.
Post-write/pre-commit fault injection verifies rollback of model/thinking and outbox state.
See [cycle/configuration evidence](pipkin-rust-engine-evidence/model-cycle.md).

Required checks: **1491 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-model-cycle-{workspace,clippy,fmt}.log`. Initial test compilation used
the wrong snapshot field for revision; corrected before the passing run. No runtime failure
was waived. SVC-MODELS-CYCLE moves PA → NC with measured evidence.

Recomputed inventory: **504 rows, 488 non-OS; NC 412, NE 26, NP 37, PA 12, BL 0,
LB 1, OS 16; 435 measured, 7 observed, 8 unverified, 54 not started; 0 qualified**.
Native remains opt-in. Session removal is confirmed as a required built-in service
capability, exposed to non-interactive clients despite no desktop delete button. Its native
root/child teardown, persistent cleanup, invalidation and restart/race work remains open;
the evidence records the concrete contract. Provider identity/OAuth and prompt identity/docs
audits plus broader reliability, installed workflows and release qualification also remain.

## Prior follow-up — prompt-resource lifetimes and provider-path audit

Following **`e92c4f1`**, fixed a reproduced instruction-cache mismatch: toggling offered
tools could reread AGENTS.md mid-session, and a newly created subagent could see different
project instructions/skill metadata from its parent. Native now shares a frozen resource
snapshot per cwd within a live root/child group, while independent roots remain separate.
Tool contributions still update. The last member's retirement releases the cache; reopening
or restarting loads edited resources. Tests verify allocation release as well as behavior.

Measured evidence includes an actual subagent tool run, tool toggles, independent roots,
restart, root retirement while a child remains live, and full-group retirement/reload.
Seven executed pinned-reference cases compare resource-cache lifetimes across cwd/tool/
root changes. See [resource/provider audit](pipkin-rust-engine-evidence/prompt-resource-provider-audit.md).

The source audit also traced image-generation/classifier APIs to classic codemode, and
SDK attribution/installation telemetry to classic/interactive entry points. Those paths
are not installed by Pipkin's shipped durable coding registry or either overlay. Their
five PA rows move to NP for the shipped path; ordinary chat image input, catalog metadata,
configured headers and authentication/billing paths remain required and unchanged.
PRM-AGENTS-CACHE moves PA → NC with measured evidence. D-06 and D-08 are updated to reflect
the implemented catalog and resource-lifetime behavior.

Required checks: **1488 passed, 0 failed, 53 ignored**, clean Clippy and formatting.
Logs: `/tmp/pipkin-prompt-audit-verified-{workspace,clippy,fmt}.log`.
An initial workspace run exposed a race in the accelerated Kimi OAuth mock: it authorized
before the test observed its device code. The mock now waits for that observation before
authorizing; pending polling, header authentication and three-attempt token refresh checks
remain. The earlier failure and test-development corrections are recorded in the evidence.

Recomputed inventory: **504 rows, 488 non-OS; NC 411, NE 26, NP 37, PA 13, BL 0,
LB 1, OS 16; 434 measured, 7 observed, 8 unverified, 55 not started; 0 qualified**.
Native remains opt-in. Remaining audit work includes provider identity, OAuth, session
removal/thinking cycling and prompt identity/docs. Sustained reliability/performance,
installed workflows, release packaging/platform qualification and final Pi retirement
remain required; these row dispositions are not release acceptance.

## Prior follow-up — public catalog refresh and offline recovery

Following **`a98904f`**, native `RefreshModels` now restores and revalidates public
provider catalogs. It preserves cache freshness/ETags, applies only newer metadata,
keeps local configuration overrides, gates requests on configured credentials without
sending those credentials to the public service, and respects proxy settings and
presence of `PI_OFFLINE`. Explicit configuration import includes `models-store.json`.
Startup remains cache-only; build/start do not depend on a live catalog service.

Requests have bounded concurrency, body size, retries and timeouts. Cancellation and
newer refreshes prevent stale cache publication, including while waiting for another
process's file lock. Engine shutdown cancels catalog work. Tests exercise corrupt
cache preservation, malformed data, type-safe chat lookup and provider-key isolation.
An engine-protocol workflow selects a refreshed model, completes a local HTTP coding
turn, restarts offline and completes another turn with the persisted selection.
Eleven cases compare native behavior with original pinned-reference catalog functions.
See [catalog evidence](pipkin-rust-engine-evidence/remote-catalog.md).

Required checks: **1485 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-catalog-verified-{workspace,clippy,fmt}.log`. The chrono dependency
was already cached; no package download was needed. Protocol 9 / schema 8 unchanged.

PCFG-11 and ENV-OFFLINE move PA → NC with measured evidence. Recomputed inventory:
**504 rows, 488 non-OS; NC 410, NE 26, NP 32, PA 19, BL 0, LB 1, OS 16;
434 measured, 7 observed, 8 unverified, 55 not started; 0 qualified**.
Native remains opt-in. Remaining provider/auth/settings audits, wider sustained
reliability/performance, installed workflows and packaging/platform gates still apply.
The public metadata UA does not settle provider-wire identity/attribution decisions.

## Prior follow-up — history qualification and reachability audit

Following **`ea6150f`**, added public-protocol coverage for session rename (other-client
invalidation, Unicode/trim, restart, clear, unchanged history and nonexistent-session
refusal), a real native desktop-backend workflow for compaction/history paging/update/
restart, and real-tool coverage for live custom-shell settings. The desktop workflow is
headless integration, not rendered-window acceptance.

Pinned-source and overlay audit resolves six PA rows: P05-RENAME/API-key configuration
→ NC; custom shell → NE; faux-only deferred polling/classic-only cache warming → NP;
custom replacement-edit producer compatibility → OS under the existing extension exclusion.
Unknown replacement-edited imports continue to refuse explicitly with source preserved.
Ordinary history/omit/Stop/compaction remain required. See
[history/reachability evidence](pipkin-rust-engine-evidence/history-reachability-audit.md).

Required checks: **1468 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-history-audit-final-{workspace,clippy,fmt}.log`. An initial test-target
mistake and test-only retained engine handle are recorded in the evidence; an initial
Clippy single-match suggestion was applied and Clippy/fmt rerun.

Recomputed inventory: **504 rows, 488 non-OS; NC 408, NE 26, NP 32, PA 21, BL 0,
LB 1, OS 16; 432 measured, 7 observed, 8 unverified, 57 not started; 0 qualified**.
Native remains opt-in; this is measured progress and scope reconciliation, not cutover.

Next implementation: **PCFG-11 / ENV-OFFLINE**. The pinned built-in path really restores
and revalidates provider catalogs; native currently only rereads local configuration.
The audit records required cache freshness/ETag, generation timestamps, auth gating,
bounded retries, cancellation/stale-publication protection and offline presence semantics.
Provider/auth audits, wider reliability/performance and installed/platform/release gates remain.

## Prior follow-up — durable queued summary placement

Following **`138d879`**, schema **8** (protocol **9**) adds durable built-in summary
writes. Manual/background results queue while busy, place before selected inputs at
tool/final boundaries, and survive Stop/restart. Equal cuts are allowed; only backward
cuts become stale. Usage/submission/terminal classification is atomic; boundary markers
make post-tools placement and one-at-a-time steering restart-safe. A per-session bound
retains up to 64 accepted nonblocking summaries without another provider call on overflow.
See [queued-write evidence](pipkin-rust-engine-evidence/queued-summary-writes.md).

Actual SIGKILL tests cover both sides of the tool boundary and busy summary submission:
no lost write, extra steer, duplicate summary usage or replayed completed shell effect.
Runtime/store tests cover Stop, final/tool ordering, capacity recovery, rollback and
schema-7 backup/history preservation. Four original pinned-reference inbox cases are
captured; native placement and user selection have bounded function differentials.
This does not qualify the full reference scheduler, desktop or live-provider lifecycle.

Required checks: **1465 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-summary-writes-final-{workspace,clippy,fmt}.log`. An initial Clippy
test-module placement warning was fixed and Clippy/fmt rerun; earlier failed assertions
and sandbox-only test failures remain recorded in the evidence.

`Q-WRITES` moves **PA → NC**, measured. Recomputed inventory: **504 rows, 489 non-OS;
NC 406, NE 25, NP 30, PA 27, BL 0, LB 1, OS 15; 432 measured, 7 observed,
8 unverified, 57 not started; 0 qualified**. Native remains opt-in.

Next: remaining context replacement/history and provider/settings/auth audits, wider
reliability/performance, installed migration/workflows, packaging/CI/platform qualification,
then native default and Pi/Node removal. Extension compatibility stays out of scope.

## Prior follow-up — durable compaction runtime

Journal foundation **`74898a4`** is now activated by schema **7** (protocol **9**).
All compaction scopes admit durable jobs; startup/reopen resumes selected requests,
saved responses and absolute retries. Usage, summary placement and settlement commit
atomically. Stable overflow identities close the summary/generation handoff crash window.
Foreground Stop, including orphaned-child abort, reaches the journal and live drivers;
background work survives. Recovery pages ids through an indexed query, with eight recovery
futures and a separate eight-permit summary pool. See [runtime evidence](pipkin-rust-engine-evidence/compaction-runtime.md).

Pinned-reference source audit confirms retry settings stay live at response classification;
attempt counts and previously committed deadlines survive restart. An intermediate pinned
retry-policy approach was replaced to preserve this behavior. The new live-policy regression
covers changing the settings while a response is pending.

Final required checks: **1455 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-compaction-runtime-final-{workspace,clippy,fmt}.log`. Tests include actual
SIGKILL at admission/selection/response/classification and overflow handoff, ordered blocking
recovery, background startup, storage-failure reopen, late-summary races, eighteen queued jobs
under an eight-driver limit, orphaned-child Stop, and schema-6 backup/preservation. Earlier
targeted compile errors and their corrections are retained in the evidence; no failing runtime
test was waived. No GUI/live-provider/installed-platform acceptance is inferred.

`CTX-CHECKPOINT` moves **PA → NC** with measured runtime evidence, not full qualification.
Recomputed inventory: **504 rows, 489 non-OS; NC 405, NE 25, NP 30, PA 28, BL 0, LB 1,
OS 15; 431 measured, 7 observed, 8 unverified, 58 not started; 0 qualified**.

Next: **Q-WRITES is confirmed required built-in behavior.** The pinned reference's manual
and background summaries use durable write submissions, queued when the conversation is
busy and placed at a boundary. Native still appends at classification. Implement durable
queued placement and reconcile boundary ordering, Stop and recovery before qualifying
compaction. Context replacement/history, provider/settings/auth audits, wider reliability,
performance, packaging and platform gates remain. Native stays opt-in; Pi retirement is not
ready. Pi extension compatibility remains out of scope.

## Previous follow-up — durable compaction journal foundation

The Stop/compaction lifecycle fix is committed as **`0721977`**. Schema **6** now stores
stable compaction identities and select/summarize/retry/terminal progress, selected request
input/options without credentials, saved provider responses, absolute deadlines and revisioned
Stop state. A summary/usage/terminal transaction either commits once or rolls back completely.
Conversation Stop persists foreground journal intent even when no coding run is open; background
scope is preserved. See [journal evidence](pipkin-rust-engine-evidence/compaction-journal.md).

Required checks: **1445 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-compaction-journal-{workspace,clippy,fmt}.log`. Migration and store-reopen
tests are measured; this is not evidence of runtime compaction recovery or a worker crash test.
`CTX-CHECKPOINT` remains **PA**. The current driver does not yet produce or consume these jobs.
Next: integrate journal admission/selection/execution/classification/recovery for every
compaction scope, including overflow/generation handoff, then test phase-boundary crashes.
Advance the schema at activation so this intermediate journal-only runtime cannot ignore
active jobs written by the recovery-capable runtime.

Inventory remains **504 rows, 489 non-OS; NC 404, NE 25, NP 30, PA 29, BL 0, LB 1, OS 15**;
**431 measured, 7 observed, 8 unverified, 58 not started; 0 qualified**. Native stays opt-in.

## Current follow-up — Stop and compaction lifecycle

Request cancellation is committed as **`7c27693`**. Stop now reaches every manual
compaction even when no coding run is active; per-operation drop guards clean up the
registration, and summary commit is serialized with Stop and rechecks cancellation.
Threshold background compaction retains its independent scope. Tests cover concurrent
manual compactions, subsequent reuse, background survival without restoring the stopped
instruction, Stop after completion, and unstarted sequential tools. See
[lifecycle evidence](pipkin-rust-engine-evidence/stop-compaction-lifecycle.md).

Final required checks: **1439 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-stop-compaction-final-{workspace,clippy,fmt}.log`. The first workspace run
failed a new test that incorrectly assumed built-ins were sequential; the corrected fixture
explicitly declares its test tool sequential, with no production scheduling change.

Source audit confirmed a further required gap: Pi persists compaction selection and retry
checkpoints, while native compaction futures are volatile. New row **CTX-CHECKPOINT** records
this explicitly. Next batch: durable compaction operations/checkpoints, restart/Stop recovery,
retry deadlines and summary placement. This is built-in context management, not extensions.
Current inventory: **504 rows, 489 non-OS; NC 404, NE 25, NP 30, PA 29, BL 0, LB 1, OS 15**;
**430 measured, 7 observed, 8 unverified, 59 not started; 0 qualified**. Native remains opt-in.

## Current follow-up — connection-scoped request cancellation

The inactive-profile migration is committed as **`03b7e5e`**. The Pi timeout-path audit
confirmed generic RPC cancellation is used by the desktop. Native protocol **9** now has
`CancelRequest` on the control lane, with per-connection registration before enqueue,
cancellation-safe prompt/queue/wait suspension points, cooperative compaction cancellation,
and retained task capacity for started blocking jobs. Desktop timeouts send cancellation
without changing unknown-outcome reconciliation. See [cancellation evidence](pipkin-rust-engine-evidence/request-cancellation.md).

Required final checks: **1435 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-request-cancel-final-{workspace,clippy,fmt}.log`. `XPORT-CANCEL` moves PA → NC
with measured regression evidence; full transport differential remains unqualified.
Current inventory: **503 rows; NC 404, NE 25, NP 30, PA 28, BL 0, LB 1, OS 15**;
**428 measured, 7 observed, 10 unverified, 58 not started; 0 qualified**.
Native remains opt-in. Remaining core history/settings/provider, Stop/compaction lifecycle,
resource and installed/platform qualification work still precedes default cutover.

## Current follow-up — migrate histories with inactive Pi profiles

Historical Pi extension selections no longer block conversation migration. Exact profile
bytes and hashes are preserved in the atomic import manifest; unknown formats and old paths
remain inactive provenance. Root imports pin empty native profiles. Unreadable/oversized
files and changed snapshots still refuse safely. The rich-context importer regression now
has a nonempty selection; the desktop backend test confirms ownership transfer when a
selection appears after discovery. See [migration evidence](pipkin-rust-engine-evidence/inactive-profile-import.md).

Required checks: **1430 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-import-scope-{workspace,clippy,fmt}.log`. No GUI or live-provider acceptance
is inferred. Counts remain **503 inventory rows, 488 non-OS; 0 qualified**. Native is opt-in.
Next: remaining core/history and cancellation audits, reliability and release qualification.

## Current owner decision — Pi extensions removed from release scope

The user explicitly stated on 2026-10-11 that Pi extensions are not necessary for Pipkin.
The plan now has an overriding scope amendment. No more TypeScript loader/port, extension
remote-service, hot-reload or optional extension API parity work is required. Existing tested
native extension/profile/wait code remains available, but does not delay native default.

Fifteen extension-specific ledger rows are now **OS (out of scope by owner decision)**,
including their historical evidence. They are not marked passed or absent from the baseline.
Inventory remains **503 rows: NC 403, NE 25, NP 30, PA 29, BL 0, LB 1, OS 15**;
**488 non-OS rows**, none fully qualified. Evidence totals retain optional work:
427 measured, 7 observed, 10 unverified, 59 not started. This scope change supersedes historical
extension-blocker statements below and in ADR 0006.

Import selection policy is reconciled above. Next work is remaining core behavior and
migration, reliability,
provider/settings audits, packaging/install/CI and release qualification. Preserve source
history/state and credentials. Built-in coding tools, providers and subagents remain required.

## Current follow-up — durable input waits

Native profiles are committed as **`67ba56f`**. Input waits/reliability fixes are committed as
**`2066443`**. Protocol **8** / schema **5** adds durable
prompt/steer/follow-up waits through the engine protocol and both extension hosts. Results
come from the input's committed final answer or unanswered reason, including preserved keyed
Pi submissions. Wait capacity and reply delivery are bounded separately from normal/control
calls. Cancelling a wait leaves the input running. See
[wait evidence](pipkin-rust-engine-evidence/prompt-waits.md).

Twelve native wait tests cover final text, queue/Stop/error outcomes, registration races,
restart/storage recovery, capacity, unread replies, Rust/process cancellation and eleven
executed Pi service-result projection cases. Startup additionally recovers queues left after
settlement but before the next run was started. The full suite exposed an endpoint bind-error
lock-release race; a dedicated RAII lock guard and regression cover inherited descriptors on
that error path. Final checks: **1430 passed, 0 failed, 53 ignored**, Clippy/formatting clean;
logs `/tmp/pipkin-wait-{workspace,clippy,fmt}-complete.log`. The original endpoint failure is
retained in `/tmp/pipkin-wait-workspace.log`.

Ledger: **503 rows; NC 404, NE 29, NP 30, PA 30, BL 9, LB 1; 427 measured, 7 observed,
10 unverified, 59 not started, 0 currently failed, 0 qualified**. Input waits are implemented;
general durable identities for historical write/compaction and unkeyed Pi submissions remain
open. Remote extension services, reload and TypeScript package compatibility are now out of scope
under the owner decision above. Provider/history gaps and release qualification remain. Native remains opt-in.

## Previous follow-up — persistent native extension profiles

Desktop byte pressure/Stop is committed as **`9047b57`**. Protocol **7** / schema **5** now
persists native defaults and per-session selections, preserves empty pins, rejects conflicting
active selections, and supports explicit default clearing at startup. Plugin-free Pi imports
pin empty atomically rather than inheriting unrelated native defaults. First-pin storage faults
keep work refused until explicit reopen persists the original selection.

Thirteen observations from the executed pinned Pi profile helpers match native protocol results;
additional native tests cover selection races, retirement/stale questions, invalid profiles,
process identities/argv, storage faults and schema backup/upgrade. See
[profile evidence](pipkin-rust-engine-evidence/extension-profiles.md). Required checks:
**1417 passed, 0 failed, 53 ignored**, Clippy/formatting clean; logs
`/tmp/pipkin-profiles-{workspace,clippy,fmt}-final.log`. Earlier failed checks are recorded in
that evidence file. No native-window or live-provider check was performed in this batch.

Ledger: **503 rows; NC 404, NE 28, NP 30, PA 31, BL 9, LB 1; 426 measured, 7 observed,
10 unverified, 60 not started, 0 currently failed, 0 qualified**. Profile mechanisms are native
equivalents; existing TypeScript package execution stays blocked. Next: extension wait/control,
remote services and reload, remaining provider/history gaps and full release qualification.
Native remains opt-in, and Pi remains the default.

## Previous follow-up — desktop byte pressure and Stop

Bounded provider progress is committed as **`baf8eff`**. Native desktop events now use
128-event / 8 MiB owned-payload admission, charged until the controller applies each batch.
One indivisible oversized event is admitted alone without losing its contents. Stop uses a
separate bounded worker and preserves submission/connection ordering; shutdown releases
blocked UI producers. See [desktop pressure evidence](pipkin-rust-engine-evidence/desktop-pressure.md).

The real engine/core/storage tests prove durable Stop before UI consumption, exact saved
history and journal recovery with/without restart, and shutdown under byte pressure. Required
checks: **1404 passed, 0 failed, 53 ignored**, Clippy and formatting clean. Logs:
`/tmp/pipkin-ui-pressure-{workspace,clippy,fmt}.log`. No native-window soak was run.
Ledger counts remain **503 rows; 423 measured, 7 observed, 0 currently failed, 10 unverified,
63 not started; 0 qualified**. Lifecycle/catalog/request admission and total heap limits remain
separate work. Next: extension/profile compatibility and remaining provider/history gaps,
while completing the broader qualification gates. Native remains opt-in.

## Previous follow-up — bounded provider progress

Shared event delivery is committed as **`e87809f`**. The provider generation loop no longer
uses an unbounded stream-event channel. An engine-owned sink commits bounded batches directly,
with 40 ms flushing for small/silent streams and cancellation on checkpoint failure. Text is
split losslessly; indivisible oversized tool identities commit immediately. Late callbacks
cannot mutate completed entries or retain the engine. See
[provider progress evidence](pipkin-rust-engine-evidence/provider-progress.md).

Focused burst/Unicode, count/byte backlog, stale callback, failure, silent-stream and synchronous
producer Stop regressions pass. Required checks: **1395 passed, 0 failed, 53 ignored**;
Clippy and formatting clean (`/tmp/pipkin-progress-final-{workspace,clippy,fmt}.log`). The two
release streaming benchmarks passed: no missing markers, all cadence p95 values below 50 ms,
and all 24 scaling cases produce the expected output lengths. CPU/frame growth is approximately linear;
64-byte chunks show extra write amplification, recorded as a remaining performance concern.
Raw evidence: `fixtures/native-bench/2026-10-11-provider-progress/`.
At that revision, frontend bus byte/control-pressure work remained next; this change does not cap the full
assistant message or every parser allocation. Ledger counts remain **503 rows; 423 measured,
7 observed, 0 currently failed, 10 unverified, 63 not started; 0 qualified**. Native remains opt-in.

## Previous follow-up — shared event delivery

External attachment is committed as **`cb98d0a`**. The next change shares one immutable typed
and encoded event payload across subscriber queues. Private pipes and external sockets write
the cached body with a connection-specific envelope; in-process consumers retain typed frames.
Oversized encoding stops at the presentation budget and becomes a resync marker. Subscriber
pressure, reply barriers, auth scoping and per-client sequence numbers are preserved.
See [shared delivery evidence](pipkin-rust-engine-evidence/shared-event-delivery.md).

Initial focused and workspace tests passed. Clippy identified a large output-enum variant;
boxing its reply arm keeps the shared event wrapper small. Final required checks passed:
**1389 passed, 0 failed, 53 ignored**, Clippy and formatting clean. Logs:
`/tmp/pipkin-fanout-final-{workspace,clippy,fmt}.log`. The unchanged 1600-turn headless soak
passes the 100 MiB budget: **29,164 KiB idle / 39,912 KiB sampled peak**, 13 idle fds,
one process. Raw measurements are under `fixtures/native-bench/2026-10-11-shared-events/`.
Latency/CPU were higher than the older positional run; the evidence records both and does
not infer a speed improvement from unmatched runs. Counts remain **503 rows; 423 measured, 7 observed,
0 currently failed, 10 unverified, 63 not started; 0 qualified**. Frontend/provider-internal
queue budgets, extension compatibility, transport differentials and release gates remain open.

## Previous follow-up — external native attachment

Implemented on top of **`e738037`**, protocol **6** / schema **4**. A separately owned
foreground native server exposes an explicit private Unix endpoint. Both sides verify the
OS user; clients cannot shut down the server. Bounded handshake/client capacity, shared
runs/questions, disconnect, reconnect and stale-socket recovery are covered. The desktop
supports `--engine-attach` and endpoint-specific state; `--engine-server` runs without GPUI.
See [attachment evidence](pipkin-rust-engine-evidence/external-attachment.md) and
[user commands](../docs/native-attachment.md).

Required checks: **1385 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-attach-final-{workspace,clippy,fmt}.log`. The first full run exposed
store lock retention through inherited descriptors; a failing deterministic regression
reproduced it, and explicit owner unlock fixes both store and endpoint lifetimes. Failed
runs remain recorded in the attachment evidence. Counts: **503 rows; NC 404, NE 25,
NP 30, PA 31, BL 12, LB 1; 423 measured, 7 observed, 0 currently failed, 10 unverified,
63 not started; 0 qualified**. Pi-specific attachment flags have explicit native equivalents,
not literal translations. Linux scripted/process evidence does not qualify live GUI, macOS,
other OS users, transport differentials or performance. Native remains opt-in.

## Previous follow-up — native session attachments and retirement

Implemented on top of **`c3134df`**, protocol **6** / schema **4**. OpenSession now holds
client attachment demand; ReleaseSession and disconnect release it without cancelling work.
Idle retirement releases provider pools by affinity ID, Codex fallback/debug state, prompt
caches, questions and extension tasks. In-flight work holds retirement; short disconnects
allow readoption. Old extension generations cannot write into new ones, and reopening waits
for prior teardown. The desktop releases its previous conversation when switching.
See [lifecycle evidence](pipkin-rust-engine-evidence/session-lifecycle.md).

Required checks: **1371 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-lifecycle-release-{workspace,clippy,fmt}.log`. The first workspace run
found a device-login mock race, now made deterministic; the second found a protocol fixture
pinned to version 5, now updated. Failed logs are retained in the evidence record. Final
review also made first provider identity assignment atomic and contained deactivation panics.
Counts remain **503 rows; 420 measured, 7 observed, 0 currently failed, 10 unverified,
66 not started; 0 qualified**. External attachment/server re-adoption, full differentials,
extension package compatibility and release qualification remain open. Native remains opt-in.

## Previous follow-up — verified WebSocket proxy TLS

Constrained-tool integration is committed as **`1077005`**. Codex WebSockets now support
HTTP and HTTPS CONNECT proxies for WS and WSS targets. TLS to the proxy and TLS inside the
tunnel verify certificates independently. Tests cover all four combinations, percent-decoded
proxy authentication without forwarding it to the target, NO_PROXY, connection reuse/close,
timeouts/cancellation, and refused/malformed/oversized proxy responses. Verification also
found and repaired grammar response conversion in the cached continuation path.
See [transport evidence](pipkin-rust-engine-evidence/websocket-proxy-tls.md).

Required checks: **1358 passed, 0 failed, 53 ignored**, Clippy and formatting clean. Counts remain **503 rows; 420 measured, 7 observed,
0 currently failed, 10 unverified, 66 not started; 0 qualified**. Remaining transport work:
conversation lifecycle cleanup, shared-client HTTP CONNECT parity, auth proxy error parity,
external attachment and full transport differentials. Native remains opt-in.

## Previous follow-up — constrained tool declarations and execution

The positional context/paging batch is committed as **`ea0a98b`**. The next batch carries
`constrainedSampling` through native tool specs and durable declarations. Strict JSON-schema
sampling now reaches the applicable adapters; Responses, Azure and Codex support grammar
custom tools, declared argument properties, paired replay, and interrupted-input preservation.
Fifteen declaration cases match executed pinned Pi helpers. A native runtime test executes a
custom grammar tool and verifies durable replay after restart. See
[constrained-tool evidence](pipkin-rust-engine-evidence/constrained-tools.md).

Required checks: **1353 passed, 0 failed, 53 ignored**, Clippy and formatting clean. Counts remain **503 rows; 420 measured,
7 observed, 0 currently failed, 10 unverified, 66 not started; 0 qualified**. This closes
specific implementation gaps within already measured adapter rows, not full release qualification.
Native remains opt-in; extension compatibility and the other release gates remain open.

## Previous follow-up — positional system context

Implemented on top of **`7872c91`**. Protocol **5** / schema **4** preserve ordered system
sections and tool declarations in durable history. Runtime commits minimal patches and
compaction baselines; imported systems now participate in context equality and replay.
Provider encoding retains supported positions/tool anchors or collapses the current state.
Ten pure planner/replay cases match output captured by executing the pinned Pi reference.
See [context and paging evidence](pipkin-rust-engine-evidence/positional-system.md).

Verification also reproduced and fixed the subagent inspector paging race: an identical live
page no longer cancels an older page in flight. Hidden system-only pages advance the raw cursor.
Required checks: **1346 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
The repeated 1600-turn Linux/glibc headless soak passes at **28,940 KiB idle / 40,148 KiB peak**,
with 13 idle fds and one process. Desktop/platform/performance qualification remains open.

Ledger: **503 rows; 420 measured, 7 observed, 0 currently failed, 10 unverified,
66 not started; 0 qualified**. PRM-PATCH is measured with bounded differential evidence;
provider/extension/installed acceptance is not inferred from those pure cases. Native remains opt-in.

## Previous follow-up — usage subset preservation

Implemented on top of **`e8993de`**. Protocol, adapters, persistence/import, aggregate views,
core and both desktop adapters now retain optional `reasoning` and `cacheWrite1h` counts.
The usage panel shows them as subsets; costs and total tokens do not double count them.
The existing Codex reference-golden comparisons now assert those fields too.
See [usage evidence](pipkin-rust-engine-evidence/usage-subsets.md).

Required checks: **1334 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Ledger counts remain **503 rows; 419 measured, 7 observed, 0 currently failed, 10 unverified,
67 not started; 0 qualified**. GEN-17 / USE-LEDGER retain partial qualification status, but
no longer have the missing-counter implementation gap. Positional system prompt updates,
extension compatibility, provider/external attachment gaps, and release qualification remain.

## Previous follow-up — durable delivery and memory reclamation

The prior import/retry batch is committed as **`63bd9ae`**. This follow-up (2026-10-11)
implements protocol **4** / schema **3**: transactionally recorded invalidations,
acknowledged bounded catch-up pages, explicit retention resets, bounded per-client and
reader presentation, separate reply/control capacity, and serialized commit/publication.
Catch-up preserves paged history and the selected subagent; a missed final event is recovered
without requiring subsequent live traffic. A real SIGKILL before publication is covered.

Linux/glibc private workers now reuse one allocator arena, keep large allocations releasable,
and trim free pages before settlement. The unchanged 100 + 1500-turn headless soak went from
**114,804 KiB idle / 116,692 KiB sampled peak** to **27,456 / 39,840 KiB**. Second-half
slope fell from **54.848 to 1.440 KiB/turn**; idle fds stayed 13 and there was one process.
This fixes the measured 100 MiB budget failure, not every desktop/platform memory gate.
See [implementation, tests and raw measurements](pipkin-rust-engine-evidence/durable-delivery-memory.md).

Final required checks: **1331 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Ledger: **503 rows; 419 measured, 7 observed, 0 currently failed, 10 unverified,
67 not started; 0 qualified**. Both new measured rows are explicitly partial. Historical
failures remain in their evidence records. Dispositions remain NC 402, NE 23, PA 33,
NP 30, BL 14, LB 1, RC 0. Native remains opt-in; Pi remains the default.

## Historical follow-up — plugin import protection and durable retries

Implemented on top of `92629ec` (2026-10-11). See
[import/retry evidence](pipkin-rust-engine-evidence/import-protection-durable-retries.md).
Plugin profiles are now validated during discovery and rechecked before import activation;
unsupported selections stay on Pi, without a native session or ownership transfer. Native
retry deadlines, handled error entries and failure counts survive restart; Stop cancels the
restored wait. Engine schema **2** adds generation state with a tested private v1 backup;
protocol **3** adds explicit import `serverDir` and prevents an older worker from accepting
an import without the new guard.

Ledger: **503 rows; 417 measured, 7 observed, 1 failed, 10 unverified, 68 not started;
0 qualified**. Final required checks: **1319 passed, 0 failed, 53 ignored**; Clippy and
formatting passed. Earlier attempts, including a subagent-inspector timeout, are retained in
the evidence record. NC 402, NE 23, PA 33, NP 30, BL 14, LB 1, RC 0. Import protection is
measured refusal, not TypeScript execution compatibility. The next priorities are bounded
durable event delivery and the unresolved memory-soak failure; full qualification remains open.

## Historical reconciliation — 2026-10-11, `e4c3a8c`

Claude subscription integration and its two observed UI fixes are committed in **`e4c3a8c`**.
Browser and copy-code sign-in, saved-credential reuse after restart, a forced-expiry live refresh,
a real Opus 5.5 coding turn, and imported-Pi-fixture continuation/Stop/reopen passed. Copy-code
login initially exposed a missing input mapping; successful connection then exposed misleading
management-page copy. Both are fixed. The final restarted desktop returned
`COPY_CODE_RESTART_OK` using saved Anthropic OAuth.
[Detailed evidence](pipkin-rust-engine-evidence/claude-subscription.md) retains failed attempts
and distinguishes harness errors, actual fixes and unverified cases.

The remaining ledger was reconciled against code and assertions; see
[remaining work and corrections](pipkin-rust-engine-evidence/parity-reconciliation.md).
It has **503 rows: 415 measured, 7 observed, 1 failed, 11 unverified, 69 not started;
0 qualified**. Dispositions: NC 401, NE 23, PA 33, NP 30, BL 15, LB 1, RC 0.
These labels include partial evidence and tests demonstrating known differences, not 415
completed capabilities. Required source checks passed: **1304 tests, 0 failed, 53 ignored**,
Clippy, and formatting. The subsequent reconciliation changes documentation only.

The native engine remains opt-in; Pi remains the default. Actual gaps include plugin-profile
import detection, durable retry state, outbox/backpressure, usage counters, positional system
prompt updates, external attachment, TypeScript facet compatibility and the memory-soak failure.
Historical sections below retain their original evidence; this summary, sections 1–3 and 7–8
are the current interpretation. No default cutover or parity qualification occurred.

## 1. Architecture decisions

Full records: `llm-docs/pipkin-rust-engine-adr/` (index in its `README.md`).

| ADR | Decision (summary) | Status |
| --- | --- | --- |
| 0001 Execution topology | One shipped `pipkin` executable; a private worker mode runs the GPUI-free `pipkin-engine` as an **owned child** over inherited pipes; one engine process for all conversations; in-process host (same API) for tests/headless/benchmark; not a sandbox | Accepted; implemented (`--engine-worker`, `App/adapters/native/supervisor.rs`, headless `pipkin-engine` binary) |
| 0002 Protocol and transport | Length-prefixed JSON frames (32 MiB max); typed `Call`/`Reply`/`Event`; `epoch`+`seq` events, session `revision`, snapshot on gap; control lane for Stop/Shutdown; protocol version mismatch refused | Accepted; protocol **5**, positional system declarations, durable invalidation outbox/acknowledged catch-up and bounded delivery implemented; serialize-once fan-out/external attachment open |
| 0003 Durable store and recovery | One OS-locked SQLite store per data root; explicit schema v4, backup-before-migrate, newer refused untouched; short transactions; state-driven run loop; fail closed on storage error; `synchronous=FULL` (reference `NORMAL`) | Accepted; implemented; actual worker SIGKILL and injected disk-full/read-only cases tested; v1 → current, v2 → v3 and v3 → v4 backup/migration tested; power loss unverified |
| 0004 Stop and queue | Reference queue boundaries and outcomes; durable Stop intent then cancellation; retirement by `entries.retired` flag instead of `pi.cancelled-input` omit-edit entries | Accepted; implemented; `all` delivery modes tested; differential qualification and general Stop-after-completion race pending |
| 0005 Tool replay and subagent recovery | Intent before effect; unsafe-by-default replay; `subagent` is the replay-safe registration with an atomic `(parent session, call)` → child mapping and once-only delivery; recorded-contract check | Accepted; implemented incl. recorded-contract check; mismatch, disabled recovery and Stop-child tests pass |
| 0006 Extension compatibility | Native versioned extension contract + out-of-process protocol; existing TypeScript facet packages need Node, so their rows stay **blocked** and run on the **legacy bridge** (Pi) | Proposed; native contract implemented (`E/extensions`, `docs/native-extensions.md`); TS facets blocked |
| 0007 Credential ownership and migration | Own data/config roots; `auth.json`-compatible private file (0600) until a qualified OS secret store; metadata-only lookup; explicit import, no sync; Claude subscription OAuth and explicit import implemented; failed refresh never changes billing credentials | Accepted; implemented (`E/auth`, explicit credential import) |

## 2. Baseline inventory pointers

| What | Where |
| --- | --- |
| Reference audit (RE-01) | `llm-docs/pipkin-rust-engine-audit/` — `README.md`, `engine-tools-durable-usage.md`, `prompt-settings-layout.md`, `service-api.md`, `extensions-subagents.md`, `providers-auth.md` |
| Parity ledger | `llm-docs/pipkin-rust-engine-parity.md` (503 rows; 0 qualified; current evidence: 420 measured, 7 observed, 0 currently failed, 10 unverified, 66 not started; partial evidence is explicit in each row) |
| Frozen reference fixtures (RE-02) | `fixtures/pi-sessions/` (8 scenarios a–h with `session.sqlite`, `requests.jsonl`, `view.json`, `history.json`; `BASELINE.md` for the reference measurements) |
| Evidence | `llm-docs/pipkin-rust-engine-evidence/` — `performance.md` (RE-22), `packaging.md` (RE-23), `native-walkthrough.md` (guarded GUI walkthrough) |
| Claude subscription handoff | `llm-docs/pipkin-rust-engine-claude-subscription-handoff.md` |
| Known reference defects | Parity ledger §19 (K-01..K-11) |
| Intentional improvements | Parity ledger §20 (I-01..I-13) |
| Open owner decisions | Parity ledger §21 (D-01..D-11), with the implemented state and open sign-offs; D-11 is resolved |
| Providers | Ledger §3 (34 adapter rows), §4 (43 provider ids = 42 built-in + `faux`), §5 (custom providers/config) |
| OAuth/auth | Ledger §6 (generic lifecycle, onboarding contract, every OAuth flow, Claude subscription rows `CLS-*`) |
| Desktop workflows | Ledger §16 (37 Pi `e2e.rs` workflows with their native counterparts, 6 `real_pi.rs` tests, flags) |
| Native extensions | `docs/native-extensions.md` |
| Platforms | `docs/platforms.md` (Linux qualification scope; macOS experimental; Windows not supported) |
| Product contracts | `docs/{extensions,subagent-inspector,support,packaging,platforms,scorecard}.md` |

## 3. Work packages

Owner: **Opus** = Claude Opus 5.5 main implementation; **Codex** = reserved final integration.
Status vocabulary (plan §13): planned / in progress / blocked / implemented-but-unverified /
qualified. Nothing is qualified. "Implemented-but-unverified" means the code is merged and has
native tests (cited in the ledger), but the full qualification differential and applicable
platform/provider gates have not closed it; Claude has separate live evidence.

| ID | Work package | Owner | Status | Evidence pointer |
| --- | --- | --- | --- | --- |
| RE-01 | Pinned-source/overlay inventory and complete parity ledger | Opus | in progress | Ledger reconciled at `e4c3a8c`; open: 33 `PA` rows, provider-id count (D-09), owner decisions D-01..D-11 |
| RE-02 | Frozen behavioral fixtures and performance baseline | Opus | implemented-but-unverified | `b2ff206`: `fixtures/pi-sessions/` 8 scenarios + `BASELINE.md`; reference package/runtime artifact hashes still TODO (§4) |
| RE-03A | Non-reserved provider/auth feasibility and inventory | Opus | implemented-but-unverified | All non-reserved adapters and OAuth flows implemented and tested against local mocks (ledger §3–§6); no live provider call |
| RE-03B | Claude subscription feasibility review | Codex | reviewed; user-confirmed approval | `pipkin-rust-engine-evidence/claude-subscription.md`; user-confirmed approval; live access subsequently exercised in RE-13B |
| RE-04 | Context, durable-task and extension-state import spike | Opus | implemented-but-unverified | `8290556`; all 8 reference fixtures import and continue equivalently (`rti::reference_fixtures_import_open_and_continue_equivalently`); plugin profile protection now detects/refuses nonempty selections (IMP-PLUGIN-STATE) |
| RE-05 | Extension compatibility spike and ADRs | Opus | in progress | ADR 0006 still Proposed; native ports of the two baseline examples (`ext::`, `extp::`); TS facets blocked |
| RE-06 | Typed engine contract and private worker transport | Opus | in progress | Protocol v5; typed frame/import tests; durable outbox, acknowledged bounded catch-up, gap/reset and pressure tests implemented; external attach/fan-out and complete qualification remain |
| RE-07 | Engine library, host/supervisor and headless harness | Opus | implemented-but-unverified | `nsup::` (7), `ew::` (2), `wp::` (2: SIGKILL + second-writer refusal), headless `pipkin-engine` binary |
| RE-08 | SQLite store, migrations, lock, request ledger and outbox | Opus | in progress | `st::` (12), `wp::admission_survives_sigkill_and_is_found_by_lookup_after_restart`; durable invalidation outbox implemented; schema v4 with tested v1/v2/v3 upgrade backups; injected disk-full/read-only faults and damaged-database refusal pass (`rtf::`, `cp::`) |
| RE-09 | Durable scripted generation and NativeBackend | Opus | implemented-but-unverified | `rt::` (19), `ne2e::` (20 workflows), `nt::` (9); observed in `native-walkthrough.md` |
| RE-10 | Stop, queue, steering and recovery | Opus | implemented-but-unverified | Stop/queue/steer/restart measured (`rt::`, `ne2e::`); `all` modes tested (`rts::`); durable retry wait/count and Stop across restart now pass (REC-RETRY); general Stop-after-completion race remains |
| RE-11 | Coding tools and execution environments | Opus | implemented-but-unverified | `5768dda`; `trd::`/`twr::`/`ted::`/`tbs::`/`tout::`/`trun::`/`tsch::` incl. reference-captured goldens; corpus tests ignored with no recorded run; sequential-tool rounds tested (`rt::tool_rounds_run_in_parallel_unless_a_tool_is_sequential`); reference setting override needs comparison |
| RE-12 | First provider and real coding slice | Opus | implemented-but-unverified | Walkthrough §3 (real `write` tool via an `openai-completions` custom provider to a local stub); perf harness uses the same path; Claude live coding slice now passed (RE-13B) |
| RE-13A | Generic credentials and non-reserved providers | Opus | implemented-but-unverified | `dd43920`, 10 wire adapters + `provider/api/common`; `auth::`, `oauth::`, adapter suites; Codex adapter goldens are reference captures |
| RE-13B | Claude subscription integration | Codex | implemented; live smoke passed | `pipkin-rust-engine-evidence/claude-subscription.md`; browser/copy-code sign-in, refresh, saved reuse and Pi-fixture Stop/reopen passed; full qualification pending |
| RE-14 | Context/resources, attachments, models and accounting | Opus | implemented-but-unverified | `384da75`, `e4fe919`; `drv::`/`prm::`/`res::`/`cmp::`; usage subsets and positional system patches now preserved and tested; see follow-up evidence |
| RE-15 | Durable subagent/task scheduler and recovery | Opus | implemented-but-unverified | `rt::subagents_run_once_per_call_deliver_their_answer_and_are_listed`, `rt::a_subagent_call_interrupted_by_a_restart_resumes_its_one_child`; disabled/contract-mismatch and Stop-child recovery tests pass (`rtf::`); actual child-run SIGKILL passes (`cp::`) |
| RE-16 | Native extension compatibility implementation | Opus | implemented-but-unverified (native) / blocked (TS facets) | `01e0511`, `e4fe919`; `--engine-extension`; TypeScript facet packages stay `BL / LB` (ADR 0006) |
| RE-17 | Multi-session engine, history and child presentation | Opus | implemented-but-unverified | `7ba1340` paged subagent inspection (`core-sub::`, `ui-sub::`, `nt::child_history_pages_by_cursor_and_the_selected_child_refreshes_live`); history paging flat after fixes (perf-ev) |
| RE-18 | Compaction, questions and remaining audited services | Opus | implemented-but-unverified | `rt::manual_and_overflow_compaction_summarize_older_context`, `rt::extension_questions_are_published_answered_and_validated`; background/threshold runtime paths untested; `waitForPrompt`, `RemoveSession` absent (PA) |
| RE-19 | Production importer and atomic backend ownership | Opus | implemented-but-unverified | `8290556`, `21ff99a` (protocol v2 `Import*`, desktop palette flow); desktop ownership record written in the import's copy transaction; Pi mode read-only for owned conversations (core + Pi adapter boundary), explicit release from the palette (D-11, IMP-OWNERSHIP) |
| RE-20 | Desktop integration including goals, offline state and Changes | Opus | implemented-but-unverified | `6b7980f`; `ne2e::`, `nmap::`; walkthrough observed onboarding, first conversation, Changes, SIGKILL recovery |
| RE-21 | Full parity and fault qualification | Opus (+ Codex for reserved rows) | planned | Ledger refreshed; no backend-parameterized differential harness; storage-fault injection and actual worker crash boundaries pass; full differential, power-loss and remaining ledger assertions are open |
| RE-22 | Integration advantages and performance | Opus | in progress | `c539808`, `ae95240`; `performance.md`: most §7 targets met; headless 1500-prompt RSS budget now passes; desktop/streaming/platform qualification and diagnostics-correlation advantage remain |
| RE-23 | Native build, packaging, CI, support and rollback | Opus | in progress | `c8d005c`; `packaging.md` (Linux candidate verified by `scripts/test-native-package.sh`); not in CI; no PKGBUILD/macOS package |
| RE-24 | Beta and accepted default | Opus + owner, after Codex | planned | Blocked on full parity, remaining live/platform acceptance and performance/release gates; RE-13B lifecycle smoke passed |
| RE-25 | Pi retirement and conversion completion | Opus + owner, after Codex | planned | Blocked on RE-24 |

## 4. Revisions and identities

| Item | Value |
| --- | --- |
| App base | `75f5c35f028bd2f85052ac990a20e982f35573ab` (`75f5c35`, "Add session-scoped composer prompt history") |
| Native branch | `native-engine`; head for this record: `4fc40608c00e75e9e04c45f3d3ccb9a8b3360a2e` (`4fc4060`, "Merge no-Node native packaging and diagnostics"); ledger first drafted at `e7cfc42` |
| Dirty state | The owner's checkout has an uncommitted edit of `llm-docs/pipkin-rust-engine-plan.md` (the current execution contract; read-only for implementers). Worker worktrees under `.claude/worktrees/` are untracked |
| Reference engine pin | `d2a311097cbcf669e699479587332ae3988a49d0` (`packaging/pi-engine-revision`) |
| Auth bridge overlay hash (`pipkinAuthBridgeSha256`) | `370dbef417d1cdeb68239f0d283d4dceb36f5ed539e500e0ba041238aad94e3a` = `sha256(pi-onboarding.patch ‖ pi-onboarding/provider-auth.ts ‖ pi-onboarding/provider-auth-provider.ts)` |
| Subagents overlay hash (`pipkinSubagentsSha256`) | `9fb846e55a931d829e2ca25bde08ae98d2f128c14ceb6f1bc033b0a2e6fb3664` = `sha256(pi-subagents.patch ‖ pi-subagents/subagents.ts ‖ pi-subagents/subagents-provider.ts)` |
| Model-data manifest hash | `b92d631bb8bb2cac6f824a03a814baad9452164c518426b8bc4bbc5c2142bb05` (`crates/pipkin-engine/model-data/manifest.json`, equal to the archive's `.manifest.json`) |
| Model-data archive | `packaging/pi-model-data-d2a311097cbcf669e699479587332ae3988a49d0.tar.zst`, sha256 `2be5f68d90695e71372c01ccbd08d5728b93691e07c45918846997c8949bcabb` |
| Native engine schema | **4** (`crates/pipkin-engine/src/store/schema.rs::SCHEMA_VERSION`) |
| Native protocol | **5** (`crates/pipkin-engine-protocol/src/lib.rs::PROTOCOL_VERSION`; v5 adds typed positional system declarations; v4 added durable catch-up; v3 guarded plugin-profile imports) |
| Native engine version | `0.0.1` (`pipkin_engine::runtime::ENGINE_VERSION` = crate version) |
| Reference protocol / app DB schema (unchanged) | Pi protocol 8; desktop `pipkin.sqlite3` schema 8 (`storage.rs` migration assertion) |
| Toolchain | Rust 1.99.0 (`rustc 1.99.0 (b940084d7 2026-09-28)`), installed user-locally via rustup (`~/.cargo/bin`) with the owner's approval |
| Fixture identities | `fixtures/pi-sessions/` (scenarios `a-text-multiturn` … `h-worker-killed-mid-tool`, made by the pinned reference + overlays with a local scripted provider; `_driver/` regenerates them); Codex adapter goldens `crates/pipkin-engine/src/provider/api/openai_codex/fixtures/` (`regenerate.sh`); tool goldens `crates/pipkin-engine/src/tools/tests/golden-{edit,validate}.json` (`tests/reference/*.mts`) |
| Reference package/runtime artifact hashes, dependency lockfile hashes | TODO(RE-02): record from a clean `scripts/build-engine.sh` + `scripts/package.sh` run (the Pi package was not rebuilt in this phase) |

## 5. Commands

```sh
# Mandatory for every batch (AGENTS.md)
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check

# Native engine crates only
cargo test -p pipkin-engine -p pipkin-engine-protocol

# Native desktop backend (unit + e2e workflows; run by default)
cargo test -p pipkin-app adapters::native

# Performance harness (ignored benchmarks; writes fixtures/native-bench/<date>/)
scripts/bench-native.sh

# No-Node native package candidate (Linux)
scripts/package-native.sh            # or: scripts/package.sh --engine native
scripts/test-native-package.sh       # build, package, install, probe, uninstall checks

# Run the native candidate
cargo run -p pipkin-app --release -- --engine native

# Overlay identities, exactly as scripts/build-engine.sh computes them
cat packaging/pi-onboarding.patch packaging/pi-onboarding/provider-auth.ts \
    packaging/pi-onboarding/provider-auth-provider.ts | sha256sum
cat packaging/pi-subagents.patch packaging/pi-subagents/subagents.ts \
    packaging/pi-subagents/subagents-provider.ts | sha256sum
sha256sum crates/pipkin-engine/model-data/manifest.json \
    packaging/pi-model-data-d2a311097cbcf669e699479587332ae3988a49d0.tar.zst

# Pi reference workflows (need a staged reference engine; never on the owner's projects)
PIPKIN_PI_REPO=<staged engine dir> cargo test -p pipkin-app -- --ignored
```

The rustup toolchain is user-local; in a non-login shell use `~/.cargo/bin/cargo` explicitly.
Ledger counts are recomputed by parsing each table's `Disp.` and `Evidence` columns (first
disposition code and first evidence word per row).

## 6. Evidence

| Date | Revision | Command | Result | Notes |
| --- | --- | --- | --- | --- |
| 2026-10-11 | `75f5c35` (unmodified tree, before native work) | `cargo test --workspace` | **613 passed, 0 failed, 43 ignored** | Baseline for regressions; ignored = real-engine/opt-in tests |
| 2026-10-11 | `e7cfc42` | `cargo test -p pipkin-engine -p pipkin-engine-protocol` (debug) | **39 passed** (34 engine + 5 protocol), 0 failed, 0 ignored | Native unit tests only |
| 2026-10-11 | integrated tree before the final runtime wiring (`9b64a99` era) | `cargo test --workspace` | **failed**: 3 test-expectation failures (exact-text expectations changed by ordered JSON keys; native tool-output copy expectation) | Fixed afterwards (`9b64a99` "Fix post-merge test expectations"); recorded here as a failed run |
| 2026-10-11 | integrated tree | engine runtime tests ×3, native e2e ×5, restart e2e ×10 | 19/19 (×3); 17/17 (×5); restart e2e flake diagnosed as a harness race and fixed, then 10/10 | From the interim status record |
| 2026-10-11 | `ae95240` (handoff commit) | `cargo test --workspace --no-fail-fast` | **1171 passed, 0 failed, 53 ignored** | Ignored: opt-in real-Pi tests, 8 perf benchmarks, 2 tool corpus comparisons. `cargo clippy --workspace --all-targets`: one test-only warning, fixed in the same commit; `cargo fmt --all --check`: clean |
| 2026-10-11 | packaging worker branch after merging `ae95240` (final package run at `9a5e19e`) | `cargo test --workspace`; clippy; fmt | **1179 passed / 0 failed / 53 ignored**; clippy 0 warnings; fmt clean | `llm-docs/pipkin-rust-engine-evidence/packaging.md` |
| 2026-10-11 | `9b64a99` + harness (dirty) | `scripts/bench-native.sh` (perf::, release) | measured; see `performance.md` | Missed at that run: streaming p95 52.4–53.2 ms, `OpenSession` O(history), 1500-prompt RSS growth |
| 2026-10-11 | `ae95240` | `cargo test --release -p pipkin-engine --test perf -- --ignored history stream_latency` | streaming p95 43.7–45.1 ms; `OpenSession` flat (0.65–0.92 ms p50) | `performance.md` "After fixes"; raw data `fixtures/native-bench/2026-10-11-after-fixes/`. Still open: single-session RSS growth, large-reply peak |
| 2026-10-11 | `9a5e19e` | `scripts/test-native-package.sh` (full) | 23/23 checks, native package verified | Installed tree 345 MB, archive 72.6 MB (unstripped); Pi package not rebuilt |
| 2026-10-11 | `ae95240` release build | Guarded native desktop walkthrough via `scripts/guard.sh` | observed: onboarding (Claude disabled with the reserved reason), first conversation with a real `write` tool, Changes, relaunch and desktop SIGKILL recovery | `native-walkthrough.md`; one copy defect fixed afterwards, not re-observed. IME, screen reader, scale, suspend/resume, cold boot, display latency remain unverified |
| — | `4fc4060` | `cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --all --check` | see "Handoff verification" below | Run by the lead |
| — | — | Differential runs (RE-21) | TODO | Only reference-derived goldens and import equivalence exist |
| — | — | Fault injection: disk full, read-only DB, SIGKILL at tool intent | TODO | SIGKILL after admission is covered by `wp::admission_survives_sigkill_and_is_found_by_lookup_after_restart` |
| — | — | Owner-authorized live provider checks, real ChatGPT sign-in | TODO | Never automated; never with paid shadow execution |

Retained evidence: `fixtures/native-bench/2026-10-11/`, `fixtures/native-bench/2026-10-11-after-fixes/`,
`llm-docs/pipkin-rust-engine-evidence/`. Keep private account/session material out of committed
evidence (the walkthrough's uncropped first capture stayed in a scratchpad and is not evidence).

## 7. Current blockers

1. **Correctness and migration gaps:** positional system patches and reasoning/1h-cache counters
   are now preserved and tested, including imported system context. Plugin-profile import protection
   and durable retry waits/counts are implemented. Constrained/grammar tool
   support is now implemented with focused differential/runtime evidence. TypeScript execution
   and remaining unsupported-history conversion (replacement edits) remain open. Fork imports
   are now implemented with executed-reference and installed-workflow evidence above.
2. **Transport and compatibility:** durable invalidation replay and bounded engine/reader delivery
   now have passing recovery/pressure evidence. External Unix attachment and serialize-once event fan-out are implemented;
   provider progress and native frontend event backlogs are bounded, with oversized frontend events admitted alone; Stop bypasses frontend delivery waits. Lifecycle/catalog and normal-request admission remain separate work. TypeScript facets, sticky profiles,
   remote services and reload still depend on Pi.
3. **Performance:** E2E-37's headless 100 MiB budget now passes on Linux/glibc; an indefinitely flat
   heap, desktop/long-tool/streaming soaks and other allocators/platforms remain unverified.
   Package reduction needs a comparable clean reference build; desktop latency gates remain open.
4. **Qualification:** no complete backend-parameterized differential harness; targeted test,
   live-provider, platform, installed-app and native-package CI gates remain. Claude lifecycle
   smoke has passed but is not full provider/platform qualification.
5. **Product decisions/reachability:** ledger D-01..D-10 remain open where stated; D-11 ownership
   transfer is implemented and resolved. A known missing baseline behavior is required work,
   not automatically a request to waive parity.

## 8. Next implementation batches

1. Completed this batch: plugin-profile import fails closed; retry count/deadline survive restart,
   with failure injection, actual SIGKILL and desktop ownership regression tests.
2. Completed next batch: bounded delivery, atomic invalidation outbox, acknowledged catch-up,
   commit/snapshot ordering, and the headless memory-budget fix; see delivery/memory evidence.
   Remaining transport and broader memory qualification boundaries are listed above.
3. Usage counters and positional prompt updates are implemented and tested. Constrained/grammar
   tool support is implemented with passing targeted/workspace checks. HTTPS proxy/WSS support and cached grammar continuation are now implemented with local TLS tests. Session lifecycle cleanup and external Unix attachment are now implemented. Serialize-once event delivery is implemented. Native frontend event byte/control pressure is implemented. Next: remaining shared-client proxy gaps, lifecycle/request admission and facet ports/profile lifecycle while
   keeping Pi as the continuity path.
4. Add the specific missing assertions in the reconciliation report; build the full native/Pi
   differential harness. Existing disk-full, child recovery and `all` delivery tests do not
   need to be reimplemented.
5. Wire native package/install qualification into CI, finish supported-platform and installed
   live checks, and remeasure release advantages before beta/default cutover and Pi retirement.

## 9. Owned files and concurrent changes

- This record, the parity ledger and the ADRs are owned by the documentation worker; other
  workers append evidence and update rows for their modules.
- All wave-2 worker branches are merged into `native-engine` (see "Current state of the work"
  below); no concurrent worker owns engine modules at `4fc4060`.
- Any Pipkin feature shipped during this project must enter the ledger before cutover (plan §1).

## 10. Handoff

`llm-docs/pipkin-rust-engine-claude-subscription-handoff.md` (plan §13 "Opus to Codex handoff
package") was added in `ae95240`. Its §2 note that the ledger still said "not started (in
progress)" is resolved by the refresh at `4fc4060`.

## Historical handoff state at `4fc4060` (superseded by current reconciliation)

Branch `native-engine` at `4fc4060` (not pushed). Merged and committed:

- **Engine core:** protocol v2, store (schema 1), runtime (admission, run loop, Stop retirement,
  queue and steering, crash recovery, fail-closed storage, subagents with recorded contracts,
  compaction manual/threshold/background/overflow, questions and extensions, real system
  prompt), headless `pipkin-engine` worker binary with process tests.
- **Providers and auth:** all 10 wire adapters incl. the ChatGPT subscription backend
  (`openai-codex-responses`) on the shared `provider/api/common` helpers (`aeebc8e`);
  credentials, `models.json` layering and every non-reserved OAuth flow.
- **Tools, context, extensions:** read/write/edit/bash with reference-captured goldens; context
  derivation, prompt, resources and compaction; native extension contract, process extensions,
  question host (`--engine-extension`).
- **Import:** importer, protocol v2 `Import*` calls, desktop palette flow with separate
  confirmations for conversations and credentials (`21ff99a`).
- **Desktop:** `NativeBackend` behind `--engine native` (`--engine-host child|inprocess`), 20
  native e2e workflows, paged subagent inspection (`7ba1340`), engine-neutral onboarding copy.
- **Evidence:** performance harness and results with two fixes (`c539808`, `ae95240`), guarded
  native walkthrough (`4ad9438`), no-Node Linux package candidate with installed diagnostics
  (`c8d005c`), parity ledger refresh at `4fc4060`.

Wave-2 branches (performance, no-Node packaging, import end-to-end wiring, paged subagent
inspection, provider-helper consolidation) are all merged; no worker branch is outstanding.

Not done: Claude subscription (reserved for Codex — not implemented or qualified: the adapter
refuses `anthropic` OAuth credentials, auth reports it unavailable, onboarding shows it
disabled); differential qualification harness; durable outbox; per-conversation ownership
transfer; Codex WebSocket session cleanup at conversation end; diagnostics-correlation
advantage; CI wiring of native package tests; macOS native build; owner-authorized live
provider checks; IME/screen reader/scale/suspend/cold-boot gates (unverified).

## Historical handoff verification

Earlier verification (2026-10-11, at the handoff commit `ae95240`, with the two performance
fixes): `cargo test --workspace --no-fail-fast` **1171 passed, 0 failed, 53 ignored** (measured);
clippy had one test-only warning, fixed in the same commit; `cargo fmt --all --check` clean.

At `4fc4060` (after merging import end-to-end wiring, paged subagent inspection,
provider-helper consolidation and no-Node packaging):

- Final workspace check (native-engine at the commit recording this line, after all merges): `cargo test --workspace --no-fail-fast` **1296 passed, 0 failed, 53 ignored**; `cargo clippy --workspace --all-targets` 0 warnings; `cargo fmt --all --check` clean (after formatting one merged file). Measured 2026-10-11. The memory investigation (ledger row E2E-37) was stopped unfinished on the owner's instruction to wrap up; its partial branch `worktree-agent-a88568455d7803b4a` is not merged and the row stays failed.
