//! `google-vertex`: Gemini on Vertex AI.
//!
//! Reference: `packages/ai/src/api/google-vertex.ts` (+ `google-shared.ts`). Same body and
//! stream as `google-generative-ai`; differs in endpoint and auth:
//!
//! - API key (`GOOGLE_CLOUD_API_KEY`, resolved by auth into `Credential::ApiKey`): Vertex
//!   express mode, `x-goog-api-key`, `https://aiplatform.googleapis.com/v1/publishers/google/...`.
//!   The `gcp-vertex-credentials` marker and `<placeholder>` keys mean "use ADC".
//! - Otherwise Application Default Credentials ([`super::google_auth`]) with a project
//!   (`project` option > `GOOGLE_CLOUD_PROJECT` > `GCLOUD_PROJECT`) and location (`location`
//!   option > `GOOGLE_CLOUD_LOCATION`): `Authorization: Bearer`, regional endpoint,
//!   `/v1/projects/{p}/locations/{l}/publishers/google/models/{id}`.
//! - A catalog base URL containing `{location}` is a template; any other base URL is used as a
//!   collection root (no project prefix), with `/v1` appended unless it already has a version.

use std::sync::Arc;

use pipkin_engine_protocol::Message;
use serde_json::Value;

use super::common::opt_str;
use super::google_auth;
use super::google_shared as shared;
use crate::provider::{ApiAdapter, BoxFuture, Credential, ProviderRequest, StreamCtx};

const API_VERSION: &str = "v1";
const GCP_VERTEX_CREDENTIALS_MARKER: &str = "gcp-vertex-credentials";
const MULTI_REGIONAL_LOCATIONS: &[&str] = &["us", "eu"];

pub struct GoogleVertex;

pub fn adapter() -> Arc<dyn ApiAdapter> {
    Arc::new(GoogleVertex)
}

/// How the request authenticates.
#[derive(Clone, Debug, PartialEq)]
enum VertexAuth {
    ApiKey(String),
    /// An access token supplied directly (`Credential::Bearer`).
    Token(String),
    Adc,
}

fn is_placeholder(key: &str) -> bool {
    key.len() > 2
        && key.starts_with('<')
        && key.ends_with('>')
        && !key[1..key.len() - 1].contains('>')
}

fn resolve_auth(req: &ProviderRequest) -> VertexAuth {
    match &req.credential {
        Credential::ApiKey(k) => {
            let k = k.trim();
            if k.is_empty() || k == GCP_VERTEX_CREDENTIALS_MARKER || is_placeholder(k) {
                VertexAuth::Adc
            } else {
                VertexAuth::ApiKey(k.to_owned())
            }
        }
        Credential::Bearer(t) if !t.trim().is_empty() => VertexAuth::Token(t.trim().to_owned()),
        Credential::OAuth { access, .. } if !access.trim().is_empty() => {
            VertexAuth::Token(access.trim().to_owned())
        }
        _ => VertexAuth::Adc,
    }
}

/// A setting from the request option, the ambient credential's environment, or the env.
fn setting(req: &ProviderRequest, option: Option<&str>, names: &[&str]) -> Option<String> {
    if let Some(v) = option
        .and_then(|o| opt_str(req, o))
        .filter(|v| !v.is_empty())
    {
        return Some(v.to_owned());
    }
    for name in names {
        if let Credential::Ambient(map) = &req.credential
            && let Some(v) = map
                .get(*name)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
        {
            return Some(v.to_owned());
        }
        if let Some(v) = req.env_var(name) {
            return Some(v);
        }
    }
    None
}

fn resolve_project(req: &ProviderRequest) -> Result<String, String> {
    setting(req, Some("project"), &["GOOGLE_CLOUD_PROJECT", "GCLOUD_PROJECT"]).ok_or_else(|| {
        "Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass project in options.".into()
    })
}

fn resolve_location(req: &ProviderRequest) -> Result<String, String> {
    setting(req, Some("location"), &["GOOGLE_CLOUD_LOCATION"]).ok_or_else(|| {
        "Vertex AI requires a location. Set GOOGLE_CLOUD_LOCATION or pass location in options."
            .into()
    })
}

/// A configured base URL that is not the `{location}` template.
fn custom_base_url(base: &str) -> Option<&str> {
    let trimmed = base.trim();
    (!trimmed.is_empty() && !trimmed.contains("{location}")).then_some(trimmed)
}

fn base_url_includes_api_version(base: &str) -> bool {
    let path = match url::Url::parse(base) {
        Ok(u) => u.path().to_owned(),
        Err(_) => base.to_owned(),
    };
    path.split('/').any(|part| {
        let Some(rest) = part.strip_prefix('v') else {
            return false;
        };
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        let tail = &rest[digits..];
        tail.is_empty()
            || tail
                .strip_prefix("beta")
                .is_some_and(|d| d.chars().all(|c| c.is_ascii_digit()))
    })
}

/// SDK `tModel` for Vertex.
fn model_path(id: &str) -> String {
    if id.starts_with("publishers/") || id.starts_with("projects/") || id.starts_with("models/") {
        id.to_owned()
    } else if let Some((publisher, model)) = id.split_once('/') {
        let model = model.split('/').next().unwrap_or(model);
        format!("publishers/{publisher}/models/{model}")
    } else {
        format!("publishers/google/models/{id}")
    }
}

/// Where the request goes. `project` is `None` in express (API key) mode.
pub(crate) fn request_url(req: &ProviderRequest, project: Option<(&str, &str)>) -> String {
    let model = model_path(&req.model.id);
    let tail = format!("{model}:streamGenerateContent?alt=sse");
    if let Some(base) = custom_base_url(req.base_url()) {
        let base = base.trim_end_matches('/');
        return if base_url_includes_api_version(base) {
            format!("{base}/{tail}")
        } else {
            format!("{base}/{API_VERSION}/{tail}")
        };
    }
    let env_base = req.env_var("GOOGLE_VERTEX_BASE_URL");
    match project {
        None => {
            let base = env_base
                .as_deref()
                .unwrap_or("https://aiplatform.googleapis.com");
            format!("{}/{API_VERSION}/{tail}", base.trim_end_matches('/'))
        }
        Some((project, location)) => {
            let host = if location == "global" {
                "https://aiplatform.googleapis.com".to_owned()
            } else if MULTI_REGIONAL_LOCATIONS.contains(&location) {
                format!("https://aiplatform.{location}.rep.googleapis.com")
            } else {
                format!("https://{location}-aiplatform.googleapis.com")
            };
            let base = env_base.unwrap_or(host);
            format!(
                "{}/{API_VERSION}/projects/{project}/locations/{location}/{tail}",
                base.trim_end_matches('/')
            )
        }
    }
}

async fn run(ctx: StreamCtx<'_>) -> Message {
    let req = ctx.req;
    let auth = resolve_auth(req);
    let mut headers = shared::base_headers(req);
    let url = match &auth {
        VertexAuth::ApiKey(key) => {
            headers.set_default("x-goog-api-key", key.clone());
            request_url(req, None)
        }
        VertexAuth::Token(_) | VertexAuth::Adc => {
            let project = match resolve_project(req) {
                Ok(p) => p,
                Err(e) => return ctx.failed(ctx.emitter(), e),
            };
            let location = match resolve_location(req) {
                Ok(l) => l,
                Err(e) => return ctx.failed(ctx.emitter(), e),
            };
            request_url(req, Some((&project, &location)))
        }
    };
    let body = match shared::build_body(req, false) {
        Ok(b) => b,
        Err(e) => return ctx.failed(ctx.emitter(), e),
    };
    match auth {
        VertexAuth::ApiKey(_) => {}
        VertexAuth::Token(token) => {
            headers.set_default("authorization", format!("Bearer {token}"));
        }
        VertexAuth::Adc => {
            let explicit = setting(
                req,
                None,
                &[
                    "GOOGLE_APPLICATION_CREDENTIALS",
                    "google_application_credentials",
                ],
            );
            let token = match google_auth::resolve(&ctx, explicit).await {
                Ok(t) => t,
                Err(e) => return ctx.failed(ctx.emitter(), e),
            };
            headers.set_default("authorization", format!("Bearer {}", token.token));
            if let Some(quota) = token.quota_project {
                headers.set_default("x-goog-user-project", quota);
            }
        }
    }
    shared::stream(&ctx, &url, &headers, &body).await
}

impl ApiAdapter for GoogleVertex {
    fn api(&self) -> &'static str {
        "google-vertex"
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(run(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::api::google_auth::tests::TEST_PRIVATE_KEY;
    use crate::provider::api::google_shared::tests::{model, request};
    use crate::provider::mock::{MockResponse, MockServer};
    use crate::provider::{Model, ThinkingLevel};
    use pipkin_engine_protocol::StopReason;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn vertex(id: &str, base: &str) -> Model {
        model(json!({
            "id": id, "api": "google-vertex", "provider": "google-vertex", "baseUrl": base,
            "reasoning": true, "input": ["text", "image"], "contextWindow": 1048576, "maxTokens": 65536
        }))
    }

    async fn run_req(req: &ProviderRequest) -> Message {
        let http = crate::provider::http::client();
        let ctx = StreamCtx {
            req,
            http: &http,
            events: None,
            cancel: CancellationToken::new(),
        };
        adapter().stream(ctx).await
    }

    const OK: &str = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]},\"finishReason\":\"STOP\"}]}\n\n";

    #[test]
    fn urls_match_the_sdk_for_every_mode() {
        let template = "https://{location}-aiplatform.googleapis.com";
        let mut req = request(vertex("gemini-2.5-pro", template));
        assert_eq!(
            request_url(&req, Some(("proj", "us-central1"))),
            "https://us-central1-aiplatform.googleapis.com/v1/projects/proj/locations/us-central1/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        assert_eq!(
            request_url(&req, Some(("proj", "global"))),
            "https://aiplatform.googleapis.com/v1/projects/proj/locations/global/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        assert_eq!(
            request_url(&req, Some(("proj", "eu"))),
            "https://aiplatform.eu.rep.googleapis.com/v1/projects/proj/locations/eu/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        assert_eq!(
            request_url(&req, None),
            "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        req.base_url = Some("https://proxy.example/vertex/v1beta1/".into());
        assert_eq!(
            request_url(&req, Some(("proj", "us-central1"))),
            "https://proxy.example/vertex/v1beta1/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        req.base_url = Some("https://proxy.example".into());
        req.model.id = "anthropic/claude-x".into();
        assert_eq!(
            request_url(&req, None),
            "https://proxy.example/v1/publishers/anthropic/models/claude-x:streamGenerateContent?alt=sse"
        );
        assert!(is_placeholder("<your key>"));
        assert!(!is_placeholder("key"));
    }

    #[tokio::test]
    async fn api_key_express_mode_and_markers_fall_back_to_adc() {
        let server = MockServer::start(|_, _| MockResponse::sse(vec![OK.into()])).await;
        let mut req = request(vertex("gemini-2.5-flash", &server.url));
        req.credential = Credential::ApiKey("vertex-key".into());
        req.thinking = ThinkingLevel::Low;
        let m = run_req(&req).await;
        assert_eq!(m.text(), "hi");
        let r = &server.requests()[0];
        assert_eq!(
            r.path,
            "/v1/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
        assert_eq!(r.header("x-goog-api-key"), Some("vertex-key"));
        assert_eq!(r.header("authorization"), None);
        assert_eq!(
            r.json()["generationConfig"]["thinkingConfig"],
            json!({"includeThoughts": true, "thinkingBudget": 2048})
        );

        // The marker means ADC; without a project this fails before any request.
        req.credential = Credential::ApiKey(GCP_VERTEX_CREDENTIALS_MARKER.into());
        req.env.insert("GOOGLE_CLOUD_PROJECT".into(), String::new());
        req.env.insert("GCLOUD_PROJECT".into(), String::new());
        // (Empty request env values mask the process environment.)
        let m = run_req(&req).await;
        assert!(
            matches!(&m, Message::Assistant { stop_reason: StopReason::Error,
            error_message: Some(e), .. } if e.starts_with("Vertex AI requires a project ID"))
        );
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn external_account_credentials_authenticate_vertex_streams() {
        for kind in ["file", "user", "executable"] {
            if kind == "executable" && !cfg!(unix) {
                continue;
            }
            let authorized_user = kind == "user";
            let server = MockServer::start(|req, _| {
                if req.path.starts_with("/v1/token") || req.path.starts_with("/v1/oauthtoken") {
                    MockResponse::json(200, &json!({"access_token":"federated-token", "expires_in":3600, "refresh_token":"rotated"}))
                } else {
                    MockResponse::sse(vec![OK.into()])
                }
            }).await;
            let dir = tempfile::tempdir().unwrap();
            let reference: Value = serde_json::from_str(include_str!(
                "../../../../../fixtures/native-google-external/reference.json"
            ))
            .unwrap();
            let mut document =
                reference["cases"][if authorized_user { 7 } else { 0 }]["document"].clone();
            document["token_url"] = json!(format!(
                "{}/v1/{}",
                server.url,
                if authorized_user {
                    "oauthtoken"
                } else {
                    "token"
                }
            ));
            if !authorized_user {
                let subject = dir.path().join("subject");
                std::fs::write(&subject, "subject-token").unwrap();
                document["credential_source"]["file"] = json!(subject.display().to_string());
            }
            if kind == "executable" {
                let helper = dir.path().join("helper.sh");
                std::fs::write(&helper, "printf '%s' '{\"version\":1,\"success\":true,\"token_type\":\"urn:ietf:params:oauth:token-type:jwt\",\"id_token\":\"executable-subject\"}'\n").unwrap();
                document["credential_source"] =
                    json!({"executable":{"command":format!("/bin/sh \"{}\"", helper.display())}});
            }
            let path = dir.path().join("adc.json");
            std::fs::write(&path, document.to_string()).unwrap();
            let mut req = request(vertex("gemini-2.5-pro", &format!("{}/vx", server.url)));
            req.credential = Credential::None;
            req.env.clear();
            req.env.insert(
                "GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES".into(),
                "1".into(),
            );
            req.options
                .insert(crate::provider::ISOLATED_ENV.into(), json!(true));
            req.options.insert("project".into(), json!("project"));
            req.options.insert("location".into(), json!("us-central1"));
            req.env.insert(
                "GOOGLE_APPLICATION_CREDENTIALS".into(),
                path.display().to_string(),
            );
            for _ in 0..2 {
                let message = run_req(&req).await;
                assert!(
                    matches!(
                        message,
                        Message::Assistant {
                            stop_reason: StopReason::Stop,
                            ..
                        }
                    ),
                    "{message:?}"
                );
            }
            let requests = server.requests();
            assert_eq!(requests.len(), 3);
            if kind == "executable" {
                let form: std::collections::HashMap<_, _> =
                    url::form_urlencoded::parse(&requests[0].body)
                        .into_owned()
                        .collect();
                assert_eq!(
                    form.get("subject_token").map(String::as_str),
                    Some("executable-subject")
                );
            }
            for model in &requests[1..] {
                assert_eq!(
                    model.header("authorization"),
                    Some("Bearer federated-token")
                );
                assert_eq!(model.header("x-goog-user-project"), Some("billing"));
            }
        }
    }

    #[tokio::test]
    async fn metadata_and_impersonation_credentials_reach_vertex_streams() {
        for impersonated in [false, true] {
            let server = MockServer::start(|req, _| {
                if req.path.starts_with("/computeMetadata/") {
                    MockResponse::json(200, &json!({"access_token":"metadata-token", "expires_in":3600}))
                        .with_header("Metadata-Flavor", "Google")
                } else if req.path == "/token" {
                    MockResponse::json(200, &json!({"access_token":"source-token", "expires_in":3600}))
                } else if req.path.ends_with(":generateAccessToken") {
                    MockResponse::json(200, &json!({"accessToken":"impersonated-token", "expireTime":"2099-01-01T00:00:00Z"}))
                } else {
                    MockResponse::sse(vec![OK.into()])
                }
            }).await;
            let dir = tempfile::tempdir().unwrap();
            let mut req = request(vertex("gemini-2.5-pro", &format!("{}/vx", server.url)));
            req.credential = if impersonated {
                Credential::None
            } else {
                Credential::ApiKey(GCP_VERTEX_CREDENTIALS_MARKER.into())
            };
            req.env.clear();
            req.options
                .insert(crate::provider::ISOLATED_ENV.into(), json!(true));
            req.options.insert("project".into(), json!("project"));
            req.options.insert("location".into(), json!("us-central1"));
            req.env
                .insert("HOME".into(), dir.path().display().to_string());
            req.env
                .insert("GCE_METADATA_HOST".into(), server.url.clone());
            req.env
                .insert("GOOGLE_CLOUD_QUOTA_PROJECT".into(), "billing".into());
            if impersonated {
                let reference: Value = serde_json::from_str(include_str!(
                    "../../../../../fixtures/native-google-adc/reference.json"
                ))
                .unwrap();
                let mut document = reference["cases"][0]["document"].clone();
                document["endpoint"] = json!(server.url);
                let file = dir.path().join("adc.json");
                std::fs::write(&file, document.to_string()).unwrap();
                req.env.insert(
                    "GOOGLE_APPLICATION_CREDENTIALS".into(),
                    file.display().to_string(),
                );
                req.options.insert(
                    "googleTokenUrl".into(),
                    json!(format!("{}/token", server.url)),
                );
            }
            for _ in 0..2 {
                let message = run_req(&req).await;
                assert!(
                    matches!(
                        message,
                        Message::Assistant {
                            stop_reason: StopReason::Stop,
                            ..
                        }
                    ),
                    "{message:?}"
                );
            }
            let requests = server.requests();
            assert_eq!(requests.len(), if impersonated { 4 } else { 3 });
            for model in &requests[requests.len() - 2..] {
                assert_eq!(
                    model.header("authorization"),
                    Some(if impersonated {
                        "Bearer impersonated-token"
                    } else {
                        "Bearer metadata-token"
                    })
                );
                assert_eq!(model.header("x-goog-user-project"), Some("billing"));
                assert!(model.path.starts_with("/vx/"));
            }
        }
    }

    #[tokio::test]
    async fn service_account_adc_exchanges_a_signed_jwt_and_caches_the_token() {
        let server = MockServer::start(|r, _| {
            if r.path == "/token" {
                MockResponse::json(
                    200,
                    &json!({"access_token": "ya29.sa", "expires_in": 3600, "token_type": "Bearer"}),
                )
            } else {
                MockResponse::sse(vec![OK.into()])
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sa.json");
        std::fs::write(
            &path,
            json!({"type": "service_account", "client_email": "svc@proj.iam.gserviceaccount.com",
                "private_key": TEST_PRIVATE_KEY, "private_key_id": "kid", "project_id": "proj",
                "quota_project_id": "billing-proj"})
            .to_string(),
        )
        .unwrap();
        let mut req = request(vertex("gemini-2.5-pro", &format!("{}/vx", server.url)));
        req.credential = Credential::Ambient(Default::default());
        req.env.insert(
            "GOOGLE_APPLICATION_CREDENTIALS".into(),
            path.display().to_string(),
        );
        req.env.insert("GOOGLE_CLOUD_PROJECT".into(), "proj".into());
        req.env
            .insert("GOOGLE_CLOUD_LOCATION".into(), "us-central1".into());
        req.options.insert(
            "googleTokenUrl".into(),
            json!(format!("{}/token", server.url)),
        );
        assert_eq!(run_req(&req).await.text(), "hi");
        assert_eq!(run_req(&req).await.text(), "hi");

        let all = server.requests();
        let tokens: Vec<_> = all.iter().filter(|r| r.path == "/token").collect();
        assert_eq!(tokens.len(), 1, "the token is cached");
        let form: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(&tokens[0].body)
                .into_owned()
                .collect();
        assert_eq!(
            form["grant_type"],
            "urn:ietf:params:oauth:grant-type:jwt-bearer"
        );
        let parts: Vec<&str> = form["assertion"].split('.').collect();
        use base64::Engine;
        let claims: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[1])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["iss"], "svc@proj.iam.gserviceaccount.com");
        assert_eq!(claims["scope"], google_auth::CLOUD_PLATFORM_SCOPE);
        assert_eq!(claims["aud"], google_auth::GOOGLE_TOKEN_URL);
        assert_eq!(
            claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(),
            3600
        );
        let calls: Vec<_> = all.iter().filter(|r| r.path != "/token").collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].path,
            "/vx/v1/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        assert_eq!(calls[0].header("authorization"), Some("Bearer ya29.sa"));
        assert_eq!(calls[0].header("x-goog-user-project"), Some("billing-proj"));
    }

    #[tokio::test]
    async fn authorized_user_refresh_and_token_errors() {
        let server = MockServer::start(|r, n| {
            if r.path.starts_with("/token") && n == 0 {
                MockResponse::json(200, &json!({"access_token": "ya29.user", "expires_in": 3599}))
            } else if r.path.starts_with("/token") {
                MockResponse::json(400, &json!({"error": "invalid_grant", "error_description": "Token has been expired or revoked."}))
            } else {
                MockResponse::sse(vec![OK.into()])
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        // Discovered through HOME like `gcloud auth application-default login` writes it.
        let adc = dir.path().join(".config/gcloud");
        std::fs::create_dir_all(&adc).unwrap();
        std::fs::write(
            adc.join("application_default_credentials.json"),
            json!({"type": "authorized_user", "client_id": "cid.apps.googleusercontent.com",
                "client_secret": "csecret", "refresh_token": "1//refresh"})
            .to_string(),
        )
        .unwrap();
        let mut req = request(vertex("gemini-2.5-pro", &server.url));
        req.credential = Credential::None;
        req.env
            .insert("HOME".into(), dir.path().display().to_string());
        req.env
            .insert("GOOGLE_APPLICATION_CREDENTIALS".into(), String::new());
        req.options.insert("project".into(), json!("p1"));
        req.options.insert("location".into(), json!("europe-west4"));
        req.options.insert(
            "googleTokenUrl".into(),
            json!(format!("{}/token", server.url)),
        );
        // An empty request env value masks the process environment, so HOME's file is used.
        assert_eq!(run_req(&req).await.text(), "hi");
        let all = server.requests();
        let form: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(&all[0].body)
                .into_owned()
                .collect();
        assert_eq!(
            form,
            [
                ("grant_type", "refresh_token"),
                ("refresh_token", "1//refresh"),
                ("client_id", "cid.apps.googleusercontent.com"),
                ("client_secret", "csecret"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
        );
        assert_eq!(all[1].header("authorization"), Some("Bearer ya29.user"));

        // A different token endpoint is a different cache entry: the failing exchange surfaces.
        req.options.insert(
            "googleTokenUrl".into(),
            json!(format!("{}/token?second", server.url)),
        );
        let m = run_req(&req).await;
        assert!(
            matches!(&m, Message::Assistant { stop_reason: StopReason::Error,
            error_message: Some(e), .. }
            if e == "400: invalid_grant: Token has been expired or revoked."),
            "{m:?}"
        );
        assert_eq!(server.requests().len(), 3);
    }

    #[tokio::test]
    async fn missing_credentials_file_fails_without_a_model_request() {
        let server = MockServer::start(|_, _| MockResponse::sse(vec![OK.into()])).await;
        let dir = tempfile::tempdir().unwrap();
        let mut req = request(vertex("gemini-2.5-pro", &server.url));
        req.credential = Credential::None;
        req.env.insert(
            "GOOGLE_APPLICATION_CREDENTIALS".into(),
            dir.path().join("missing.json").display().to_string(),
        );
        req.env.insert("GOOGLE_CLOUD_PROJECT".into(), "p".into());
        req.env
            .insert("GOOGLE_CLOUD_LOCATION".into(), "global".into());
        let m = run_req(&req).await;
        assert!(
            matches!(&m, Message::Assistant { stop_reason: StopReason::Error,
            error_message: Some(e), .. } if e == google_auth::NO_CREDENTIALS)
        );
        assert!(server.requests().is_empty());
    }
}
