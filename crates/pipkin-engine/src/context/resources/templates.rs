//! Prompt templates (reference `coding-agent/src/core/prompt-templates.ts`, deduplication from
//! `resource-loader.ts:1148-1172`).
//!
//! Discovery is the reference's default directories in its `includeDefaults` order:
//! `<config_dir>/prompts`, then `<cwd>/.pi/prompts`, each scanned non-recursively for `*.md`
//! (symlinks followed). The first template of a name wins.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::{Captures, Regex};

use crate::context::{frontmatter, js, paths};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptTemplate {
    /// File name without `.md`; invoked as `/name`.
    pub name: String,
    /// Frontmatter `description`, else the first non-blank body line (60 UTF-16 units, then
    /// `...` when longer).
    pub description: String,
    /// Frontmatter `argument-hint` when a non-empty string.
    pub argument_hint: Option<String>,
    /// The body after the frontmatter.
    pub content: String,
    pub file_path: PathBuf,
}

fn load_template_file(path: &Path) -> Result<PromptTemplate, String> {
    let raw = std::fs::read(path)
        .map(js::decode_utf8)
        .map_err(|err| err.to_string())?;
    let (fm, body) = frontmatter::parse(&raw)?;
    let file_name = paths::file_name(path);
    let name = file_name
        .strip_suffix(".md")
        .unwrap_or(&file_name)
        .to_string();
    let mut description = fm.string("description").unwrap_or_default().to_string();
    if description.is_empty()
        && let Some(first) = body.split('\n').find(|line| !js::trim(line).is_empty())
    {
        description = js::prefix(first, 60).to_string();
        if js::len(first) > 60 {
            description.push_str("...");
        }
    }
    let argument_hint = fm
        .string("argument-hint")
        .filter(|hint| !hint.is_empty())
        .map(str::to_string);
    Ok(PromptTemplate {
        name,
        description,
        argument_hint,
        content: body,
        file_path: path.to_path_buf(),
    })
}

fn load_dir(dir: &Path, templates: &mut Vec<PromptTemplate>, warnings: &mut Vec<String>) {
    let Some(entries) = paths::sorted_entries(dir) else {
        return;
    };
    for entry in entries {
        let Some((_, is_file)) = paths::entry_kind(&entry) else {
            continue;
        };
        if !is_file || !entry.file_name().to_string_lossy().ends_with(".md") {
            continue;
        }
        let path = entry.path();
        match load_template_file(&path) {
            Ok(template) => templates.push(template),
            Err(err) => warnings.push(format!("{}: {err}", path.display())),
        }
    }
}

/// Templates from the global then the project prompt directory, first name wins, with
/// warnings (`"<path>: <message>"`; unreadable files and YAML errors first, then name
/// collisions).
pub fn load_templates(cwd: &Path, config_dir: &Path) -> (Vec<PromptTemplate>, Vec<String>) {
    let cwd = paths::absolute(cwd);
    let config_dir = paths::absolute(config_dir);
    let mut all = Vec::new();
    let mut warnings = Vec::new();
    load_dir(&config_dir.join("prompts"), &mut all, &mut warnings);
    load_dir(&cwd.join(".pi").join("prompts"), &mut all, &mut warnings);

    let mut seen: HashSet<String> = HashSet::new();
    let mut templates: Vec<PromptTemplate> = Vec::new();
    for template in all {
        if seen.contains(&template.name) {
            let winner = templates
                .iter()
                .find(|kept| kept.name == template.name)
                .map(|kept| kept.file_path.display().to_string())
                .unwrap_or_default();
            warnings.push(format!(
                "{}: name \"/{}\" collision (kept {winner})",
                template.file_path.display(),
                template.name
            ));
        } else {
            seen.insert(template.name.clone());
            templates.push(template);
        }
    }
    (templates, warnings)
}

/// `parseCommandArgs`: whitespace-separated, `'`/`"` quoting, no escapes; empty quoted
/// arguments are dropped.
pub fn parse_command_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for ch in text.chars() {
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if js::is_space(ch) {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

fn placeholder() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // `\d` in the reference (no `u` flag) is ASCII-only.
    RE.get_or_init(|| {
        Regex::new(
            r"\$\{([0-9]+|ARGUMENTS|@):-([^}]*)\}|\$\{@:([0-9]+)(?::([0-9]+))?\}|\$(ARGUMENTS|@|[0-9]+)",
        )
        .expect("placeholder regex")
    })
}

/// 1-based positional argument (`$0` and out-of-range numbers are missing).
fn positional<'a>(args: &'a [String], digits: &str) -> Option<&'a str> {
    let index = digits.parse::<usize>().ok()?.checked_sub(1)?;
    args.get(index).map(String::as_str)
}

/// `substituteArgs`: `$1`.., `$@`/`$ARGUMENTS`, `${N:-default}`, `${@:-default}`,
/// `${ARGUMENTS:-default}`, `${@:N}`, `${@:N:L}`. Not recursive: substituted values are never
/// rescanned.
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let all = args.join(" ");
    placeholder()
        .replace_all(content, |caps: &Captures| {
            if let Some(target) = caps.get(1) {
                let value = match target.as_str() {
                    "@" | "ARGUMENTS" => Some(all.as_str()),
                    digits => positional(args, digits),
                };
                return match value {
                    Some(value) if !value.is_empty() => value.to_string(),
                    _ => caps[2].to_string(),
                };
            }
            if let Some(start) = caps.get(3) {
                // 1-based; 0 is treated as 1. An unparsable (huge) start is past the end.
                let start = start
                    .as_str()
                    .parse::<usize>()
                    .map_or(usize::MAX, |n| n.saturating_sub(1));
                let start = start.min(args.len());
                let end = match caps.get(4) {
                    Some(length) => {
                        let length = length.as_str().parse::<usize>().unwrap_or(usize::MAX);
                        start.saturating_add(length).min(args.len())
                    }
                    None => args.len(),
                };
                return args[start..end].join(" ");
            }
            match &caps[5] {
                "ARGUMENTS" | "@" => all.clone(),
                digits => positional(args, digits).unwrap_or_default().to_string(),
            }
        })
        .into_owned()
}

/// `expandPromptTemplate`: `Some(expanded)` when `text` is `/<name>[<whitespace> args]` for a
/// known template; `None` means the text is not a template invocation and passes through.
pub fn expand_template(text: &str, templates: &[PromptTemplate]) -> Option<String> {
    let rest = text.strip_prefix('/')?;
    let name_end = rest.find(js::is_space).unwrap_or(rest.len());
    let name = &rest[..name_end];
    if name.is_empty() {
        return None;
    }
    let args = rest[name_end..].trim_start_matches(js::is_space);
    let template = templates.iter().find(|template| template.name == name)?;
    Some(substitute_args(
        &template.content,
        &parse_command_args(args),
    ))
}
