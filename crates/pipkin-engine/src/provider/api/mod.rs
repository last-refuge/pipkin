//! Wire adapters, one module per API kind. Each module exposes `pub fn adapter() -> Arc<dyn
//! ApiAdapter>` and is listed in [`all`].

use std::sync::Arc;

use super::ApiAdapter;

pub mod azure_openai_responses;
pub mod openai_completions;
pub mod openai_responses;
pub mod openai_responses_shared;
pub mod transform;
pub mod aws_auth;
pub mod bedrock_converse_stream;
pub mod bedrock_eventstream;
pub mod cloudflare;
pub mod mistral_conversations;
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
    ]
}
