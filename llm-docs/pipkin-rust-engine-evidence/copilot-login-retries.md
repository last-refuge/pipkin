# Copilot login retry parity and bounds

Follow-up to `4a0601f`, 2026-10-11. No live Copilot sign-in was performed.

The native retry helper now accepts numeric prefixes (the reference's `Number.parseFloat`
behavior) and standard HTTP dates, rejects non-finite/unrepresentable delays without panicking,
and avoids retries whose wait exhausts the overall budget. Obsolete HTTP dates without an
explicit zone are interpreted as UTC. The reference capture explicitly uses UTC rather than
depending on the capture machine's timezone.

The audit also found three lifecycle differences: each attempt could receive a fresh timeout
after most of the total budget elapsed; cancellation did not interrupt backoff directly; and
the helper read an entire 429 response body before retrying. The helper now applies one login
budget across requests, bodies and waits, lets cancellation interrupt all phases, and drops a
retryable 429 body before sleeping. Refresh still uses zero retries, with the existing
per-attempt bound. Credential ownership and provider identities are unchanged.

Five tests in `auth/oauth/github_copilot/retry_tests.rs` cover:

- First-delay/retry decisions for 21 cases captured by executing the original pinned helper:
  absent/empty headers, numeric prefixes, fractional/exponent values, negative values,
  invalid/non-finite/huge delays, and three HTTP-date formats.
- Actual HTTP retry counts for dates/prefix numbers/invalid values, including the retry limit.
- Retrying a 429 whose body never terminates.
- A stalled later attempt bounded by the single total budget.
- Cancellation during backoff with no subsequent request.

`scripts/capture-copilot-retry-reference.mjs` verifies clean source at pin
`d2a311097cbcf669e699479587332ae3988a49d0`, extracts and executes the original helper with
fake fetch/time/sleep, and records source SHA-256 in `fixtures/native-copilot-retry/reference.json`.
The native fixture assertion compares first-delay decisions, not a complete real-time trace.
Native local HTTP tests separately measure timing, cancellation and retry counts.

The eight-test Copilot target passes (five new tests, two existing helper tests and the
existing full device-login test). An initial fixture run exposed ambient timezone dependence
in JavaScript's obsolete-date parser; the capture now explicitly fixes UTC. Native HTTP-date
parsing remains independent of host timezone. Unused response-header storage was removed
after the retry helper moved to inspect headers before reading the body.

Full workspace validation is recorded with the accompanying subagent accounting batch in
`subagent-accounting.md`. Provider/platform/installed qualification remains open; native
remains opt-in.
