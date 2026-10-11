//! Process extensions end to end, with real child processes (POSIX sh) and real time.

use std::path::Path;
use std::time::Instant;

use pipkin_engine_protocol::{Call, NoticeLevel};

use super::*;
use crate::extensions::ExtensionRunner;
use crate::extensions::testing::TestHost;
use crate::extensions::tests::{answer_until_status, assert_reference_questions, mint_yes};

const WAIT: Duration = Duration::from_secs(10);

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions")
        .join(name)
        .display()
        .to_string()
}

/// `sh -c <script>`; the script starts after a correct handshake when `handshake` is set.
fn script(id: &str, body: &str, handshake: bool) -> ProcessConfig {
    let prelude = "IFS= read -r l || exit 1\nprintf '%s\\n' '{\"type\":\"hello\",\"api\":1}'\nIFS= read -r l || exit 1\n";
    let script = if handshake {
        format!("{prelude}{body}")
    } else {
        body.to_string()
    };
    ProcessConfig::new(id, vec!["sh".into(), "-c".into(), script])
}

async fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < WAIT, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn notice_levels(host: &TestHost) -> Vec<(NoticeLevel, String)> {
    host.questions()
        .view()
        .notices
        .into_iter()
        .map(|n| (n.level, n.message))
        .collect()
}

/// Whether `pid` is a live (non-zombie) process.
#[cfg(target_os = "linux")]
fn alive(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => false,
        Ok(stat) => {
            let state = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next());
            !matches!(state, Some("Z" | "X"))
        }
    }
}

fn pid_from(host: &TestHost, prefix: &str) -> Option<i32> {
    host.notices()
        .iter()
        .find_map(|n| n.strip_prefix(prefix).and_then(|p| p.trim().parse().ok()))
}

#[tokio::test]
async fn the_sh_questions_fixture_behaves_like_the_reference_example() {
    let (host, mut views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    let config = ProcessConfig::new(
        "pi-example-questions",
        vec!["sh".into(), fixture("questions.sh")],
    );
    runner
        .start(Arc::new(ProcessExtension::new(config)))
        .unwrap();
    let asked = tokio::time::timeout(
        WAIT,
        answer_until_status(&host, &mut views, mint_yes("pip")),
    )
    .await
    .expect("the fixture finishes");
    assert_reference_questions(&asked);
    assert_eq!(
        host.notices(),
        vec!["You picked mint.", "Keep going: true.", "Called pip."]
    );
    assert_eq!(
        host.questions().view().status,
        vec![("example-questions".to_string(), "flavor mint".to_string())]
    );
    // It waits for deactivation, then exits cleanly: no notice.
    let started = Instant::now();
    runner.shutdown(Duration::from_secs(5)).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a prompt, clean exit"
    );
    assert_eq!(host.notices().len(), 3);
}

#[tokio::test]
async fn the_sh_questions_fixture_reports_declined_questions() {
    let (host, mut views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    let config = ProcessConfig::new("q", vec!["sh".into(), fixture("questions.sh")]);
    runner
        .start(Arc::new(ProcessExtension::new(config)))
        .unwrap();
    tokio::time::timeout(
        WAIT,
        answer_until_status(&host, &mut views, |q| match q.kind {
            QuestionKind::Confirm => Some(Answer::Confirm { value: false }),
            _ => None,
        }),
    )
    .await
    .expect("the fixture finishes");
    assert_eq!(
        host.notices(),
        vec![
            "No flavor was picked.",
            "Keep going: false.",
            "No name was given."
        ]
    );
    assert_eq!(
        host.questions().view().status,
        vec![("example-questions".to_string(), "flavor none".to_string())]
    );
    runner.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_crash_is_reported_once_and_never_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let launches = dir.path().join("launches");
    let (host, _views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    let body = format!(
        "echo launched >> '{}'\necho 'it broke' >&2\nexit 3\n",
        launches.display()
    );
    let config = script("crashy", &body, true);
    runner
        .start(Arc::new(ProcessExtension::new(config)))
        .unwrap();
    wait_for("the crash notice", || !host.notices().is_empty()).await;
    assert_eq!(
        notice_levels(&host),
        vec![(
            NoticeLevel::Error,
            "Extension crashy stopped unexpectedly (exit code 3). It said: it broke".into()
        )]
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(std::fs::read_to_string(&launches).unwrap(), "launched\n");
    assert_eq!(host.notices().len(), 1, "no restart, no second notice");
    runner.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn a_killed_extension_is_reported_with_its_signal() {
    let (host, _views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    let config = script("doomed", "kill -9 $$\n", true);
    runner
        .start(Arc::new(ProcessExtension::new(config)))
        .unwrap();
    wait_for("the crash notice", || !host.notices().is_empty()).await;
    assert_eq!(
        host.notices(),
        vec!["Extension doomed stopped unexpectedly (signal 9)."]
    );
    runner.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn a_clean_exit_is_not_a_crash() {
    let (host, _views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    let body = "printf '%s\\n' '{\"id\":1,\"method\":\"notify\",\"params\":{\"level\":\"info\",\"message\":\"bye\"}}'\nexit 0\n";
    runner
        .start(Arc::new(ProcessExtension::new(script("done", body, true))))
        .unwrap();
    wait_for("the last notice", || !host.notices().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(host.notices(), vec!["bye"]);
    runner.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn an_oversized_line_stops_the_extension() {
    let (host, _views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    let mut config = script("big", "printf '%2000s\\n' x\nsleep 30\n", true);
    config.max_line_bytes = 1024;
    let started = Instant::now();
    runner
        .start(Arc::new(ProcessExtension::new(config)))
        .unwrap();
    wait_for("the violation notice", || !host.notices().is_empty()).await;
    assert_eq!(
        notice_levels(&host),
        vec![(
            NoticeLevel::Error,
            "Extension big sent a line longer than 1024 bytes; it was stopped.".into()
        )]
    );
    runner.shutdown(Duration::from_secs(1)).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "killed, not waited for"
    );
}

#[tokio::test]
async fn handshake_failures_stop_the_extension() {
    let (host, _views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    let v2 = script(
        "v2",
        "IFS= read -r l\nprintf '%s\\n' '{\"type\":\"hello\",\"api\":2}'\nsleep 30\n",
        false,
    );
    let mut mute = script("mute", "sleep 30\n", false);
    mute.handshake_timeout = Duration::from_millis(200);
    let chatty = script(
        "chatty",
        "IFS= read -r l\necho 'not json'\nsleep 30\n",
        false,
    );
    let garbage = script("garbage", "printf '{\"id\":1}\\n'\nsleep 30\n", true);
    for config in [v2, mute, chatty, garbage] {
        runner
            .start(Arc::new(ProcessExtension::new(config)))
            .unwrap();
    }
    wait_for("four notices", || host.notices().len() == 4).await;
    let mut notices = host.notices();
    notices.sort();
    assert_eq!(
        notices,
        vec![
            "Extension chatty did not start with a hello; it was stopped.",
            "Extension garbage sent a message that is neither a request nor a known message; it was stopped.",
            "Extension mute did not answer the hello within 0.2 s; it was stopped.",
            "Extension v2 speaks extension API 2, but this engine speaks 1; it was stopped.",
        ]
    );
    runner.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn requests_reach_the_agent_and_errors_come_back_as_replies() {
    let (host, _views) = TestHost::new("s7");
    let mut runner = ExtensionRunner::new(host.clone());
    // Each check notifies "ok <name>" or "bad <name>: <reply>"-less "bad <name>".
    let body = r#"
check() { case $reply in $2) say "ok $1" ;; *) say "bad $1" ;; esac; }
say() { printf '{"id":"say-%s","method":"notify","params":{"level":"info","message":"%s"}}\n' "$1" "$1"; IFS= read -r ignored; }
printf '%s\n' '{"id":1,"method":"prompt","params":{"text":"hello agent","requestKey":"k1"}}'
IFS= read -r reply; check prompt '{"id":1,"result":{"entry":null,"run":1}}'
printf '%s\n' '{"id":"two","method":"lookup","params":{"requestKey":"k1"}}'
IFS= read -r reply; check lookup '{"id":"two","result":{*"state":"known"*}}'
printf '%s\n' '{"id":3,"method":"fly"}'
IFS= read -r reply; check unknown '{"id":3,"error":{"code":"unsupported","message":"Unknown method fly."}}'
printf '%s\n' '{"id":4,"method":"ask","params":{"kind":"maybe","title":"x"}}'
IFS= read -r reply; check params '{"id":4,"error":{"code":"invalid",*}}'
printf '%s\n' '{"id":"a","method":"ask","params":{"kind":"confirm","title":"Wait","timeoutMs":0}}'
printf '%s\n' '{"type":"cancel","id":"a"}'
IFS= read -r reply; check cancel '{"id":"a","result":null}'
printf '%s\n' '{"type":"from-the-future"}'
printf '%s\n' '{"id":5,"method":"historyPage","params":{"limit":10}}'
IFS= read -r reply; check history '{"id":5,"result":{"entries":[],"hasOlder":false}}'
printf '%s\n' '{"id":6,"method":"setStatus","params":{"key":"k","text":"v"}}'
IFS= read -r reply; check status '{"id":6,"result":null}'
say finished
while IFS= read -r l; do :; done
"#;
    runner
        .start(Arc::new(ProcessExtension::new(script("rpc", body, true))))
        .unwrap();
    wait_for("the last check", || {
        host.notices().iter().any(|n| n == "finished")
    })
    .await;
    assert_eq!(
        host.notices(),
        vec![
            "ok prompt",
            "ok lookup",
            "ok unknown",
            "ok params",
            "ok cancel",
            "ok history",
            "ok status",
            "finished"
        ]
    );
    assert!(host.questions().view().questions.is_empty());
    assert_eq!(
        host.questions().view().status,
        vec![("k".to_string(), "v".to_string())]
    );
    let calls = host.calls();
    assert!(matches!(
        &calls[0],
        Call::Prompt { session, request, text, .. }
            if session == "s7" && request == "k1" && text == "hello agent"
    ));
    runner.shutdown(Duration::from_secs(2)).await;
    assert_eq!(host.notices().len(), 8, "a clean stop posts nothing");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn stopping_kills_the_whole_process_group() {
    let (host, _views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    // Leaves a background child behind and exits as soon as it is deactivated.
    let body = r#"
sleep 1000 &
printf '{"id":1,"method":"notify","params":{"level":"info","message":"child %s"}}\n' "$!"
printf '{"id":2,"method":"notify","params":{"level":"info","message":"leader %s"}}\n' "$$"
while IFS= read -r l; do case $l in *deactivate*) exit 0 ;; esac; done
"#;
    runner
        .start(Arc::new(ProcessExtension::new(script("group", body, true))))
        .unwrap();
    wait_for("both pids", || {
        pid_from(&host, "child ").is_some() && pid_from(&host, "leader ").is_some()
    })
    .await;
    let child = pid_from(&host, "child ").unwrap();
    let leader = pid_from(&host, "leader ").unwrap();
    assert!(alive(child) && alive(leader));
    runner.shutdown(Duration::from_secs(5)).await;
    wait_for("the group to be gone", || !alive(child) && !alive(leader)).await;
    assert_eq!(host.notices().len(), 2, "a clean stop posts nothing");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_extension_ignoring_deactivate_is_killed_after_the_grace_period() {
    let (host, _views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    let body = r#"
trap '' TERM HUP
printf '{"id":1,"method":"notify","params":{"level":"info","message":"leader %s"}}\n' "$$"
exec 0</dev/null
while :; do sleep 1; done
"#;
    let mut config = script("stubborn", body, true);
    config.stop_grace = Duration::from_millis(200);
    runner
        .start(Arc::new(ProcessExtension::new(config)))
        .unwrap();
    wait_for("the pid", || pid_from(&host, "leader ").is_some()).await;
    let leader = pid_from(&host, "leader ").unwrap();
    let started = Instant::now();
    runner.shutdown(Duration::from_secs(5)).await;
    assert!(started.elapsed() < Duration::from_secs(3));
    wait_for("the leader to be gone", || !alive(leader)).await;
}

#[tokio::test]
async fn a_missing_program_is_reported() {
    let (host, _views) = TestHost::new("s1");
    let mut runner = ExtensionRunner::new(host.clone());
    let config = ProcessConfig::new("ghost", vec!["/nonexistent/pipkin-extension".into()]);
    runner
        .start(Arc::new(ProcessExtension::new(config)))
        .unwrap();
    wait_for("the notice", || !host.notices().is_empty()).await;
    let notice = host.notices().remove(0);
    assert!(
        notice.starts_with("Extension ghost could not start: "),
        "{notice}"
    );
    let empty = ProcessConfig::new("nothing", vec![]);
    runner
        .start(Arc::new(ProcessExtension::new(empty)))
        .unwrap();
    wait_for("the second notice", || host.notices().len() == 2).await;
    assert_eq!(
        host.notices()[1],
        "Extension nothing has no command to run."
    );
    runner.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn bounded_line_reader_handles_limits_and_partial_lines() {
    let data: &[u8] = b"short\nexactly10c\nthis line is too long\n";
    let mut reader = tokio::io::BufReader::with_capacity(4, data);
    let mut buf = Vec::new();
    assert!(matches!(
        read_line_bounded(&mut reader, &mut buf, 10).await.unwrap(),
        LineRead::Line
    ));
    assert_eq!(buf, b"short");
    assert!(matches!(
        read_line_bounded(&mut reader, &mut buf, 10).await.unwrap(),
        LineRead::Line
    ));
    assert_eq!(buf, b"exactly10c");
    assert!(matches!(
        read_line_bounded(&mut reader, &mut buf, 10).await.unwrap(),
        LineRead::TooLong
    ));
    let mut tail: &[u8] = b"no newline";
    assert!(matches!(
        read_line_bounded(&mut tail, &mut buf, 100).await.unwrap(),
        LineRead::Line
    ));
    assert_eq!(buf, b"no newline");
    assert!(matches!(
        read_line_bounded(&mut tail, &mut buf, 100).await.unwrap(),
        LineRead::Eof
    ));
}

#[test]
fn replies_keep_the_id_first_and_respect_the_line_limit() {
    let id = json!(7);
    assert_eq!(
        encode_reply(&id, Ok(json!("mint")), 100),
        r#"{"id":7,"result":"mint"}"#
    );
    assert_eq!(
        encode_reply(&json!("x"), Err(RpcError::new("busy", "no")), 100),
        r#"{"id":"x","error":{"code":"busy","message":"no"}}"#
    );
    assert_eq!(
        encode_reply(&id, Ok(json!("y".repeat(200))), 100),
        r#"{"id":7,"error":{"code":"tooLarge","message":"The reply is longer than 100 bytes."}}"#
    );
    let proto: RpcError =
        ProtoError::new(pipkin_engine_protocol::ErrorCode::NotFound, "gone").into();
    assert_eq!(proto.code, "notFound");
}
