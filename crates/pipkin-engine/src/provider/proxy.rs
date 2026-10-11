//! Proxy selection (reference `ai/src/utils/node-http-proxy.ts` and
//! `coding-agent/src/core/http-dispatcher.ts` `applyHttpProxySettings`).
//!
//! The configuration is read from an explicit lookup (the engine's [`Env`], or a request's
//! provider env), never from the real process environment behind the engine's back, so an
//! isolated engine never picks up ambient proxies. Lowercase names win over uppercase ones
//! (`https_proxy` before `HTTPS_PROXY`); settings `httpProxy` fills `HTTP_PROXY` and
//! `HTTPS_PROXY` when they are unset. A target is proxied through `<scheme>_proxy`, else
//! `all_proxy`, unless `no_proxy` excludes it (`*`, exact host, domain suffix with an optional
//! leading `.` or `*.`, optional `:port`, bracketed IPv6). Only `http://` and `https://` proxy
//! URLs are supported: SOCKS and PAC URLs are rejected with the reference message.

use std::collections::HashMap;

use url::Url;

use crate::config::Env;

/// Reference `UNSUPPORTED_PROXY_PROTOCOL_MESSAGE`.
pub const UNSUPPORTED_PROXY_PROTOCOL_MESSAGE: &str = "Unsupported proxy protocol. SOCKS and PAC proxy URLs are not supported; use an HTTP or HTTPS proxy URL.";

/// The proxy variables in effect, already resolved lowercase-before-uppercase.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyConfig {
    http: Option<String>,
    https: Option<String>,
    all: Option<String>,
    no_proxy: Option<String>,
}

impl ProxyConfig {
    /// Resolve from a variable lookup plus settings `httpProxy` (global settings only, as the
    /// reference applies it before anything reads the environment).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>, http_proxy: Option<&str>) -> Self {
        let pick = |key: &str| {
            get(&key.to_ascii_lowercase())
                .filter(|v| !v.is_empty())
                .or_else(|| get(&key.to_ascii_uppercase()).filter(|v| !v.is_empty()))
        };
        let setting = http_proxy
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned);
        // `process.env.HTTP_PROXY ??= proxy`: the setting stands in for the uppercase name only.
        let with_setting = |key: &str| {
            get(&key.to_ascii_lowercase())
                .filter(|v| !v.is_empty())
                .or_else(|| match get(&key.to_ascii_uppercase()) {
                    Some(v) => Some(v).filter(|v| !v.is_empty()),
                    None => setting.clone(),
                })
        };
        ProxyConfig {
            http: with_setting("http_proxy"),
            https: with_setting("https_proxy"),
            all: pick("all_proxy"),
            no_proxy: pick("no_proxy"),
        }
    }

    /// From the engine environment (isolated or ambient) and settings `httpProxy`.
    pub fn from_env(env: &Env, http_proxy: Option<&str>) -> Self {
        ProxyConfig::from_lookup(|k| env.get(k), http_proxy)
    }

    /// From a request's provider env only (what [`ProxyConfig::env_pairs`] put there).
    pub fn from_map(env: &HashMap<String, String>) -> Self {
        ProxyConfig::from_lookup(|k| env.get(k).cloned(), None)
    }

    pub fn is_empty(&self) -> bool {
        self.http.is_none() && self.https.is_none() && self.all.is_none()
    }

    /// The resolved values under their lowercase names (for a request's provider env, so
    /// transports outside the shared HTTP client, like Codex WebSockets, see the same proxy).
    pub fn env_pairs(&self) -> Vec<(String, String)> {
        [
            ("http_proxy", &self.http),
            ("https_proxy", &self.https),
            ("all_proxy", &self.all),
            ("no_proxy", &self.no_proxy),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.clone().map(|v| (k.to_owned(), v)))
        .collect()
    }

    /// Reference `resolveHttpProxyUrlForTarget`: `Ok(None)` connects directly; an invalid or
    /// unsupported proxy URL is an error (the request must not silently bypass the proxy).
    /// `ws`/`wss` targets use the `http`/`https` proxies.
    pub fn proxy_for(&self, target: &Url) -> Result<Option<Url>, String> {
        let protocol = match target.scheme() {
            "ws" => "http",
            "wss" => "https",
            other => other,
        };
        let Some(host) = target.host_str() else {
            return Ok(None);
        };
        let port = target
            .port()
            .or_else(|| default_port(protocol))
            .unwrap_or(0);
        if !should_proxy(self.no_proxy.as_deref().unwrap_or(""), host, port) {
            return Ok(None);
        }
        let scheme_proxy = match protocol {
            "http" => self.http.as_ref(),
            "https" => self.https.as_ref(),
            _ => None,
        };
        let Some(mut proxy) = scheme_proxy.or(self.all.as_ref()).cloned() else {
            return Ok(None);
        };
        if !proxy.contains("://") {
            proxy = format!("{protocol}://{proxy}");
        }
        let url = Url::parse(&proxy)
            .map_err(|e| format!("Invalid proxy URL {}: {e}", serde_json::json!(proxy)))?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(format!(
                "{UNSUPPORTED_PROXY_PROTOCOL_MESSAGE} Got {}:",
                url.scheme()
            ));
        }
        Ok(Some(url))
    }

    /// Problems with the configured proxy URLs (said once when the client is built).
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, value, protocol) in [
            ("http_proxy", &self.http, "http"),
            ("https_proxy", &self.https, "https"),
            ("all_proxy", &self.all, "https"),
        ] {
            let Some(value) = value else { continue };
            let target = Url::parse(&format!("{protocol}://proxy-check.invalid/")).expect("valid");
            let probe = ProxyConfig {
                http: (protocol == "http").then(|| value.clone()),
                https: (protocol == "https").then(|| value.clone()),
                all: None,
                no_proxy: None,
            };
            if let Err(e) = probe.proxy_for(&target) {
                out.push(format!("{name}: {e}"));
            }
        }
        out
    }
}

fn default_port(protocol: &str) -> Option<u16> {
    match protocol {
        "ftp" => Some(21),
        "gopher" => Some(70),
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}

fn strip_brackets(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
}

/// JavaScript `parseInt(text, 10)`: leading sign and digits; `None` for NaN.
fn parse_int(text: &str) -> Option<i64> {
    let t = text.trim_start();
    let (sign, digits) = match t.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, t.strip_prefix('+').unwrap_or(t)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse::<i64>().ok().map(|n| sign * n)
}

/// Reference `parseNoProxyEntry`.
fn parse_no_proxy_entry(entry: &str) -> Option<(String, i64)> {
    let trimmed = entry.trim().to_lowercase();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('[')
        && let Some(close) = trimmed.find(']')
    {
        let host = trimmed[1..close].to_owned();
        let rest = &trimmed[close + 1..];
        return Some(match rest.strip_prefix(':') {
            Some(port) => (host, parse_int(port).unwrap_or(0)),
            None => (host, 0),
        });
    }
    if trimmed.contains(':') && trimmed.split(':').count() > 2 {
        return Some((trimmed, 0));
    }
    if let (Some(last), Some(first)) = (trimmed.rfind(':'), trimmed.find(':'))
        && last == first
        && let Some(port) = parse_int(&trimmed[last + 1..])
    {
        return Some((trimmed[..last].to_owned(), port));
    }
    Some((trimmed, 0))
}

/// Reference `shouldProxyHostname`: false when `no_proxy` excludes `host:port`.
pub fn should_proxy(no_proxy: &str, host: &str, port: u16) -> bool {
    let no_proxy = no_proxy.to_lowercase();
    if no_proxy.is_empty() {
        return true;
    }
    if no_proxy == "*" {
        return false;
    }
    let target = host.to_lowercase();
    let target = strip_brackets(&target);
    no_proxy
        .split(|c: char| c == ',' || c.is_whitespace())
        .all(|entry| {
            let Some((host, entry_port)) = parse_no_proxy_entry(entry) else {
                return true;
            };
            if entry_port != 0 && entry_port != i64::from(port) {
                return true;
            }
            let mut domain = strip_brackets(&host);
            if let Some(rest) = domain.strip_prefix("*.") {
                domain = rest;
            } else if let Some(rest) = domain
                .strip_prefix('.')
                .or_else(|| domain.strip_prefix('*'))
            {
                domain = rest;
            }
            if domain.is_empty() {
                return true;
            }
            target != domain && !target.ends_with(&format!(".{domain}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)], setting: Option<&str>) -> ProxyConfig {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ProxyConfig::from_lookup(|k| map.get(k).cloned(), setting)
    }

    fn proxy(c: &ProxyConfig, target: &str) -> Option<String> {
        c.proxy_for(&Url::parse(target).unwrap())
            .unwrap()
            .map(|u| u.to_string())
    }

    #[test]
    fn scheme_proxies_all_proxy_and_lowercase_precedence() {
        let c = config(
            &[
                ("HTTP_PROXY", "http://upper:1"),
                ("http_proxy", "http://lower:1"),
                ("HTTPS_PROXY", "http://secure:2"),
                ("ALL_PROXY", "http://all:3"),
            ],
            None,
        );
        assert_eq!(
            proxy(&c, "http://api.test/v1").as_deref(),
            Some("http://lower:1/")
        );
        assert_eq!(
            proxy(&c, "https://api.test/v1").as_deref(),
            Some("http://secure:2/")
        );
        assert_eq!(
            proxy(&c, "wss://api.test/v1").as_deref(),
            Some("http://secure:2/")
        );
        let only_all = config(&[("all_proxy", "proxy.test:8080")], None);
        // A proxy without a scheme takes the target's.
        assert_eq!(
            proxy(&only_all, "https://api.test/").as_deref(),
            Some("https://proxy.test:8080/")
        );
        assert_eq!(proxy(&config(&[], None), "https://api.test/"), None);
    }

    #[test]
    fn settings_http_proxy_fills_unset_variables_only() {
        let c = config(&[], Some(" http://setting:9 "));
        assert_eq!(
            proxy(&c, "http://a.test/").as_deref(),
            Some("http://setting:9/")
        );
        assert_eq!(
            proxy(&c, "https://a.test/").as_deref(),
            Some("http://setting:9/")
        );
        let c = config(&[("HTTPS_PROXY", "http://env:1")], Some("http://setting:9"));
        assert_eq!(
            proxy(&c, "https://a.test/").as_deref(),
            Some("http://env:1/")
        );
        assert_eq!(
            proxy(&c, "http://a.test/").as_deref(),
            Some("http://setting:9/")
        );
        // The setting outranks ALL_PROXY (it becomes HTTP(S)_PROXY).
        let c = config(&[("ALL_PROXY", "http://all:1")], Some("http://setting:9"));
        assert_eq!(
            proxy(&c, "https://a.test/").as_deref(),
            Some("http://setting:9/")
        );
    }

    #[test]
    fn no_proxy_matches_exact_suffix_wildcards_ports_and_ipv6() {
        let c = config(
            &[
                ("https_proxy", "http://p:1"),
                ("http_proxy", "http://p:1"),
                (
                    "NO_PROXY",
                    "localhost, .internal.test,*.corp.test example.org:8443 [::1]:443,10.0.0.1,fe80::1",
                ),
            ],
            None,
        );
        let direct = |t: &str| proxy(&c, t).is_none();
        assert!(direct("http://localhost:3000/"));
        assert!(direct("https://internal.test/"));
        assert!(direct("https://a.internal.test/"));
        assert!(direct("https://b.corp.test/"));
        assert!(direct("https://corp.test/"));
        assert!(direct("https://example.org:8443/"));
        assert!(!direct("https://example.org/"), "port 443 is not excluded");
        assert!(direct("https://[::1]/"));
        assert!(!direct("http://[::1]/"), "port 80 is not excluded");
        assert!(direct("http://10.0.0.1/"));
        assert!(direct("http://[fe80::1]:8080/"));
        assert!(!direct("https://notinternal.test/"));
        assert!(!direct("https://api.openai.com/"));
        let star = config(&[("https_proxy", "http://p:1"), ("no_proxy", "*")], None);
        assert_eq!(proxy(&star, "https://anything.test/"), None);
    }

    #[test]
    fn socks_pac_and_invalid_proxies_are_rejected() {
        let c = config(&[("HTTPS_PROXY", "socks5://127.0.0.1:1080")], None);
        let err = c
            .proxy_for(&Url::parse("https://api.test/").unwrap())
            .unwrap_err();
        assert_eq!(
            err,
            format!("{UNSUPPORTED_PROXY_PROTOCOL_MESSAGE} Got socks5:")
        );
        let pac = config(&[("HTTPS_PROXY", "pac+http://wpad/proxy.pac")], None);
        assert!(
            pac.proxy_for(&Url::parse("https://api.test/").unwrap())
                .unwrap_err()
                .ends_with("Got pac+http:")
        );
        let bad = config(&[("HTTPS_PROXY", "http://[bad")], None);
        assert!(
            bad.proxy_for(&Url::parse("https://api.test/").unwrap())
                .unwrap_err()
                .starts_with("Invalid proxy URL \"http://[bad\"")
        );
        assert_eq!(c.warnings().len(), 1);
        assert!(
            config(&[("HTTPS_PROXY", "http://ok:1")], None)
                .warnings()
                .is_empty()
        );
    }

    #[test]
    fn env_pairs_round_trip_through_a_request_env() {
        let c = config(
            &[("HTTPS_PROXY", "http://p:1"), ("NO_PROXY", "internal.test")],
            Some("http://s:2"),
        );
        let map: HashMap<String, String> = c.env_pairs().into_iter().collect();
        assert_eq!(ProxyConfig::from_map(&map), c);
    }

    #[test]
    fn an_isolated_env_sees_only_its_own_variables() {
        let env = Env::isolated(HashMap::from([(
            "HTTPS_PROXY".to_owned(),
            "http://p:1".to_owned(),
        )]));
        let c = ProxyConfig::from_env(&env, None);
        assert_eq!(proxy(&c, "https://a.test/").as_deref(), Some("http://p:1/"));
        assert_eq!(proxy(&c, "http://a.test/"), None);
    }
}
