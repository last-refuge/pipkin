//! Wire adapters, one module per API kind. Each module exposes `pub fn adapter() -> Arc<dyn
//! ApiAdapter>` and is listed in [`all`].

use std::sync::Arc;

use super::ApiAdapter;

pub mod anthropic_messages;
pub mod aws_auth;
pub mod azure_openai_responses;
pub mod bedrock_converse_stream;
pub mod bedrock_eventstream;
pub mod cloudflare;
pub mod common;
pub mod google_auth;
pub mod google_generative_ai;
pub mod google_shared;
pub mod google_vertex;
pub mod mistral_conversations;
pub mod openai_codex_responses;
pub mod openai_completions;
pub mod openai_responses;
pub mod openai_responses_shared;
pub mod pi_messages;

/// Every built-in adapter.
pub fn all() -> Vec<Arc<dyn ApiAdapter>> {
    vec![
        openai_completions::adapter(),
        openai_responses::adapter(),
        azure_openai_responses::adapter(),
        mistral_conversations::adapter(),
        bedrock_converse_stream::adapter(),
        pi_messages::adapter(),
        anthropic_messages::adapter(),
        google_generative_ai::adapter(),
        google_vertex::adapter(),
        // One adapter per line, so parallel additions merge cleanly.
        openai_codex_responses::adapter(),
    ]
}

#[cfg(test)]
pub(crate) mod constrained_tests;
