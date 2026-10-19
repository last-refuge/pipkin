//! One commit/delivery critical section shared with native SessionRt publication.
//! The sink is captured by a trusted runtime owner, never reconstructed from CLI output.
use crate::{
    execution::claude_cli::orchestration_protocol::Scope,
    store::{self, StoreError, cli_agent_turns::projection::Update},
};
use std::sync::{Arc, Mutex};

pub struct Publication {
    gate: Arc<Mutex<()>>,
    scope: Scope,
    current: Arc<dyn Fn() -> bool + Send + Sync>,
    emit: Arc<dyn Fn(Update) + Send + Sync>,
}
impl Publication {
    pub(crate) fn new(
        gate: Arc<Mutex<()>>,
        scope: Scope,
        current: impl Fn() -> bool + Send + Sync + 'static,
        emit: impl Fn(Update) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            gate,
            scope,
            current: Arc::new(current),
            emit: Arc::new(emit),
        })
    }
    /// No await under this gate. Commit order and delivery order cannot diverge, including
    /// between SDK text, authenticated tool receipts and ordinary Shared::commit writers.
    /// Failure after commit never grants permission to retry the transaction/effect.
    pub(crate) fn commit<T>(
        &self,
        scope: &Scope,
        f: impl FnOnce() -> store::Result<(T, Option<Update>)>,
    ) -> store::Result<T> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| StoreError::ExecutionConflict)?;
        if scope != &self.scope || !(self.current)() {
            return Err(StoreError::ExecutionConflict);
        }
        let (value, update) = f()?;
        if let Some(update) = update {
            if update.scope != self.scope || !(self.current)() {
                return Err(StoreError::ExecutionConflict);
            }
            if !update.changes.is_empty() {
                (self.emit)(update);
            }
        }
        Ok(value)
    }
}
