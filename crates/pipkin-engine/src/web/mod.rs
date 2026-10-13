//! Web access: search through a configured provider, fetch pages within the network
//! boundary, retain what was fetched as engine-owned sources, and read them back in bounded
//! slices. The model sees source ids (`s<n>`), never raw storage.
//!
//! Configuration (`settings.json`, user or project level):
//!
//! ```json
//! { "web": { "search": { "provider": "brave", "apiKey": "…", "apiKeyEnv": "BRAVE_SEARCH_API_KEY" } } }
//! { "web": { "search": { "provider": "searxng", "url": "http://search.lan:8080" } } }
//! ```
//!
//! `apiKeyEnv` names an environment variable holding the key; `apiKey` is the key itself.
//! Neither is ever shown in results, snapshots or logs. Without a provider, `web_search`
//! fails with a typed `not_configured` error that says what to set; `web_fetch` works
//! regardless.
//!
//! Fetched and searched content is untrusted evidence: nothing in it changes engine
//! configuration or is executed. The network boundary ([`policy`]) applies to every fetch
//! and every redirect hop; a configured self-hosted search instance may be on the local
//! network, but fetched URLs may not reach it or any other local or private target.

pub mod extract;
pub mod policy;
pub mod providers;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::Settings;
use crate::provider::http::NetSettings;
use crate::store::Store;
use providers::{SearchError, SearchProvider};

/// Most bytes downloaded for one page.
pub const FETCH_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Most characters of extracted text retained per source.
pub const RETAINED_TEXT_CHARS: usize = 1024 * 1024;
/// Most original bytes retained per source (text and markup only).
pub const RETAINED_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Most sources with retained content per session; older ones expire (metadata stays).
pub const MAX_RETAINED_SOURCES: usize = 200;
/// Most retained bytes per session.
pub const MAX_RETAINED_BYTES: u64 = 64 * 1024 * 1024;
/// Redirect hops followed.
pub const MAX_REDIRECTS: usize = 5;
/// Whole-fetch deadline.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Search deadline.
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Results per search by default and at most.
pub const DEFAULT_SEARCH_COUNT: usize = 5;
pub const MAX_SEARCH_COUNT: usize = 10;
/// Characters of text `web_fetch` returns inline; the rest is read with `web_read`.
pub const FETCH_PREVIEW_CHARS: usize = 8 * 1024;
/// Default and largest `web_read` slice.
pub const READ_DEFAULT_CHARS: usize = 8 * 1024;
pub const READ_MAX_CHARS: usize = 32 * 1024;
/// Most `find` matches returned, with this much context on each side.
pub const FIND_MAX_MATCHES: usize = 20;
pub const FIND_CONTEXT_CHARS: usize = 160;

/// A retained source record (content held separately).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRecord {
    pub id: u64,
    /// `search` or `fetch`.
    pub kind: String,
    pub provider: String,
    pub requested_url: String,
    pub final_url: String,
    pub title: Option<String>,
    pub excerpt: Option<String>,
    /// As supplied by the provider or the page, verbatim.
    pub published: Option<String>,
    pub retrieved_at_ms: i64,
    pub content_type: Option<String>,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub text_chars: u64,
    pub expired: bool,
    pub query: Option<String>,
    pub call_id: String,
}

impl SourceRecord {
    pub fn label(&self) -> String {
        format!("s{}", self.id)
    }
}

/// What a fetch produced before storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub requested_url: String,
    pub final_url: String,
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
    pub text: String,
    pub title: Option<String>,
    pub published: Option<String>,
    /// Decoding replaced invalid sequences or guessed the charset.
    pub lossy: bool,
    pub redirects: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    Blocked(String),
    Network(String),
    Status(u16, String),
    UnsupportedContent(String),
    TooLarge(usize),
    TooManyRedirects,
    Timeout,
    Cancelled,
}

impl FetchError {
    pub fn kind(&self) -> &'static str {
        match self {
            FetchError::Blocked(_) => "blocked",
            FetchError::Network(_) => "network",
            FetchError::Status(..) => "status",
            FetchError::UnsupportedContent(_) => "unsupported_content",
            FetchError::TooLarge(_) => "too_large",
            FetchError::TooManyRedirects => "too_many_redirects",
            FetchError::Timeout => "timeout",
            FetchError::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Blocked(m) => write!(f, "fetch refused: {m}"),
            FetchError::Network(m) => write!(f, "fetch failed: {m}"),
            FetchError::Status(code, m) => write!(f, "the server answered {code}: {m}"),
            FetchError::UnsupportedContent(t) => {
                write!(
                    f,
                    "unsupported content type {t}; only text, HTML, JSON and XML are read"
                )
            }
            FetchError::TooLarge(max) => write!(f, "the response exceeds the {max}-byte limit"),
            FetchError::TooManyRedirects => write!(f, "more than {MAX_REDIRECTS} redirects"),
            FetchError::Timeout => f.write_str("the fetch timed out"),
            FetchError::Cancelled => f.write_str("the fetch was cancelled"),
        }
    }
}

/// How search is configured for a session (from merged settings).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebSettings {
    pub provider: Option<String>,
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    /// Providers tried in order when the one before fails (`web.search.fallback`, an array
    /// of the same shape). A fallback never has fallbacks of its own.
    pub fallbacks: Vec<WebSettings>,
}

/// Most providers in a chain (the primary plus fallbacks).
pub const MAX_PROVIDER_CHAIN: usize = 4;
/// The providers to try in order, and warnings about fallbacks that were skipped.
pub type ProviderChain = (Vec<Arc<dyn SearchProvider>>, Vec<String>);

impl WebSettings {
    pub fn from_settings(settings: &Settings) -> WebSettings {
        let search = settings
            .other
            .get("web")
            .and_then(|w| w.get("search"))
            .cloned()
            .unwrap_or_default();
        let mut parsed = Self::from_value(&search);
        parsed.fallbacks = search
            .get("fallback")
            .and_then(serde_json::Value::as_array)
            .map(|list| {
                list.iter()
                    .filter(|v| v.is_object())
                    .take(MAX_PROVIDER_CHAIN - 1)
                    .map(Self::from_value)
                    .collect()
            })
            .unwrap_or_default();
        parsed
    }

    fn from_value(search: &serde_json::Value) -> WebSettings {
        let s = |k: &str| {
            search
                .get(k)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        WebSettings {
            provider: s("provider").map(|p| p.to_ascii_lowercase()),
            url: s("url"),
            api_key: s("apiKey"),
            api_key_env: s("apiKeyEnv"),
            fallbacks: Vec::new(),
        }
    }

    /// The primary provider followed by each configured fallback. A misconfigured fallback is
    /// reported as a warning and skipped; a misconfigured primary is the error.
    pub fn providers(
        &self,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<ProviderChain, SearchError> {
        let mut chain = vec![self.provider(&env)?];
        let mut warnings = Vec::new();
        for (i, fallback) in self.fallbacks.iter().enumerate() {
            match fallback.provider(&env) {
                Ok(p) => chain.push(p),
                Err(e) => warnings.push(format!("fallback {} skipped: {e}", i + 1)),
            }
        }
        Ok((chain, warnings))
    }

    /// Build the configured provider, reading the key from the environment when named.
    pub fn provider(
        &self,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Arc<dyn SearchProvider>, SearchError> {
        match self.provider.as_deref() {
            None => Err(SearchError::NotConfigured(
                "set web.search.provider to \"brave\" (with apiKey or apiKeyEnv) or \"searxng\" (with url) in settings.json".into(),
            )),
            Some("brave") => {
                let key = self
                    .api_key
                    .clone()
                    .or_else(|| self.api_key_env.as_deref().and_then(&env))
                    .ok_or_else(|| {
                        SearchError::NotConfigured(
                            "the Brave provider needs web.search.apiKey, or apiKeyEnv naming an environment variable that holds the key".into(),
                        )
                    })?;
                let endpoint = match &self.url {
                    Some(url) => Url::parse(url).map_err(|e| {
                        SearchError::NotConfigured(format!("web.search.url is not a URL: {e}"))
                    })?,
                    None => Url::parse(providers::Brave::ENDPOINT).expect("constant"),
                };
                Ok(Arc::new(providers::Brave {
                    endpoint,
                    api_key: key,
                }))
            }
            Some("searxng") => {
                let url = self.url.as_deref().ok_or_else(|| {
                    SearchError::NotConfigured(
                        "the SearXNG provider needs web.search.url (the instance's base URL)".into(),
                    )
                })?;
                let endpoint = Url::parse(url).map_err(|e| {
                    SearchError::NotConfigured(format!("web.search.url is not a URL: {e}"))
                })?;
                if !matches!(endpoint.scheme(), "http" | "https") {
                    return Err(SearchError::NotConfigured(
                        "web.search.url must be an http or https URL".into(),
                    ));
                }
                Ok(Arc::new(providers::Searxng { endpoint }))
            }
            Some(other) => Err(SearchError::NotConfigured(format!(
                "unknown web.search.provider {other:?} (brave, searxng)"
            ))),
        }
    }
}

fn readable_content_type(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime.starts_with("text/")
        || matches!(
            mime.as_str(),
            "application/json"
                | "application/xml"
                | "application/xhtml+xml"
                | "application/javascript"
                | "application/ld+json"
                | "application/rss+xml"
                | "application/atom+xml"
        )
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
}

fn is_html(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime == "text/html" || mime == "application/xhtml+xml"
}

/// Fetch one URL within the boundary: every hop checked and pinned, redirects followed
/// manually, the body bounded, then decoded and extracted off the async worker.
pub async fn fetch(
    net: &NetSettings,
    requested: &str,
    allow_private: bool,
    cancel: CancellationToken,
) -> Result<Fetched, FetchError> {
    let work = fetch_inner(net, requested, allow_private);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(FetchError::Cancelled),
        result = tokio::time::timeout(FETCH_TIMEOUT, work) => match result {
            Ok(r) => r,
            Err(_) => Err(FetchError::Timeout),
        },
    }
}

async fn fetch_inner(
    net: &NetSettings,
    requested: &str,
    allow_private: bool,
) -> Result<Fetched, FetchError> {
    let mut url =
        Url::parse(requested.trim()).map_err(|e| FetchError::Blocked(format!("not a URL: {e}")))?;
    let mut redirects = Vec::new();
    let mut hops = 0;
    loop {
        let addrs = policy::resolve_checked(&url, allow_private)
            .await
            .map_err(|b| FetchError::Blocked(b.to_string()))?;
        let host = url.host_str().unwrap_or_default().to_owned();
        let client = crate::provider::http::builder(net)
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(&host, &addrs)
            .build()
            .map_err(|e| FetchError::Network(e.to_string()))?;
        let response = client
            .get(url.clone())
            .header(
                "Accept",
                "text/html, application/xhtml+xml, application/json, text/plain;q=0.9, */*;q=0.5",
            )
            .header("Accept-Encoding", "identity")
            .send()
            .await
            .map_err(|e| FetchError::Network(e.to_string()))?;
        let status = response.status();
        if status.is_redirection() {
            let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            else {
                return Err(FetchError::Status(
                    status.as_u16(),
                    "redirect without a location".into(),
                ));
            };
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err(FetchError::TooManyRedirects);
            }
            let next = policy::redirect_target(&url, location)
                .map_err(|b| FetchError::Blocked(b.to_string()))?;
            redirects.push(url.to_string());
            url = next;
            continue;
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        if let Some(encoding) = response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            && !encoding.eq_ignore_ascii_case("identity")
        {
            return Err(FetchError::UnsupportedContent(format!(
                "content-encoding {encoding} (compressed responses are not decoded)"
            )));
        }
        if let Some(length) = response.content_length()
            && length > FETCH_MAX_BYTES as u64
        {
            return Err(FetchError::TooLarge(FETCH_MAX_BYTES));
        }
        let mut bytes: Vec<u8> = Vec::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::Network(e.to_string()))?
        {
            if bytes.len() + chunk.len() > FETCH_MAX_BYTES {
                return Err(FetchError::TooLarge(FETCH_MAX_BYTES));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let preview = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).into_owned();
            return Err(FetchError::Status(
                status.as_u16(),
                preview.trim().to_owned(),
            ));
        }
        let ct = content_type.clone().unwrap_or_else(|| "text/plain".into());
        if !readable_content_type(&ct) {
            return Err(FetchError::UnsupportedContent(
                ct.split(';').next().unwrap_or(&ct).trim().to_owned(),
            ));
        }
        let final_url = url.to_string();
        let requested = requested.trim().to_owned();
        let header_charset = extract::charset_param(&ct);
        let html = is_html(&ct);
        // Decoding and extraction are CPU work: off the async worker.
        let extracted = tokio::task::spawn_blocking(move || {
            let charset = header_charset.or_else(|| {
                html.then(|| {
                    // Sniff a meta charset from the first part of the document.
                    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
                    extract::html_to_text(&head, 1).meta_charset
                })
                .flatten()
            });
            let (decoded, lossy) = extract::decode_body(&bytes, charset.as_deref());
            let (text, title, published) = if html {
                let e = extract::html_to_text(&decoded, RETAINED_TEXT_CHARS);
                (e.text, e.title, e.published)
            } else {
                let text: String = decoded.chars().take(RETAINED_TEXT_CHARS).collect();
                (text, None, None)
            };
            (bytes, text, title, published, lossy)
        })
        .await
        .map_err(|e| FetchError::Network(format!("extraction failed: {e}")))?;
        let (bytes, text, title, published, lossy) = extracted;
        return Ok(Fetched {
            requested_url: requested,
            final_url,
            content_type,
            bytes,
            text,
            title,
            published,
            lossy,
            redirects,
        });
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// One `find` match: the character offset and the surrounding text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindMatch {
    pub offset: usize,
    pub context: String,
}

/// A bounded slice of retained text by character offset.
pub fn slice_chars(text: &str, offset: usize, limit: usize) -> (String, usize, bool) {
    let total = text.chars().count();
    let limit = limit.clamp(1, READ_MAX_CHARS);
    let piece: String = text.chars().skip(offset).take(limit).collect();
    let end = (offset + piece.chars().count()).min(total);
    (piece, total, end < total)
}

/// Case-insensitive `find` with context, by character offsets.
pub fn find_in(text: &str, needle: &str) -> (Vec<FindMatch>, usize) {
    let needle = needle.trim();
    if needle.is_empty() {
        return (vec![], 0);
    }
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let pat: Vec<char> = needle.to_lowercase().chars().collect();
    // Lowercasing can change lengths for a few scripts; fall back to byte search then.
    if lower.len() != chars.len() {
        let hay = text.to_lowercase();
        let pat_s = needle.to_lowercase();
        let mut matches = Vec::new();
        let mut total = 0;
        let mut from = 0;
        while let Some(at) = hay[from..].find(&pat_s) {
            total += 1;
            let start = from + at;
            if matches.len() < FIND_MAX_MATCHES {
                let offset = hay[..start].chars().count();
                let context: String = text
                    .chars()
                    .skip(offset.saturating_sub(FIND_CONTEXT_CHARS))
                    .take(FIND_CONTEXT_CHARS * 2 + pat_s.chars().count())
                    .collect();
                matches.push(FindMatch { offset, context });
            }
            from = start + pat_s.len().max(1);
        }
        return (matches, total);
    }
    let mut matches = Vec::new();
    let mut total = 0;
    let mut i = 0;
    while i + pat.len() <= lower.len() {
        if lower[i..i + pat.len()] == pat[..] {
            total += 1;
            if matches.len() < FIND_MAX_MATCHES {
                let start = i.saturating_sub(FIND_CONTEXT_CHARS);
                let end = (i + pat.len() + FIND_CONTEXT_CHARS).min(chars.len());
                matches.push(FindMatch {
                    offset: i,
                    context: chars[start..end]
                        .iter()
                        .collect::<String>()
                        .replace('\n', " "),
                });
            }
            i += pat.len();
        } else {
            i += 1;
        }
    }
    (matches, total)
}

/// Reads an environment variable by name (for `apiKeyEnv`).
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The engine's web service for one session's calls: configured search, bounded fetch and
/// retained sources in the store.
pub struct WebService {
    pub store: Arc<Store>,
    pub net: NetSettings,
    pub http: reqwest::Client,
    pub settings: WebSettings,
    pub env: EnvLookup,
    pub allow_private: bool,
}

impl WebService {
    /// Search, record each result as a `search` source and return the records.
    pub async fn search(
        &self,
        session: &str,
        call_id: &str,
        query: &str,
        count: usize,
        cancel: CancellationToken,
    ) -> Result<(String, Vec<SourceRecord>), SearchError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(SearchError::InvalidResponse("the query is empty".into()));
        }
        let (chain, warnings) = self.settings.providers(|k| (self.env)(k))?;
        for warning in &warnings {
            log::warn!("web search: {warning}");
        }
        let count = count.clamp(1, MAX_SEARCH_COUNT);
        // The chain: a provider that is down, refusing or rate-limited hands over to the
        // next; a cancellation or the last failure is the outcome.
        let mut last = None;
        let mut used: Option<Arc<dyn SearchProvider>> = None;
        let mut results = Vec::new();
        for provider in chain {
            let attempt = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(SearchError::Cancelled),
                r = tokio::time::timeout(SEARCH_TIMEOUT, provider.search(&self.http, query, count, cancel.clone())) => match r {
                    Ok(r) => r,
                    Err(_) => Err(SearchError::Network("the search timed out".into())),
                },
            };
            match attempt {
                Ok(found) => {
                    results = found;
                    used = Some(provider);
                    break;
                }
                Err(SearchError::Cancelled) => return Err(SearchError::Cancelled),
                Err(e) => {
                    log::warn!("web search via {} failed: {e}", provider.name());
                    last = Some(e);
                }
            }
        }
        let Some(provider) = used else {
            return Err(last.unwrap_or_else(|| SearchError::NotConfigured("no provider".into())));
        };
        let now = crate::util::now_ms();
        let name = provider.name().to_owned();
        let records = self
            .store
            .tx(|tx| Store::record_search(tx, session, call_id, &name, query, &results, now))
            .map_err(|e| SearchError::Network(format!("could not record sources: {e}")))?;
        Ok((name, records))
    }

    /// Fetch and retain a page.
    pub async fn fetch(
        &self,
        session: &str,
        call_id: &str,
        url: &str,
        cancel: CancellationToken,
    ) -> Result<(SourceRecord, Fetched), FetchError> {
        let fetched = fetch(&self.net, url, self.allow_private, cancel).await?;
        let now = crate::util::now_ms();
        let record = self
            .store
            .tx(|tx| Store::record_fetch(tx, session, call_id, &fetched, now))
            .map_err(|e| FetchError::Network(format!("could not retain the source: {e}")))?;
        Ok((record, fetched))
    }

    pub fn source(&self, session: &str, id: u64) -> Result<Option<SourceRecord>, String> {
        self.store
            .read(|c| Store::web_source(c, session, id))
            .map_err(|e| e.to_string())
    }

    pub fn text(&self, session: &str, id: u64) -> Result<Option<String>, String> {
        self.store
            .read(|c| Store::web_source_text(c, session, id))
            .map_err(|e| e.to_string())
    }

    pub fn sources(&self, session: &str) -> Result<Vec<SourceRecord>, String> {
        self.store
            .read(|c| Store::web_sources(c, session))
            .map_err(|e| e.to_string())
    }
}

/// Parse `s12` (or `12`) into a source id.
pub fn parse_source_id(s: &str) -> Option<u64> {
    let s = s.trim();
    let digits = s
        .strip_prefix('s')
        .or_else(|| s.strip_prefix('S'))
        .unwrap_or(s);
    digits.parse::<u64>().ok().filter(|n| *n > 0)
}
