//! Wire adapters, one module per API kind. Each module exposes `pub fn adapter() -> Arc<dyn
//! ApiAdapter>` and is listed in [`all`].

use std::sync::Arc;

use super::ApiAdapter;

pub mod anthropic_messages;
pub mod google_auth;
pub mod google_generative_ai;
pub mod google_shared;
pub mod google_vertex;

/// Every built-in adapter.
pub fn all() -> Vec<Arc<dyn ApiAdapter>> {
    vec![
        anthropic_messages::adapter(),
        google_generative_ai::adapter(),
        google_vertex::adapter(),
    ]
}
