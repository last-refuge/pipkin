# Architecture

Desktop dependency direction: `pipkin-app → pipkin-ui → pipkin-core`.

```text
GPUI views ──Command──▶ Model entity ──▶ AppState (pipkin-core, pure Rust)
                              │                 │
                              │◀── Effects ─────┘
                              ▼
                 controller (pipkin-app) executes effects
                    ├─ Backend port: NativeBackend or explicit simulated DemoBackend
                    └─ Storage: SQLite, single writer thread
        BackendEvent ◀── backend ──▶ Model::apply_event (stale-guarded)
```

- **pipkin-core** owns identities (project, conversation, item, operation, queue), run-state transitions, queue contents, command availability (`AppState::availability`) and draft/save state. No GPUI, colors, geometry or transport frames. 13 transition tests cover stale events, cancellation, unknown outcome, queue, drafts.
- **pipkin-ui** owns focus, selection geometry, text layout, scroll handles and open overlays: `theme` (semantic tokens), `text` (`ComposerEditor`), `transcript` (`TranscriptDocument`, `TranscriptView`), `shell` (workspace, navigation, inspector, overlays, command registry).
- **pipkin-app** owns the entry point, controller, `adapters/demo.rs`, `storage.rs`, `platform.rs`.

Stale-update guards: every `BackendEvent` carries `(conversation, generation, op)`. `AppState::apply_event` routes by conversation id, drops events from an older attachment generation, and drops operation-scoped events whose op is not the live one. A late event never lands in whichever conversation is selected.

Execution states (`RunState`) are separate from persistence (`SaveState`) and, later, connection state. Cancel stays `Stopping` until the backend confirms. `OutcomeUnknown` is never resent automatically; the only exits are `CheckStatus` (backend answers) or a failed resolution.

Render functions do no I/O. Draft saves are debounced (500 ms) and flushed on conversation change and orderly exit; "Draft saved" is shown only after the writer acknowledges a commit.

The built-in `pipkin-engine` owns providers, coding tools, durable sessions and retries;
`pipkin-engine-protocol` defines the versioned framed boundary. The desktop normally starts the same
binary as a private engine worker, with optional in-process and explicit native socket-host modes.
There is no Pi client crate or Node runtime.
