// Golden text for the durable server's prompt plus the applicable cases of
// coding-agent/test/system-prompt.test.ts (custom/forced prompts, the addendum and extension
// guidelines do not exist in the durable server and are not ported).

use std::path::PathBuf;

use super::*;
use crate::context::resources::SkillSource;

fn skill(name: &str, description: &str, hidden: bool) -> Skill {
    Skill {
        name: name.into(),
        description: description.into(),
        file_path: PathBuf::from(format!("/skills/{name}/SKILL.md")),
        base_dir: PathBuf::from(format!("/skills/{name}")),
        source: SkillSource::User,
        disable_model_invocation: hidden,
    }
}

fn input(tools: &[&str]) -> PromptInput {
    PromptInput {
        cwd: PathBuf::from("/repo"),
        tools: tools.iter().map(|t| t.to_string()).collect(),
        ..PromptInput::default()
    }
}

/// The durable server's tool order is read, write, edit, bash (durable/src/tools/index.ts).
const GOLDEN: &str = r#"You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.

<tools>
- read: Read file contents
- write: Create or overwrite files
- edit: Make precise file edits with exact text replacement, including multiple disjoint edits in one call
- bash: Execute bash commands (ls, grep, find, etc.)

In addition to the tools above, you may have access to other custom tools depending on the project.
</tools>

<rules>
- Use bash for file operations like ls, rg, find
- Use read to examine files instead of cat or sed.
- Use write only for new files or complete rewrites.
- Use edit for precise changes (edits[].oldText must match exactly)
- When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls
- Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.
- Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.
- You can inspect PI_* environment variables for current model and session details.
- Be concise in your responses
- Show file paths clearly when working with files
</rules>

<project_context>
Project-specific instructions and guidelines:

<project_instructions path="/home/me/.config/pipkin/AGENTS.md">
Global rules.
</project_instructions>

<project_instructions path="/repo/AGENTS.md">
# Repo

Use cargo.
</project_instructions>
</project_context>

<skills>
The following skills provide specialized instructions for specific tasks.
Use the read tool to load a skill's file when the task matches its description.
When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.

<available_skills>
  <skill>
    <name>deploy</name>
    <description>Ship it &amp; &lt;verify&gt;.</description>
    <location>/skills/deploy/SKILL.md</location>
  </skill>
</available_skills>
</skills>

<cwd>
/repo
</cwd>"#;

#[test]
fn golden_prompt_for_a_fixture_project() {
    let mut input = input(&["read", "write", "edit", "bash", "subagent"]);
    input.context_files = vec![
        ContextFile {
            path: PathBuf::from("/home/me/.config/pipkin/AGENTS.md"),
            content: "Global rules.".into(),
        },
        ContextFile {
            path: PathBuf::from("/repo/AGENTS.md"),
            content: "# Repo\n\nUse cargo.".into(),
        },
    ];
    input.skills = vec![
        skill("deploy", "Ship it & <verify>.", false),
        skill("manual", "Only by command.", true),
    ];
    assert_eq!(SystemPrompt::build(&input), GOLDEN);
    let keys: Vec<&str> = SystemPrompt::sections(&input)
        .iter()
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(
        keys,
        [
            "preamble",
            "tools",
            "rules",
            "project_context",
            "skills",
            "cwd"
        ]
    );
}

#[test]
fn minimal_prompt_has_preamble_tools_rules_and_cwd_only() {
    let prompt = SystemPrompt::build(&input(&[]));
    assert_eq!(
        prompt,
        format!(
            "{PREAMBLE}\n\n<tools>\n(none)\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n</tools>\n\n<rules>\n- Be concise in your responses\n- Show file paths clearly when working with files\n</rules>\n\n<cwd>\n/repo\n</cwd>"
        )
    );
}

#[test]
fn tools_without_a_contribution_are_not_listed() {
    let prompt = SystemPrompt::build(&input(&["read", "subagent", "dynamic_tool"]));
    assert!(prompt.contains("<tools>\n- read: Read file contents\n\nIn addition"));
    assert!(!prompt.contains("subagent"));
    assert!(!prompt.contains("dynamic_tool"));
}

#[test]
fn shell_rule_depends_on_the_selected_tools() {
    let rules = |tools: &[&str]| {
        SystemPrompt::sections(&input(tools))
            .into_iter()
            .find(|(k, _)| *k == "rules")
            .unwrap()
            .1
    };
    assert!(rules(&["powershell"]).contains("- Use PowerShell for file operations like listing"));
    assert!(
        rules(&["bash", "powershell"]).contains("- Use bash or PowerShell for file operations")
    );
    assert!(!rules(&["bash", "grep"]).contains("for file operations"));
    assert!(!rules(&["read"]).contains("for file operations"));
}

#[test]
fn skills_use_the_read_tool_then_bash_and_need_one() {
    let mut with_bash = input(&["bash"]);
    with_bash.skills = vec![skill("test-skill", "A test skill.", false)];
    let prompt = SystemPrompt::build(&with_bash);
    assert!(prompt.contains("<skills>\nThe following skills"));
    assert!(prompt.contains("Use bash to load a skill's file"));
    assert!(prompt.contains("<name>test-skill</name>"));

    let mut write_only = input(&["write"]);
    write_only.skills = with_bash.skills.clone();
    assert!(!SystemPrompt::build(&write_only).contains("<available_skills>"));

    let mut hidden_only = input(&["read"]);
    hidden_only.skills = vec![skill("x", "y", true)];
    assert!(!SystemPrompt::build(&hidden_only).contains("<skills>"));
}

#[test]
fn docs_section_only_with_a_bundle() {
    assert!(!SystemPrompt::build(&input(&["read"])).contains("<docs>"));
    let mut with_docs = input(&["read"]);
    with_docs.docs_dir = Some(PathBuf::from("/opt/pi"));
    let prompt = SystemPrompt::build(&with_docs);
    assert!(prompt.contains("<docs>\nPi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):\n- Main documentation: /opt/pi/README.md\n- Additional docs: /opt/pi/docs\n- Examples: /opt/pi/examples (extensions, custom tools, SDK)\n- When reading pi docs or examples, resolve docs/... under Additional docs and examples/... under Examples, not the current working directory\n"));
    assert!(prompt.contains(
        "environment variables (docs/environment-variables.md), MCP servers (docs/mcp.md)"
    ));
    // docs sits between rules and cwd.
    let keys: Vec<&str> = SystemPrompt::sections(&with_docs)
        .iter()
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(keys, ["preamble", "tools", "rules", "docs", "cwd"]);
}

#[test]
fn cwd_backslashes_render_as_slashes() {
    let mut windowsish = input(&[]);
    windowsish.cwd = PathBuf::from(r"C:\work\repo");
    assert!(SystemPrompt::build(&windowsish).ends_with("<cwd>\nC:/work/repo\n</cwd>"));
}

#[test]
fn section_keys_match_the_durable_server() {
    assert_eq!(
        SECTION_KEYS,
        [
            "preamble",
            "tools",
            "rules",
            "docs",
            "project_context",
            "skills",
            "cwd"
        ]
    );
    assert!(tool_contribution("subagent").is_none());
    assert_eq!(tool_contribution("edit").unwrap().guidelines.len(), 4);
}
