//! The web service as a session's tool host sees it: settings are read per call (a key or
//! provider change applies to the next search), the engine's network settings and client
//! are reused, and private targets are allowed only when the engine configuration says so
//! (tests against a local server).

use std::sync::Arc;

use pipkin_engine_protocol::*;

use super::Shared;
use crate::tools::Tool;
use crate::tools::web::{WebFetchTool, WebReadTool, WebSearchTool};
use crate::web::{WebService, WebSettings};

impl Shared {
    pub fn web_tools(&self) -> [Arc<dyn Tool>; 3] {
        [
            Arc::new(WebSearchTool),
            Arc::new(WebFetchTool),
            Arc::new(WebReadTool),
        ]
    }

    /// The service for one session's call, with that session's merged settings.
    pub fn web_service(&self, session: &str) -> WebService {
        let settings = WebSettings::from_settings(&self.settings_for(session));
        let (net, http) = self.refresh_net_snapshot();
        let env = self.config.env.clone();
        WebService {
            store: self.store.clone(),
            net,
            http,
            settings,
            env: Arc::new(move |key| env.get(key)),
            allow_private: self.config.web_allow_private_hosts,
        }
    }

    pub fn web_sources_view(&self, session: &str) -> Result<Vec<WebSourceView>, ProtoError> {
        if self.store.session(session)?.is_none() {
            return Err(ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            ));
        }
        let records = self
            .store
            .read(|c| crate::store::Store::web_sources(c, session))?;
        Ok(records
            .iter()
            .map(|r| WebSourceView {
                id: r.label(),
                kind: r.kind.clone(),
                provider: r.provider.clone(),
                requested_url: r.requested_url.clone(),
                final_url: r.final_url.clone(),
                title: r.title.clone(),
                published: r.published.clone(),
                retrieved_at_ms: r.retrieved_at_ms,
                content_type: r.content_type.clone(),
                bytes: r.bytes,
                text_chars: r.text_chars,
                expired: r.expired,
                query: r.query.clone(),
            })
            .collect())
    }
}
