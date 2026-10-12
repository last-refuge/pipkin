//! The real `pipkin --version / --diagnose --probe --engine native` paths: the built binary
//! identifies its built-in engine and starts a throwaway worker of itself with a bare
//! environment (no Node, no Pi, no credentials, no network), without touching a real profile.

use std::process::Command;

fn bare(dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pipkin"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir.join("home"))
        .env("TMPDIR", dir)
        .current_dir(dir);
    command
}

#[test]
fn default_and_explicit_native_version_lines_are_identical() {
    let dir = tempfile::tempdir().unwrap();
    let default = bare(dir.path()).arg("--version").output().unwrap();
    let default = String::from_utf8(default.stdout).unwrap();
    assert!(default.starts_with("pipkin "), "{default}");
    assert!(
        default.trim_end().ends_with(')') && default.contains("engine native"),
        "the default identifies the built-in engine: {default}"
    );
    let native = bare(dir.path())
        .args(["--version", "--engine", "native"])
        .output()
        .unwrap();
    assert!(native.status.success());
    let native = String::from_utf8(native.stdout).unwrap();
    assert_eq!(native, default);
    assert!(
        native.contains(&format!(
            "engine protocol {}",
            pipkin_engine_protocol::PROTOCOL_VERSION
        )),
        "{native}"
    );
}

#[test]
fn diagnose_probe_with_the_native_engine_passes_with_a_bare_environment() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let output = bare(dir.path())
        .args(["--diagnose", "--probe", "--data-dir"])
        .arg(&data)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("engine: native (built into this binary)"),
        "{text}"
    );
    assert!(text.contains("engine probe: OK (hello: protocol"), "{text}");
    assert!(text.contains("sessions in a fresh profile: 0"), "{text}");
    assert!(text.contains("explicit data directory"), "{text}");
    assert!(!text.contains("node: "), "{text}");
    // The probe ran in its own scratch profile, which it removed; nothing was created in the
    // person's (here: the test's) engine directories.
    assert!(!data.join("engine").exists(), "{text}");
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("pipkin-native-probe")
        })
        .collect();
    assert!(leftovers.is_empty(), "the scratch profile is removed");
}
