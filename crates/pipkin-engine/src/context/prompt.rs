//! The system prompt (reference `coding-agent/src/core/system-prompt.ts:72-180` as the
//! durable server installs it, `coding-agent/src/experimental/durable/prompt.ts`).
//!
//! Sections, in order: `preamble` (untagged), `tools`, `rules`, `docs`, `project_context`,
//! `skills`, `cwd`; every section but the preamble is wrapped `<name>\n...\n</name>`; the
//! prompt is the non-empty sections joined by a blank line. Like the durable server there is
//! no `SYSTEM.md` replacement, no `APPEND_SYSTEM.md` addendum, no extension prompt guidelines
//! and no date.
//!
//! Only `read`, `bash`, `edit` and `write` contribute a tool line and guidelines (the
//! reference's `CONTRIBUTIONS`, prompt.ts:12-17); other selected tools (for example
//! `subagent`) are not listed but still count as selected for the shell rule and the skills
//! read tool.
//!
//! Product-specific sections identify Pipkin and embed its engine reference. They require
//! no Pi package paths, filesystem reads or network access. Other prompt sections preserve
//! the durable server's contract.

use std::path::PathBuf;

use super::resources::format_skills;
pub use super::resources::{ContextFile, Skill};

/// The durable server's section order (prompt.ts:20).
pub const SECTION_KEYS: [&str; 7] = [
    "preamble",
    "tools",
    "rules",
    "docs",
    "project_context",
    "skills",
    "cwd",
];

pub const PREAMBLE: &str = "You are an expert coding assistant operating inside Pipkin, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.";

/// Product help is shipped as data inside every native engine binary.
pub const ENGINE_REFERENCE: &str = include_str!("../../../../docs/native-engine-reference.md");

/// A tool's prompt line and guideline bullets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolContribution {
    pub snippet: &'static str,
    pub guidelines: &'static [&'static str],
}

/// The reference's per-tool contributions (read.ts:20-23, bash.ts:45-48, edit.ts:43-51,
/// write.ts:16-19). `None` for every other tool.
pub fn tool_contribution(name: &str) -> Option<ToolContribution> {
    match name {
        "read" => Some(ToolContribution {
            snippet: "Read file contents",
            guidelines: &["Use read to examine files instead of cat or sed."],
        }),
        "bash" => Some(ToolContribution {
            snippet: "Execute bash commands (ls, grep, find, etc.)",
            guidelines: &[
                "You can inspect PI_* environment variables for current model and session details.",
            ],
        }),
        "edit" => Some(ToolContribution {
            snippet: "Make precise file edits with exact text replacement, including multiple disjoint edits in one call",
            guidelines: &[
                "Use edit for precise changes (edits[].oldText must match exactly)",
                "When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls",
                "Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.",
                "Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.",
            ],
        }),
        "write" => Some(ToolContribution {
            snippet: "Create or overwrite files",
            guidelines: &["Use write only for new files or complete rewrites."],
        }),
        _ => None,
    }
}

/// What the prompt is built from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PromptInput {
    /// The session's working directory (backslashes are rendered as `/`).
    pub cwd: PathBuf,
    /// Selected tool names, in the order the tools are offered.
    pub tools: Vec<String>,
    /// From [`super::resources::load_context_files`].
    pub context_files: Vec<ContextFile>,
    /// From [`super::resources::load_skills`].
    pub skills: Vec<Skill>,
}

pub struct SystemPrompt;

impl SystemPrompt {
    /// The present sections in [`SECTION_KEYS`] order, each already tagged (except the
    /// preamble). Project context without files and skills without a read tool or visible
    /// skill are left out; embedded product documentation is always available.
    pub fn sections(input: &PromptInput) -> Vec<(&'static str, String)> {
        let selected: Vec<&str> = input.tools.iter().map(String::as_str).collect();
        let has = |name: &str| selected.contains(&name);

        let mut out: Vec<(&'static str, String)> = vec![("preamble", PREAMBLE.to_string())];

        let lines: Vec<String> = selected
            .iter()
            .filter_map(|name| tool_contribution(name).map(|c| format!("- {name}: {}", c.snippet)))
            .collect();
        let tools = if lines.is_empty() {
            "(none)".to_string()
        } else {
            lines.join("\n")
        };
        out.push((
            "tools",
            format!(
                "{tools}\n\nIn addition to the tools above, you may have access to other custom tools depending on the project."
            ),
        ));

        out.push(("rules", rules(&selected, &has)));

        out.push(("docs", ENGINE_REFERENCE.trim().to_owned()));

        if !input.context_files.is_empty() {
            let mut parts = vec!["Project-specific instructions and guidelines:".to_string()];
            for file in &input.context_files {
                parts.push(format!(
                    "<project_instructions path=\"{}\">\n{}\n</project_instructions>",
                    file.path.display(),
                    file.content
                ));
            }
            out.push(("project_context", parts.join("\n\n")));
        }

        let read_tool = ["read", "bash"].into_iter().find(|tool| has(tool));
        if let Some(tool) = read_tool
            && !input.skills.is_empty()
        {
            let listing = format_skills(&input.skills, tool);
            let listing = super::js::trim(&listing);
            if !listing.is_empty() {
                out.push(("skills", listing.to_string()));
            }
        }

        out.push(("cwd", input.cwd.to_string_lossy().replace('\\', "/")));

        out.into_iter()
            .map(|(name, content)| {
                if name == "preamble" {
                    (name, content)
                } else {
                    (name, format!("<{name}>\n{content}\n</{name}>"))
                }
            })
            .collect()
    }

    /// The full prompt text: the sections' non-empty texts joined by `"\n\n"`
    /// (`getSystemMessageText`, ai/src/utils/text.ts:15-21).
    pub fn build(input: &PromptInput) -> String {
        Self::sections(input)
            .into_iter()
            .map(|(_, text)| text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// `buildRules` (system-prompt.ts:81-118) without extension guidelines.
fn rules(selected: &[&str], has: &dyn Fn(&str) -> bool) -> String {
    let mut rules: Vec<String> = Vec::new();
    let mut add = |rule: &str| {
        let rule = super::js::trim(rule);
        if !rule.is_empty() && !rules.iter().any(|seen| seen == rule) {
            rules.push(rule.to_string());
        }
    };
    let (bash, powershell) = (has("bash"), has("powershell"));
    if (bash || powershell) && !has("grep") && !has("find") && !has("ls") {
        if bash && powershell {
            add(
                "Use bash or PowerShell for file operations like listing, searching, and finding files",
            );
        } else if powershell {
            add("Use PowerShell for file operations like listing, searching, and finding files");
        } else {
            add("Use bash for file operations like ls, rg, find");
        }
    }
    // Guidelines are keyed by tool name, so a tool selected twice contributes twice (and is
    // then deduplicated), like the reference.
    for name in selected {
        if let Some(contribution) = tool_contribution(name) {
            for rule in contribution.guidelines {
                add(rule);
            }
        }
    }
    add("Be concise in your responses");
    add("Show file paths clearly when working with files");
    rules
        .iter()
        .map(|rule| format!("- {rule}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
