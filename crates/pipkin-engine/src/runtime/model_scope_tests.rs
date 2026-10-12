use super::*;
use serde_json::{Value, json};

#[test]
fn scope_matches_executed_reference_patterns_and_ordering() {
    check(include_str!(
        "../../../../fixtures/native-model-scope/reference.json"
    ));
}

fn check(raw: &str) {
    let corpus: Value = serde_json::from_str(raw).unwrap();
    let mut mismatches = Vec::new();
    for case in corpus["cases"].as_array().unwrap() {
        let models: Vec<Model> =
            serde_json::from_value(case.get("models").unwrap_or(&corpus["catalog"]).clone())
                .unwrap();
        let patterns: Vec<String> = serde_json::from_value(case["patterns"].clone()).unwrap();
        let collator = order::for_locale(corpus["locale"].as_str().unwrap());
        let actual = resolve_scope_with_collator(&patterns, &models, &collator);
        let actual = json!({
            "models": actual.models.iter().map(|(m, level)| json!({"key":m.key(),"thinking":level})).collect::<Vec<_>>(),
            "warnings": actual.warnings,
        });
        if actual != case["expected"] {
            mismatches.push(json!({"name":case["name"],"patterns":patterns,"actual":actual,"expected":case["expected"]}));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{}",
        serde_json::to_string_pretty(&mismatches).unwrap()
    );
}

#[test]
fn fuzzy_ordering_matches_executed_reference_in_other_locales() {
    for corpus in [
        include_str!("../../../../fixtures/native-model-scope/ordering-sv-SE.json"),
        include_str!("../../../../fixtures/native-model-scope/ordering-tr-TR.json"),
        include_str!("../../../../fixtures/native-model-scope/ordering-de-DE.json"),
        include_str!("../../../../fixtures/native-model-scope/ordering-th-TH.json"),
    ] {
        check(corpus);
    }
}

#[test]
fn model_ordering_uses_the_engine_environment_and_locale_precedence() {
    use crate::config::Env;
    use std::cmp::Ordering;
    let compare = |vars: &[(&str, &str)]| {
        let env = Env::isolated(
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        order::from_env(&env).compare("z", "ä")
    };
    assert_eq!(compare(&[]), Ordering::Greater);
    assert_eq!(compare(&[("LANG", "sv_SE.UTF-8")]), Ordering::Less);
    assert_eq!(
        compare(&[("LANG", "de_DE.UTF-8"), ("LC_MESSAGES", "sv_SE.UTF-8")]),
        Ordering::Less
    );
    assert_eq!(
        compare(&[
            ("LANG", "sv_SE.UTF-8"),
            ("LC_ALL", "de_DE.UTF-8@collation=phonebook")
        ]),
        Ordering::Greater
    );
    assert_eq!(compare(&[("LC_ALL", "C")]), Ordering::Greater);
    assert_eq!(compare(&[("LC_ALL", "POSIX")]), Ordering::Greater);
}
