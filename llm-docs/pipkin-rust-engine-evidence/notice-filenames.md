# Published notice filename discovery

2026-10-12. The current native license inventory marked `yaml-rust2` 0.13.0 as having no
copied notices. Inspection of the actual locked published crate found `MIT-LICENSE` and
`Apache-LICENSE`; the collector only recognized filenames beginning with LICENSE, LICENCE,
COPYING, COPYRIGHT or NOTICE. The ledger's earlier claim that this crate ships no license
file was incorrect.

The collector now accepts MIT/Apache-prefixed notice filenames as well. A regression first
failed with only the conventional `LICENSE.txt` copied. After the fix it verifies all three
full notice contents are retained, while an unrelated `NOT-A-LICENSE` file is excluded.
Existing fixture coverage for nested notices, engine workspace links, escaping symlinks,
embedded package fixtures, source provision, host-path omission and regeneration passes.

The inventory was regenerated from the actual locked Linux Rust graph into a fresh scratch
stage. All **614 Rust packages** remain represented. The two yaml-rust2 notices match their
published-crate files byte for byte:

| File | SHA-256 |
| --- | --- |
| Apache-LICENSE | `278f0e5c23f023a2f1af8d528dd5c29dc3a5ea2dd63558aa8d4f077c09e037c7` |
| MIT-LICENSE | `3dd30063cdb4a378736348c215cb84b78b0711ba7e39903162cae5ce71dc9757` |

The crate's published VCS identity is `e12069447073e0c8dd9ceab415159c4c92927534`.
No downloaded or fabricated license text was substituted. The generator records package-file
provenance and retains both offered licenses. Review flags decrease **34 → 33**; missing
notice entries decrease **6 → 5**. Those five are hexf-parse 0.2.1, pathfinder_geometry 0.5.1,
seahash 4.1.0, simd_helpers 0.1.0 and zune-inflate 0.2.54. Remaining recovered/ancestor notice
applicability and external runtime/asset provenance still require review.

Commands: `python3 scripts/test-license-inventory.py`;
`cargo metadata --locked --offline --format-version 1 --filter-platform x86_64-unknown-linux-gnu`
(via `scripts/write-license-inventory.py /tmp/pipkin-notice-prefix-stage`), followed by exact
byte/hash comparisons against the Cargo registry sources. This verifies the production
inventory generation step; previously recorded package archives remain immutable and retain
their earlier inventories. It does not claim complete license clearance or qualify a rebuilt
release artifact.
