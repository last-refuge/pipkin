//! Request cancellation must be checked at admission, not only by the outer select.
//! Its cancellation branch may have been polled before a queued lock became available.
//! This scope is intentionally not inherited by spawned runs/cleanup after admission.
use std::future::Future;
use tokio_util::sync::CancellationToken;

tokio::task_local! { static REQUEST: CancellationToken; }

pub(super) async fn scope<T>(cancel: &CancellationToken, future: impl Future<Output = T>) -> T {
    REQUEST.scope(cancel.clone(), future).await
}

/// Only pre-admission owned work explicitly inherits this; spawned execution must not.
pub(super) fn capture() -> CancellationToken {
    REQUEST.try_with(Clone::clone).unwrap_or_default()
}

pub(super) async fn scope_owned<T>(
    cancel: CancellationToken,
    future: impl Future<Output = T>,
) -> T {
    REQUEST.scope(cancel, future).await
}

pub(super) fn is_cancelled() -> bool {
    REQUEST
        .try_with(CancellationToken::is_cancelled)
        .unwrap_or(false)
}
