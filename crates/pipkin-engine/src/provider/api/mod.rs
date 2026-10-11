//! Wire adapters, one module per API kind. Each module exposes `pub fn adapter() -> Arc<dyn
//! ApiAdapter>` and is listed in [`all`].

use std::sync::Arc;

use super::ApiAdapter;

/// Every built-in adapter.
pub fn all() -> Vec<Arc<dyn ApiAdapter>> {
    vec![]
}
