//! Wire adapters, one module per API kind. Each module exposes `pub fn adapter() -> Arc<dyn
//! ApiAdapter>` and is listed in [`all`].

use std::sync::Arc;

use super::ApiAdapter;

pub mod openai_codex_responses;

/// Every built-in adapter.
pub fn all() -> Vec<Arc<dyn ApiAdapter>> {
    vec![
        // One adapter per line, so parallel additions merge cleanly.
        openai_codex_responses::adapter(),
    ]
}
