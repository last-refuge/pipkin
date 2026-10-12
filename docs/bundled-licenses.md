# Bundled licenses and provenance audit

**Status: partial; not release clearance.** Generated inventories and copied license text are evidence, not a
substitute for reviewing missing notices, upstream applicability, or redistribution requirements.

## What packages carry

`usr/share/doc/pipkin/third-party/inventory.json` lists:

- The locked Linux Rust graph reachable from `pipkin-app` through normal/build edges. Cargo workspace feature
  unification can include extra feature dependencies, so this is a conservative inventory, not a minimal binary
  SBOM or proof of which functions were linked.
- Installed npm package roots and engine workspace packages actually in the staged tree. Workspace symlinks are
  resolved inside the engine; embedded `package.json` test fixtures are not treated as installed packages.
- Declared licenses, source identifiers, copied notice-file hashes and explicit review flags. Host cache paths are
  omitted. Ancestor/repository fallback notices are marked for applicability review.

Full package/vendored-component notice files are copied under `third-party/notices`. Pi's root MIT license and
existing font/icon notices remain in their normal package locations. The unmodified registry source for the
MPL-2.0 `option-ext` dependency is shipped under `third-party/sources/option-ext-0.2.0`; its inventory entry points to
that source. This source provision is specific to the current locked version, not a blanket claim about future MPL
changes. Source/update obligations must be rechecked when dependencies change.

For the current graph, the offered Apache-2.0 alternative is selected for Rust `self_cell`, and BSD-3-Clause for
npm `node-forge`. Their original notices are retained. Copying an optional GPL license text alongside an offered
permissive alternative does not mean Pipkin chose the GPL alternative. GPUI crates declare Apache-2.0; Zed's GPL
application crates are not imported merely because the source repository contains them.

## Initial findings (2026-10-06)

The first actual staged-tree inventory found **552 Rust** and **155 npm/workspace** entries. These counts include
conservatively overincluded feature/build dependencies and shipped example workspaces; they are not runtime-only
counts. All reachable Rust crates declare a license expression. Some published packages omit notice files despite
that declaration, and some npm packages omit the metadata license field.

Three npm packages (`buildcheck`, `cpu-features`, `ssh2`) have no metadata license declaration, but their installed
LICENSE files were inspected and contain the MIT grant and Brian White copyright. They remain flagged rather than
silently rewriting upstream metadata. `cpu-features` also carries a vendored component notice, which is retained.

Open findings include:

- Missing distributed notice files in several AWS SDK packages and other npm packages, including the platform
  esbuild binary package; validate applicable upstream notices and retain them with immutable provenance.
- Missing notice files in published Rust crates such as AccessKit, Lyon, profiling, pathfinder and others; use the
  package's published VCS revision to recover applicable upstream notices rather than copying a random current one.
- Confirm applicability of inherited notices for engine/workspace crates and source examples.
- Complete generated-provider-data provenance review (`packaging/engine-model-data.md`), asset provenance, external
  runtime dependency inventory, and any Apache NOTICE/source-offer requirements.

### Immutable notice recovery

`packaging/upstream-notices.json` records supplemental notice inputs with exact upstream commit URLs and SHA-256
hashes. The initial recovery obtained 33 notice files for 20 Rust package versions using the published crates'
`.cargo_vcs_info.json` revisions. Inventory generation verifies those hashes before copying the files. No network
fetch is needed during packaging. Recovered repository notices remain explicitly flagged for applicability review;
downloading a notice is not an automatic legal-clearance decision.

This reduced packages with no copied notice from 32 to 12, while the total review flags remain 59. Remaining
missing-notice cases initially included five Rust package versions and seven npm package versions.

A subsequent npm metadata review recovered the MIT notice for `@esbuild/linux-x64` 0.28.2 from its published
`gitHead`, and the MIT **libraries/LICENSE** notice for `standardwebhooks` 1.1.1 from its `gitHead`. The latter's
repository-root LICENSE is Apache-2.0 for the specification, not the JavaScript library's MIT license: do not
substitute that root text. These inputs retain immutable URLs/hashes and remain applicability-review flagged.
The current missing-notice count is **10 (five Rust, five npm)**, with **59 total review flags** unchanged.

The remaining AWS packages and `proxy-agent-negotiate` publish no `gitHead`; attempted version-tag lookup did not
provide corresponding AWS tags. The negotiate version's tag resolves to an exact commit with matching package
name/version but no package LICENSE, and no repository-root license. `data-uri-to-buffer`'s published commit also
has no notice file. Do not fabricate copyrights or copy an unrelated current license to make the list appear closed.

The inventory's `reviewRequired` entries are the machine-readable follow-up list. Do not mark the release-plan
license/provenance task complete until those findings are resolved or an explicit acceptable review decision is
recorded. Regenerate/review the inventory for every changed candidate; a prior snapshot is not clearance of a new
app/engine pair.

## Native Pi source port

The native Claude subscription OAuth and request conversion contain an MIT-licensed port of
Pi source at `d2a311097cbcf669e699479587332ae3988a49d0` (copyright 2025 Mario Zechner).
The notice is retained in `packaging/upstream-notices/pi/LICENSE`; native packages install it
as `usr/share/licenses/pipkin/pi-MIT-LICENSE`. This source attribution remains necessary even
though the native package contains no Pi or Node runtime.

Anthropic workload identity federation also ports the token-exchange and cache contract from
`@anthropic-ai/sdk` **0.129.0**, the version pinned by that Pi revision (copyright 2023
Anthropic, PBC.; MIT). Its exact package notice is retained at
`packaging/upstream-notices/anthropic-sdk/LICENSE`. Linux packages install it as
`usr/share/licenses/pipkin/anthropic-sdk-MIT-LICENSE`; the experimental macOS bundle includes
`Contents/Resources/anthropic-sdk-MIT-LICENSE`. No JavaScript SDK runtime is added to native.

## Native runtime inspection

Packages also carry `usr/share/doc/pipkin/runtime-inventory.json`, generated using **readelf without executing any
artifact**. It lists staged ELF hashes/machines, direct NEEDED shared-library names and referenced GLIBC/GLIBCXX/
CXXABI versions, plus non-ELF `.node` files. It flags foreign architectures/formats without pretending those are
supported runtimes. This is evidence, not a complete static/dlopen/linked-component SBOM or legal clearance.

The initial local scan found 33 native artifacts (including build object files), with seven foreign-format/architecture
flags. Pi's full source/dependency tree includes ARM, Darwin and Windows artifacts; their presence does not extend
Pipkin's supported platform. The scan also exposes optional sandbox/VM components, C++ runtime dependencies and
bundled `libkrun` depending on `libcap-ng`; these need applicability/licensing/runtime review in addition to npm
wrapper declarations. Do not infer that every optional dependency is exercised by the scripted-provider suite.

The actual locally built app references **GLIBC_2.44**. The package's ABI requirements depend on its build inputs;
this unsigned local artifact is not a promise of portability to older Linux systems. The final supported-platform
candidate needs its own ABI/runtime qualification. Linux packages now include a checksum-pinned private Node 22.23.3 executable and full Node LICENSE. The PKGBUILD's desktop/GPU dependencies and Git remain external runtime requirements. Node archive checksums alone do not establish publisher identity or legal clearance. Rust/Go standard libraries and bundled native components also require provenance/
notice review beyond the Cargo/npm package graph.

## Reproduction

```sh
scripts/package.sh "$PI_CHECKOUT"  # includes inventory generation from locked metadata and the actual engine
python3 scripts/test-license-inventory.py
python3 scripts/test-runtime-inventory.py
```

Python 3 is a build/qualification dependency, not a new installed-app runtime dependency. The fixture test checks
notice retention, workspace-link handling, exclusion of embedded fixture manifests, MPL source provision and
omission of private host paths. No provider credentials or inference requests are involved.

## Native notice discovery correction (2026-10-12)

The native inventory previously missed yaml-rust2 0.13.0's published `MIT-LICENSE` and
`Apache-LICENSE` because notice discovery only recognized filenames starting with the notice
type. Both files now enter the generated package notices unchanged. A fixture reproduces the
old omission, and the actual locked crate's two copied files were checked byte for byte.

The current native graph has 614 Rust entries, 33 review flags and five entries without a
copied notice. This corrects discovery for one crate; it does not resolve other notice
applicability or runtime/asset provenance findings. Previously measured archives retain their
original inventories. See [filename evidence](../llm-docs/pipkin-rust-engine-evidence/notice-filenames.md).

## MCP SDK dependencies (2026-10-12)

MCP-1 adds the official `rmcp` 3.5.1 and `rmcp-macros` 3.5.1 (Apache-2.0) to the engine
with client and child-process transport features only, pulling in `nix` 0.31.3 (MIT),
`process-wrap` 10.0.1 (Apache-2.0 OR MIT), `tokio-stream` 0.1.19 (MIT), `pastey` (MIT OR
Apache-2.0), `darling` 0.24.1 with `darling_core`/`darling_macro`/`strsim` (MIT) and
`ident_case` (MIT/Apache-2.0). The test-only fixture server also uses `schemars` 1.2.2 (MIT). These
enter the native inventory through the locked graph like every other crate; their notices
still need the same applicability review before a release, and the Apache-2.0 crates'
`NOTICE` files (if any) must be carried. No HTTP transport feature is enabled, so no second
`reqwest` is present.

