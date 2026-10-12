# Notice findings relevant to the conversion

Baseline revision: `75f5c35f028bd2f85052ac990a20e982f35573ab`. The inventory in [notice recovery](notice-source-recovery.md)
has 33 review flags. Comparing name, version and source against the pre-conversion lockfile
finds **31 unchanged baseline entries**. All four entries missing original notice files are
in that set. Their findings and notices remain visible; no flag is silently cleared.

The two introduced flagged entries are Pipkin's own `pipkin-engine` and
`pipkin-engine-protocol`. Both explicitly inherit `workspace.package.license = "MIT"`;
the repository LICENSE is collected as an ancestor notice. Applicability of that inherited
notice is verified for these two workspace crates. Existing Pi/Anthropic SDK/minimatch/
brace-expansion port notices and source provenance remain required package inputs, checked
by the existing hash and installed-package tests.

The [comparison](../../fixtures/native-notices/2026-10-12/conversion-delta.json) retains
package/source identities, notice hashes, original inventory hash and root LICENSE hash.
Retiring Pi removes the pi-client entry; notices for retained source/data must remain.

This distinguishes conversion notice preservation from the application's broader inherited
licensing review. It is not blanket legal clearance or approval to publish a 1.0 release.
The native cutover retains the audited inventory, existing notices, declared alternatives
and source-provision bundles. No introduced missing-notice entry was found among these
33 flags. Broader inherited findings remain recorded in the inventory.
