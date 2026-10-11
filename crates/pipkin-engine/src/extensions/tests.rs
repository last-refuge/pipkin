//! The extension runner, the call-backed host and the native example ports.

use std::sync::atomic::{AtomicBool, Ordering};

use pipkin_engine_protocol::{QuestionKind, QuestionOption, QuestionView, UiView};
use tokio::sync::mpsc;

use super::examples::{GreetingExample, QuestionsExample};
use super::testing::TestHost;
use super::*;

fn opt(value: &str, label: &str, description: Option<&str>) -> QuestionOption {
    QuestionOption {
        value: value.into(),
        label: label.into(),
        description: description.map(str::to_string),
    }
}

/// Answer questions as they appear until a status line is set; returns the questions asked.
pub(crate) async fn answer_until_status(
    host: &TestHost,
    views: &mut mpsc::UnboundedReceiver<UiView>,
    answer: impl Fn(&QuestionView) -> Option<Answer>,
) -> Vec<QuestionView> {
    let mut asked: Vec<QuestionView> = Vec::new();
    loop {
        let view = views.recv().await.expect("the host is alive");
        for question in &view.questions {
            if asked.iter().all(|q| q.id != question.id) {
                asked.push(question.clone());
                match answer(question) {
                    Some(reply) => host.questions().respond(&question.id, reply).unwrap(),
                    None => assert!(host.questions().cancel(&question.id)),
                }
            }
        }
        if !view.status.is_empty() {
            return asked;
        }
    }
}

pub(crate) fn assert_reference_questions(asked: &[QuestionView]) {
    assert_eq!(asked.len(), 3);
    assert_eq!(asked[0].kind, QuestionKind::Select);
    assert_eq!(asked[0].title, "Pick a flavor");
    assert_eq!(
        asked[0].message.as_deref(),
        Some("The example extension is asking.")
    );
    assert_eq!(
        asked[0].options,
        vec![
            opt("mint", "Mint", Some("cool and sharp")),
            opt("vanilla", "Vanilla", Some("plain and sweet")),
            opt("pepper", "Pepper", None),
        ]
    );
    assert_eq!(asked[1].kind, QuestionKind::Confirm);
    assert_eq!(asked[1].title, "Keep going?");
    assert_eq!(asked[1].message.as_deref(), Some("Answer yes or no."));
    assert_eq!(asked[2].kind, QuestionKind::Input);
    assert_eq!(asked[2].title, "What should it be called?");
    assert_eq!(asked[2].message, None);
    assert_eq!(asked[2].placeholder.as_deref(), Some("a name"));
    assert_eq!(asked[2].default_value.as_deref(), Some("pipsqueak"));
    for question in asked {
        assert!(question.deadline_ms.is_some(), "default two-minute timeout");
    }
}

/// Answers the three example questions with mint / yes / `name`.
pub(crate) fn mint_yes(name: &'static str) -> impl Fn(&QuestionView) -> Option<Answer> {
    move |q| {
        Some(match q.kind {
            QuestionKind::Select => Answer::Choice {
                value: "mint".into(),
            },
            QuestionKind::Confirm => Answer::Confirm { value: true },
            QuestionKind::Input => Answer::Text { value: name.into() },
        })
    }
}

#[tokio::test(start_paused = true)]
async fn the_questions_example_asks_what_the_reference_asks_and_reports_the_answers() {
    let (host, mut views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    runner.start(Arc::new(QuestionsExample)).unwrap();
    let asked = answer_until_status(&host, &mut views, mint_yes("pip")).await;
    assert_reference_questions(&asked);
    assert_eq!(
        host.notices(),
        vec!["You picked mint.", "Keep going: true.", "Called pip."]
    );
    assert_eq!(
        host.questions().view().status,
        vec![("example-questions".to_string(), "flavor mint".to_string())]
    );
    runner.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(start_paused = true)]
async fn the_questions_example_reports_declined_and_negative_answers() {
    let (host, mut views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    runner.start(Arc::new(QuestionsExample)).unwrap();
    answer_until_status(&host, &mut views, |q| match q.kind {
        QuestionKind::Select => None,
        QuestionKind::Confirm => Some(Answer::Confirm { value: false }),
        QuestionKind::Input => Some(Answer::Text {
            value: q.default_value.clone().unwrap(),
        }),
    })
    .await;
    assert_eq!(
        host.notices(),
        vec![
            "No flavor was picked.",
            "Keep going: false.",
            "Called pipsqueak."
        ]
    );
    assert_eq!(
        host.questions().view().status,
        vec![("example-questions".to_string(), "flavor none".to_string())]
    );
    runner.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(start_paused = true)]
async fn unanswered_example_questions_time_out_instead_of_blocking() {
    let (host, _views) = TestHost::new("s1");
    let started = tokio::time::Instant::now();
    let mut runner = ExtensionRunner::new(host.clone());
    runner.start(Arc::new(QuestionsExample)).unwrap();
    while host.questions().view().status.is_empty() {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(360) && elapsed < Duration::from_secs(362),
        "three default timeouts, got {elapsed:?}"
    );
    assert_eq!(
        host.notices(),
        vec![
            "No answer to \"Pick a flavor\"; it was cancelled.",
            "No flavor was picked.",
            "No answer to \"Keep going?\"; it was cancelled.",
            "No answer to keep going.",
            "No answer to \"What should it be called?\"; it was cancelled.",
            "No name was given.",
        ]
    );
    assert_eq!(
        host.questions().view().status,
        vec![("example-questions".to_string(), "flavor none".to_string())]
    );
    runner.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn the_greeting_example_counts_activations_and_greets_like_the_reference() {
    let greeting = Arc::new(GreetingExample::new());
    assert_eq!(greeting.greet("Armin").worker_activations, 0);
    let (host, _views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    runner.start(greeting.clone()).unwrap();
    let reply = greeting.greet("Armin");
    assert_eq!(
        reply.message,
        "Hello Armin from the bundled Session worker facet!!!"
    );
    assert_eq!(reply.worker_activations, 1);
    runner.shutdown(Duration::from_secs(1)).await;

    let mut again = ExtensionRunner::new(host.clone());
    again.start(greeting.clone()).unwrap();
    assert_eq!(greeting.greet("x").worker_activations, 2);

    let (shown, admitted) = greeting.hello(host.as_ref(), "").await;
    assert_eq!(
        shown,
        "We got this: Hello from the TUI from the bundled Session worker facet!!! Worker activations: 2."
    );
    assert_eq!(
        admitted.unwrap(),
        Admission {
            run: Some(1),
            entry: None
        }
    );
    let (_, _) = greeting.hello(host.as_ref(), "Armin").await;
    let prompts: Vec<String> = host
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Prompt { text, session, .. } => {
                assert_eq!(session, "s1");
                Some(text)
            }
            _ => None,
        })
        .collect();
    assert_eq!(prompts, vec!["pong: from the TUI", "pong: Armin"]);
    again.shutdown(Duration::from_secs(1)).await;
}

struct Waiter {
    deactivated: Arc<AtomicBool>,
}

impl Extension for Waiter {
    fn id(&self) -> &str {
        "waiter"
    }

    fn activate(&self, host: Arc<dyn ExtensionHost>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let _ = host
                .ask(
                    Question::confirm("Forever?"),
                    Some(Duration::ZERO),
                    CancellationToken::new(),
                )
                .await;
            host.notify(NoticeLevel::Info, "unreachable");
        })
    }

    fn deactivate(&self) {
        self.deactivated.store(true, Ordering::SeqCst);
    }
}

struct Future2;

impl Extension for Future2 {
    fn id(&self) -> &str {
        "from-the-future"
    }

    fn api_version(&self) -> u32 {
        2
    }

    fn activate(&self, _host: Arc<dyn ExtensionHost>) -> BoxFuture<'static, ()> {
        Box::pin(async { panic!("must not be activated") })
    }
}

struct Panics;

impl Extension for Panics {
    fn id(&self) -> &str {
        "panics"
    }

    fn activate(&self, _host: Arc<dyn ExtensionHost>) -> BoxFuture<'static, ()> {
        Box::pin(async { panic!("boom") })
    }
}

#[tokio::test(start_paused = true)]
async fn shutdown_deactivates_and_aborts_extensions_and_withdraws_their_questions() {
    let (host, _views) = TestHost::new("s1");
    let deactivated = Arc::new(AtomicBool::new(false));
    let mut runner = ExtensionRunner::new(host.clone());
    runner
        .start(Arc::new(Waiter {
            deactivated: deactivated.clone(),
        }))
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(host.questions().view().questions.len(), 1);
    assert_eq!(runner.ids(), vec!["waiter"]);
    runner.shutdown(Duration::from_millis(100)).await;
    assert!(deactivated.load(Ordering::SeqCst));
    assert!(host.questions().view().questions.is_empty());
    assert!(host.notices().is_empty());
}

#[tokio::test]
async fn the_runner_refuses_other_api_versions_and_duplicates_and_reports_panics() {
    let (host, _views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    let refused = runner.start(Arc::new(Future2)).unwrap_err();
    assert_eq!(
        refused,
        "Extension from-the-future needs extension API 2; this engine provides 1. It was not started."
    );
    runner.start(Arc::new(GreetingExample::new())).unwrap();
    let duplicate = runner.start(Arc::new(GreetingExample::new())).unwrap_err();
    assert_eq!(
        duplicate,
        "Extension pi-example-plugin is already running; the duplicate was not started."
    );
    runner.start(Arc::new(Panics)).unwrap();
    runner.shutdown(Duration::from_secs(1)).await;
    assert_eq!(
        host.notices(),
        vec![
            refused,
            duplicate,
            "Extension panics failed and was stopped.".into()
        ]
    );
}

#[tokio::test]
async fn the_call_host_drives_the_agent_through_protocol_calls() {
    let (host, _views) = TestHost::new("s9");
    let prompt = host.prompt("hi".into(), Some("k1".into())).await.unwrap();
    assert_eq!(
        prompt,
        Admission {
            run: Some(1),
            entry: None
        }
    );
    let steer = host.steer("left".into(), None).await.unwrap();
    assert_eq!(steer.entry, Some(2));
    host.follow_up("later".into(), Some(String::new()))
        .await
        .unwrap();
    assert_eq!(
        host.lookup("k1".into()).await.unwrap(),
        LookupResult::Known {
            mode: InputMode::Prompt,
            entry: None,
            run: Some(1),
            open: true,
            outcome: None
        }
    );
    assert_eq!(
        host.lookup("nope".into()).await.unwrap(),
        LookupResult::Unknown
    );
    assert_eq!(host.abort().await.unwrap(), None);
    host.compact(Some("short".into())).await.unwrap();
    let page = host.history_page(Some(10), 5_000).await.unwrap();
    assert!(page.entries.is_empty() && !page.has_older);
    assert_eq!(
        host.cancel_queued(2).await.unwrap(),
        CancelQueuedOutcome::NotFound
    );

    let calls = host.calls();
    assert_eq!(calls.len(), 9);
    match &calls[1] {
        Call::Enqueue {
            session,
            request,
            mode,
            text,
            images,
        } => {
            assert_eq!(session, "s9");
            assert!(request.starts_with("ext-"), "a fresh key: {request}");
            assert_eq!(*mode, InputMode::Steer);
            assert_eq!(text, "left");
            assert!(images.is_empty());
        }
        other => panic!("unexpected {other:?}"),
    }
    match &calls[2] {
        Call::Enqueue { request, mode, .. } => {
            assert!(request.starts_with("ext-"), "an empty key is no key");
            assert_eq!(*mode, InputMode::FollowUp);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        calls[5],
        Call::Stop {
            session: "s9".into()
        }
    );
    assert_eq!(
        calls[6],
        Call::Compact {
            session: "s9".into(),
            instructions: Some("short".into())
        }
    );
    assert_eq!(
        calls[7],
        Call::HistoryPage {
            session: "s9".into(),
            before: Some(10),
            limit: HISTORY_MAX_PAGE
        }
    );

    host.fail_next(ProtoError::new(ErrorCode::Busy, "a run is active"));
    let error = host.prompt("again".into(), None).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Busy);
}

#[tokio::test]
async fn an_unexpected_reply_is_an_internal_error() {
    let dispatch: Dispatch = Arc::new(|_| Box::pin(async { Ok(Reply::Ok) }));
    let host = CallHost::new("s".into(), QuestionHost::default(), dispatch);
    let error = host.prompt("x".into(), None).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Internal);
}
