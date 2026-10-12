# HTTP timeout validation and live repair

2026-10-12, following `2eec3e1`. Protocol 11 / schema 9 unchanged.

`httpIdleTimeoutMs` previously treated supplied invalid values as absent and silently used
300 seconds. Native now preserves explicit null and reports the reference settings error for
invalid values. Absent still defaults to 300 seconds; zero/disabled disables the idle timeout.
Numeric strings use JavaScript Number syntax and whitespace rules, including hexadecimal,
octal, binary and BOM whitespace. Unrelated settings survive invalid timeout values.

The engine's global network snapshot retains an invalid-setting error. Shared HTTP rejects
requests with a typed, non-retryable builder error before I/O, including login and catalog
requests; identity clients use the same builder. Model preparation and pinned compaction
request reconstruction check global configuration before credential/provider calls. Codex's
WebSocket path checks it before connecting or falling back to SSE.

Global settings determine the shared dispatcher; merged global/project settings determine the
provider request timeout unless retry.provider.timeoutMs overrides it. A provider override
cannot hide invalid global dispatcher settings. New AuthStart/AuthReuse operations refresh
network settings off the async workers and refuse invalid configuration before authorization
starts. Existing in-flight operations retain their captured client snapshot. A corrected file
allows new requests and sign-in attempts without restarting the engine.

This deliberately improves recovery over reference startup failure: native stays available to
report and repair configuration, while refusing affected operations. It does not silently use
a substitute timeout. The parser's existing u64 millisecond representation still saturates
larger finite values; behavior beyond that range and actual extremely long timers are not
qualified here. Broader per-adapter stall/zero-timeout and HTTP-version comparisons remain open.

The capture executes **31** pinned SettingsManager/parser cases (global and project-merged),
using unchanged production function bodies with unused I/O dependencies isolated. No reference
network requests or full Pi application execution are claimed. Native compares exact errors,
effective values, network-snapshot errors and preserved unrelated settings. See the fixture
README for source hashes and bounds.

Five new tests cover reference cases, running-engine global/project error and repair, sign-in
startup/repair/cancellation, shared HTTP and OAuth rejection/rebuilt clients, and WebSocket
rejection without TCP connection. The initial new WebSocket test used the wrong stream return
and event-sink types; corrected before execution. The first corrected focused run passed 25
tests; the final run with runtime sign-in coverage passed **26 tests**. Logs:
`/tmp/pipkin-http-timeout-check.log`, `/tmp/pipkin-http-timeout-tests{,2,3}.log`.
Final required checks passed: **1631 tests, 0 failed, 53 ignored**, clean workspace
Clippy (all targets) and formatting. Logs: `/tmp/pipkin-http-timeout-workspace.log`,
`/tmp/pipkin-http-timeout-clippy.log`, `/tmp/pipkin-http-timeout-fmt.log`.

No new installed package, live account authorization, desktop session or platform acceptance is
claimed. Other settings/model-scope gaps, credential retries, trust scope, rollback history
labels, sustained workloads and native-default/Pi-removal qualification remain open.
