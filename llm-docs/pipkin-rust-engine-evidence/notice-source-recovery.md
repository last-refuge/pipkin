# Original notice recovery and remaining gaps

2026-10-12, following `e0c0ab5`. No dependency, protocol or schema changed.

The locked Linux inventory had five packages without notice files. Their actual published
manifests and VCS metadata were inspected, followed by public upstream trees at the recorded
revisions. Findings and generated-inventory identities are retained in
`fixtures/native-notices/2026-10-12/report.json`.

| Package | Finding at the published revision |
| --- | --- |
| zune-inflate 0.2.54 | Original root `LICENSE-ZLIB` and `LICENSE.md` recovered at `69502ce83fdfecdd0beefd677e2abb3781b29d98`; all 14 original published package files and both notices match that tree's Git blobs. |
| hexf-parse 0.2.1 | Complete upstream tree at `4225763d744183d720f575ae96d04161b4d08ea0` contains no license/notice file. Published manifest declares CC0-1.0. |
| simd_helpers 0.1.0 | Complete upstream tree at `ca1a2f84aa386d758e98f8a609d990263932fb85` contains no license/notice file. Published manifest declares MIT. |
| seahash 4.1.0 | GitLab API tree at `94b632aeac099031c373599313d5b5f0acbbaec0` has 17 entries and no next page or notice file. Published manifest declares MIT. A direct LICENSE request returned 404. |
| pathfinder_geometry 0.5.1 | Recorded `a5e98fac00f433fe2ff4b2135383d82491b8bfc5` returns HTTP 422 “Invalid object” from both servo/pathfinder and pcwalton/pathfinder GitHub tree endpoints. Published source retains the 2019 project attribution and MIT/Apache declaration. No different revision was represented as this release. |

These are missing-notice or source-recovery findings, **not claims that the packages have no
license grant**. The declarations remain represented. Later resolution may use those exact
declarations and applicable standard terms; no invented author notice has been substituted.

Zune's [published manifest](https://github.com/etemesi254/zune-image/blob/69502ce83fdfecdd0beefd677e2abb3781b29d98/zune-inflate/Cargo.toml)
offers the Zlib alternative, and the package source headers repeat that offer. The inventory
now records that choice for this exact version and copies the
[full original Zlib notice](https://github.com/etemesi254/zune-image/blob/69502ce83fdfecdd0beefd677e2abb3781b29d98/LICENSE-ZLIB)
and original repository attribution byte for byte. See
`packaging/upstream-notices/zune-inflate-0.2.54/PROVENANCE.md` and its source comparison.
No network access is added to packaging: reviewed inputs are stored locally and checksummed.

The regenerated native inventory still includes **614 Rust entries**, now with **1,081
notice files** and **four** entries missing notices. **33 applicability/review flags remain**;
recovering original text does not silently clear them. The supplemental regression checks
the exact two full texts, immutable revision provenance, chosen alternative and retained
review flag. Existing notice/ancestor/link/source-provision tests also pass.

Source-tree lookup failures and absent files are recorded rather than weakening assertions
or declaring all notices resolved. Runtime libraries, assets and other supplemental/ancestor
applicability reviews remain separate release work. Existing package archives and historical
measurements are immutable; the new artifact is identified separately in the fixture report.

Validation: `scripts/test-license-inventory.py` passes; the actual regenerated inventory
and the newly built offline native Arch archive both contain the two recovered notices.
The artifact verifier checks all **1,081 notice hashes**, metadata/binary identity, 31
installed guides, and isolated import/write/edit/bash/read/restart/closed archive restore
with no repeated settled effects. Node/npm/npx/Pi traps were unused. Build uses the same
previously documented user-managed Rust `--nodeps` exception; no system installation.

Archive SHA-256: `56009e41eb13ec197ee85ac2cc64023947fb9e83addd7907dd6d28af41a72583`;
app SHA-256: `2f59d67b63ed8ae16d567ff760d267fba3d29c36e84b282f6bcabdb0668f79da`, stamped
`e0c0ab5` plus working changes. `fixtures/native-notices/2026-10-12/arch-package.json`
records the full identity. Required checks: **1,643 passed, 0 failed, 56 ignored**, clean
Clippy/all-targets and formatting. Logs: `/tmp/pipkin-notices-workspace.log`, companion
`-clippy.log` / `-fmt.log`, and `/tmp/pipkin-notices-arch-{build,verify}.log`.
