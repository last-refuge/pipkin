//! How a tool call reads in the transcript: a friendly verb, the thing it acts on, and an icon.
//! Pure, so the mapping is tested without a window.

use pipkin_core::{ItemKind, JobsState, NoticeLevel, ToolStatus, TranscriptItem};

/// What to show for a tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolLabel {
    pub icon: &'static str,
    pub verb: String,
    /// The path, pattern or command it works on; may be empty.
    pub target: String,
}

/// The string value of `"key": "…"` in a JSON object's text, with the common escapes undone.
/// Tolerant: a truncated or malformed input just yields `None`.
pub fn json_string_field(input: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut from = 0;
    while let Some(found) = input[from..].find(&needle) {
        let after = from + found + needle.len();
        let rest = input[after..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start();
            if let Some(body) = rest.strip_prefix('"') {
                let mut out = String::new();
                let mut chars = body.chars();
                while let Some(ch) = chars.next() {
                    match ch {
                        '"' => return Some(out),
                        '\\' => match chars.next() {
                            Some('n') => out.push('\n'),
                            Some('t') => out.push('\t'),
                            Some(other) => out.push(other),
                            None => return None,
                        },
                        _ => out.push(ch),
                    }
                }
                return None;
            }
        }
        from = after;
    }
    None
}

fn first_field(input: &str, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| json_string_field(input, k))
        .unwrap_or_default()
}

pub fn label(name: &str, input: &str) -> ToolLabel {
    let lower = name.to_ascii_lowercase();
    let (icon, verb, keys): (&'static str, &str, &[&str]) = match lower.as_str() {
        "read" | "read_file" | "cat" | "view" | "open" => {
            ("file-text", "Reading files", &["path", "file_path", "file"])
        }
        "grep" | "search" | "find" | "glob" | "rg" | "ls" | "list" | "list_files" => {
            ("search", "Searching", &["pattern", "query", "path", "glob"])
        }
        "edit" | "multiedit" | "apply_patch" | "patch" | "replace" => {
            ("pencil", "Editing files", &["path", "file_path", "file"])
        }
        "write" | "write_file" | "create" => (
            "file-plus",
            "Writing a file",
            &["path", "file_path", "file"],
        ),
        "bash" | "shell" | "sh" | "exec" | "run" | "terminal" => {
            ("terminal", "Running", &["command", "cmd", "script"])
        }
        _ => (
            "wrench",
            name,
            &["path", "command", "query", "pattern", "url"],
        ),
    };
    let mut target = first_field(input, keys);
    let known = icon != "wrench";
    if target.is_empty() && (!known || !input.trim_start().starts_with('{')) {
        // Not the JSON shape we know: show the input's first line, as was shown before.
        target = input.lines().next().unwrap_or("").trim().to_string();
    }
    ToolLabel {
        icon,
        verb: verb.to_string(),
        target,
    }
}

/// Whether a notice is the model's thinking (`Thinking\n<text>` or `Thinking...`).
pub fn is_thinking(text: &str) -> bool {
    text.starts_with("Thinking")
}

/// The first words of a thinking notice, without Markdown emphasis; empty when it has none.
pub fn thinking_first_line(text: &str) -> String {
    if matches!(text, "Thinking..." | "Thinking (not shown)") {
        return String::new();
    }
    text.strip_prefix("Thinking")
        .unwrap_or(text)
        .trim()
        .lines()
        .map(|l| l.replace("**", "").trim().to_string())
        .find(|l| !l.is_empty())
        .unwrap_or_default()
}

/// How long a stretch of work took, for display: nothing under five seconds, then "12s",
/// "1m 38s" (whole minutes alone as "2m"), "1h 5m".
pub fn format_elapsed(secs: i64) -> Option<String> {
    if secs < 5 {
        return None;
    }
    Some(if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        match secs % 60 {
            0 => format!("{}m", secs / 60),
            s => format!("{}m {s}s", secs / 60),
        }
    } else {
        match secs % 3600 / 60 {
            0 => format!("{}h", secs / 3600),
            m => format!("{}h {m}m", secs / 3600),
        }
    })
}

/// A short phrase for a run of steps: "Read 2 files, edited 1 file, ran 3 commands". While the run is
/// still going the verbs are progressive ("Reading 2 files, running 1 command").
pub fn run_summary<'a>(names: impl Iterator<Item = &'a str>, running: bool) -> String {
    // read, search, edit, write, run, other
    let mut counts = [0usize; 6];
    for name in names {
        let slot = match label(name, "").icon {
            "file-text" => 0,
            "search" => 1,
            "pencil" => 2,
            "file-plus" => 3,
            "terminal" => 4,
            _ => 5,
        };
        counts[slot] += 1;
    }
    const PAST: [&str; 6] = ["Read", "Searched", "Edited", "Wrote", "Ran", "Used"];
    const NOW: [&str; 6] = [
        "Reading",
        "Searching",
        "Editing",
        "Writing",
        "Running",
        "Using",
    ];
    const NOUN: [(&str, &str); 6] = [
        ("file", "files"),
        ("time", "times"),
        ("file", "files"),
        ("file", "files"),
        ("command", "commands"),
        ("tool", "tools"),
    ];
    let mut parts = Vec::new();
    for (i, n) in counts.iter().enumerate() {
        if *n == 0 {
            continue;
        }
        let verb = if running { NOW[i] } else { PAST[i] };
        let noun = if *n == 1 { NOUN[i].0 } else { NOUN[i].1 };
        let mut part = format!("{verb} {n} {noun}");
        if !parts.is_empty() {
            part = part[..1].to_lowercase() + &part[1..];
        }
        parts.push(part);
    }
    parts.join(", ")
}

/// What is happening right now, for the "Working…" strip: a background job still running (with
/// how long it has run), then what the turn has done so far, then how long the turn has been
/// waiting on the model since the last step landed. Something is always named: a person
/// watching a long turn should never see a bare spinner.
pub fn live_activity(items: &[TranscriptItem], jobs: &JobsState, now_secs: i64) -> String {
    let mut parts = Vec::new();
    if let Some(job) = jobs.jobs.iter().find(|j| j.state.active()) {
        let mut part = format!("Background job #{}", job.id);
        if let Some(name) = &job.name {
            part.push_str(&format!(" \u{201c}{}\u{201d}", clip_chars(name, 48)));
        }
        part.push_str(match job.state {
            pipkin_core::JobState::Starting => " starting",
            pipkin_core::JobState::Cancelling => " stopping",
            _ => " running",
        });
        if let Some(elapsed) = job
            .elapsed_ms(now_secs * 1000)
            .and_then(|ms| format_elapsed(ms / 1000))
        {
            part.push_str(&format!(" for {elapsed}"));
        }
        parts.push(part);
    }
    if let Some(recent) = recent_activity(items) {
        parts.push(recent);
    }
    let waiting = items
        .iter()
        .rev()
        .find(|i| !matches!(i.kind, ItemKind::Job(_)))
        .map(|i| now_secs.saturating_sub(i.at))
        .and_then(format_elapsed);
    let model_wait = match (&waiting, parts.is_empty()) {
        (Some(elapsed), _) => format!("waiting for the model for {elapsed}"),
        (None, true) => "waiting for the model".to_string(),
        (None, false) => String::new(),
    };
    if !model_wait.is_empty() {
        parts.push(if parts.is_empty() {
            model_wait[..1].to_uppercase() + &model_wait[1..]
        } else {
            model_wait
        });
    }
    parts.join(" · ")
}

fn clip_chars(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// What this run has actually done so far, based on reported tool calls, not the prompt text.
/// Only the latest prompt is counted. Bound the scan on unusually long/compacted transcripts;
/// when its start is out of view, say "Recent" rather than implying these are lifetime totals.
pub fn recent_activity(items: &[TranscriptItem]) -> Option<String> {
    const MAX_ITEMS: usize = 256;
    let mut completed = Vec::new();
    let mut running = Vec::new();
    let mut failed = 0;
    let mut thought = None;
    let mut found_prompt = false;
    for item in items.iter().rev().take(MAX_ITEMS) {
        match &item.kind {
            ItemKind::User { .. } => {
                found_prompt = true;
                break;
            }
            ItemKind::Tool(tool) => match tool.status {
                ToolStatus::Ok => completed.push(tool.name.as_str()),
                ToolStatus::Running => running.push(tool.name.as_str()),
                ToolStatus::Failed => failed += 1,
            },
            ItemKind::Notice {
                text,
                level: NoticeLevel::Info,
            } if is_thinking(text)
                && thought.is_none()
                && completed.is_empty()
                && running.is_empty()
                && failed == 0 =>
            {
                let line = thinking_first_line(text);
                if !line.is_empty() {
                    thought = Some(line);
                }
            }
            _ => {}
        }
    }
    if let Some(thought) = thought {
        return Some(thought);
    }
    let mut parts = Vec::new();
    let done = run_summary(completed.into_iter(), false);
    if !done.is_empty() {
        parts.push(done);
    }
    let active = run_summary(running.into_iter(), true);
    if !active.is_empty() {
        parts.push(if parts.is_empty() {
            active
        } else {
            format!("{}{}", active[..1].to_ascii_lowercase(), &active[1..])
        });
    }
    if failed > 0 {
        parts.push(format!(
            "{failed} tool{} failed",
            if failed == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        return None;
    }
    let summary = parts.join(", ");
    if !found_prompt && items.len() > MAX_ITEMS {
        Some(format!("Recent: {summary}"))
    } else {
        Some(summary)
    }
}

/// The run of consecutive tool rows that contains `ix`, as `(first, last)` inclusive, or `None`
/// when `ix` is not a tool. Scans only outward from `ix`, so it stays cheap in a long transcript.
pub fn tool_run(len: usize, ix: usize, is_tool: impl Fn(usize) -> bool) -> Option<(usize, usize)> {
    if ix >= len || !is_tool(ix) {
        return None;
    }
    let mut first = ix;
    while first > 0 && is_tool(first - 1) {
        first -= 1;
    }
    let mut last = ix;
    while last + 1 < len && is_tool(last + 1) {
        last += 1;
    }
    Some((first, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_field_is_read_with_its_escapes_undone() {
        let input = r#"{"command":"echo \"hi\"\nls","cwd":"/tmp"}"#;
        assert_eq!(
            json_string_field(input, "command").as_deref(),
            Some("echo \"hi\"\nls")
        );
        assert_eq!(json_string_field(input, "cwd").as_deref(), Some("/tmp"));
        assert_eq!(json_string_field(input, "missing"), None);
        assert_eq!(
            json_string_field(r#"{"command":"unterminated"#, "command"),
            None
        );
        assert_eq!(
            json_string_field(r#"{"count":3}"#, "count"),
            None,
            "not a string"
        );
        assert_eq!(
            json_string_field("{ \"path\" :  \"a b\" }", "path").as_deref(),
            Some("a b")
        );
    }

    #[test]
    fn known_tools_get_a_verb_an_icon_and_their_target() {
        let l = label("read_file", r#"{"path":"src/lib.rs"}"#);
        assert_eq!(
            (l.icon, l.verb.as_str(), l.target.as_str()),
            ("file-text", "Reading files", "src/lib.rs")
        );
        let l = label("bash", r#"{"command":"cargo check"}"#);
        assert_eq!(
            (l.icon, l.verb.as_str(), l.target.as_str()),
            ("terminal", "Running", "cargo check")
        );
        let l = label("Edit", r#"{"path":"a.rs","old":"x"}"#);
        assert_eq!(
            (l.icon, l.verb.as_str(), l.target.as_str()),
            ("pencil", "Editing files", "a.rs")
        );
        let l = label("grep", r#"{"pattern":"error handling"}"#);
        assert_eq!(
            (l.icon, l.verb.as_str(), l.target.as_str()),
            ("search", "Searching", "error handling")
        );
    }

    #[test]
    fn a_plain_text_input_is_shown_as_it_is() {
        let l = label("bash", "cargo clippy -p pipkin-core -- -D warnings\nmore");
        assert_eq!(l.target, "cargo clippy -p pipkin-core -- -D warnings");
        let l = label("read_file", r#"{"unrelated":1}"#);
        assert_eq!(
            l.target, "",
            "a known tool with an unfamiliar JSON shape shows no target"
        );
    }

    #[test]
    fn an_unknown_tool_keeps_its_own_name_and_shows_its_input() {
        let l = label("deploy_preview", "{\"env\":\"staging\"}\nmore");
        assert_eq!(l.verb, "deploy_preview");
        assert_eq!(l.icon, "wrench");
        assert_eq!(l.target, "{\"env\":\"staging\"}");
    }

    #[test]
    fn a_run_is_the_consecutive_tools_around_a_row() {
        // A user message, a thinking note, three tools, a reply, two tools.
        let tools = [false, false, true, true, true, false, true, true];
        let is = |i: usize| tools[i];
        assert_eq!(tool_run(tools.len(), 3, is), Some((2, 4)));
        assert_eq!(tool_run(tools.len(), 2, is), Some((2, 4)));
        assert_eq!(tool_run(tools.len(), 4, is), Some((2, 4)));
        assert_eq!(tool_run(tools.len(), 7, is), Some((6, 7)));
        assert_eq!(tool_run(tools.len(), 5, is), None, "a reply is not a tool");
        assert_eq!(tool_run(tools.len(), 99, is), None, "out of range");
        assert_eq!(tool_run(1, 0, |_| true), Some((0, 0)));
    }

    #[test]
    fn a_run_is_summed_up_in_a_phrase() {
        let names = ["read_file", "read", "edit", "bash", "bash", "bash"];
        assert_eq!(
            run_summary(names.iter().copied(), false),
            "Read 2 files, edited 1 file, ran 3 commands"
        );
        assert_eq!(
            run_summary(names.iter().copied(), true),
            "Reading 2 files, editing 1 file, running 3 commands"
        );
        assert_eq!(run_summary(["grep"].into_iter(), false), "Searched 1 time");
        assert_eq!(
            run_summary(["deploy", "other"].into_iter(), false),
            "Used 2 tools"
        );
        assert_eq!(run_summary(std::iter::empty(), false), "");
    }

    #[test]
    fn elapsed_time_reads_naturally_and_hides_the_trivial() {
        assert_eq!(format_elapsed(0), None);
        assert_eq!(format_elapsed(4), None);
        assert_eq!(format_elapsed(5).as_deref(), Some("5s"));
        assert_eq!(format_elapsed(59).as_deref(), Some("59s"));
        assert_eq!(format_elapsed(98).as_deref(), Some("1m 38s"));
        assert_eq!(format_elapsed(120).as_deref(), Some("2m"));
        assert_eq!(format_elapsed(3900).as_deref(), Some("1h 5m"));
        assert_eq!(format_elapsed(7200).as_deref(), Some("2h"));
    }

    #[test]
    fn thinking_is_recognised_and_its_first_words_found() {
        assert!(is_thinking("Thinking\n**Checking git status**\nmore"));
        assert!(is_thinking("Thinking..."));
        assert!(!is_thinking("A notice"));
        assert_eq!(
            thinking_first_line("Thinking\n\n**Checking git status**\nmore"),
            "Checking git status"
        );
        assert_eq!(thinking_first_line("Thinking..."), "");
        assert_eq!(thinking_first_line("Thinking (not shown)"), "");
        assert_eq!(thinking_first_line("Thinking"), "");
    }
}

#[cfg(test)]
mod live_activity_tests {
    use super::*;
    use pipkin_core::{Delivery, ItemId, Job, JobOrigin, JobState};

    fn running_job(started_at_ms: i64) -> Job {
        Job {
            id: 7,
            name: Some("Full workspace tests".into()),
            command: "cargo test --workspace".into(),
            cwd: "/p".into(),
            origin: JobOrigin::Model,
            state: JobState::Running,
            exit_code: None,
            reason: None,
            call_id: None,
            task: None,
            wake: true,
            delivery: "pending".into(),
            log_bytes: 0,
            log_truncated: false,
            created_at_ms: started_at_ms,
            started_at_ms: Some(started_at_ms),
            ended_at_ms: None,
        }
    }

    #[test]
    fn the_strip_names_the_running_job_the_steps_and_the_wait() {
        let now = 1_000;
        let mut jobs = JobsState::default();
        jobs.jobs.push(running_job(900_000));
        jobs.active = 1;
        let items = vec![TranscriptItem {
            native_only: false,
            id: ItemId(1),
            at: 940,
            kind: ItemKind::User {
                text: "go".into(),
                attachments: vec![],
                delivery: Delivery::Sent,
                steer: false,
            },
        }];
        let line = live_activity(&items, &jobs, now);
        assert_eq!(
            line,
            "Background job #7 \u{201c}Full workspace tests\u{201d} running for 1m 40s · waiting for the model for 1m"
        );
        assert_eq!(
            live_activity(&[], &JobsState::default(), now),
            "Waiting for the model"
        );
    }
}
