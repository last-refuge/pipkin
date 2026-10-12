use super::*;
use serde_json::json;

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn valid() -> Value {
    json!({"Version":1,"AccessKeyId":"PROCESS","SecretAccessKey":"synthetic","SessionToken":"session","Expiration":"2099-01-01T00:00:00Z"})
}
fn command(value: &Value) -> String {
    format!("printf '%s' {}", quote(&value.to_string()))
}

#[tokio::test]
async fn helper_shell_environment_validation_and_redaction() {
    let env = BTreeMap::from([
        ("PIPKIN_HELPER_ID".into(), "SCOPED".into()),
        ("HOME".into(), "".into()),
    ]);
    let cmd = r#"test -z "$HOME" && printf '{"Version":1,"AccessKeyId":"%s","SecretAccessKey":"synthetic"}' "$PIPKIN_HELPER_ID"; printf 'private-helper-detail' >&2"#;
    assert_eq!(
        credentials(cmd, &env, &CancellationToken::new())
            .await
            .unwrap()
            .access_key_id,
        "SCOPED"
    );
    for (value, expected) in [
        (json!({"Version":2}), "Version 1"),
        (json!({"Version":1}), "missing"),
        (
            {
                let mut v = valid();
                v["Expiration"] = json!("2000-01-01T00:00:00Z");
                v
            },
            "expired",
        ),
        (
            {
                let mut v = valid();
                v["Expiration"] = json!("nonsense");
                v
            },
            "invalid Expiration",
        ),
    ] {
        let error = credentials(&command(&value), &env, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    for (cmd, expected) in [
        (
            "printf private-helper-detail; printf private-helper-detail >&2; exit 7",
            "exited",
        ),
        ("printf private-helper-detail", "invalid JSON"),
    ] {
        let error = credentials(cmd, &env, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("private-helper-detail"));
    }
    assert_eq!(
        credentials(
            &format!("printf ' \\n'; {}; printf '\\n'", command(&valid())),
            &env,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .session_token
        .as_deref(),
        Some("session")
    );
}

#[tokio::test]
async fn helper_bounds_both_streams_and_recovers() {
    for (suffix, expected) in [("", "stdout"), (" >&2", "stderr")] {
        let cmd = format!("/usr/bin/head -c {} /dev/zero{suffix}", LIMIT + 1);
        let error = credentials(&cmd, &BTreeMap::new(), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            error.contains(expected) && error.contains("too large"),
            "{error}"
        );
    }
    assert!(
        credentials(
            &command(&valid()),
            &BTreeMap::new(),
            &CancellationToken::new()
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn helper_cancellation_and_timeout_kill_descendants() {
    for cancel_run in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let marker = dir.path().join("escaped");
        // The child inherits both output pipes. It must die even if the shell is still waiting.
        let cmd = format!(
            "(/bin/sleep 0.4; printf escaped > {}) & printf ready > {}; wait",
            quote(marker.to_str().unwrap()),
            quote(ready.to_str().unwrap())
        );
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let ready_task = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !ready.exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            if cancel_run {
                trigger.cancel();
            }
        });
        let result = execute(
            &cmd,
            &BTreeMap::new(),
            &cancel,
            if cancel_run {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(150)
            },
        )
        .await
        .unwrap_err();
        ready_task.await.unwrap();
        assert!(
            result.contains(if cancel_run { "aborted" } else { "timed out" }),
            "{result}"
        );
        tokio::time::sleep(Duration::from_millis(450)).await;
        assert!(!marker.exists(), "helper descendant survived");
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        credentials("exit 0", &BTreeMap::new(), &cancel)
            .await
            .unwrap_err()
            .contains("aborted")
    );
    assert!(
        credentials(
            &command(&valid()),
            &BTreeMap::new(),
            &CancellationToken::new()
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn helper_outcomes_match_twelve_executed_sdk_cases() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/native-aws-process/reference.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 12);
    for case in cases {
        let actual = credentials(
            case["command"].as_str().unwrap(),
            &BTreeMap::new(),
            &CancellationToken::new(),
        )
        .await;
        if let Some(expected) = case.get("credentials") {
            let c = actual.unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
            assert_eq!(c.access_key_id, expected["accessKeyId"].as_str().unwrap());
            assert_eq!(
                c.secret_access_key,
                expected["secretAccessKey"].as_str().unwrap()
            );
            assert_eq!(
                c.session_token.as_deref(),
                expected["sessionToken"].as_str()
            );
            assert_eq!(
                c.expires_at,
                expected["expiration"]
                    .as_str()
                    .and_then(super::super::parse_iso8601)
            );
        } else {
            assert!(case["error"].is_string());
            let error = actual.unwrap_err();
            let expected = match case["name"].as_str().unwrap() {
                "expired" => "expired",
                "invalid-version" => "Version 1",
                "missing-keys" => "missing",
                "invalid-json" => "invalid JSON",
                "exit-error" => "exited",
                "stdout-bound" => "stdout is too large",
                "stderr-bound" => "stderr is too large",
                name => panic!("unexpected {name}"),
            };
            assert!(error.contains(expected), "{}: {error}", case["name"]);
            assert!(!error.contains("synthetic-detail"));
        }
    }
}
