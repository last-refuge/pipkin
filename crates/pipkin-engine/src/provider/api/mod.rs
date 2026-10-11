//! Wire adapters, one module per API kind. Each module exposes `pub fn adapter() -> Arc<dyn
//! ApiAdapter>` and is listed in [`all`].

use std::sync::Arc;

use super::ApiAdapter;

pub mod aws_auth;
pub mod bedrock_converse_stream;
pub mod bedrock_eventstream;
pub mod cloudflare;
pub mod mistral_conversations;
pub mod pi_messages;

/// Every built-in adapter.
pub fn all() -> Vec<Arc<dyn ApiAdapter>> {
    vec![
        mistral_conversations::adapter(),
        bedrock_converse_stream::adapter(),
        pi_messages::adapter(),
    ]
}
