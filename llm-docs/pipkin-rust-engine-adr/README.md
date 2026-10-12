# Native engine architecture decision records

Decision records for the built-in Rust engine (plan: `llm-docs/pipkin-rust-engine-plan.md`,
progress: `llm-docs/pipkin-rust-engine-progress.md`, parity ledger:
`llm-docs/pipkin-rust-engine-parity.md`, reference audit: `llm-docs/pipkin-rust-engine-audit/`).

Reference revision for every `<pi-staged>` citation: `d2a311097cbcf669e699479587332ae3988a49d0`
with both Pipkin overlays applied exactly as `scripts/build-engine.sh` stages a clean release.
Native status was reconciled at `e4c3a8c`, then updated for schema 2/protocol 3 import protection
and durable retries on 2026-10-11 (see the import/retry evidence record); historical
measurements retain their original identities in the evidence records.

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](0001-execution-topology.md) | Owned worker child of the same executable; in-process host for tests and benchmarks | Accepted; host, desktop supervisor and worker entry implemented |
| [0002](0002-protocol-and-transport.md) | Length-prefixed JSON frames over inherited pipes; epoch/seq events, snapshots on gaps, control lane | Accepted; durable outbox/cursor not yet implemented |
| [0003](0003-durable-store-and-recovery.md) | One SQLite store per data root, explicit schema, short transactions, state-driven run loop, fail closed, `synchronous=FULL` | Accepted; implemented; injected storage faults and actual worker SIGKILL tested |
| [0004](0004-stop-and-queue-semantics.md) | Reference queue boundaries; Stop retires inputs with a `retired` flag instead of omit-edit entries | Accepted; differential context test pending |
| [0005](0005-tool-replay-and-subagent-recovery.md) | Unsafe-by-default replay; subagent is the replay-safe registration with an atomic child mapping | Accepted; tools and recorded-contract check implemented; build provenance proposed |
| [0006](0006-extension-compatibility.md) | Native extension trait + process protocol; TypeScript facets need Node, so the legacy bridge stays and the row stays blocked | Proposed; native hosts/example ports exist; TypeScript parity remains blocked |
| [0007](0007-credential-ownership-and-migration.md) | Own config root; `auth.json`-compatible private file until a qualified secret store; explicit import; Claude subscription integration | Accepted; auth implemented; Claude live lifecycle checks passed, full qualification pending |

An ADR records a decision and its evidence boundary. It is not qualification: a decision is
only proven by the tests named in the parity ledger.
