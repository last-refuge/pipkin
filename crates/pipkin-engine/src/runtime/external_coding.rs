//! Owned coding-run resources, selected only by separately opted-in current grants.
#![cfg(unix)]
#![allow(dead_code)] // Qualification observer entry points and platform-gated paths.
use super::{
    Shared,
    external_permission_server::{Control, Server},
    external_permissions::{Broker, Error, Lease, Review, Seal},
};
use crate::{execution::claude_cli::write_policy::Hook, store::ExternalPromptReceipt};
use std::{
    path::PathBuf,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Engine-captured consent/transport destinations, never helper or user JSON fields.
/// Public routing derives these from the connection-owned current grant; dispatch
/// rechecks that exact grant under the setup registry lock before durable admission.
pub(super) struct Admission {
    pub owner: u64,
    pub expires: Instant,
    pub revoked: CancellationToken,
    pub parent: PathBuf,
    pub feedback: Feedback,
}
/// Pull-only actors retain Control in the owned server. Test observers are an explicit
/// alternative, not a required sink that production must drain in detached work.
pub(super) enum Feedback {
    PullOnly,
    Observe {
        reviews: mpsc::Sender<Review>,
        control: oneshot::Sender<Arc<Control>>,
    },
}
pub(super) struct Coding {
    pub broker: Arc<Broker>,
    admission: Admission,
    shared: Weak<Shared>,
    registration: Option<super::local_cli_approvals::Registration>,
}
pub(super) struct Bound {
    pub server: Server,
    pub hook: Hook,
    pub reviews: Option<mpsc::Sender<Review>>,
}
impl Coding {
    pub fn reserve(
        shared: &Arc<Shared>,
        receipt: &ExternalPromptReceipt,
        cancel: CancellationToken,
        admission: Admission,
    ) -> Result<Self, Error> {
        let broker = Broker::reserve(
            shared,
            receipt,
            Lease {
                owner: admission.owner,
                expires: admission.expires,
                revoked: admission.revoked.clone(),
                run_cancel: cancel,
            },
        )?;
        Ok(Self {
            broker,
            admission,
            shared: Arc::downgrade(shared),
            registration: None,
        })
    }
    /// Offload filesystem setup only. Registration precedes offload, including a
    /// late/unobserved result after timeout/actor cancellation. Never spawn a CLI here.
    pub async fn bind(&mut self) -> Result<Bound, Error> {
        let token = self.broker.tracker()?.token();
        let broker = self.broker.clone();
        let parent = self.admission.parent.clone();
        let work = tokio::task::spawn_blocking(move || {
            let _completion = token;
            Server::bind(&parent, broker)
        });
        let result = tokio::select! {
            biased;
            _ = self.broker.retired() => Err(Error::Unavailable),
            result = tokio::time::timeout(Duration::from_secs(2), work) => result.map_err(|_| Error::Unavailable).and_then(|r| r.map_err(|_| Error::Unavailable)).and_then(|r|r),
        };
        let (server, control, launch) = match result {
            Ok(value) => value,
            Err(error) => {
                self.broker.fence();
                return Err(error);
            }
        };
        if !self.broker.permits_launch() {
            self.broker.fence();
            return Err(Error::Unavailable);
        }
        let hook = Hook::capture(launch).map_err(|_| Error::Unavailable)?;
        self.registration = Some(
            self.shared
                .upgrade()
                .ok_or(Error::Unavailable)?
                .local_cli_approvals
                .install(&control)
                .map_err(|_| Error::Unavailable)?,
        );
        let reviews = match std::mem::replace(&mut self.admission.feedback, Feedback::PullOnly) {
            Feedback::PullOnly => None,
            Feedback::Observe {
                reviews,
                control: sender,
            } => {
                sender.send(control).map_err(|_| Error::Unavailable)?;
                Some(reviews)
            }
        };
        Ok(Bound {
            server,
            hook,
            reviews,
        })
    }
    pub fn seal(&self) -> (Seal, bool) {
        self.broker.completion_seal()
    }
}
impl Drop for Coding {
    fn drop(&mut self) {
        self.broker.fence();
    }
}
