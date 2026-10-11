//! Model selection, availability and thinking levels (reference `model-resolver.ts`,
//! `services/models-provider.ts`).

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

/// The model and thinking level a new root session starts with (reference
/// `findInitialModel` steps 3-5; there is no CLI model in the desktop path except the engine's
/// configured default).
pub fn initial(
    auth: &Auth,
    catalog: &Catalog,
    settings: &Settings,
    configured: Option<&str>,
) -> (Option<String>, ThinkingLevel) {
    let thinking = settings
        .default_thinking_level
        .as_deref()
        .and_then(ThinkingLevel::parse)
        .unwrap_or(DEFAULT_THINKING);
    let usable = |key: &str| catalog.find(key).filter(|m| auth.availability(m).is_ok());
    if let Some(key) = configured
        && let Some(m) = usable(key)
    {
        return (Some(m.key()), m.clamp_thinking(DEFAULT_THINKING));
    }
    if let (Some(p), Some(id)) = (&settings.default_provider, &settings.default_model)
        && let Some(m) = usable(&format!("{p}/{id}"))
    {
        let level = settings
            .other
            .get("modelThinkingLevels")
            .and_then(|v| v.get(m.key()))
            .and_then(|v| v.as_str())
            .and_then(ThinkingLevel::parse)
            .unwrap_or(thinking);
        return (Some(m.key()), m.clamp_thinking(level));
    }
    let models = available(auth, catalog);
    for (provider, id) in DEFAULT_MODEL_PER_PROVIDER {
        if let Some(m) = models
            .iter()
            .find(|m| m.provider == *provider && m.id == *id)
        {
            return (Some(m.key()), m.clamp_thinking(DEFAULT_THINKING));
        }
    }
    match models.first() {
        Some(m) => (Some(m.key()), m.clamp_thinking(DEFAULT_THINKING)),
        None => (None, DEFAULT_THINKING),
    }
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
/// always shown honestly).
pub fn models_view(auth: &Auth, catalog: &Catalog, selected: Option<&str>) -> ModelsView {
    let mut models: Vec<ModelView> = available(auth, catalog)
        .iter()
        .map(|m| view(m, auth))
        .collect();
    if let Some(key) = selected
        && !models.iter().any(|m| m.key == key)
        && let Some(m) = catalog.find(key)
    {
        models.push(view(m, auth));
    }
    ModelsView {
        models,
        selected: selected.map(str::to_owned),
        warnings: catalog.warnings.clone(),
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
