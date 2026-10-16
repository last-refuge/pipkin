# Minimum CLI version policy

Requested change: accept CLI2.1.299 or newer, not exact equality. This supersedes the earlier
exact-version blocker without extending the exhausted live prompt allowance.

- Stable numeric major.minor.patch is compared numerically against 2.1.299, with no upper bound.
  Old, malformed, overflowing, extra-output and prerelease/build-tag responses fail closed.
- Required flags, explicit executable/project trust, normal first-party CLI login and all
  per-turn freshness, identity, supervision, approval and no-fallback checks remain in place.
- Prepared metadata and durable bindings now record the actual observed CLI version. A newer
  executable cannot silently rewrite an existing conversation's binding.
- The setup view's legacy `qualified_version` field carries the minimum; the advanced UI labels
  it Minimum CLI version. The historical QUALIFIED_VERSION constant remains evidence, not policy.
- Runtime qualification summaries now report the stored actual version rather than hardcoding
  the historical baseline. The older standalone probe intentionally remains a baseline fixture.
- Targeted213 passed numeric version tests, preflight tests, runtime setup tests and all7 app
  fake-CLI workflows. Fake2.1.301 supports setup, explicit confirmation, Create, fresh/resumed
  turns, CLI/native switching and reapproval without native inference. Production SDK execution
  was not tested, and no live 2.1.301 qualification is claimed.
- Added actual-version assertions for both chat and restricted-Write durable setup/status.
  Full214 passed workspace tests (UI198), strict all-target Clippy, fmt/diff, guard37 and app build.
  Package215/install216 activated `1.1.0-native-ffe95dc723a1`. Candidate/installed isolated probes
  and synthetic installed workflow passed. All prior versions retained and previous binary hash
  unchanged; running session untouched. Fully quit/reopen to load the minimum-version policy.
  Package/evidence root: `/home/jason/.pg.79fHH1`; installation record:
  `minimum-version-installation-observation.json`. Changes remain uncommitted.
- Live ledger remains unchanged. All earlier live qualification evidence is still for2.1.299.
