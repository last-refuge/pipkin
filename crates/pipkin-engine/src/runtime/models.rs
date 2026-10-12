//! Model selection, availability and thinking levels (reference `model-resolver.ts`,
//! `services/models-provider.ts`).

mod order;
mod pattern;

use pipkin_engine_protocol::{ModelView, ModelsView, ThinkingView};

use crate::auth::{Auth, Catalog};
use crate::config::Settings;
use crate::provider::{Model, ThinkingLevel};

/// Default thinking level for a new session (reference `DEFAULT_THINKING_LEVEL`).
pub const DEFAULT_THINKING: ThinkingLevel = ThinkingLevel::Medium;

/// Default chat model per provider, in the reference's preference order.
pub const DEFAULT_MODEL_PER_PROVIDER: &[(&str, &str)] = &[
    ("amazon-bedrock", "us.anthropic.claude-opus-4-6-v1"),
    ("ant-ling", "Ring-2.6-1T"),
    ("anthropic", "claude-opus-4-8"),
    ("openai", "gpt-5.5"),
    ("azure-openai-responses", "gpt-5.4"),
    ("openai-codex", "gpt-6.1-sol"),
    ("radius", "balanced"),
    ("nvidia", "nvidia/nemotron-3-ultra-550b-a55b"),
    ("deepseek", "deepseek-v4-pro"),
    ("google", "gemini-3.1-pro-preview"),
    ("google-vertex", "gemini-3.1-pro-preview"),
    ("github-copilot", "gpt-5.4"),
    ("openrouter", "moonshotai/kimi-k2.6"),
    ("vercel-ai-gateway", "zai/glm-5.1"),
    ("xai", "grok-4.7"),
    ("groq", "openai/gpt-oss-120b"),
    ("cerebras", "gpt-oss-120b"),
    ("zai", "glm-5.3"),
    ("zai-coding-cn", "glm-5.3"),
    ("mistral", "devstral-medium-latest"),
    ("minimax", "MiniMax-M2.7"),
    ("minimax-cn", "MiniMax-M2.7"),
    ("moonshotai", "kimi-k2.6"),
    ("moonshotai-cn", "kimi-k2.6"),
    ("huggingface", "moonshotai/Kimi-K2.6"),
    ("fireworks", "accounts/fireworks/models/kimi-k3"),
    ("together", "moonshotai/Kimi-K3"),
    ("baseten", "zai-org/GLM-5.2"),
    ("opencode", "kimi-k2.6"),
    ("opencode-go", "kimi-k3"),
    ("kimi-coding", "kimi-for-coding"),
    ("meta", "muse-spark-1.3"),
    ("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
    (
        "cloudflare-ai-gateway",
        "workers-ai/@cf/moonshotai/kimi-k2.6",
    ),
    ("qwen-token-plan", "qwen3.7-max"),
    ("qwen-token-plan-cn", "qwen3.7-max"),
    ("qwen-token-plan-individual", "qwen3.8-max"),
    ("xiaomi", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-cn", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-ams", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-sgp", "mimo-v2.5-pro"),
];

/// Chat models whose provider is usable now (metadata only), in catalog order.
pub fn available(auth: &Auth, catalog: &Catalog) -> Vec<Model> {
    catalog
        .models
        .iter()
        .filter(|m| m.is_chat() && auth.availability(m).is_ok())
        .cloned()
        .collect()
}

/// The thinking level for a model a session starts with or switches to (reference
/// `sdk.ts:236-260` and `_getThinkingLevelForModelSwitch`): an explicit level (a scope
/// pattern's `:level`), else the model's `modelThinkingLevels` entry, else
/// `defaultThinkingLevel`, else `fallback`; clamped to the model.
pub fn thinking_for(
    settings: &Settings,
    model: &Model,
    explicit: Option<ThinkingLevel>,
    fallback: ThinkingLevel,
) -> ThinkingLevel {
    let level = explicit
        .or_else(|| settings.model_thinking_level(&model.key()))
        .or_else(|| settings.default_thinking())
        .unwrap_or(fallback);
    model.clamp_thinking(level)
}

/// The model and thinking level a new root session starts with (reference
/// `buildSessionOptions` + `findInitialModel`): the engine's configured model (the desktop's
/// stand-in for `--model`), then the `enabledModels` scope (the saved default when it is in
/// scope, else the first scoped model), then the saved default, then the first available
/// provider default, then the first available model.
pub fn initial(
    auth: &Auth,
    catalog: &Catalog,
    settings: &Settings,
    configured: Option<&str>,
) -> (Option<String>, ThinkingLevel) {
    let usable = |key: &str| catalog.find(key).filter(|m| auth.availability(m).is_ok());
    let pick = |m: &Model, explicit: Option<ThinkingLevel>| {
        (
            Some(m.key()),
            thinking_for(settings, m, explicit, DEFAULT_THINKING),
        )
    };
    if let Some(key) = configured
        && let Some(m) = usable(key)
    {
        return pick(m, None);
    }
    let saved = match (&settings.default_provider, &settings.default_model) {
        (Some(p), Some(id)) => usable(&format!("{p}/{id}")),
        _ => None,
    };
    if let Some(scope) = scope(auth, catalog, settings)
        && let Some(first) = scope.models.first()
    {
        let chosen = saved
            .and_then(|s| scope.models.iter().find(|(m, _)| m.key() == s.key()))
            .unwrap_or(first);
        return pick(&chosen.0, chosen.1);
    }
    if let Some(m) = saved {
        return pick(m, None);
    }
    let models = available(auth, catalog);
    for (provider, id) in DEFAULT_MODEL_PER_PROVIDER {
        if let Some(m) = models
            .iter()
            .find(|m| m.provider == *provider && m.id == *id)
        {
            return pick(m, None);
        }
    }
    match models.first() {
        Some(m) => pick(m, None),
        None => (None, DEFAULT_THINKING),
    }
}

/// The models `enabledModels` selects, with any `:level` each pattern names, plus warnings
/// for patterns that matched nothing (reference `resolveModelScopeFromModels`).
#[derive(Clone, Debug, Default)]
pub struct Scope {
    pub models: Vec<(Model, Option<ThinkingLevel>)>,
    pub warnings: Vec<String>,
}

/// The `enabledModels` scope over the available models; `None` when no patterns are set.
pub fn scope(auth: &Auth, catalog: &Catalog, settings: &Settings) -> Option<Scope> {
    let patterns = settings.enabled_models.as_ref().filter(|p| !p.is_empty())?;
    Some(resolve_scope_with_collator(
        patterns,
        &available(auth, catalog),
        &order::from_env(auth.env()),
    ))
}

/// Reference `findExactModelReferenceMatch`: `provider/id` (case-insensitive), then a unique
/// bare id.
fn exact_match<'a>(reference: &str, models: &'a [Model]) -> Option<&'a Model> {
    let reference = reference.trim();
    if reference.is_empty() {
        return None;
    }
    let lower = reference.to_lowercase();
    let canonical: Vec<&Model> = models
        .iter()
        .filter(|m| m.key().to_lowercase() == lower)
        .collect();
    match canonical.len() {
        1 => return Some(canonical[0]),
        n if n > 1 => return None,
        _ => {}
    }
    if let Some((provider, id)) = reference.split_once('/') {
        let (provider, id) = (provider.trim().to_lowercase(), id.trim().to_lowercase());
        if !provider.is_empty() && !id.is_empty() {
            let matches: Vec<&Model> = models
                .iter()
                .filter(|m| m.provider.to_lowercase() == provider && m.id.to_lowercase() == id)
                .collect();
            match matches.len() {
                1 => return Some(matches[0]),
                n if n > 1 => return None,
                _ => {}
            }
        }
    }
    let ids: Vec<&Model> = models
        .iter()
        .filter(|m| m.id.to_lowercase() == lower)
        .collect();
    (ids.len() == 1).then(|| ids[0])
}

/// Reference `isAlias`: ends in `-latest` or has no `-YYYYMMDD` suffix.
fn is_alias(id: &str) -> bool {
    if id.ends_with("-latest") {
        return true;
    }
    let bytes = id.as_bytes();
    !(bytes.len() >= 9
        && bytes[bytes.len() - 9] == b'-'
        && bytes[bytes.len() - 8..].iter().all(u8::is_ascii_digit))
}

/// Reference `tryMatchModel`: exact, else substring of id or name preferring aliases (highest
/// id first), else the latest dated id.
fn try_match<'a>(
    pattern: &str,
    models: &'a [Model],
    collator: &icu_collator::CollatorBorrowed<'_>,
) -> Option<&'a Model> {
    if let Some(m) = exact_match(pattern, models) {
        return Some(m);
    }
    let needle = pattern.to_lowercase();
    let mut matches: Vec<&Model> = models
        .iter()
        .filter(|m| {
            m.id.to_lowercase().contains(&needle) || m.name.to_lowercase().contains(&needle)
        })
        .collect();
    if matches.is_empty() {
        return None;
    }
    let aliases: Vec<&Model> = matches
        .iter()
        .copied()
        .filter(|m| is_alias(&m.id))
        .collect();
    if !aliases.is_empty() {
        matches = aliases;
    }
    matches.sort_by(|a, b| collator.compare(&b.id, &a.id));
    matches.first().copied()
}

/// Reference `parseModelPattern` (scope mode): the full pattern, else split a trailing
/// `:level` (an invalid level warns and is ignored).
fn parse_pattern<'a>(
    pattern: &str,
    models: &'a [Model],
    collator: &icu_collator::CollatorBorrowed<'_>,
) -> (Option<&'a Model>, Option<ThinkingLevel>, Option<String>) {
    if let Some(m) = try_match(pattern, models, collator) {
        return (Some(m), None, None);
    }
    let Some(colon) = pattern.rfind(':') else {
        return (None, None, None);
    };
    let (prefix, suffix) = (&pattern[..colon], &pattern[colon + 1..]);
    let (model, level, warning) = parse_pattern(prefix, models, collator);
    if model.is_none() {
        return (model, level, warning);
    }
    match ThinkingLevel::parse(suffix) {
        Some(l) => (
            model,
            if warning.is_some() { None } else { Some(l) },
            warning,
        ),
        None => (
            model,
            None,
            Some(format!(
                "Invalid thinking level \"{suffix}\" in pattern \"{pattern}\". Using default instead."
            )),
        ),
    }
}

/// Reference `resolveModelScopeFromModels`.
fn resolve_scope_with_collator(
    patterns: &[String],
    models: &[Model],
    collator: &icu_collator::CollatorBorrowed<'_>,
) -> Scope {
    let mut scope = Scope::default();
    let push = |scope: &mut Scope, m: &Model, level: Option<ThinkingLevel>| {
        if !scope.models.iter().any(|(s, _)| s.key() == m.key()) {
            scope.models.push((m.clone(), level));
        }
    };
    for pattern in patterns {
        if pattern.contains(['*', '?', '[']) {
            let (glob_pattern, level) = match pattern.rfind(':') {
                Some(i) => match ThinkingLevel::parse(&pattern[i + 1..]) {
                    Some(l) => (&pattern[..i], Some(l)),
                    None => (pattern.as_str(), None),
                },
                None => (pattern.as_str(), None),
            };
            if let Some(m) = exact_match(glob_pattern, models) {
                push(&mut scope, m, level);
                continue;
            }
            let matcher = pattern::Pattern::new(glob_pattern);
            let matching: Vec<&Model> = models
                .iter()
                .filter(|m| {
                    matcher
                        .as_ref()
                        .is_some_and(|g| g.is_match(&m.key()) || g.is_match(&m.id))
                })
                .collect();
            if matching.is_empty() {
                scope
                    .warnings
                    .push(format!("No models match pattern \"{pattern}\""));
                continue;
            }
            for m in matching {
                push(&mut scope, m, level);
            }
            continue;
        }
        let (model, level, warning) = parse_pattern(pattern, models, collator);
        if let Some(w) = warning {
            scope.warnings.push(w);
        }
        match model {
            Some(m) => push(&mut scope, m, level),
            None => scope
                .warnings
                .push(format!("No models match pattern \"{pattern}\"")),
        }
    }
    scope
}

pub fn view(model: &Model, auth: &Auth) -> ModelView {
    let availability = auth.availability(model);
    ModelView {
        key: model.key(),
        name: model.name.clone(),
        provider: model.provider.clone(),
        api: model.api.clone(),
        reasoning: model.reasoning,
        images: model.supports_images(),
        context_window: model.context_window,
        max_tokens: model.max_tokens,
        available: availability.is_ok(),
        note: availability.err().map(|e| e.message().to_owned()),
    }
}

/// Available models plus the selected one when it is not available (so the selection is
/// always shown honestly). A non-empty `enabledModels` scope narrows the list to the scoped
/// models in scope order (the reference model selector opens on its "scoped" view); patterns
/// that matched nothing become warnings.
pub fn models_view(
    auth: &Auth,
    catalog: &Catalog,
    settings: &Settings,
    selected: Option<&str>,
) -> ModelsView {
    let mut warnings = catalog.warnings.clone();
    let listed: Vec<Model> = match scope(auth, catalog, settings) {
        Some(scope) => {
            warnings.extend(scope.warnings);
            if scope.models.is_empty() {
                available(auth, catalog)
            } else {
                scope.models.into_iter().map(|(m, _)| m).collect()
            }
        }
        None => available(auth, catalog),
    };
    let mut models: Vec<ModelView> = listed.iter().map(|m| view(m, auth)).collect();
    if let Some(key) = selected
        && !models.iter().any(|m| m.key == key)
        && let Some(m) = catalog.find(key)
    {
        models.push(view(m, auth));
    }
    ModelsView {
        models,
        selected: selected.map(str::to_owned),
        warnings,
    }
}

pub fn thinking_view(model: Option<&Model>, level: Option<&str>) -> ThinkingView {
    let levels = model.map_or(vec![ThinkingLevel::Off], |m| m.thinking_levels());
    let current = level
        .and_then(ThinkingLevel::parse)
        .map(|l| model.map_or(ThinkingLevel::Off, |m| m.clamp_thinking(l)))
        .unwrap_or(ThinkingLevel::Off);
    ThinkingView {
        level: current.as_str().into(),
        levels: levels.iter().map(|l| l.as_str().to_owned()).collect(),
    }
}

#[cfg(test)]
#[path = "model_scope_tests.rs"]
mod tests;
