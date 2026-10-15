use pipkin_core::*;
#[test]
fn approval_admission_counts_full_display_spare_capacity_and_box() {
    let run = CliApprovalRun {
        epoch: 1,
        session: "s".into(),
        binding_generation: 1,
        op: "op".into(),
        prompt: 1,
    };
    let mut arguments = String::with_capacity(2 * CLI_APPROVAL_DISPLAY_BYTES);
    arguments.push_str(&"x".repeat(CLI_APPROVAL_DISPLAY_BYTES));
    let capacity = arguments.capacity();
    let mut reviews = Vec::with_capacity(16);
    let storage = reviews.capacity() * size_of::<CliApprovalReview>();
    reviews.push(CliApprovalReview {
        id: CliApprovalId {
            run: run.clone(),
            invocation: "ab".repeat(32),
            arguments_sha256: "cd".repeat(32),
        },
        arguments,
        expires_at_ms: 10000,
    });
    let event = BackendEvent {
        conversation: ConversationId(1),
        generation: 1,
        op: Some(OperationId(3)),
        kind: EventKind::CliApproval(Box::new(CliApprovalEvent::Discovered {
            query: CliApprovalQuery {
                conversation: ConversationId(1),
                generation: 1,
                run_op: OperationId(2),
                op: OperationId(3),
            },
            run: Some(run),
            reviews,
        })),
    };
    assert!(
        super::size::event_bytes(&event)
            >= size_of::<BackendEvent>() + size_of::<CliApprovalEvent>() + capacity + storage + 128
    );
}
