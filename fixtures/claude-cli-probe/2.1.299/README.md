# 2.1.299 qualification — experimental chat/Write-only testing, not production readiness

The installed unmodified CLI changed from 2.1.296 to 2.1.299. Job111's old exact-version
check refused it before auth/inference. The source now pins the exact 2.1.299 candidate;
it does not accept arbitrary versions. Earlier 2.1.296 artifacts remain historical.

## Authorization and budget

The user initially authorized metadata/isolation checks and six short live CLI runs,
then explicitly authorized **four additional runs (ten total)** after job128. All use the existing CLI login and requested edits only in private scratch projects,
plus guarded input in an owned desktop window. No credential extraction, installation,
binary modification, billing changes, unrelated-window input or general activation.
CLI-owned normal session state may be updated by the CLI itself.

`runtime-budget.txt` is append-only: each numbered reservation is committed before a
Prompt RPC. Failed/unknown attempts still consume allowance. Do not reset, truncate,
replace or refund this ledger. Unknown prompts and decisions are never resent/resumed.

## Recorded observations so far

- Job115: `metadata-summary.jsonl`, `isolation-summary.jsonl`,
  `runtime-inspect-summary.json`. Required flags present, no routing overrides,
  recognized CLI account login. Startup controls exercise user/project/local/plugin
  hooks; safe policy suppresses them. Hook policy suppresses ordinary settings but
  intentionally does not suppress an explicitly injected plugin. MCP discovery control
  actually launches its sentinel, both isolation policies suppress it. Public runtime
  Inspect→Check→Create works without inference. Managed-policy isolation remains unverified.
- Job117: `runtime-chat-summary.json`. **One** fresh live attempt, Unknown; no automatic
  resume. Some text was preserved, but there was no terminal/clean-completion evidence.
  Configuration canaries stayed clear; no native provider call. The failed fixture remains
  in the job-owned `/home` temporary root for diagnosis. Its exact stream failure was not
  captured by that build; do not retroactively assert a particular cause.
- Job121: `runtime-deny-summary.json`. **One** new scratch Write-deny scenario, Unknown
  before any permission review or decision. Closed diagnostic facts identify
  `ObserverFailure / MalformedRecord / message_delta`; root reaped and pipes drained.
  The requested file was independently observed absent, and durable permission count was
  zero. No native provider call and configuration canaries clear. This is a **failed deny
  qualification**, not a successful permission test.

- Job123: `runtime-stop-summary.json`. **One** new scratch Stop scenario, also Unknown
  before review/Stop. Diagnostic shapes identify an object-valued `context_management`
  on `message_delta`, rejected by the earlier null-only parser. No approval was sent.
  This is a **failed Stop qualification**, not a successful Stop test.

- Job125: `runtime-allow-summary.json` plus read-only `runtime-allow-observation.json`.
  **One** fresh Write, exact review matched and one Allow consumed. The expected regular
  scratch file contained the exact requested bytes; CLI terminal success and exit0 were
  recorded. Nevertheless the run stayed Unknown: omitted tool-result `is_error` was treated
  as unknown. No resume was attempted. The owned CLI history confirms the field omission
  (history is not raw stream capture). This is partial Allow/effect evidence, **not completed
  fresh/resume qualification**, and its receipt is not retroactively reclassified.
- Job127: `runtime-deny-case2-summary.json`. **One** new scope after fixes. Exact review,
  Denied receipt, no expected file effect, positive owned-root cancellation cleanup, clear
  canaries and zero native inference. Denial case passed; the run conservatively stays Unknown.
- Job128: `runtime-stop-case2-summary.json`. **One** new scope. Explicit Stop, Revoked receipt,
  no approval decision, no expected file effect, positive cancellation cleanup and Interrupted
  receipt. Stop case passed; no general descendant containment is inferred.

The original **6/6** allowance was exhausted at job128. The user then explicitly chose
“Authorize four additional runs”: **10/10 consumed, none remain**, same restrictions and same
append-only ledger. Native146 completed fresh/resumed chat and fresh Write; native158 completed
resumed Write after exact full-review validation before Allow. Guarded
native input now includes setup, chat and a Write decision. The example's reservation-only mode
allocates a slot before a guarded desktop submission; it launches no execution itself.

## Native live cases (jobs146/158)

- Reservations7/8: `native-chat-summary.json`. Completed Fresh and Resume through the built
  desktop app/worker, exact expected remembered reply both times, terminal success and clean
  independent supervision. No native runs and clear configuration canaries. Billing unverified.
- Reservation9: `native-write-fresh-observation.json` and
  `native-write-fresh-permission-observation.json`. Fresh Write Completed; one Allow was Consumed
  and the expected regular scratch file contained exact bytes. **Full-review interaction FAILED**:
  the arguments handle lacked `tab_stop(true)`, so counted navigation reached Allow instead of
  the intended Copy button. Allow happened before the clipboard comparison, which then failed.
  No clipboard contents were logged/retained; no decision was replayed. This is completion/effect
  evidence, **not a passed full-review interaction**. Job147 reproduces actual Tab unreachability;
  the fix adds an explicit tab stop and an inert Ctrl+Shift+C complete-review copy shortcut, with
  navigation/copy/expiry tests. Full149/156 and scoped157 passed (UI193, guard37).
- Reservation10, native158: `native-write-resume-review.json` records exact complete-review
  clipboard validation before any decision, then inert Tab/PageDown/Space navigation and a
  second exact copy check. `native-write-resume-observation.json` records Completed Resume,
  terminal success plus independent clean supervision, one Consumed Allow, exact expected
  regular-file bytes, valid context, clear canaries and zero native runs. No decision replay.
  `native-write-changes-observation.json` records both scratch paths in the inspector-only
  cropped OCR after explicit refresh; Git state is not action attribution. Native158 exited0.
  The short review fit; nonzero native argument overflow scrolling was not measured.

Native146 also rejected a metadata Create after a delayed Check. The independent preflight
freshness limit is 30 seconds, separate from the five-minute approval ticket; this is now stated
in setup guidance. An explicit lookup returned no creation; the identity was parked, not reused.
A separate setup created promptly after Check succeeded. The earlier uncertain identity remains
retained. Native146 exited code0. No production activation or billing verification is inferred.

## Owned desktop setup checks (no inference)

Job129 passed the full gates with default-off, process-local execution/Write host flags,
including child/in-process/server forwarding. These switches grant no trust or approvals.
Job130 used isolated desktop/engine data and an owned pinned window; Tab failed to leave the
executable editor, before any Inspect/trust/Create. `native-initial-observation.json` records
zero bindings/prompts. AT-SPI's bus refused connection; no service/configuration was changed.

Job137 passed the full workspace, strict Clippy, fmt/diff and app/example builds (UI189).
Tracked focus handles now carry explicit tab metadata; CLI dialogs trap Tab/Shift+Tab,
protect marked text from literal Tab replacement, and expose full-argument keyboard scrolling.
These are headless composition tests, **not real IME qualification**.
Job138's native recheck confirmed separate executable/project entry, but Inspect then disabled
its focused button and stranded keyboard navigation. It reached inspection only, no Check,
Create, prompt or approval. Jobs139/142 passed full gates (UI190) for neutral focus handoff
and guarded post-frame repair when expiry removes a focused control. Headless tests explicitly
deliver frame callbacks; GPUI's native callback ordering is before draw. Window138 did not exit
on its stranded Ctrl+Q; it was cancelled only after confirming zero bindings/prompts.

Native143 confirmed post-Inspect scrolling/navigation, then exposed native key-down/key-up
double activation: trust toggled on and back off. Job144 reproduced this as a failing paired-event
regression (ordinary GPUI `simulate_keystrokes` only sends key-down). The follow-on fix handles
plain, non-held button/checkbox activation in capture phase so GPUI cannot arm another key-up
click. Full145 passed (UI192 plus guard37). Native143 exited cleanly (code0); it submitted no Check,
Create or prompt. See `native-navigation-observation.json`.

The compositor reused the exited window's address for the new owned PID. `guard.sh` now
pins/checks both address and PID; legacy address-only pins refuse until explicitly repinned.
All37 stub guard tests passed, including address reuse. Window-only captures/OCR remain in
the private fixture; they are not an accessibility or visual-inspection pass.

## Offline findings and limits

Job118 reproduces rejection of published nullable message-delta metadata (synthetic
schema, not a captured CLI frame). At job120 only known null/empty forms were accepted;
nonempty/unqualified context/container/refusal metadata and unknown mutation fields still
failed closed. This narrow fix did **not** resolve job121's message-delta rejection.
Job123 isolates a further incompatibility: object-valued context-management telemetry.
The follow-on parser accepts the published closed `applied_edits` schema (empty or known
numeric clear-thinking/clear-tool-use records), discarding it without content/events/native
compaction or authority. Unknown edits/fields, malformed numbers and unqualified container,
refusal and input-transformation metadata still fail closed. Job125 subsequently reached
review, a successful CLI terminal and clean exit without decoder rejection.
Source schemas consulted (2026-10-15): Anthropic Python SDK `raw_message_delta_event.py`,
`beta/beta_raw_message_delta_event.py`, `beta_context_management_response.py`,
`beta_clear_thinking_20251015_edit_response.py` and
`beta_clear_tool_uses_20250919_edit_response.py` on its public main branch.

Closed failure diagnostics preserve supervisor stop, decoder error and record class,
without raw stderr/output, IDs, arguments or unknown type names. Follow-on diagnostics
add a digest of a rejected schema-field label (never its value/error text), plus closed
message-delta value-shape tags. These facts may veto completion but cannot authorize it.
Existing receipt JSON remains readable through optional/defaulted fields.

Job126 passed workspace/strict-Clippy/fmt/diff (UI187), including closed
context telemetry and protocol-default result status. Job122 scoped diagnostics and job124
full context-telemetry gates also passed. Job119 had an unexplained fixture metadata-spawn failure;
its old generic error lost the OS category. New redacted busy/resource/access/missing
categories and a deterministic busy-executable/no-retry test improve diagnosis, but do
not prove the historical failure's cause. Do not hide it by calling the rerun a root-cause fix.

Job126 normalizes **omitted** tool-result `is_error` to protocol-default reported success,
not delivery/action proof. Explicit null remains Unknown; malformed flags reject; missing
results, unresolved actions/permissions, absent terminal or unclean supervision still block
completion. Old Unknown receipts are never upgraded/replayed. Offline decoder/full-runtime
regressions pass; positive native chat fresh/resume and Write fresh completion are now recorded,
and native158 adds resumed Write with full-review validation before Allow. Official source:
https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls
(retrieved 2026-10-15; its successful-result example omits `is_error`).

The runtime example exercises the real library actor and helper entry in the **example
image**. Native146/158 separately exercise the actual built desktop worker/helper, with the
fresh Write review failure explicitly recorded above. Billing, managed policy, actual orphan
containment, installed release packages and real IME/accessibility remain unverified. An
already-authorized action may finish after engine death. Execution stays default-off; explicit
owned-host experimental flags and testing instructions are in `docs/local-claude-cli.md`.

Restart checks150–155 submitted no inference. Full-window OCR missed the working palette;
owned-client keyboard delivery, temporary closed-field diagnostics and cropped OCR corrected
the initial impression of a keyboard failure. The diagnostics were removed before full156.
150/151 were cancelled;153/155 exited0. Native158 explicitly looked up and parked the retained
older uncertain creation, then reapproved the existing Write conversation; Create was not replayed.
Job148 failed an unchanged exporter-health assertion. Twelve focused repeats and full149/156
passed; its cause remains unresolved, not claimed fixed by reruns.

## Final gate and artifact integrity

Full159 exited0: workspace tests, strict all-target Clippy, fmt/diff, guard37, desktop and
qualification-example builds (UI193). `native-final-summary.json` rechecks the ten-entry ledger,
four Completed desktop runs, zero native runs/pending permissions, exact scratch files, clear
canaries and absence of the owned diagnostic/final windows. Its `artifact_sha256` map covers the
budget, fresh/resumed chat and Write observation/review/Changes artifacts. No raw clipboard or
external argument body is stored in these observations. All failed historical evidence remains.
