//! Every rule of the question host, ported from the reference's
//! `coding-agent/test/experimental-ui-requests.test.ts` plus the limits it does not cover.

use std::sync::Mutex;

use super::*;

fn host() -> QuestionHost {
    QuestionHost::with_clock(None, Arc::new(|| 1_000))
}

fn recording() -> (QuestionHost, Arc<Mutex<Vec<UiView>>>) {
    let views = Arc::new(Mutex::new(Vec::new()));
    let sink = views.clone();
    let host = QuestionHost::with_clock(
        Some(Arc::new(move |view| sink.lock().unwrap().push(view))),
        Arc::new(|| 1_000),
    );
    (host, views)
}

fn never() -> CancellationToken {
    CancellationToken::new()
}

fn choice(value: &str) -> Answer {
    Answer::Choice {
        value: value.into(),
    }
}

fn text(value: &str) -> Answer {
    Answer::Text {
        value: value.into(),
    }
}

fn yes_no(value: bool) -> Answer {
    Answer::Confirm { value }
}

fn last_notice(host: &QuestionHost) -> NoticeView {
    host.view().notices.last().cloned().expect("a notice")
}

#[tokio::test(start_paused = true)]
async fn a_select_waits_for_an_answer_which_must_be_one_of_its_choices() {
    let ui = host();
    let answer = tokio::spawn(ui.ask(
        Question::select(
            "Pick a branch",
            vec![
                option("main", "main", None),
                option("dev", "dev", Some("work in progress")),
            ],
        ),
        None,
        never(),
    ));
    let view = ui.view();
    assert_eq!(view.questions.len(), 1);
    let q = &view.questions[0];
    assert_eq!(q.id, "q1");
    assert_eq!(q.kind, QuestionKind::Select);
    assert_eq!(q.title, "Pick a branch");
    assert_eq!(q.deadline_ms, Some(1_000 + 120_000));
    assert_eq!(
        q.options,
        vec![
            option("main", "main", None),
            option("dev", "dev", Some("work in progress"))
        ]
    );
    // Wrong answers are refused with the reference's reasons and the question stays open.
    assert_eq!(
        ui.respond("q1", choice("nope")),
        Err("That is not one of the choices.".into())
    );
    assert_eq!(
        ui.respond("q1", yes_no(true)),
        Err("An answer must be text.".into())
    );
    assert_eq!(ui.view().questions.len(), 1);
    // A text answer naming a choice is accepted, and comes back as that choice.
    assert_eq!(ui.respond("q1", text("dev")), Ok(()));
    assert_eq!(answer.await.unwrap(), Some(choice("dev")));
    assert!(ui.view().questions.is_empty());
    // Answering twice is refused.
    assert_eq!(
        ui.respond("q1", choice("dev")),
        Err("That question is no longer open.".into())
    );
}

#[tokio::test(start_paused = true)]
async fn confirm_and_input_take_their_own_kinds_of_answer() {
    let ui = host();
    let yes = tokio::spawn(ui.ask(
        Question::confirm("Delete it?").with_message("This cannot be undone"),
        None,
        never(),
    ));
    let name = tokio::spawn(
        ui.ask(
            Question::input("Name?")
                .with_placeholder("a name")
                .with_default("x"),
            None,
            never(),
        ),
    );
    let view = ui.view();
    let (confirm, input) = (&view.questions[0], &view.questions[1]);
    assert_eq!(confirm.kind, QuestionKind::Confirm);
    assert_eq!(confirm.message.as_deref(), Some("This cannot be undone"));
    assert!(confirm.options.is_empty());
    assert_eq!(input.kind, QuestionKind::Input);
    assert_eq!(input.placeholder.as_deref(), Some("a name"));
    assert_eq!(input.default_value.as_deref(), Some("x"));

    assert_eq!(
        ui.respond(&confirm.id, text("yes")),
        Err("A confirmation needs yes or no.".into())
    );
    assert_eq!(ui.respond(&confirm.id, yes_no(false)), Ok(()));
    assert_eq!(
        ui.respond(&input.id, yes_no(true)),
        Err("An answer must be text.".into())
    );
    assert_eq!(
        ui.respond(&input.id, text(&"y".repeat(10_001))),
        Err("An answer can be at most 10000 characters.".into())
    );
    // Counted in UTF-16 units like the reference: 5001 astral characters are 10002 units.
    assert_eq!(
        ui.respond(&input.id, text(&"😀".repeat(5_001))),
        Err("An answer can be at most 10000 characters.".into())
    );
    assert_eq!(ui.respond(&input.id, text("ada")), Ok(()));
    assert_eq!(yes.await.unwrap(), Some(yes_no(false)));
    assert_eq!(name.await.unwrap(), Some(text("ada")));
}

#[tokio::test(start_paused = true)]
async fn an_input_answer_of_exactly_the_limit_is_accepted() {
    let ui = host();
    let name = tokio::spawn(ui.ask(Question::input("Name?"), None, never()));
    let long = "y".repeat(UI_MAX_INPUT_CHARS);
    assert_eq!(ui.respond("q1", choice(&long)), Ok(()));
    assert_eq!(name.await.unwrap(), Some(text(&long)));
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_question_is_cancelled_at_its_deadline() {
    let ui = host();
    let answer = tokio::spawn(ui.ask(Question::confirm("Proceed?"), None, never()));
    assert_eq!(ui.view().questions[0].deadline_ms, Some(121_000));
    tokio::time::sleep(Duration::from_millis(119_999)).await;
    assert_eq!(ui.view().questions.len(), 1, "still open just before");
    tokio::time::sleep(Duration::from_millis(2)).await;
    assert_eq!(answer.await.unwrap(), None);
    assert!(ui.view().questions.is_empty());
    let notice = last_notice(&ui);
    assert_eq!(notice.level, NoticeLevel::Info);
    assert_eq!(
        notice.message,
        "No answer to \"Proceed?\"; it was cancelled."
    );
    assert_eq!(notice.id, "n2", "questions and notices share one counter");

    // A caller's own timeout, and no timeout at all.
    let short = tokio::spawn(ui.ask(
        Question::confirm("Quick?"),
        Some(Duration::from_millis(50)),
        never(),
    ));
    let forever =
        tokio::spawn(ui.ask(Question::confirm("Patient?"), Some(Duration::ZERO), never()));
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(short.await.unwrap(), None);
    let open = ui.view().questions;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].title, "Patient?");
    assert_eq!(open[0].deadline_ms, None);
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(ui.view().questions.len(), 1, "zero waits indefinitely");
    ui.cancel_all("done");
    assert_eq!(forever.await.unwrap(), None);
    assert_eq!(last_notice(&ui).message, "done");
}

#[tokio::test(start_paused = true)]
async fn the_timeout_notice_quotes_at_most_eighty_characters_of_the_title() {
    let ui = host();
    let title = "t".repeat(100);
    let answer = tokio::spawn(ui.ask(
        Question::confirm(title),
        Some(Duration::from_millis(10)),
        never(),
    ));
    assert_eq!(answer.await.unwrap(), None);
    assert_eq!(
        last_notice(&ui).message,
        format!("No answer to \"{}\"; it was cancelled.", "t".repeat(80))
    );
}

#[tokio::test(start_paused = true)]
async fn cancelling_aborting_dropping_and_shutting_down_all_release_the_asker() {
    let ui = host();
    let a = tokio::spawn(ui.ask(Question::confirm("A?"), None, never()));
    let id = ui.view().questions[0].id.clone();
    assert!(ui.cancel(&id));
    assert!(!ui.cancel(&id));
    assert_eq!(a.await.unwrap(), None);

    let token = CancellationToken::new();
    let b = tokio::spawn(ui.ask(Question::confirm("B?"), None, token.clone()));
    assert_eq!(ui.view().questions.len(), 1);
    token.cancel();
    assert_eq!(b.await.unwrap(), None);
    assert!(ui.view().questions.is_empty());

    let already = CancellationToken::new();
    already.cancel();
    assert_eq!(ui.ask(Question::confirm("C?"), None, already).await, None);
    assert!(ui.view().questions.is_empty());

    // Dropping the asking future withdraws its question.
    let dropped = ui.ask(Question::confirm("Gone?"), None, never());
    assert_eq!(ui.view().questions.len(), 1);
    drop(dropped);
    assert!(ui.view().questions.is_empty());

    let d = tokio::spawn(ui.ask(Question::input("D?"), None, never()));
    let e = tokio::spawn(ui.ask(
        Question::select("E?", vec![option("1", "one", None)]),
        None,
        never(),
    ));
    ui.cancel_all("The session ended.");
    assert_eq!(d.await.unwrap(), None);
    assert_eq!(e.await.unwrap(), None);
    assert!(ui.view().questions.is_empty());
    assert_eq!(last_notice(&ui).message, "The session ended.");
}

#[tokio::test(start_paused = true)]
async fn dispose_cancels_everything_and_refuses_new_questions() {
    let (ui, views) = recording();
    let a = tokio::spawn(ui.ask(Question::confirm("A?"), Some(Duration::ZERO), never()));
    let b = tokio::spawn(ui.ask(Question::input("B?"), None, never()));
    ui.dispose();
    assert_eq!(a.await.unwrap(), None);
    assert_eq!(b.await.unwrap(), None);
    assert!(ui.view().questions.is_empty());
    assert!(ui.view().notices.is_empty(), "disposal posts no notice");
    let published = views.lock().unwrap().len();
    assert_eq!(
        ui.ask(Question::confirm("Late?"), None, never()).await,
        None
    );
    assert!(ui.view().questions.is_empty());
    assert_eq!(views.lock().unwrap().len(), published, "nothing shown");
}

#[tokio::test(start_paused = true)]
async fn the_number_of_open_questions_is_bounded() {
    let ui = host();
    let open: Vec<_> = (0..UI_MAX_PENDING)
        .map(|i| {
            tokio::spawn(ui.ask(
                Question::confirm(format!("Q{i}")),
                Some(Duration::ZERO),
                never(),
            ))
        })
        .collect();
    assert_eq!(ui.view().questions.len(), UI_MAX_PENDING);
    assert_eq!(
        ui.ask(Question::confirm("one too many"), None, never())
            .await,
        None
    );
    assert_eq!(ui.view().questions.len(), UI_MAX_PENDING);
    let notice = last_notice(&ui);
    assert_eq!(notice.level, NoticeLevel::Warning);
    assert_eq!(
        notice.message,
        "Too many open questions; \"one too many\" was not shown."
    );
    ui.cancel_all("");
    for answer in open {
        assert_eq!(answer.await.unwrap(), None);
    }
    assert_eq!(ui.view().notices.len(), 1, "an empty reason posts nothing");
}

#[tokio::test(start_paused = true)]
async fn an_empty_select_is_not_shown_and_notices_stay_bounded() {
    let ui = host();
    assert_eq!(
        ui.ask(Question::select("Nothing to pick", vec![]), None, never())
            .await,
        None
    );
    assert!(ui.view().questions.is_empty());
    assert!(ui.view().notices.is_empty());
    for i in 0..40 {
        ui.notify(NoticeLevel::Warning, &format!("note {i}"));
    }
    let notices = ui.view().notices;
    assert_eq!(notices.len(), UI_MAX_NOTICES);
    assert_eq!(notices[0].message, "note 20", "oldest dropped first");
    assert_eq!(notices.last().unwrap().message, "note 39");
    // The empty select consumed no id.
    assert_eq!(notices[0].id, "n21");

    ui.set_status("build", Some("compiling"));
    ui.set_status("lint", Some("clean"));
    ui.set_status("build", None);
    assert_eq!(ui.view().status, vec![("lint".into(), "clean".into())]);
    ui.set_status("lint", Some("dirty"));
    assert_eq!(ui.view().status, vec![("lint".into(), "dirty".into())]);
}

#[tokio::test(start_paused = true)]
async fn texts_are_cut_to_their_limits() {
    let ui = host();
    let _q = ui.ask(
        Question::confirm("t".repeat(400)).with_message("m".repeat(2_500)),
        None,
        never(),
    );
    let q = ui.view().questions[0].clone();
    assert_eq!(q.title.len(), UI_MAX_TITLE_CHARS);
    assert_eq!(q.message.unwrap().len(), UI_MAX_MESSAGE_CHARS);

    ui.notify(NoticeLevel::Error, &"n".repeat(600));
    assert_eq!(last_notice(&ui).message.len(), UI_MAX_NOTICE_CHARS);
    assert_eq!(last_notice(&ui).level, NoticeLevel::Error);

    ui.set_status(&"k".repeat(100), Some(&"s".repeat(400)));
    let (key, text) = ui.view().status[0].clone();
    assert_eq!(key.len(), UI_MAX_STATUS_KEY_CHARS);
    assert_eq!(text.len(), UI_MAX_STATUS_TEXT_CHARS);
    // Like the reference, clearing uses the key as given: the long key does not match.
    ui.set_status(&"k".repeat(100), None);
    assert_eq!(ui.view().status.len(), 1);
    ui.set_status(&key, None);
    assert!(ui.view().status.is_empty());
}

#[test]
fn utf16_clipping_never_splits_a_character() {
    assert_eq!(clip_utf16("abc", 2), "ab");
    assert_eq!(clip_utf16("abc", 5), "abc");
    // An astral character is two units; one unit left cannot hold it.
    assert_eq!(clip_utf16("a😀b", 2), "a");
    assert_eq!(clip_utf16("a😀b", 3), "a😀");
    assert_eq!(clip_utf16("é", 1), "é");
}

#[tokio::test(start_paused = true)]
async fn the_first_answer_wins_and_every_change_is_published_in_order() {
    let (ui, views) = recording();
    let answer = tokio::spawn(ui.ask(
        Question::select(
            "Which?",
            vec![option("a", "A", None), option("b", "B", None)],
        ),
        None,
        never(),
    ));
    assert_eq!(ui.respond("q1", choice("a")), Ok(()));
    assert_eq!(
        ui.respond("q1", choice("b")),
        Err("That question is no longer open.".into())
    );
    assert_eq!(answer.await.unwrap(), Some(choice("a")));
    ui.notify(NoticeLevel::Info, "hi");
    ui.set_status("k", Some("v"));
    ui.set_status("k", Some("v"));
    ui.dismiss_notices();
    ui.dismiss_notices();
    let views = views.lock().unwrap().clone();
    let shape: Vec<(usize, usize, usize)> = views
        .iter()
        .map(|v| (v.questions.len(), v.notices.len(), v.status.len()))
        .collect();
    // asked, answered, notice, status, dismissed; refusals and no-op changes publish nothing.
    assert_eq!(
        shape,
        vec![(1, 0, 0), (0, 0, 0), (0, 1, 0), (0, 1, 1), (0, 0, 1)]
    );
    assert!(ui.view().notices.is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_answer_racing_the_deadline_is_kept() {
    let ui = host();
    let answer = ui.ask(
        Question::confirm("Race?"),
        Some(Duration::from_millis(10)),
        never(),
    );
    tokio::time::advance(Duration::from_millis(10)).await;
    // The deadline has passed but the asker has not looked yet: the answer still wins.
    assert_eq!(ui.respond("q1", yes_no(true)), Ok(()));
    assert_eq!(answer.await, Some(yes_no(true)));
    assert!(ui.view().notices.is_empty());
}
