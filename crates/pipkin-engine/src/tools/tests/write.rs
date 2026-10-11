//! `write`: ports of `tools.test.ts` "write".

use std::time::Duration;

use serde_json::json;

use super::{ctx, execute, output_text, temp};
use crate::tools::path::mutation_key;
use crate::tools::write::WriteTool;

#[tokio::test]
async fn writes_files_and_creates_parent_directories() {
    let dir = temp();
    let (result, _) = execute(
        &WriteTool,
        json!({"path": "nested/dir/file.txt", "content": "hello"}),
        &ctx(dir.path()),
    )
    .await;
    assert_eq!(
        output_text(&result.unwrap()),
        "Successfully wrote to nested/dir/file.txt"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/dir/file.txt")).unwrap(),
        "hello"
    );
}

#[tokio::test]
async fn writes_content_as_is_and_overwrites() {
    let dir = temp();
    let c = ctx(dir.path());
    std::fs::write(dir.path().join("f.txt"), "old content that is longer").unwrap();
    let (result, _) = execute(
        &WriteTool,
        json!({"path": "f.txt", "content": "\u{feff}a\r\nb"}),
        &c,
    )
    .await;
    result.unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("f.txt")).unwrap(),
        "\u{feff}a\r\nb".as_bytes()
    );
}

#[tokio::test]
async fn reports_node_errors() {
    let dir = temp();
    let c = ctx(dir.path());
    std::fs::write(dir.path().join("file"), "x").unwrap();
    let (result, _) = execute(
        &WriteTool,
        json!({"path": "file/sub/x.txt", "content": "y"}),
        &c,
    )
    .await;
    assert_eq!(
        result.unwrap_err().message,
        format!(
            "ENOTDIR: not a directory, realpath '{}'",
            dir.path().join("file/sub/x.txt").display()
        )
    );
    std::fs::create_dir(dir.path().join("d")).unwrap();
    let (result, _) = execute(&WriteTool, json!({"path": "d", "content": "y"}), &c).await;
    assert_eq!(
        result.unwrap_err().message,
        format!(
            "EISDIR: illegal operation on a directory, open '{}'",
            dir.path().join("d").display()
        )
    );
}

/// A write waits for the file's mutation lock; cancelled meanwhile it fails with
/// `Operation aborted` and leaves the file alone, and the next write proceeds.
#[tokio::test]
async fn keeps_the_mutation_queue_locked_and_aborts_a_cancelled_waiting_write() {
    let dir = temp();
    let c = ctx(dir.path());
    let lock = c
        .locks
        .lock_for(mutation_key(&dir.path().join("file.txt")).unwrap());
    let guard = lock.lock().await;
    let first = tokio::spawn({
        let c = c.clone();
        async move {
            execute(
                &WriteTool,
                json!({"path": "file.txt", "content": "first\n"}),
                &c,
            )
            .await
            .0
        }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!dir.path().join("file.txt").exists());
    c.cancel.cancel();
    drop(guard);
    assert_eq!(
        first.await.unwrap().unwrap_err().message,
        "Operation aborted"
    );
    let (second, _) = execute(
        &WriteTool,
        json!({"path": "file.txt", "content": "second\n"}),
        &ctx(dir.path()),
    )
    .await;
    second.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "second\n"
    );
}

#[cfg(unix)]
#[test]
fn keys_a_new_file_under_a_symlinked_directory_by_its_canonical_path() {
    let dir = temp();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
    let via_link = mutation_key(&dir.path().join("link/new.txt")).unwrap();
    let direct = mutation_key(&dir.path().join("real/new.txt")).unwrap();
    assert_eq!(via_link, direct);
    // A name containing a backslash keys like the created file.
    let odd = dir.path().join("a\\b.txt");
    let before = mutation_key(&odd).unwrap();
    std::fs::write(&odd, "x").unwrap();
    assert_eq!(mutation_key(&odd).unwrap(), before);
}
