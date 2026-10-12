//! The runtime's seam onto `crate::context`: model-context derivation and the system prompt.
//!
//! Resources are loaded once per cwd for a live root/child group, matching the reference's
//! worker-local resource cache. Tool changes rebuild sections from that snapshot. Retiring
//! the entire group or restarting the engine releases it, so a new lifetime sees file edits.

use std::collections::HashMap;
use std::path::Path;

use pipkin_engine_protocol::{Entry, Message};

use crate::context::prompt::{PromptInput, SystemPrompt};
use crate::context::resources;

/// The next request's messages from the raw context range (newest head marker first).
pub fn model_messages(entries: &[Entry]) -> Vec<Message> {
    crate::context::derive::model_messages(entries)
}

#[derive(Default)]
pub(super) struct Resources {
    directories: HashMap<String, PromptInput>,
}

impl Resources {
    /// Render current tool contributions using the group's frozen context files/skills.
    pub(super) fn system_sections(
        &mut self,
        cwd: &str,
        config_dir: &Path,
        tools: &[String],
        skill_paths: &[String],
    ) -> (serde_json::Map<String, serde_json::Value>, Vec<String>) {
        let mut warnings = Vec::new();
        let input = self.directories.entry(cwd.to_owned()).or_insert_with(|| {
            let cwd_path = Path::new(cwd);
            let context_files = resources::load_context_files(cwd_path, config_dir);
            let (skills, found_warnings) =
                resources::load_skills(cwd_path, config_dir, skill_paths);
            warnings = found_warnings;
            PromptInput {
                cwd: cwd_path.to_path_buf(),
                tools: Vec::new(),
                context_files,
                skills,
                docs_dir: None,
            }
        });
        input.tools.clear();
        input.tools.extend_from_slice(tools);
        (
            SystemPrompt::sections(input)
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.into()))
                .collect(),
            warnings,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_resources_match_the_executed_reference_cache_lifetimes() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/native-prompt-resources/reference.json"
        ))
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut groups: HashMap<String, Resources> = HashMap::new();
        for case in fixture["cases"].as_array().unwrap() {
            let cwd = dir.path().join(case["cwd"].as_str().unwrap());
            let skill_dir = cwd.join(".pi/skills/example");
            std::fs::create_dir_all(&skill_dir).unwrap();
            let version = case["version"].as_u64().unwrap();
            std::fs::write(cwd.join("AGENTS.md"), format!("instruction-{version}")).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: example\ndescription: skill-{version}\n---\nBody.\n"),
            )
            .unwrap();
            let tools: Vec<String> = serde_json::from_value(case["tools"].clone()).unwrap();
            let (sections, warnings) = groups
                .entry(case["group"].as_str().unwrap().into())
                .or_default()
                .system_sections(
                    cwd.to_str().unwrap(),
                    &dir.path().join("config"),
                    &tools,
                    &[],
                );
            assert!(warnings.is_empty(), "{warnings:?}");
            for key in ["project_context", "skills"] {
                assert!(
                    sections[key]
                        .as_str()
                        .unwrap()
                        .contains(case["expected"][key].as_str().unwrap()),
                    "reference case {case}: {sections:?}"
                );
            }
            let expected_tools: Vec<String> =
                serde_json::from_value(case["expected"]["tools"].clone()).unwrap();
            for tool in expected_tools {
                assert!(
                    sections["tools"]
                        .as_str()
                        .unwrap()
                        .contains(&format!("- {tool}:"))
                );
            }
        }
    }
}
