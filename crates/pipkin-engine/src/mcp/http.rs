//! The SDK's Streamable HTTP client trait over the engine's own `reqwest` client, so MCP
//! traffic follows the same proxy, CA and idle-timeout policy as every other request. The
//! SDK's reqwest implementation is not used (it would bring a second reqwest).

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue, WWW_AUTHENTICATE,
};
use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError, SseError, StreamableHttpClient, StreamableHttpError,
    StreamableHttpPostResponse,
};
use sse_stream::{Sse, SseStream};

const HEADER_SESSION_ID: &str = "mcp-session-id";
const HEADER_LAST_EVENT_ID: &str = "last-event-id";
const JSON: &str = "application/json";
const EVENT_STREAM: &str = "text/event-stream";

#[derive(Debug)]
pub struct HttpError(pub String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HttpError {}

pub type Error = StreamableHttpError<HttpError>;

#[derive(Clone)]
pub struct EngineHttp {
    pub client: reqwest::Client,
}

fn client_error(e: &reqwest::Error) -> Error {
    StreamableHttpError::Client(HttpError(crate::provider::http::transport_error(e)))
}

fn header_text(resp: &reqwest::Response, name: HeaderName) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn session_of(resp: &reqwest::Response) -> Option<String> {
    header_text(resp, HeaderName::from_static(HEADER_SESSION_ID))
}

fn content_type(resp: &reqwest::Response) -> Option<String> {
    header_text(resp, CONTENT_TYPE).map(|c| c.to_ascii_lowercase())
}

/// 401 and 403 become the SDK's auth errors so the service can show "sign in" instead of a
/// transport failure.
fn auth_failure(resp: &reqwest::Response) -> Option<Error> {
    let status = resp.status();
    let header = header_text(resp, WWW_AUTHENTICATE);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Some(StreamableHttpError::AuthRequired(AuthRequiredError::new(
            header.unwrap_or_default(),
        )));
    }
    if status == reqwest::StatusCode::FORBIDDEN
        && let Some(h) = &header
        && h.contains("insufficient_scope")
    {
        let scope = h.split(',').map(str::trim).find_map(|part| {
            part.strip_prefix("scope=")
                .map(|s| s.trim_matches('"').to_owned())
        });
        return Some(StreamableHttpError::InsufficientScope(
            InsufficientScopeError::new(h.clone(), scope),
        ));
    }
    None
}

fn sse(resp: reqwest::Response) -> BoxStream<'static, Result<Sse, SseError>> {
    SseStream::from_bytes_stream(resp.bytes_stream()).boxed()
}

fn parse_json_rpc_error(body: &str) -> Option<ServerJsonRpcMessage> {
    match serde_json::from_str::<ServerJsonRpcMessage>(body) {
        Ok(message @ JsonRpcMessage::Error(_)) => Some(message),
        _ => None,
    }
}

impl StreamableHttpClient for EngineHttp {
    type Error = HttpError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let mut req = self
            .client
            .post(&*uri)
            .header(ACCEPT, format!("{JSON}, {EVENT_STREAM}"))
            .header(CONTENT_TYPE, JSON);
        if let Some(sid) = &session_id {
            req = req.header(HEADER_SESSION_ID, &**sid);
        }
        if let Some(auth) = auth_header {
            req = req.header(AUTHORIZATION, auth);
        }
        for (name, value) in custom_headers {
            req = req.header(name, value);
        }
        let resp = req
            .json(&message)
            .send()
            .await
            .map_err(|e| client_error(&e))?;
        if let Some(e) = auth_failure(&resp) {
            return Err(e);
        }
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(StreamableHttpError::SessionExpired);
        }
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            if let Some(message) = parse_json_rpc_error(&body) {
                return Ok(StreamableHttpPostResponse::Json(message, None));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(
                format!("HTTP {}", status.as_u16()).into(),
            ));
        }
        let sid = session_of(&resp);
        match content_type(&resp).as_deref() {
            Some(ct) if ct.starts_with(JSON) => {
                let message: ServerJsonRpcMessage =
                    resp.json().await.map_err(|e| client_error(&e))?;
                Ok(StreamableHttpPostResponse::Json(message, sid))
            }
            Some(ct) if ct.starts_with(EVENT_STREAM) => {
                Ok(StreamableHttpPostResponse::Sse(sse(resp), sid))
            }
            other => Err(StreamableHttpError::UnexpectedContentType(
                other.map(str::to_owned),
            )),
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), Error> {
        let mut req = self
            .client
            .delete(&*uri)
            .header(HEADER_SESSION_ID, &*session_id);
        if let Some(auth) = auth_header {
            req = req.header(AUTHORIZATION, auth);
        }
        for (name, value) in custom_headers {
            req = req.header(name, value);
        }
        let resp = req.send().await.map_err(|e| client_error(&e))?;
        if resp.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportDeleteSession);
        }
        if let Some(e) = auth_failure(&resp) {
            return Err(e);
        }
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Error> {
        let mut req = self.client.get(&*uri).header(ACCEPT, EVENT_STREAM);
        if let Some(sid) = &session_id {
            req = req.header(HEADER_SESSION_ID, &**sid);
        }
        if let Some(id) = last_event_id {
            req = req.header(HEADER_LAST_EVENT_ID, id);
        }
        if let Some(auth) = auth_header {
            req = req.header(AUTHORIZATION, auth);
        }
        for (name, value) in custom_headers {
            req = req.header(name, value);
        }
        let resp = req.send().await.map_err(|e| client_error(&e))?;
        if resp.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        if let Some(e) = auth_failure(&resp) {
            return Err(e);
        }
        if !resp.status().is_success() {
            return Err(StreamableHttpError::UnexpectedServerResponse(
                format!("HTTP {}", resp.status().as_u16()).into(),
            ));
        }
        match content_type(&resp).as_deref() {
            Some(ct) if ct.starts_with(EVENT_STREAM) => Ok(sse(resp)),
            other => Err(StreamableHttpError::UnexpectedContentType(
                other.map(str::to_owned),
            )),
        }
    }
}

/// The `WWW-Authenticate` header of an auth-required failure, from any SDK error that wraps
/// this transport's errors (initialize errors and service errors carry the transport error
/// as a field rather than a source, so those are unwrapped explicitly).
pub fn auth_required(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(e) = current {
        if let Some(a) = e.downcast_ref::<AuthRequiredError>() {
            return Some(a.www_authenticate_header.clone());
        }
        if let Some(StreamableHttpError::AuthRequired(a)) = e.downcast_ref::<Error>() {
            return Some(a.www_authenticate_header.clone());
        }
        if let Some(dynamic) = e.downcast_ref::<rmcp::transport::DynamicTransportError>() {
            current = Some(dynamic.error.as_ref() as &(dyn std::error::Error + 'static));
            continue;
        }
        if let Some(rmcp::service::ClientInitializeError::TransportError { error, .. }) =
            e.downcast_ref::<rmcp::service::ClientInitializeError>()
        {
            current = Some(error.error.as_ref() as &(dyn std::error::Error + 'static));
            continue;
        }
        if let Some(rmcp::service::ServiceError::TransportSend(dynamic)) =
            e.downcast_ref::<rmcp::service::ServiceError>()
        {
            current = Some(dynamic.error.as_ref() as &(dyn std::error::Error + 'static));
            continue;
        }
        current = e.source();
    }
    None
}
