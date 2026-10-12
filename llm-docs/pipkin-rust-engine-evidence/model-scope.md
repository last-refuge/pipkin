# Native model patterns and locale ordering

2026-10-12, following `52b60c7`, protocol 12 / schema 9 unchanged. Native remains opt-in;
this batch does not qualify the release or introduce Pi extension support.

## Changes and reference comparison

The original `enabledModels` implementation used globset and byte ordering. An executed
93-case reference corpus exposed 48 differences, including hidden names, negation, brace
sequences, extglobs, UTF-16 matching and fuzzy selection order. The native matcher now
implements the model resolver's minimatch options in Rust, with ICU collation for fuzzy
model selection. Exact matches, alias preference, first-match deduplication, thinking-level
suffixes and warning text continue through the resolver.

The expanded corpus passes **304 en-US cases and 112 additional locale cases** (sv-SE,
tr-TR, de-DE, th-TH). Expectations come from executing the pinned Pi model resolver, not
from native output. Fixtures record source/dependency hashes, versions and locale; see
`fixtures/native-model-scope/README.md`. Native tests compare complete ordered model keys,
thinking levels and warnings. Intermediate expanded corpora exposed further negative-group,
class/case-fold and surrogate differences that were repaired before the final capture.

Locale comes from the engine's environment, in `LC_ALL`, `LC_MESSAGES`, `LANG` order;
isolated engines do not read the host environment. Encoding/modifier suffixes are removed;
C/POSIX/unset use en-US. A separate test checks this precedence and Swedish/German ordering.
This avoids a process-global locale cache leaking ambient settings into isolated engines.

## Runtime and package evidence

`runtime::settings_tests::model_patterns_drive_initial_selection_live_listing_and_restart`
uses the real engine, saved settings/models, and a local HTTP provider. It verifies:

- Fuzzy selection chooses `a-z` over `a_z` under the default locale, with `high` thinking.
- A completed generation sends that selected model in its actual HTTP request.
- Live extglob settings list both models in catalog order; a fresh session after restart
  selects the first with `low` thinking.
- A plain star excludes the hidden model.

An initial version of this test incorrectly configured an unreachable HTTP URL while
expecting a scripted adapter to serve it; the test was corrected to use an actual local
HTTP server. No live provider account or desktop visual acceptance is claimed here.

Required checks passed: **1,638 tests, 0 failed, 54 ignored**, clean workspace Clippy with
all targets and formatting. Reference metadata was then refreshed and the focused corpus
was rechecked. Vendored-source verification, its tamper tests, license-inventory regression
tests, package script syntax and diff whitespace checks pass.

The Linux x86_64 native package builds and passes `scripts/test-native-package.sh` from a
scratch installation, including migration/coding/restart, source preservation, uninstall,
and traps that fail any attempted Node/npm/Pi invocation. New parser notices are checked
byte-for-byte against retained licenses. Archive 77,907,114 bytes; installed tree
369,344,144 bytes; unstripped app binary 362,875,616 bytes. The initial test invocation
mistyped the archive version as 0.1.0; using the produced 0.0.1 archive passed. No user
installation was modified. Existing package license-review flags remain open.

The release readiness benchmark passes 20 measured starts plus two discarded warmups.
New-profile prompt-ready p50/p95 is 28.328/29.091 ms; existing-session reopen is
20.058/21.078 ms. Idle worker RSS p50/p95 is 24,120/24,440 KiB, with 13 fds and no
descendants in every measured sample. Raw machine identity, timings and binary sizes are
in `fixtures/native-bench/2026-10-12-model-scope/`. This is worker startup, not desktop
presentation latency or a matched comparison with Pi. Initial host load was 9.67.
The stripped engine is 20,826,096 bytes, up 2,237,120 bytes (about 12.0%) from the
prior 18,588,976-byte build. The compatibility dependencies therefore have a measurable
size cost. Current release engine SHA-256:
`cf3d653c104f14e0de60b72d2328f4feb64c8beeacd45a166907f194f944c4b8`.

## Dependencies and provenance

The engine now directly uses native ICU collation and regress ECMAScript regex matching;
it no longer directly depends on globset (another dependency still uses it). No JavaScript
engine, Node runtime, or npm payload is added. Character matching uses UTF-16/UCS-2 as
appropriate so non-Unicode question marks retain the reference's code-unit behavior.

The pinned regress 0.12.0 copy has two small ECMAScript fixes: legacy case folding must not
map non-ASCII characters into ASCII, and legacy adjacent surrogate escapes must remain
separate code units. Original archive identity, every vendored file, patched-file hashes
and the explicit diff are recorded and checked by `scripts/verify-regress-vendor.py`.
Tamper tests cover original source, patched source, license, diff and extra files. CI runs
both checks; package inventories include the patch and provenance. The minimatch Blue Oak
and brace-expansion MIT notices are retained for the adapted native parser semantics.

## Boundaries still open

This finite corpus is not proof of every possible minimatch pattern, every Unicode value
or every ICU locale/version. Brace sequences currently use i64 arithmetic; JavaScript
number extremes and patterns at expansion/depth limits require further comparison.
Expansion bounds and the reference's corresponding truncation behavior are not qualified
by this corpus. Backtracking regex behavior under pathological settings is not performance
qualified. The full minimatch API and filesystem matching are not the product contract.

Installed visual model selection, broader settings/provider comparisons, desktop sustained
workloads, platform/CI release evidence, migration/rollback qualification, the default
switch and removal of legacy Pi/Node paths remain open. No ledger row is marked qualified.
