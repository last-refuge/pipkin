//! Project resources: instruction files, skills and prompt templates, plus the input
//! expansions that use them. Discovery reads the filesystem; nothing is cached here (the
//! session runtime decides when to reload, like the reference's per-cwd cache in
//! `experimental/durable/prompt.ts:27-39`).
//!
//! Project trust: none of this is trust-gated, matching the durable server, which loads
//! `AGENTS.md`/`CLAUDE.md` and `<cwd>/.pi/skills` regardless of trust.

mod context_files;
mod skills;
mod templates;

pub use context_files::{CONTEXT_FILE_NAMES, ContextFile, load_context_files};
pub use skills::{
    Skill, SkillSource, expand_skill_command, format_skills, load_skills, skill_block,
};
pub use templates::{
    PromptTemplate, expand_template, load_templates, parse_command_args, substitute_args,
};

#[cfg(test)]
mod tests;
