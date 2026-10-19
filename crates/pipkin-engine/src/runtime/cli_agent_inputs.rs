//! Private queued human input. Steering joins an active root at authenticated native tool
//! boundaries, otherwise the next clean turn. Original tickets own all delivery and batching.
#![allow(dead_code)] // Includes private qualification seams.
use super::{
    Shared,
    cli_agent_host::ChildResources,
    cli_agent_owner::{Error as OwnerError, InputTicket},
    cli_agent_runtime::admission::Start,
    cli_bridge_driver::Services,
};
use crate::{
    store::cli_agent_turns::{
        Receipt,
        inputs::{Kind, Queued},
    },
    util::uuid_v4,
};
use pipkin_engine_protocol::{Event, SessionChange};
use std::{sync::Arc, time::Duration};
use tokio::{sync::oneshot, time::Instant};
pub(super) struct Input {
    pub owner: u64,
    pub session: String,
    pub request: String,
    pub generation: u64,
    pub operation: String,
    pub kind: Kind,
    pub text: String,
    pub images: Vec<pipkin_engine_protocol::ImageInput>,
}
pub(super) struct Enqueued {
    pub entry: u64,
    /// None means exact durable lookup only; never a second actor.
    pub completion: Option<oneshot::Receiver<Result<Receipt, String>>>,
}
impl Shared {
    pub(super) fn enqueue_cli_input(
        self: &Arc<Self>,
        input: Input,
        resources: Arc<ChildResources>,
    ) -> Result<Enqueued, String> {
        self.enqueue_cli_input_during(input, resources, None)
    }
    pub(super) fn enqueue_cli_input_during(
        self: &Arc<Self>,
        input: Input,
        resources: Arc<ChildResources>,
        expected: Option<&super::cli_agent_owner::InputEpoch>,
    ) -> Result<Enqueued, String> {
        let row = self
            .store
            .session(&input.session)
            .map_err(|e| e.to_string())?
            .ok_or("Missing session")?;
        if resources.prompt.cwd != std::path::Path::new(&row.cwd) {
            return Err("Input resources do not match the approved project".into());
        }
        let text = if input.kind == Kind::Compact {
            if input.text.contains(['\n', '\r', '\0']) {
                return Err("Compaction instructions must be one line".into());
            }
            if input.text.trim().is_empty() {
                "/compact".into()
            } else {
                format!("/compact {}", input.text)
            }
        } else {
            input.text
        };
        let q = Queued {
            session: input.session,
            request: input.request,
            text,
            images: input.images,
            kind: input.kind,
            generation: input.generation,
            operation: input.operation,
            ticket: uuid_v4(),
        };
        let (ticket, entry) = {
            let _gate = self.publication.lock().unwrap_or_else(|e| e.into_inner());
            let result = self
                .cli_agents
                .queue_input_during(&self.store, input.owner, q.clone(), expected)
                .map_err(|e| format!("CLI input refused: {e:?}"))?;
            self.publish_cli_queue(&q.session);
            result
        };
        let Some(ticket) = ticket else {
            return Ok(Enqueued {
                entry,
                completion: None,
            });
        };
        let tracking = self.external_tasks.token();
        let shared = self.clone();
        let (send, receive) = oneshot::channel();
        self.spawn(async move {
            let result = shared.run_cli_input(ticket, resources).await;
            {
                let _gate = shared.publication.lock().unwrap_or_else(|e| e.into_inner());
                if shared.store.withdraw_cli_input(&q).is_err() {
                    shared.publish(Event::ResyncRequired);
                }
                shared.publish_cli_queue(&q.session);
            }
            let _ = send.send(result);
            drop(tracking);
        });
        Ok(Enqueued {
            entry,
            completion: Some(receive),
        })
    }
    pub(super) fn publish_cli_queue(&self, session: &str) {
        match (self.store.queue(session), self.store.session(session)) {
            (Ok(rows), Ok(Some(row))) => self.publish(Event::Session {
                session: session.into(),
                revision: row.revision,
                change: SessionChange::QueueChanged {
                    queue: crate::store::Store::queue_view(&rows),
                },
            }),
            _ => self.publish(Event::ResyncRequired),
        }
    }
    async fn run_cli_input(
        self: &Arc<Self>,
        mut ticket: InputTicket,
        resources: Arc<ChildResources>,
    ) -> Result<Receipt, String> {
        let epoch = ticket.cancellation();
        let revoked = ticket.revoked();
        let deadline = Instant::now() + Duration::from_secs(30 * 60);
        let ownership = loop {
            if let Some(receipt) = ticket
                .delivered(&self.store)
                .map_err(|e| format!("Input receipt: {e:?}"))?
            {
                return if receipt.state == crate::store::cli_agent_turns::State::Completed
                    && !receipt.fenced
                {
                    Ok(receipt)
                } else {
                    Err("Delivered input has an unconfirmed root outcome; do not replay".into())
                };
            }
            if epoch.is_cancelled()
                || revoked.is_cancelled()
                || self.stopping.is_cancelled()
                || Instant::now() >= deadline
            {
                return Err("Queued CLI input cancelled before delivery".into());
            }
            let attempt = tokio::task::block_in_place(|| {
                let _gate = self.publication.lock().unwrap_or_else(|e| e.into_inner());
                if ticket.consumed(&self.store)? {
                    Err(OwnerError::Busy)
                } else {
                    ticket.begin(&self.store)
                }
            });
            match attempt {
                Ok(turn) => break turn,
                Err(OwnerError::Busy) => {}
                Err(e) => return Err(format!("Queued CLI input refused: {e:?}")),
            }
            tokio::select! { _=epoch.cancelled()=>{},_=revoked.cancelled()=>{},_=self.stopping.cancelled()=>{},_=tokio::time::sleep(Duration::from_millis(50))=>{} }
        };
        #[cfg(test)]
        let (send, receive) = oneshot::channel();
        let q = &ticket.queued;
        let start = Start {
            ownership,
            request: q.request.clone(),
            generation: q.generation,
            operation: q.operation.clone(),
            text: q.text.clone(),
            images: q.images.clone(),
            system: resources.root_system.to_string(),
            assets_parent: resources.image.assets_parent().into(),
            image: resources.image.clone(),
            tools: resources.tools.clone(),
            services: Services {
                child_resources: Some(resources.clone()),
                shell: resources.shell.clone(),
                #[cfg(test)]
                child_peers: resources.peers.clone(),
                ..Services::default()
            },
            deadline,
            cancel: epoch,
            #[cfg(test)]
            test_peer: Some(send),
            #[cfg(test)]
            test_preparing: None,
        };
        let completion = self.submit_cli_agent_root(start);
        #[cfg(test)]
        if let Some(peers) = resources.peers.clone() {
            self.spawn(async move {
                if let Ok(peer) = receive.await {
                    let _ = peers.send(peer);
                }
            });
        }
        completion
            .await
            .map_err(|_| "CLI input reply lost; do not replay".to_string())?
            .map_err(|e| format!("CLI input failed: {e:?}"))
    }
}
