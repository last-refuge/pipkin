use super::tests::Fixture;
use crate::{
    provider::scripted::Scripted,
    store::{CliBindingSpec, Store},
};
use pipkin_engine_protocol::*;

fn seed(store: &Store, fixture: &Fixture) -> crate::store::ExternalPromptReceipt {
    store
        .create_cli_session(
            "external",
            "create-external",
            &CliBindingSpec {
                cli_session_id: "11111111-1111-4111-8111-111111111111".into(),
                project_path: fixture.project.to_str().unwrap().into(),
                executable_path: "/not-executed/claude".into(),
                cli_version: "2.1.296".into(),
                config_ref: "fixture".into(),
                account_scope: "fixture-account".into(),
                launch_policy: "fixture".into(),
            },
            1,
        )
        .unwrap();
    store
        .tx(|tx| {
            let (admission, _) = Store::admit_external_prompt_in(
                tx,
                "external",
                "opaque request / not an op",
                "display only",
                "op",
                1,
                2,
            )?;
            Store::bump(tx, "external", 2)?;
            Ok(admission.receipt)
        })
        .unwrap()
}

#[test]
fn active_external_receipt_is_visible_without_exposing_native_execution() {
    let fixture = Fixture::new();
    let provider = Scripted::echo("must not run");
    let mut engine = fixture.start(&provider);
    let receipt = seed(&engine.handle.shared().store, &fixture);
    let view = engine.snapshot("external");
    assert_eq!(view.run.unwrap().id, receipt.id);
    assert!(view.summary.blocked.is_some());
    assert!(matches!(
        view.entries[0].kind,
        EntryKind::ExternalMessage {
            role: ExternalRole::User,
            ..
        }
    ));
    assert!(
        matches!(engine.call(Call::Lookup { session: "external".into(), request: receipt.request.clone() }).unwrap(), Reply::Lookup(LookupResult::Known { open: true, run: Some(id), .. }) if id == receipt.id)
    );
    assert_eq!(
        engine
            .call(Call::Prompt {
                session: "external".into(),
                request: "no-new-run".into(),
                text: "not enabled".into(),
                images: vec![],
                model: None
            })
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    assert!(provider.requests().is_empty());
}

#[test]
fn recovered_external_request_remains_known_and_wait_returns_its_own_partial_answer() {
    use crate::execution::claude_cli::events::{Event, TextKind};
    let fixture = Fixture::new();
    let store = Store::open(&fixture.config().data_dir).unwrap();
    let receipt = seed(&store, &fixture);
    store
        .tx(|tx| {
            Store::begin_external_launch_in(tx, &receipt.key, 3)?;
            Store::begin_external_run_in(tx, &receipt.key, 4)?;
            Store::project_external_event_in(tx, &receipt.key, 1, &Event::Initialized, 5)?;
            Store::project_external_event_in(
                tx,
                &receipt.key,
                2,
                &Event::BlockSnapshot {
                    message_id: "external-answer".into(),
                    block_index: 0,
                    kind: TextKind::Text,
                    text: "saved external prefix".into(),
                },
                6,
            )?;
            // Foreign native content must never become the external request's answer.
            Store::append_entry(
                tx,
                "external",
                7,
                Some(receipt.id),
                &EntryKind::Message {
                    message: Message::user_text("foreign native input", 7),
                    mode: InputMode::Prompt,
                    request: Some(receipt.request.clone()),
                    partial: false,
                },
            )?;
            Store::bump(tx, "external", 7)?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let provider = Scripted::echo("must not run");
    let mut engine = fixture.start(&provider);
    assert!(matches!(
        engine
            .call(Call::Lookup {
                session: "external".into(),
                request: receipt.request.clone()
            })
            .unwrap(),
        Reply::Lookup(LookupResult::Known {
            open: false,
            outcome: Some(RunOutcome::Interrupted { .. }),
            ..
        })
    ));
    assert!(
        matches!(engine.call(Call::WaitForPrompt { session: "external".into(), request: receipt.request }).unwrap(), Reply::PromptResult(PromptResult::Unanswered { text: Some(text), .. }) if text.contains("saved external prefix") && !text.contains("foreign native"))
    );
    assert!(engine.snapshot("external").run.is_none());
    assert_eq!(
        engine
            .call(Call::WaitForPrompt {
                session: "external".into(),
                request: "never admitted".into()
            })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert!(provider.requests().is_empty());
}
