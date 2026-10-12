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
const GOLDEN: &str = r#"You are an expert coding assistant operating inside Pipkin, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.

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
    let expected = GOLDEN.replace(
        "</rules>\n\n<project_context>",
        &format!(
            "</rules>\n\n<docs>\n{}\n</docs>\n\n<project_context>",
            ENGINE_REFERENCE.trim()
        ),
    );
    assert_eq!(SystemPrompt::build(&input), expected);
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
            "docs",
            "project_context",
            "skills",
            "cwd"
        ]
    );
}

#[test]
fn minimal_prompt_includes_embedded_help_without_external_resources() {
    let prompt = SystemPrompt::build(&input(&[]));
    assert_eq!(
        prompt,
        format!(
            "{PREAMBLE}\n\n<tools>\n(none)\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n</tools>\n\n<rules>\n- Be concise in your responses\n- Show file paths clearly when working with files\n</rules>\n\n<docs>\n{}\n</docs>\n\n<cwd>\n/repo\n</cwd>",
            ENGINE_REFERENCE.trim()
        )
    );
}

#[test]
fn tools_without_a_contribution_are_not_listed() {
    let prompt = SystemPrompt::build(&input(&["read", "subagent", "dynamic_tool"]));
    assert!(prompt.contains("<tools>\n- read: Read file contents\n\nIn addition"));
    let tools = SystemPrompt::sections(&input(&["read", "subagent", "dynamic_tool"]))
        .into_iter()
        .find(|(key, _)| *key == "tools")
        .unwrap()
        .1;
    assert!(!tools.contains("subagent"));
    assert!(!tools.contains("dynamic_tool"));
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

#[test]
fn native_sections_match_executed_reference_except_documented_product_identity() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/native-prompt-identity/reference.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input = PromptInput {
            cwd: PathBuf::from(case["cwd"].as_str().unwrap()),
            tools: serde_json::from_value(case["selectedTools"].clone()).unwrap(),
            context_files: case["contextFiles"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| ContextFile {
                    path: f["path"].as_str().unwrap().into(),
                    content: f["content"].as_str().unwrap().into(),
                })
                .collect(),
            skills: vec![],
        };
        let mut expected = case["expected"].as_object().unwrap().clone();
        let reference_preamble = expected["preamble"].as_str().unwrap();
        assert!(reference_preamble.contains("inside pi,"));
        expected.insert(
            "preamble".into(),
            reference_preamble
                .replace("inside pi,", "inside Pipkin,")
                .into(),
        );
        assert!(
            expected["docs"]
                .as_str()
                .unwrap()
                .contains("/reference/docs")
        );
        expected.insert(
            "docs".into(),
            format!("<docs>\n{}\n</docs>", ENGINE_REFERENCE.trim()).into(),
        );
        let got: serde_json::Map<String, serde_json::Value> = SystemPrompt::sections(&input)
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        assert_eq!(got, expected, "{}", case["name"]);
    }
}
