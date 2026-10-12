//! Search providers. Each turns a query into normalized [`SearchResult`]s or a classified
//! [`SearchError`]; none invents a publication date (a date is carried only when the provider
//! returned one, as it returned it).
//!
//! - **Brave Search API** (documented, API key): `GET
//!   https://api.search.brave.com/res/v1/web/search?q=…&count=…` with the
//!   `X-Subscription-Token` header; results under `web.results[]` with `title`, `url` and
//!   `description` (per the Brave Web Search "get started" documentation, read 2026-10-12).
//!   `page_age`/`age` are carried verbatim when present; the documentation read does not
//!   describe them.
//! - **SearXNG** (self-hosted): `GET {base}/search?q=…&format=json` (JSON output must be
//!   enabled in the instance's `settings.yml`; otherwise it answers 403). Result fields
//!   `results[].{title,url,content,publishedDate}` are taken from SearXNG's own JSON output,
//!   not from its API documentation, which does not list them.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::provider::BoxFuture;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub url: String,
    pub title: String,
    pub excerpt: String,
    /// As the provider reported it, verbatim; never derived.
    pub published: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchError {
    NotConfigured(String),
    Auth(String),
    RateLimited(String),
    Network(String),
    InvalidResponse(String),
    Cancelled,
}

impl SearchError {
    pub fn kind(&self) -> &'static str {
        match self {
            SearchError::NotConfigured(_) => "not_configured",
            SearchError::Auth(_) => "auth",
            SearchError::RateLimited(_) => "rate_limited",
            SearchError::Network(_) => "network",
            SearchError::InvalidResponse(_) => "invalid_response",
            SearchError::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::NotConfigured(m) => write!(f, "search is not configured: {m}"),
            SearchError::Auth(m) => write!(f, "the search provider refused the credentials: {m}"),
            SearchError::RateLimited(m) => {
                write!(f, "the search provider is rate limiting requests: {m}")
            }
            SearchError::Network(m) => write!(f, "the search request failed: {m}"),
            SearchError::InvalidResponse(m) => {
                write!(f, "the search provider's answer could not be read: {m}")
            }
            SearchError::Cancelled => f.write_str("the search was cancelled"),
        }
    }
}

pub trait SearchProvider: Send + Sync {
    /// The provider's name as shown in results and citations.
    fn name(&self) -> &'static str;
    /// The host this provider talks to; a local self-hosted instance is allowed for search
    /// only, never for page fetches.
    fn endpoint(&self) -> &url::Url;
    fn search<'a>(
        &'a self,
        http: &'a reqwest::Client,
        query: &'a str,
        count: usize,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<SearchResult>, SearchError>>;
}

/// Longest excerpt kept from a provider.
pub const EXCERPT_CHARS: usize = 500;
const TITLE_CHARS: usize = 300;

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

async fn get_json(
    http: &reqwest::Client,
    request: reqwest::RequestBuilder,
    cancel: CancellationToken,
) -> Result<Value, SearchError> {
    let _ = http;
    let send = async {
        let response = request
            .send()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(SearchError::Auth(crate::provider::http::format_error(
                Some(status),
                &body,
                "",
            )));
        }
        if status.as_u16() == 429 {
            return Err(SearchError::RateLimited(
                crate::provider::http::format_error(Some(status), &body, ""),
            ));
        }
        if !status.is_success() {
            return Err(SearchError::Network(crate::provider::http::format_error(
                Some(status),
                &body,
                "",
            )));
        }
        serde_json::from_str::<Value>(&body)
            .map_err(|e| SearchError::InvalidResponse(format!("not JSON: {e}")))
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(SearchError::Cancelled),
        result = send => result,
    }
}

/// Keep the first occurrence of each URL.
pub fn dedupe(results: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut seen = std::collections::HashSet::new();
    results
        .into_iter()
        .filter(|r| seen.insert(r.url.clone()))
        .collect()
}

pub struct Brave {
    pub endpoint: url::Url,
    pub api_key: String,
}

impl Brave {
    pub const ENDPOINT: &'static str = "https://api.search.brave.com/res/v1/web/search";
    /// The documented upper bound of `count`.
    pub const MAX_COUNT: usize = 20;
}

impl SearchProvider for Brave {
    fn name(&self) -> &'static str {
        "brave"
    }

    fn endpoint(&self) -> &url::Url {
        &self.endpoint
    }

    fn search<'a>(
        &'a self,
        http: &'a reqwest::Client,
        query: &'a str,
        count: usize,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<SearchResult>, SearchError>> {
        Box::pin(async move {
            let request = http
                .get(self.endpoint.clone())
                .query(&[
                    ("q", query),
                    ("count", &count.min(Self::MAX_COUNT).to_string()),
                ])
                .header("Accept", "application/json")
                .header("X-Subscription-Token", &self.api_key);
            let json = get_json(http, request, cancel).await?;
            let Some(results) = json.pointer("/web/results").and_then(Value::as_array) else {
                // No `web` section is a valid empty answer only when the object is otherwise
                // well formed; anything else is unreadable.
                if json.is_object() {
                    return Ok(vec![]);
                }
                return Err(SearchError::InvalidResponse("no web.results".into()));
            };
            Ok(dedupe(
                results
                    .iter()
                    .filter_map(|r| {
                        let url = str_of(r, "url")?;
                        Some(SearchResult {
                            url: url.to_owned(),
                            title: clip(str_of(r, "title").unwrap_or(url), TITLE_CHARS),
                            excerpt: clip(str_of(r, "description").unwrap_or(""), EXCERPT_CHARS),
                            published: str_of(r, "page_age")
                                .or_else(|| str_of(r, "age"))
                                .map(|s| clip(s, 64)),
                        })
                    })
                    .take(count)
                    .collect(),
            ))
        })
    }
}

pub struct Searxng {
    pub endpoint: url::Url,
}

impl SearchProvider for Searxng {
    fn name(&self) -> &'static str {
        "searxng"
    }

    fn endpoint(&self) -> &url::Url {
        &self.endpoint
    }

    fn search<'a>(
        &'a self,
        http: &'a reqwest::Client,
        query: &'a str,
        count: usize,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<SearchResult>, SearchError>> {
        Box::pin(async move {
            let mut url = self.endpoint.clone();
            url.set_query(None);
            if !url.path().ends_with("/search") {
                let path = format!("{}/search", url.path().trim_end_matches('/'));
                url.set_path(&path);
            }
            let request = http
                .get(url)
                .query(&[("q", query), ("format", "json")])
                .header("Accept", "application/json");
            let json = get_json(http, request, cancel).await?;
            let Some(results) = json.get("results").and_then(Value::as_array) else {
                return Err(SearchError::InvalidResponse("no results array".into()));
            };
            Ok(dedupe(
                results
                    .iter()
                    .filter_map(|r| {
                        let url = str_of(r, "url")?;
                        Some(SearchResult {
                            url: url.to_owned(),
                            title: clip(str_of(r, "title").unwrap_or(url), TITLE_CHARS),
                            excerpt: clip(str_of(r, "content").unwrap_or(""), EXCERPT_CHARS),
                            published: str_of(r, "publishedDate").map(|s| clip(s, 64)),
                        })
                    })
                    .take(count)
                    .collect(),
            ))
        })
    }
}
