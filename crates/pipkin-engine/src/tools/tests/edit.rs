//! `edit`: reference outputs (`golden-edit.json`) and ports of `tools.test.ts` "edit".

use std::time::Duration;

use serde_json::{Value, json};

use super::{ctx, execute, output_text, temp};
use crate::tools::edit::{
    Edit, EditTool, apply_edits, normalize_for_fuzzy_match, prepare_edit_arguments,
};
use crate::tools::path::mutation_key;
use crate::tools::{Tool, run};

#[tokio::test]
async fn edits_match_the_reference() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("golden-edit.json")).expect("golden json");
    assert!(cases.len() > 20);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let dir = temp();
        std::fs::write(dir.path().join("f.txt"), case["file"].as_str().unwrap()).unwrap();
        let (result, _) = execute(&EditTool, case["args"].clone(), &ctx(dir.path())).await;
        match (&result, case["result"].get("ok"), case["result"].get("err")) {
            (Ok(output), Some(expected), None) => {
                assert_eq!(
                    output_text(output),
                    expected["content"][0]["text"].as_str().unwrap(),
                    "{name}"
                );
                assert_eq!(
                    output.details.as_ref(),
                    Some(&expected["details"]),
                    "{name}"
                );
            }
            (Err(error), None, Some(expected)) => {
                assert_eq!(error.message, expected.as_str().unwrap(), "{name}")
            }
            _ => panic!("{name}: {result:?}"),
        }
        let after = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
        assert_eq!(after, case["after"].as_str().unwrap(), "{name}");
    }
}

#[test]
fn repairs_edits_sent_as_a_json_string_a_single_object_or_top_level_text() {
    let edit = json!({"oldText": "a", "newText": "b"});
    let as_string = json!({"path": "f", "edits": edit_string(std::slice::from_ref(&edit))});
    assert_eq!(
        prepare_edit_arguments(as_string),
        json!({"path": "f", "edits": [edit]})
    );
    assert_eq!(
        prepare_edit_arguments(json!({"path": "f", "edits": edit.to_string()})),
        json!({"path": "f", "edits": [edit]})
    );
    assert_eq!(
        prepare_edit_arguments(json!({"path": "f", "edits": edit})),
        json!({"path": "f", "edits": [edit]})
    );
    assert_eq!(
        prepare_edit_arguments(
            json!({"path": "f", "edits": [edit], "oldText": "c", "newText": "d"})
        ),
        json!({"path": "f", "edits": [edit, {"oldText": "c", "newText": "d"}]})
    );
    assert_eq!(
        prepare_edit_arguments(json!({"path": "f", "edits": "not json"})),
        json!({"path": "f", "edits": "not json"})
    );
}

fn edit_string(edits: &[Value]) -> String {
    Value::Array(edits.to_vec()).to_string()
}

#[tokio::test]
async fn applies_disjoint_edits_and_returns_both_diff_formats() {
    let dir = temp();
    std::fs::write(dir.path().join("edit.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
    let (result, _) = execute(
        &EditTool,
        json!({"path": "edit.txt", "edits": [
            {"oldText": "alpha\n", "newText": "ALPHA\n"},
            {"oldText": "gamma\n", "newText": "GAMMA\n"}]}),
        &ctx(dir.path()),
    )
    .await;
    let output = result.unwrap();
    assert_eq!(
        output_text(&output),
        "Successfully replaced 2 block(s) in edit.txt."
    );
    let details = output.details.unwrap();
    assert_eq!(
        details["patch"],
        "--- edit.txt\n+++ edit.txt\n@@ -1,4 +1,4 @@\n-alpha\n+ALPHA\n beta\n-gamma\n+GAMMA\n delta\n"
    );
    assert_eq!(details["firstChangedLine"], 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("edit.txt")).unwrap(),
        "ALPHA\nbeta\nGAMMA\ndelta\n"
    );
}

#[tokio::test]
async fn matches_all_edits_against_the_original_and_rejects_overlaps() {
    let dir = temp();
    std::fs::write(dir.path().join("edit.txt"), "one\ntwo\nthree\n").unwrap();
    let (result, _) = execute(
        &EditTool,
        json!({"path": "edit.txt", "edits": [
            {"oldText": "one\ntwo\n", "newText": "ONE\nTWO\n"},
            {"oldText": "two\nthree\n", "newText": "TWO\nTHREE\n"}]}),
        &ctx(dir.path()),
    )
    .await;
    assert!(result.unwrap_err().message.contains("overlap"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("edit.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
}

#[tokio::test]
async fn rejects_missing_and_duplicate_target_text() {
    let dir = temp();
    std::fs::write(dir.path().join("edit.txt"), "foo foo foo").unwrap();
    let c = ctx(dir.path());
    let (missing, _) = execute(
        &EditTool,
        json!({"path": "edit.txt", "edits": [{"oldText": "bar", "newText": "baz"}]}),
        &c,
    )
    .await;
    assert!(
        missing
            .unwrap_err()
            .message
            .starts_with("Could not find the exact text")
    );
    let (dup, _) = execute(
        &EditTool,
        json!({"path": "edit.txt", "edits": [{"oldText": "foo", "newText": "bar"}]}),
        &c,
    )
    .await;
    assert!(dup.unwrap_err().message.starts_with("Found 3 occurrences"));
}

#[tokio::test]
async fn reports_access_errors_with_the_reference_codes() {
    let dir = temp();
    let c = ctx(dir.path());
    let args = |path: &str| json!({"path": path, "edits": [{"oldText": "a", "newText": "b"}]});
    let (missing, _) = execute(&EditTool, args("nope.txt"), &c).await;
    assert_eq!(
        missing.unwrap_err().message,
        "Could not edit file: nope.txt. Error code: not_found."
    );
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let (directory, _) = execute(&EditTool, args("sub"), &c).await;
    assert_eq!(
        directory.unwrap_err().message,
        "Could not edit file: sub. Path is not a file."
    );
    std::fs::write(dir.path().join("file"), "a").unwrap();
    let (not_dir, _) = execute(&EditTool, args("file/x.txt"), &c).await;
    // The mutation key's realpath fails first, as in the reference.
    assert_eq!(
        not_dir.unwrap_err().message,
        format!(
            "ENOTDIR: not a directory, realpath '{}'",
            dir.path().join("file/x.txt").display()
        )
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("missing-target", dir.path().join("dangling")).unwrap();
        let (dangling, _) = execute(&EditTool, args("dangling"), &c).await;
        assert_eq!(
            dangling.unwrap_err().message,
            "Could not edit file: dangling. Error code: not_found."
        );
    }
    let (empty, _) = execute(&EditTool, json!({"path": "file", "edits": []}), &c).await;
    assert_eq!(
        empty.unwrap_err().message,
        "Edit tool input is invalid. edits must contain at least one replacement."
    );
}

#[cfg(unix)]
#[tokio::test]
async fn reports_permission_denied() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return; // root ignores permissions
    }
    let dir = temp();
    let path = dir.path().join("locked.txt");
    std::fs::write(&path, "a\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let (result, _) = execute(
        &EditTool,
        json!({"path": "locked.txt", "edits": [{"oldText": "a", "newText": "b"}]}),
        &ctx(dir.path()),
    )
    .await;
    assert_eq!(
        result.unwrap_err().message,
        "Could not edit file: locked.txt. Error code: permission_denied."
    );
}

#[cfg(unix)]
#[tokio::test]
async fn edits_regular_files_through_symlinks() {
    let dir = temp();
    std::fs::write(dir.path().join("target.txt"), "before\n").unwrap();
    std::os::unix::fs::symlink("target.txt", dir.path().join("link.txt")).unwrap();
    let (result, _) = execute(
        &EditTool,
        json!({"path": "link.txt", "edits": [{"oldText": "before", "newText": "after"}]}),
        &ctx(dir.path()),
    )
    .await;
    result.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("target.txt")).unwrap(),
        "after\n"
    );
    assert!(
        dir.path()
            .join("link.txt")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[tokio::test]
async fn preserves_bom_and_crlf_line_endings() {
    let dir = temp();
    std::fs::write(dir.path().join("edit.txt"), "\u{feff}one\r\ntwo\r\n").unwrap();
    let (result, _) = execute(
        &EditTool,
        json!({"path": "edit.txt", "edits": [{"oldText": "two", "newText": "TWO"}]}),
        &ctx(dir.path()),
    )
    .await;
    result.unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("edit.txt")).unwrap(),
        "\u{feff}one\r\nTWO\r\n".as_bytes()
    );
}

#[test]
fn fuzzy_matching_overlays_only_the_touched_lines() {
    let original = "keep\u{a0}me  \nsay \u{201c}hi\u{201d}   \nend\u{2014}line\n";
    let (_, new) = apply_edits(
        original,
        &[Edit {
            old_text: "say \"hi\"".into(),
            new_text: "say \"bye\"".into(),
        }],
        "f",
    )
    .unwrap();
    // Untouched lines keep their original bytes; the touched line comes from fuzzy space.
    assert_eq!(new, "keep\u{a0}me  \nsay \"bye\"\nend\u{2014}line\n");
    assert_eq!(
        normalize_for_fuzzy_match("ﬁ\u{2018}x\u{2019} \u{2212}1\u{3000}\t \n"),
        "fi'x' -1\n"
    );
}

#[test]
fn multibyte_offsets_stay_on_character_boundaries() {
    let (_, new) = apply_edits(
        "é😀é\n中文\u{2019}s\n",
        &[
            Edit {
                old_text: "😀".into(),
                new_text: "x".into(),
            },
            Edit {
                old_text: "文'".into(),
                new_text: "字\u{2019}".into(),
            },
        ],
        "f",
    )
    .unwrap();
    assert_eq!(new, "éxé\n中字\u{2019}s\n");
}

#[tokio::test]
async fn result_text_through_run_has_no_harness_block() {
    let dir = temp();
    std::fs::write(dir.path().join("notes.txt"), "hello world\n").unwrap();
    let result = run(
        &EditTool,
        json!({"path": "notes.txt", "edits": [{"oldText": "world", "newText": "durable"}]}),
        &ctx(dir.path()),
    )
    .await;
    assert!(!result.is_error);
    assert_eq!(
        super::result_text(&result),
        "Successfully replaced 1 block(s) in notes.txt."
    );
}

/// The mutation queue: a held lock on the canonical path blocks an edit through a symlink, and
/// a call cancelled while it waits fails with `Operation aborted` once it gets the lock.
#[cfg(unix)]
#[tokio::test]
async fn serializes_edits_through_canonical_and_symlink_paths() {
    let dir = temp();
    std::fs::write(dir.path().join("target.txt"), "alpha\nbeta\ngamma\n").unwrap();
    std::os::unix::fs::symlink("target.txt", dir.path().join("link.txt")).unwrap();
    let c = ctx(dir.path());
    let key = mutation_key(&dir.path().join("target.txt")).unwrap();
    assert_eq!(mutation_key(&dir.path().join("link.txt")).unwrap(), key);
    let lock = c.locks.lock_for(key);
    let guard = lock.lock().await;
    let edit = tokio::spawn({
        let c = c.clone();
        async move {
            execute(
                &EditTool,
                json!({"path": "link.txt", "edits": [{"oldText": "beta", "newText": "BETA"}]}),
                &c,
            )
            .await
            .0
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("target.txt")).unwrap(),
        "alpha\nbeta\ngamma\n"
    );
    drop(guard);
    edit.await.unwrap().unwrap();
    // Concurrent edits through both names both land.
    let a = execute(
        &EditTool,
        json!({"path": "target.txt", "edits": [{"oldText": "alpha", "newText": "ALPHA"}]}),
        &c,
    );
    let b = execute(
        &EditTool,
        json!({"path": "link.txt", "edits": [{"oldText": "gamma", "newText": "GAMMA"}]}),
        &c,
    );
    let (a, b) = tokio::join!(a, b);
    a.0.unwrap();
    b.0.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("target.txt")).unwrap(),
        "ALPHA\nBETA\nGAMMA\n"
    );
}

#[tokio::test]
async fn an_edit_cancelled_while_waiting_for_the_lock_is_aborted() {
    let dir = temp();
    std::fs::write(dir.path().join("f.txt"), "alpha\n").unwrap();
    let c = ctx(dir.path());
    let lock = c
        .locks
        .lock_for(mutation_key(&dir.path().join("f.txt")).unwrap());
    let guard = lock.lock().await;
    let edit = tokio::spawn({
        let c = c.clone();
        async move {
            execute(
                &EditTool,
                json!({"path": "f.txt", "edits": [{"oldText": "alpha", "newText": "ALPHA"}]}),
                &c,
            )
            .await
            .0
        }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    c.cancel.cancel();
    drop(guard);
    assert_eq!(
        edit.await.unwrap().unwrap_err().message,
        "Operation aborted"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
        "alpha\n"
    );
}

#[test]
fn spec_offers_edit_with_its_repair() {
    assert_eq!(EditTool.spec().name, "edit");
}

/// Compares against a randomized corpus produced by the reference implementation itself
/// (`PIPKIN_EDIT_CORPUS=<json>`; see the report); skipped unless the corpus is given.
#[tokio::test]
#[ignore = "needs a corpus generated by the reference (node)"]
async fn matches_a_reference_corpus() {
    let Ok(corpus) = std::env::var("PIPKIN_EDIT_CORPUS") else {
        return;
    };
    let cases: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(corpus).unwrap()).unwrap();
    let dir = temp();
    let c = ctx(dir.path());
    let mut checked = 0;
    for case in &cases {
        std::fs::write(dir.path().join("f.txt"), case["file"].as_str().unwrap()).unwrap();
        let (result, _) = execute(&EditTool, case["args"].clone(), &c).await;
        let actual = match result {
            Ok(out) => json!({"ok": {"text": output_text(&out), "details": out.details}}),
            Err(e) => json!({"err": e.message}),
        };
        assert_eq!(actual, case["result"], "case {}", case["id"]);
        let after = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
        assert_eq!(
            after,
            case["after"].as_str().unwrap(),
            "case {}",
            case["id"]
        );
        checked += 1;
    }
    eprintln!("checked {checked} reference cases");
}
