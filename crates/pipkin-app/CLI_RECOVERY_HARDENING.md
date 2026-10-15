# CLI bridge recovery and safe handoffs

Offline implementation; not permission to enable production execution. Engine protocol/schema
remain 39/25; desktop schema 10 adds approval recovery to schema 9's creation journal.

## Durable creation and approval facts

Creation journals the exact public session/setup pair before Create. Restart restores at most
eight parked identities plus one active unknown identity. No ticket, trust or automatic request
returns. Receipt-plus-catalog reconciliation deletes only the exact pair. Selection at capacity
swaps records without eviction. Invalid or unreadable recovery disables creation.

Approval journals the exact external run/invocation/digest and attempted choice before releasing
one decision RPC. A matching successful writer acknowledgment still needs a live captured scope,
current eligibility and unexpired review. Storage failure, clock/scope retirement and duplicate or
stale acknowledgments never release or rearm it. Terminal receipt metadata is saved separately;
failed saving leaves conservative uncertainty on disk and a scoped user notice.

Approval recovery contains **no argument bodies, prompts, grants, private helper nonce or owner
connection proof**. Startup restores retired, plaintext-free views and operation high water, not
pending requests or authority. Explicit reattachment cannot revive decisions. Original-owner
receipt isolation is unchanged: without the original adapter connection/receipt proof, lookup
fails closed, rather than querying a cross-owner archive. Unknown outcomes may therefore remain
unresolvable after restart. Inspect files/transcript; never resend an uncertain decision.

The approval limit is eight saved conversation contexts per desktop namespace, with 128 attempts
per run. A resolved scope can advance to a separately verified later run in that conversation.
Other contexts are not silently evicted; capacity survives restart. Do not suggest profile/database
deletion to bypass uncertainty. This intentionally conservative limit is a product limitation.

All writes use the existing bounded writer and transactions. Namespace isolation, bounded SQL
values before JSON decoding, closed record schema, exact identity/choice checks and whole-set
startup validation fail closed. There is no automatic retry or eviction on storage failure.
Core remains framework-free; render performs no storage I/O or argument parsing.

## Explicit fresh setup and diagnostics

Fresh setup uses captured saved executable/project/model/mode hints from an exact existing target.
The source must not be running, stopping or outcome-unknown in the desktop. Any current setup
ticket is retired. No source draft, transcript, prompt, tool arguments, goals, trust or CLI UUID
transfers. Inspect, both trust choices, Check, journaled Create and a new user-authored prompt
remain separate steps. Metadata-only integration tests prove a distinct CLI UUID without launching
a process or calling a native model. Fresh setup does not erase old approval uncertainty.

Copy redacted diagnostics is an explicit captured-route command with no probes or RPCs. It emits
only closed enums, booleans, counts and static recovery guidance. Tests seed private paths, IDs,
errors and prompt text and verify they cannot enter the report. Guidance covers login through the
installed CLI itself, supported version/flag policy, expired approvals, invalid context, uncertain
creation and recovery-storage failure. No credential copying, silent API fallback or automatic
context repair is offered.

## Abrupt engine death: measured limitation, not containment

`runtime/external_coding_crash_tests.rs` kills an owned engine fixture during a pending Write
review and after an Allow was consumed/delivered but before the fake CLI performs its action.
On restart, the run is Unknown, partial assistant text remains, the context cannot resume, a
pending permission becomes Revoked, and a consumed receipt remains Consumed. Duplicate lookup
never launches again; native execution tables/providers remain unused.

The consumed case deliberately demonstrates that the detached fake CLI can **still perform its
already-authorized effect after the engine dies**. Consumption is not action proof, and startup
recovery cannot infer that a missing effect will stay absent. This is a measured failure of any
claim of complete crash containment, not a passing containment test. Tests clean up only their
owned fake process through a captured, fixture-validated Linux pidfd. That is **test cleanup**, not
a production reaper or authorization to kill arbitrary remembered PIDs.

The pending fake exits on broker EOF. Real CLI helper/descendant behavior, process-group escapes,
managed policy and crash containment remain independent live/platform qualification questions.
Process groups are not a sandbox. After a real crash, inspect processes and files before starting
fresh work; a new metadata conversation does not terminate an old orphan. Production remains
default-off. No billing verification or live account request is implied by these offline tests.
