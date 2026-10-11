//! Skills (reference `coding-agent/src/core/skills.ts`) and `/skill:name` expansion
//! (`coding-agent/src/core/agent-session.ts:2100-2124`).
//!
//! Discovery is the durable server's (`experimental/durable/prompt.ts:34`, `loadSkills` with
//! `includeDefaults: true`): `<config_dir>/skills`, then `<cwd>/.pi/skills`, then the
//! settings' skill paths resolved against `cwd`. `.agents/skills` and packages are not
//! scanned, like the durable server.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::context::{frontmatter, js, paths};

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;
const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

/// Where a skill was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillSource {
    /// `<config_dir>/skills`.
    User,
    /// `<cwd>/.pi/skills`.
    Project,
    /// An explicit settings path.
    Path,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The skill file as discovered (not canonicalized).
    pub file_path: PathBuf,
    /// The skill file's directory.
    pub base_dir: PathBuf,
    pub source: SkillSource,
    /// `disable-model-invocation: true`: hidden from the prompt listing, still invocable with
    /// `/skill:name`.
    pub disable_model_invocation: bool,
}

/// A loader diagnostic, rendered as `"<path>: <message>"`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Diagnostic {
    message: String,
    path: PathBuf,
}

impl Diagnostic {
    fn new(message: impl Into<String>, path: &Path) -> Self {
        Diagnostic {
            message: message.into(),
            path: path.to_path_buf(),
        }
    }

    fn render(&self) -> String {
        format!("{}: {}", self.path.display(), self.message)
    }
}

#[derive(Default)]
struct Loaded {
    skills: Vec<Skill>,
    diagnostics: Vec<Diagnostic>,
}

/// The accumulated ignore rules of one scan (`ignore()` shared through the recursion; rules
/// from a directory's ignore files are prefixed with that directory's path relative to the
/// scan root). Case-insensitive like the `ignore` npm package's default.
struct IgnoreRules {
    root: PathBuf,
    patterns: Vec<String>,
    matcher: Option<Gitignore>,
}

impl IgnoreRules {
    fn new(root: &Path) -> Self {
        IgnoreRules {
            root: root.to_path_buf(),
            patterns: Vec::new(),
            matcher: None,
        }
    }

    fn add_from(&mut self, dir: &Path) {
        let relative = dir.strip_prefix(&self.root).unwrap_or(Path::new(""));
        let relative = relative.to_string_lossy().replace('\\', "/");
        let prefix = if relative.is_empty() {
            String::new()
        } else {
            format!("{relative}/")
        };
        let before = self.patterns.len();
        for name in IGNORE_FILE_NAMES {
            let Ok(bytes) = std::fs::read(dir.join(name)) else {
                continue;
            };
            let content = js::decode_utf8(bytes);
            for line in content.split('\n') {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if let Some(pattern) = prefix_pattern(line, &prefix) {
                    self.patterns.push(pattern);
                }
            }
        }
        if self.patterns.len() != before {
            let mut builder = GitignoreBuilder::new(&self.root);
            builder.case_insensitive(true).ok();
            for pattern in &self.patterns {
                // Patterns the glob syntax rejects are skipped, as the npm package does.
                let _ = builder.add_line(None, pattern);
            }
            self.matcher = builder.build().ok();
        }
    }

    fn ignores(&self, path: &Path, is_dir: bool) -> bool {
        self.matcher
            .as_ref()
            .is_some_and(|matcher| matcher.matched(path, is_dir).is_ignore())
    }
}

/// `prefixIgnorePattern` (skills.ts:24-45).
fn prefix_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = js::trim(line);
    if trimmed.is_empty() || (trimmed.starts_with('#') && !trimmed.starts_with("\\#")) {
        return None;
    }
    let mut pattern = line;
    let mut negated = false;
    if let Some(rest) = pattern.strip_prefix('!') {
        negated = true;
        pattern = rest;
    } else if pattern.starts_with("\\!") {
        pattern = &pattern[1..];
    }
    let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
    let prefixed = format!("{prefix}{pattern}");
    Some(if negated {
        format!("!{prefixed}")
    } else {
        prefixed
    })
}

fn validate_name(name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let length = js::len(name);
    if length > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({length})"
        ));
    }
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        errors.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
                .to_string(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }
    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }
    errors
}

fn validate_description(description: Option<&str>) -> Vec<String> {
    match description {
        Some(text) if !js::trim(text).is_empty() => {
            let length = js::len(text);
            if length > MAX_DESCRIPTION_LENGTH {
                vec![format!(
                    "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({length})"
                )]
            } else {
                vec![]
            }
        }
        _ => vec!["description is required".to_string()],
    }
}

/// `loadSkillFromFile` (skills.ts:277-344).
fn load_skill_file(file_path: &Path, source: SkillSource) -> (Option<Skill>, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let is_declared = paths::file_name(file_path) == "SKILL.md";
    let raw = match std::fs::read(file_path) {
        Ok(bytes) => js::decode_utf8(bytes),
        Err(err) => {
            diagnostics.push(Diagnostic::new(err.to_string(), file_path));
            return (None, diagnostics);
        }
    };
    let fm = match frontmatter::parse(&raw) {
        Ok((fm, _)) => fm,
        Err(err) => {
            if is_declared {
                diagnostics.push(Diagnostic::new(err, file_path));
            }
            return (None, diagnostics);
        }
    };
    let description = fm.string("description");
    let has_description = description.is_some_and(|text| !js::trim(text).is_empty());
    if !is_declared && !has_description {
        return (None, diagnostics);
    }
    let skill_dir = file_path.parent().unwrap_or(Path::new("/")).to_path_buf();
    for error in validate_description(description) {
        diagnostics.push(Diagnostic::new(error, file_path));
    }
    let name = fm
        .string("name")
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| paths::file_name(&skill_dir));
    for error in validate_name(&name) {
        diagnostics.push(Diagnostic::new(error, file_path));
    }
    let Some(description) = description.filter(|_| has_description) else {
        return (None, diagnostics);
    };
    let skill = Skill {
        name,
        description: description.to_string(),
        file_path: file_path.to_path_buf(),
        base_dir: skill_dir,
        source,
        disable_model_invocation: fm.boolean("disable-model-invocation") == Some(true),
    };
    (Some(skill), diagnostics)
}

/// `loadSkillsFromDirInternal` (skills.ts:173-275): a directory with `SKILL.md` is one skill
/// and is not descended; otherwise subdirectories are scanned (dot entries and `node_modules`
/// skipped, symlinks followed) and loose `.md` files count only at the scan root.
fn load_dir(dir: &Path, source: SkillSource, include_root_files: bool) -> Loaded {
    let mut loaded = Loaded::default();
    if !dir.exists() {
        return loaded;
    }
    let mut rules = IgnoreRules::new(dir);
    let mut stack = Vec::new();
    load_dir_inner(
        dir,
        source,
        include_root_files,
        &mut rules,
        &mut stack,
        &mut loaded,
    );
    loaded
}

fn load_dir_inner(
    dir: &Path,
    source: SkillSource,
    include_root_files: bool,
    rules: &mut IgnoreRules,
    stack: &mut Vec<PathBuf>,
    loaded: &mut Loaded,
) {
    // Divergence (safe): a symlink cycle back to a directory being scanned is not descended
    // again. The reference recurses until the OS reports ELOOP; the skills found are the same
    // after its realpath deduplication.
    let canonical = paths::canonical(dir);
    if stack.contains(&canonical) {
        return;
    }
    rules.add_from(dir);
    let Some(entries) = paths::sorted_entries(dir) else {
        return;
    };

    for entry in &entries {
        if entry.file_name() != "SKILL.md" {
            continue;
        }
        let full = entry.path();
        let Some((_, is_file)) = paths::entry_kind(entry) else {
            continue;
        };
        if !is_file || rules.ignores(&full, false) {
            continue;
        }
        let (skill, diagnostics) = load_skill_file(&full, source);
        loaded.skills.extend(skill);
        loaded.diagnostics.extend(diagnostics);
        return;
    }

    stack.push(canonical);
    for entry in &entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let full = entry.path();
        let Some((is_dir, is_file)) = paths::entry_kind(entry) else {
            continue;
        };
        if rules.ignores(&full, is_dir) {
            continue;
        }
        if is_dir {
            load_dir_inner(&full, source, false, rules, stack, loaded);
            continue;
        }
        if !is_file || !include_root_files || !name.ends_with(".md") {
            continue;
        }
        let (skill, diagnostics) = load_skill_file(&full, source);
        loaded.skills.extend(skill);
        loaded.diagnostics.extend(diagnostics);
    }
    stack.pop();
}

/// Accumulates skills with the reference's dedupe: the same real file is skipped silently;
/// a repeated name is a collision and the first one wins.
#[derive(Default)]
struct SkillSet {
    skills: Vec<Skill>,
    by_name: HashMap<String, usize>,
    real_paths: HashSet<PathBuf>,
    diagnostics: Vec<Diagnostic>,
    collisions: Vec<String>,
}

impl SkillSet {
    fn add(&mut self, loaded: Loaded) {
        self.diagnostics.extend(loaded.diagnostics);
        for skill in loaded.skills {
            let real = paths::canonical(&skill.file_path);
            if self.real_paths.contains(&real) {
                continue;
            }
            if let Some(&index) = self.by_name.get(&skill.name) {
                self.collisions.push(format!(
                    "{}: name \"{}\" collision (kept {})",
                    skill.file_path.display(),
                    skill.name,
                    self.skills[index].file_path.display()
                ));
            } else {
                self.by_name.insert(skill.name.clone(), self.skills.len());
                self.real_paths.insert(real);
                self.skills.push(skill);
            }
        }
    }
}

/// Skills for a session's prompt and `/skill:` commands, with warnings (loader diagnostics
/// first, then name collisions), each `"<path>: <message>"`.
pub fn load_skills(
    cwd: &Path,
    config_dir: &Path,
    extra_paths: &[String],
) -> (Vec<Skill>, Vec<String>) {
    let cwd = paths::absolute(cwd);
    let config_dir = paths::absolute(config_dir);
    let mut set = SkillSet::default();
    set.add(load_dir(
        &config_dir.join("skills"),
        SkillSource::User,
        true,
    ));
    set.add(load_dir(
        &cwd.join(".pi").join("skills"),
        SkillSource::Project,
        true,
    ));

    for raw in extra_paths {
        let resolved = paths::resolve(raw, &cwd, true);
        // `existsSync` is false for any stat failure.
        let Ok(meta) = std::fs::metadata(&resolved) else {
            set.diagnostics
                .push(Diagnostic::new("skill path does not exist", &resolved));
            continue;
        };
        if meta.is_dir() {
            set.add(load_dir(&resolved, SkillSource::Path, true));
        } else if meta.is_file() && resolved.to_string_lossy().ends_with(".md") {
            let (skill, diagnostics) = load_skill_file(&resolved, SkillSource::Path);
            match skill {
                Some(skill) => set.add(Loaded {
                    skills: vec![skill],
                    diagnostics,
                }),
                None => set.diagnostics.extend(diagnostics),
            }
        } else {
            set.diagnostics.push(Diagnostic::new(
                "skill path is not a markdown file",
                &resolved,
            ));
        }
    }

    let mut warnings: Vec<String> = set.diagnostics.iter().map(Diagnostic::render).collect();
    warnings.extend(set.collisions);
    (set.skills, warnings)
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// `formatSkillsForPrompt(skills, tool)` (skills.ts:355-383), exact text including its leading
/// blank line (the prompt builder trims it). `tool` is `"read"`; anything else gets the bash
/// wording. Empty when no skill is model-visible.
pub fn format_skills(skills: &[Skill], tool: &str) -> String {
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "\n\nThe following skills provide specialized instructions for specific tasks.".to_string(),
        if tool == "read" {
            "Use the read tool to load a skill's file when the task matches its description."
        } else {
            "Use bash to load a skill's file when the task matches its description."
        }
        .to_string(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];
    for skill in visible {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path.to_string_lossy())
        ));
        lines.push("  </skill>".to_string());
    }
    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

/// The user message that replaces `/skill:name args`: the skill body (frontmatter stripped,
/// trimmed) in a `<skill>` block, followed by the arguments when there are any.
pub fn skill_block(skill: &Skill, body: &str, args: &str) -> String {
    let block = format!(
        "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
        skill.name,
        skill.file_path.display(),
        skill.base_dir.display(),
        body
    );
    if args.is_empty() {
        block
    } else {
        format!("{block}\n\n{args}")
    }
}

/// `_expandSkillCommand`: `Ok(Some(text))` for `/skill:<name> [args]` naming a known skill,
/// `Ok(None)` when the text is not a skill command or the skill is unknown (pass the input
/// through), `Err(reason)` when the skill file cannot be read or parsed (the reference
/// reports it and passes the input through). The name ends at the first ASCII space.
pub fn expand_skill_command(text: &str, skills: &[Skill]) -> Result<Option<String>, String> {
    let Some(rest) = text.strip_prefix("/skill:") else {
        return Ok(None);
    };
    let (name, args) = match rest.find(' ') {
        Some(at) => (&rest[..at], js::trim(&rest[at + 1..])),
        None => (rest, ""),
    };
    let Some(skill) = skills.iter().find(|skill| skill.name == name) else {
        return Ok(None);
    };
    let raw = std::fs::read(&skill.file_path)
        .map(js::decode_utf8)
        .map_err(|err| err.to_string())?;
    let body = frontmatter::strip(&raw)?;
    Ok(Some(skill_block(skill, js::trim(&body), args)))
}
