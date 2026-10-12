//! Pure web helpers: settings parsing and provider construction, source ids, slices and
//! `find`. Network behaviour is tested against a local server in `runtime::web_tests`.

use super::*;
use crate::web::providers::SearchError;

fn settings_with(web: serde_json::Value) -> Settings {
    let mut s = Settings::load(std::path::Path::new("/nonexistent-config-dir"), None).0;
    s.other.insert("web".into(), web);
    s
}

#[test]
fn providers_are_built_from_settings_and_keys_never_leak_into_errors() {
    let none = WebSettings::from_settings(&settings_with(serde_json::json!({})));
    let err = none.provider(|_| None).err().unwrap();
    assert!(matches!(err, SearchError::NotConfigured(_)));
    assert!(err.to_string().contains("web.search.provider"));

    let brave = WebSettings::from_settings(&settings_with(
        serde_json::json!({"search": {"provider": "Brave", "apiKeyEnv": "BRAVE_KEY"}}),
    ));
    let err = brave.provider(|_| None).err().unwrap();
    assert!(err.to_string().contains("apiKeyEnv"), "{err}");
    let provider = brave
        .provider(|k| (k == "BRAVE_KEY").then(|| "secret-key-value".to_owned()))
        .unwrap();
    assert_eq!(provider.name(), "brave");
    assert_eq!(provider.endpoint().as_str(), providers::Brave::ENDPOINT);
    assert!(!format!("{err}").contains("secret-key-value"));

    let searx = WebSettings::from_settings(&settings_with(
        serde_json::json!({"search": {"provider": "searxng", "url": "http://search.lan:8080/"}}),
    ));
    let provider = searx.provider(|_| None).unwrap();
    assert_eq!(provider.name(), "searxng");
    let missing = WebSettings::from_settings(&settings_with(
        serde_json::json!({"search": {"provider": "searxng"}}),
    ));
    assert!(
        missing
            .provider(|_| None)
            .err()
            .unwrap()
            .to_string()
            .contains("web.search.url")
    );
    let bad = WebSettings::from_settings(&settings_with(
        serde_json::json!({"search": {"provider": "searxng", "url": "ftp://x"}}),
    ));
    assert!(bad.provider(|_| None).is_err());
    let unknown = WebSettings::from_settings(&settings_with(
        serde_json::json!({"search": {"provider": "bing"}}),
    ));
    assert!(
        unknown
            .provider(|_| None)
            .err()
            .unwrap()
            .to_string()
            .contains("unknown")
    );
}

#[test]
fn source_ids_slices_and_find_are_bounded_and_exact() {
    assert_eq!(parse_source_id("s12"), Some(12));
    assert_eq!(parse_source_id(" S3 "), Some(3));
    assert_eq!(parse_source_id("12"), Some(12));
    assert_eq!(parse_source_id("s0"), None);
    assert_eq!(parse_source_id("x"), None);

    let text = "héllo wörld. Hello again, hello.";
    let (piece, total, more) = slice_chars(text, 0, 5);
    assert_eq!((piece.as_str(), total, more), ("héllo", 32, true));
    let (piece, _, more) = slice_chars(text, 30, 100);
    assert_eq!((piece.as_str(), more), ("o.", false));
    let (piece, _, more) = slice_chars(text, 0, READ_MAX_CHARS * 2);
    assert_eq!((piece.chars().count(), more), (32, false));

    let (matches, total) = find_in(text, "hello");
    assert_eq!(total, 2);
    assert_eq!(
        matches.iter().map(|m| m.offset).collect::<Vec<_>>(),
        [13, 26]
    );
    assert!(matches[0].context.contains("Hello again"));
    let (matches, total) = find_in(text, "");
    assert!(matches.is_empty() && total == 0);
    let long = "ab ".repeat(100);
    let (matches, total) = find_in(&long, "ab");
    assert_eq!(total, 100);
    assert_eq!(matches.len(), FIND_MAX_MATCHES);
}

#[test]
fn content_types_are_classified() {
    assert!(readable_content_type("text/html; charset=utf-8"));
    assert!(readable_content_type("application/json"));
    assert!(readable_content_type("application/ld+json"));
    assert!(readable_content_type("image/svg+xml"));
    assert!(!readable_content_type("image/png"));
    assert!(!readable_content_type("application/pdf"));
    assert!(!readable_content_type("application/octet-stream"));
    assert!(is_html("TEXT/HTML"));
    assert!(!is_html("text/plain"));
}
