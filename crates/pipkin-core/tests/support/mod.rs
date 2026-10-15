use pipkin_core::*;

/// Native test backends explicitly advertise execution context; catalog/history do not.
pub fn native_context(state: &mut AppState) {
    let targets: Vec<_> = state
        .conversations
        .iter()
        .map(|c| (c.id, c.generation))
        .collect();
    for (conversation, generation) in targets {
        state.apply_event(BackendEvent {
            conversation,
            generation,
            op: None,
            kind: EventKind::ExecutionContext {
                profile: ExecutionProfile::NativeAgent,
                execution_blocked: None,
            },
        });
    }
}
