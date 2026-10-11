//! Request header assembly (reference `utils/headers.ts` `providerHeadersToRecord`, the SDK
//! header layering, and the per-adapter `hasHeader` helpers).

use std::collections::BTreeMap;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::provider::ProviderRequest;

/// An ordered, case-insensitive header set. Names are stored lowercased (the wire form);
/// later writes replace earlier ones and a configured `None` deletes (reference
/// `ProviderHeaders` semantics).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Headers(Vec<(String, String)>);

impl Headers {
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.remove(name);
        self.0.push((name.to_ascii_lowercase(), value.into()));
    }

    /// Set only when no header of that name exists.
    pub fn set_default(&mut self, name: &str, value: impl Into<String>) {
        if !self.has(name) {
            self.0.push((name.to_ascii_lowercase(), value.into()));
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.0.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Model-style headers (every value set).
    pub fn merge(&mut self, headers: &BTreeMap<String, String>) {
        for (k, v) in headers {
            self.set(k, v.clone());
        }
    }

    /// Request-style headers: `None` deletes.
    pub fn merge_optional(&mut self, headers: &BTreeMap<String, Option<String>>) {
        for (k, v) in headers {
            match v {
                Some(v) => self.set(k, v.clone()),
                None => self.remove(k),
            }
        }
    }

    /// The `(lowercased name, value)` pairs in insertion order.
    pub fn pairs(&self) -> &[(String, String)] {
        &self.0
    }

    pub fn into_pairs(self) -> Vec<(String, String)> {
        self.0
    }

    pub fn to_header_map(&self) -> Result<HeaderMap, String> {
        header_map(&self.0)
    }
}

/// Build a `HeaderMap`. An invalid name or value is an error (the reference's `fetch`/`Headers`
/// throws on them too) rather than a header silently dropped.
pub fn header_map(pairs: &[(String, String)]) -> Result<HeaderMap, String> {
    let mut map = HeaderMap::new();
    for (k, v) in pairs {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| format!("Invalid header name: {k}"))?;
        let value =
            HeaderValue::from_str(v).map_err(|_| format!("Invalid value for header {k}"))?;
        map.insert(name, value);
    }
    Ok(map)
}

/// Assemble request headers in SDK order: authentication, then model headers, then dynamic
/// headers, then request headers. Later layers win; a `None` request value deletes a header.
pub fn build_headers(
    auth: &[(&str, String)],
    model_headers: &BTreeMap<String, String>,
    dynamic: &[(&str, String)],
    request: &BTreeMap<String, Option<String>>,
) -> Result<HeaderMap, String> {
    let mut headers = Headers::default();
    for (k, v) in auth {
        headers.set(k, v.clone());
    }
    headers.merge(model_headers);
    for (k, v) in dynamic {
        headers.set(k, v.clone());
    }
    headers.merge_optional(request);
    headers.to_header_map()
}

/// Reference `hasHeader`: a request header of that name (case-insensitive) with a non-blank
/// value.
pub fn has_header(headers: &BTreeMap<String, Option<String>>, name: &str) -> bool {
    headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case(name) && v.as_deref().is_some_and(|v| !v.trim().is_empty())
    })
}

/// [`has_header`] over the request headers or the model's configured headers.
pub fn has_configured_header(req: &ProviderRequest, name: &str) -> bool {
    has_header(&req.headers, name)
        || req
            .model
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case(name) && !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layering_is_case_insensitive_and_none_deletes() {
        let mut model = BTreeMap::new();
        model.insert("X-Model".to_owned(), "m".to_owned());
        model.insert("Authorization".to_owned(), "model".to_owned());
        let mut request = BTreeMap::new();
        request.insert("x-model".to_owned(), None);
        request.insert("X-Extra".to_owned(), Some("e".to_owned()));
        let map = build_headers(
            &[("authorization", "Bearer k".into())],
            &model,
            &[("x-dyn", "d".into())],
            &request,
        )
        .unwrap();
        assert_eq!(map["authorization"], "model");
        assert!(map.get("x-model").is_none());
        assert_eq!(map["x-extra"], "e");
        assert_eq!(map["x-dyn"], "d");
        assert!(header_map(&[("bad name".into(), "v".into())]).is_err());
        assert!(has_header(&request, "X-EXTRA"));
        assert!(!has_header(&request, "x-model"));
    }
}
