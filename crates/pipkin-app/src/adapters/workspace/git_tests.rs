use super::*;
#[test]
fn private_index_preserves_racy_clean_timestamp_and_detects_same_size_edits() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let mut cmd = command(dir.path());
        cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ]);
        run(cmd, args, 4096).unwrap()
    };
    git(&["init", "--quiet", "--template="]);
    for name in ["a b.txt", "café.txt"] {
        fs::write(dir.path().join(name), "x\n").unwrap();
    }
    git(&["add", "--all"]);
    git(&["commit", "--quiet", "-m", "fixture"]);
    let index = dir.path().join(".git/index");
    // A nonzero old timestamp conservatively makes every entry racy. The assertion on
    // the private copy is deterministic; do not depend on the fixture hitting one clock tick.
    let modified = std::time::UNIX_EPOCH + Duration::from_secs(123);
    fs::File::open(&index)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let source = fs::read(&index).unwrap();
    for name in ["a b.txt", "café.txt"] {
        fs::write(dir.path().join(name), "y\n").unwrap();
    }
    let reader = Reader::open(dir.path()).unwrap();
    assert_eq!(
        fs::metadata(reader.private.path().join("index"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    let (names, cut) = reader
        .run(&["diff", "--name-only", "-z", "HEAD", "--"], 4096)
        .unwrap();
    assert!(!cut);
    for name in ["a b.txt", "café.txt"] {
        assert!(names.split(|b| *b == 0).any(|p| p == name.as_bytes()));
    }
    assert_eq!(fs::read(&index).unwrap(), source);
    assert_eq!(fs::metadata(index).unwrap().modified().unwrap(), modified);
}
