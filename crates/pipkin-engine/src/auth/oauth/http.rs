//! Minimal HTTP helpers for OAuth endpoints. Bodies may contain tokens: never logged.

use reqwest::RequestBuilder;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::CANCELLED;

pub struct Resp {
    pub status: u16,
    pub text: String,
}

impl Resp {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as JSON (`Null` when it is not JSON).
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    /// The body as a JSON object (empty when it is not one).
    pub fn object(&self) -> Value {
        match self.json() {
            v @ Value::Object(_) => v,
            _ => Value::Object(Default::default()),
        }
    }
}

/// Send and read the whole body. Cancellation wins with "Login cancelled".
pub async fn send(rb: RequestBuilder, cancel: Option<&CancellationToken>) -> Result<Resp, String> {
    let fut = async {
        let resp = rb
            .send()
            .await
            .map_err(|e| crate::provider::http::transport_error(&e))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| crate::provider::http::transport_error(&e))?;
        Ok(Resp { status, text })
    };
    match cancel {
        Some(c) => tokio::select! {
            _ = c.cancelled() => Err(CANCELLED.into()),
            r = fut => r,
        },
        None => fut.await,
    }
}

/// `application/x-www-form-urlencoded` body (`URLSearchParams` encoding).
pub fn form(pairs: &[(&str, &str)]) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    s.finish()
}

pub fn post_form(client: &reqwest::Client, url: &str, pairs: &[(&str, &str)]) -> RequestBuilder {
    client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(form(pairs))
}

pub fn post_json(client: &reqwest::Client, url: &str, body: &Value) -> RequestBuilder {
    client
        .post(url)
        .header("Content-Type", "application/json")
        .body(body.to_string())
}

/// `url` with query parameters appended (`URLSearchParams` encoding).
pub fn with_query(base: &str, pairs: &[(&str, &str)]) -> Result<String, String> {
    let mut url = url::Url::parse(base).map_err(|e| format!("Invalid URL {base}: {e}"))?;
    {
        let mut q = url.query_pairs_mut();
        q.clear();
        for (k, v) in pairs {
            q.append_pair(k, v);
        }
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn oauth_reports_invalid_proxy_before_sending_authorization_code() {
        use crate::provider::{
            http,
            mock::{MockResponse, MockServer},
            proxy::ProxyConfig,
        };
        let target =
            MockServer::start(|_, _| MockResponse::json(200, &serde_json::json!({}))).await;
        for proxy in [
            "socks5://127.0.0.1:1080",
            "pac+http://wpad/proxy.pac",
            "http://[bad",
        ] {
            let settings = http::NetSettings {
                proxy: ProxyConfig::from_lookup(
                    |key| (key == "HTTP_PROXY").then(|| proxy.into()),
                    None,
                ),
                ..Default::default()
            };
            let expected = settings
                .proxy
                .proxy_for(&url::Url::parse(&target.url).unwrap())
                .unwrap_err();
            let client = http::client_with(&settings);
            let error = super::send(
                super::post_form(&client, &target.url, &[("code", "synthetic-secret")]),
                None,
            )
            .await
            .err()
            .unwrap();
            assert_eq!(error, expected);
        }
        assert!(target.requests().is_empty());
    }
}
