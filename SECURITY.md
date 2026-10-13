# Security reporting and support policy

## Report privately

Do not post exploitable vulnerabilities, credentials or private session data in public issues or discussions.
Use GitHub's **Report a vulnerability** for this repository:

**https://github.com/last-refuge/pipkin/security/advisories/new**

Private vulnerability reporting is enabled. Reports begin as private security advisories for maintainers to review
and coordinate with the reporter; they are not ordinary public issues. A GitHub account is required. If that route
is unavailable, request a confidential reporting route in a public issue **without exploit details or sensitive data**.
Do not assume an unsolicited email address, public comment or ordinary issue is confidential.

Provide the minimum evidence needed:

- Pipkin version/full revision and clean/dirty state; engine revision and protocol, if known.
- Whether this is a packaged build, development checkout or externally started engine.
- Affected platform, impact and reproducible steps using synthetic data in a disposable profile/project.
- The trust boundary crossed (for example unauthorized dispatch, credential exposure, untrusted content gaining
  execution, or unintended access to a different engine/profile).
- A small reviewed reproduction or redacted excerpt, not an entire database, log, session or process environment.

**Ctrl K → Copy diagnostics** provides limited metadata. Detailed `--diagnose` output includes potentially sensitive
log/error text and must be reviewed before sharing, even privately. Never send live API keys, OAuth tokens,
`auth.json` or real provider headers. If a credential was exposed, revoke/rotate it at the provider; deleting a
report or log does not revoke the credential.

Maintainers will assess reports and coordinate fixes/disclosure through the private advisory where possible.
There is no guaranteed response or remediation SLA. Do not publish exploit details while privately coordinating
without first discussing disclosure with the maintainers. No security audit or vulnerability-free claim is made.

## Supported security-fix scope

Pipkin is currently a qualification build, not an approved v1.0 release. Review/fix work targets the latest main
branch and its pinned engine pair on the working **x86_64 Arch/Omarchy, Hyprland/Wayland** scope. There is no current
commitment to backport fixes to old beta snapshots, arbitrary engine revisions, or unsupported platforms. Final
release support and any maintenance window must be published with the approved release.

App and engine fixes may need to ship together. Follow the published pin/compatibility requirements and restart
the paired processes after upgrading. An app-only update is not evidence that an engine-side issue is fixed.
Verified signatures identify the published artifact; they are not a guarantee of safe behavior. Qualification
artifacts remain unsigned unless explicitly identified otherwise.

## Execution and data boundaries

Pipkin is a client for an engine that can run tools, change project files and cause external effects. Use trusted
projects, engines and extensions, and review requested work. Do not assume an OS sandbox or universal exactly-once
tool execution guarantee. Stop/quit cannot undo effects that already happened; recovery can repeat a partially
executed tool. The app data directory alone does not isolate engine sessions or credentials.

See [support and recovery](docs/support.md) and [packaging/compatibility](docs/packaging.md) for ownership, backup,
privacy and upgrade boundaries. Non-sensitive bugs belong in the
[public issue tracker](https://github.com/last-refuge/pipkin/issues).
