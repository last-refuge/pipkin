//! Model-context derivation, system prompt, project resources and compaction.
//!
//! Everything here is a pure function of its inputs plus, for [`resources`], read-only
//! filesystem discovery. Nothing touches the store, the network or the clock.
//!
//! - [`derive`]: the next request's messages from the store's context entries (reference
//!   `durable/src/harness/context.ts`).
//! - [`prompt`]: the system prompt text (reference `coding-agent/src/core/system-prompt.ts` as
//!   installed by the durable server, `coding-agent/src/experimental/durable/prompt.ts`).
//! - [`resources`]: `AGENTS.md`/`CLAUDE.md` discovery, skills, prompt templates and the
//!   `/skill:name` and `/template` input expansions (reference `resource-loader.ts`,
//!   `skills.ts`, `prompt-templates.ts`, `utils/frontmatter.ts`).
//! - [`compaction`]: token estimates, cut selection, summarization request text and result
//!   validation (reference `durable/src/harness/compaction.ts`, `ai/src/utils/estimate.ts`).
//!
//! Lengths that the reference measures with JavaScript `String.length` are measured here in
//! UTF-16 code units (see [`js`]) so token estimates, truncation points and validation limits
//! agree with the reference for non-ASCII text.

pub mod compaction;
pub mod derive;
pub mod frontmatter;
pub mod js;
pub mod paths;
pub mod prompt;
pub mod resources;
pub mod system;

#[cfg(test)]
pub(crate) mod test_support;
