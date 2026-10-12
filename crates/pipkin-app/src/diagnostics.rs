//! Shareable support metadata: an allowlist, never a scrubbed copy of logs or error text.
//! This is a configured launch-source snapshot, not an attestation of an external server.

use pipkin_core::Mode;

use crate::install;

pub fn prepare_demo() -> String {
    body(
        Mode::Demo,
        install::APP_REVISION,
        install::APP_DIRTY,
        install::native_identity().protocol,
        "engine: simulated (no real execution)",
    )
}

/// Support metadata for `--engine native`: the engine is built into this binary, so its identity
/// is this build's own (version, protocol, schema), never a launch-source manifest.
pub fn prepare_native() -> String {
    render_native(install::APP_REVISION, install::APP_DIRTY)
}

fn render_native(app_revision: &str, dirty: &str) -> String {
    let id = install::native_identity();
    body(
        Mode::Real,
        app_revision,
        dirty,
        id.protocol,
        &format!(
            "engine: native (built into this binary) {} (protocol {}, schema {}; source = app revision)",
            id.version, id.protocol, id.schema
        ),
    )
}

fn revision(value: &str) -> &str {
    if value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        value
    } else {
        "unidentified"
    }
}

fn body(mode: Mode, app_revision: &str, dirty: &str, protocol: u32, engine_line: &str) -> String {
    let dirty = match dirty {
        "true" => "true",
        "false" => "false",
        _ => "unknown",
    };
    format!(
        "Pipkin support metadata v1\n\
         app: {}\napp revision: {}\napp dirty: {dirty}\n\
         client protocol: {protocol}\ndatabase schema supported: {}\n\
         build platform: {}/{}\nmode: {}\n\
         {engine_line}\n\
         Scope: launch-source snapshot; not proof of running-server identity, provider authentication or database health.\n\
         Privacy: no logs, error text, paths, session IDs, prompts, attachments, provider configuration or environment values included.\n\
         Review before sharing. Detailed CLI diagnostics are separate and may contain sensitive data.\n",
        install::VERSION,
        revision(app_revision),
        crate::storage::SCHEMA_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        if mode == Mode::Demo {
            "demo (simulated)"
        } else {
            "real"
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_metadata_names_the_built_in_engine_without_a_manifest() {
        let sha = "d2a311097cbcf669e699479587332ae3988a49d0";
        let id = install::native_identity();
        let report = render_native(sha, "false");
        assert!(report.contains(&format!("client protocol: {}", id.protocol)));
        assert!(report.contains(&format!(
            "engine: native (built into this binary) {} (protocol {}, schema {}",
            id.version, id.protocol, id.schema
        )));
        assert!(report.contains("mode: real"));
        assert!(!report.contains("configured engine manifest"));
        let hostile = render_native("sk-secret /home/private", "maybe");
        assert!(!hostile.contains("sk-secret"));
        assert!(hostile.contains("app revision: unidentified"));
        assert!(hostile.contains("app dirty: unknown"));
        assert!(prepare_native().contains("Privacy: no logs"));
    }

    #[test]
    fn demo_metadata_is_explicitly_simulated() {
        let report = prepare_demo();
        assert!(report.contains("mode: demo (simulated)"));
        assert!(report.contains("engine: simulated (no real execution)"));
    }
}
