//! The runtime's seam onto `crate::context`: model-context derivation and the system prompt.
//!
//! The system prompt is built once per session (per engine process) from the session's cwd,
//! offered tools, project instruction files and skills, and then kept byte-identical between
//! turns so provider prompt caches stay warm (the reference caches context files and skills per
//! cwd for the worker's lifetime). Reopening the engine rebuilds it.

use std::path::Path;

use pipkin_engine_protocol::{Entry, Message};

use crate::context::prompt::{PromptInput, SystemPrompt};
use crate::context::resources;

/// The next request's messages from the raw context range (newest head marker first).
pub fn model_messages(entries: &[Entry]) -> Vec<Message> {
    crate::context::derive::model_messages(entries)
}

/// The durable server's system prompt for a session.
pub fn system_prompt(
    cwd: &str,
    config_dir: &Path,
    tools: &[String],
    skill_paths: &[String],
) -> (String, Vec<String>) {
    let cwd_path = Path::new(cwd);
    let context_files = resources::load_context_files(cwd_path, config_dir);
    let (skills, warnings) = resources::load_skills(cwd_path, config_dir, skill_paths);
    let input = PromptInput {
        cwd: cwd_path.to_path_buf(),
        tools: tools.to_vec(),
        context_files,
        skills,
        docs_dir: None,
    };
    (SystemPrompt::build(&input), warnings)
}
