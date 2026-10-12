//! Native counterpart to Node's default-locale String.localeCompare.
use crate::config::Env;
use icu_collator::{Collator, CollatorBorrowed};
use icu_locale_core::Locale;

pub(super) fn for_locale(locale: &str) -> CollatorBorrowed<'static> {
    let locale: Locale = locale.parse().unwrap_or_else(|_| "en-US".parse().unwrap());
    Collator::try_new(locale.into(), Default::default())
        .expect("compiled ICU collation data includes the root fallback")
}

pub(super) fn from_env(env: &Env) -> CollatorBorrowed<'static> {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| env.get(key))
        .unwrap_or_default();
    let locale = locale
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .replace('_', "-");
    for_locale(if locale.is_empty() || locale == "C" || locale == "POSIX" {
        "en-US"
    } else {
        &locale
    })
}
