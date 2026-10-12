# AWS IAM Identity Center credentials

2026-10-12, following `8a0ac3d`. Protocol 11 / schema 9 unchanged. Native Bedrock previously
recognized SSO profile fields but rejected them. It now resolves legacy and session-based
AWS CLI profiles, refreshes eligible session tokens, and signs Bedrock requests with the
resulting role credentials. The AWS CLI remains one way to establish a login; it is not
invoked by the native engine or required for an already valid cache.

## Reference and implementation

The pinned Pi revision is `d2a311097cbcf669e699479587332ae3988a49d0`. Its Bedrock adapter
uses the AWS SDK default credential chain. `scripts/capture-aws-sso-reference.mjs` executes
that installation's `credential-provider-sso`, `credential-provider-ini` and token provider
against synthetic CLI files and a recording HTTP handler. Versions, lockfile integrity
values, entry-point hashes, request bodies, results and saved cache fields are retained in
`fixtures/native-aws-sso/reference.json`. The capture checks the Pi revision, unchanged
reference source/lockfile and installed package versions; it does not independently verify
the installed package tree against registry tarballs. No live account is used.

The native implementation follows the CLI file and HTTP contracts:

- Profiles resolve `sso_account_id` and `sso_role_name`. Legacy profiles provide start URL
  and region; session profiles resolve `[sso-session NAME]` in the AWS configuration file.
- Tokens live under `HOME/.aws/sso/cache`, with SHA-1 filenames derived from the legacy
  start URL or the session name. SHA-1 is only a filename convention, not authentication.
- Session tokens within five minutes of expiry refresh through OIDC `/token`, with a
  thirty-second retry interval. Failed refresh retains a still-valid token. Expired legacy
  tokens and sessions unable to refresh report a CLI login instruction.
- `/federation/credentials` receives the account, role and SSO bearer token. Returned
  temporary credentials feed existing SigV4 signing and may supply an AssumeRole profile.
- Role cache keys include endpoint, account, role and a hash incorporating the token;
  token rotation and profile changes cannot reuse another identity's cached credentials.
  Existing explicit-key/profile/source precedence is retained.

Native improvements: concurrent callers share a session owner, cancellation only stops the
waiter, and a rotating exchange finishes under a thirty-second total budget. Session and role
caches are bounded. Token files and response bodies have a 1 MiB bound. Cache writes use the
existing private, locked, atomic credential writer, preserve unknown fields, and compare the
original token before committing so a newer login is not overwritten. Failed persistence
warns without logging credentials. HTTPS is required except for loopback development
endpoints. SSO/OIDC service endpoint environment overrides and AWS's global endpoint override
are supported. These bounds are explicit native differences, not identical SDK internals.

MFA role profiles are **not a missing baseline prompt feature**: Pi constructs its SDK client
without `mfaCodeProvider`, and the executed `fromIni` case rejects the same profile for lack
of a callback. Native continues to report the need for externally obtained temporary
credentials. This does not introduce an interactive MFA or SSO login UI.

## Verification

Ten new tests cover three executed SDK cases (valid legacy, refreshed session and expired
legacy), token-cache filename and metadata preservation, private file permissions, concurrent
refresh after cancellation, failure backoff, rotation/role isolation, a newer-login write
race, malformed and oversized cache/response rejection, AssumeRole sourcing, a real stalled
HTTP response, and an actual signed ConverseStream request with decoded tool-use events.
The SDK comparison checks parsed wire query/body/header values and credential/cache results;
it does not claim equal timing, user agents or retry middleware.

Initial focused checks passed in `/tmp/pipkin-sso-focused.log`. Expanded checks and the full
workspace run are recorded in `/tmp/pipkin-sso-focused2.log` and `/tmp/pipkin-sso-workspace.log`.
The first workspace run failed in the unrelated native launcher fixture: executing a freshly
written script returned Linux ETXTBSY while tests forked concurrently. The fixture now launches
an existing `/bin/sh` (with an `nvim` symlink to preserve terminal-editor classification) and
reads recorder scripts as input. Real process execution, configured flags, current directory
and literal file arguments remain asserted. Production launcher behavior is unchanged.

The corrected full run passed **1558 tests, 0 failed, 53 ignored** in
`/tmp/pipkin-sso-workspace2.log`. Clippy and formatting passed in
`/tmp/pipkin-sso-clippy.log` and `/tmp/pipkin-sso-fmt.log`. Expanded focused tests passed;
the stalled exchange released its owner at the thirty-second budget. Recapturing SDK evidence
with service-module hashes preserved every behavioral fixture unchanged.

A separate default-chain gap was confirmed during source review: pinned
`credential-provider-ini::resolveProfileData` selects `credential-provider-login` for
`login_session` profiles. Native does not yet implement this AWS login-cache provider.
It is distinct from IAM Identity Center SSO and remains required follow-up work; no claim
of complete AWS credential-chain parity is made.

Live AWS organization acceptance, external proxy deployments and platform qualification
remain unverified. This evidence closes the SSO implementation gap only; it does not qualify
all Bedrock/default-chain behavior or the native-default release.
