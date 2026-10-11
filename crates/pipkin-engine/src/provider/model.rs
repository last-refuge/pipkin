//! Model metadata: the pinned catalog plus user configuration (`models.json`).
//!
//! The JSON shape is the reference engine's generated model data, so the embedded files and a
//! user's `models.json` deserialize without translation. `compat` is kept as raw JSON and
//! interpreted by each wire adapter; unknown keys are preserved, never rejected.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::data;

/// Model-side thinking setting. `Off` disables reasoning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    pub const ALL: [ThinkingLevel; 7] = [
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::Xhigh,
        ThinkingLevel::Max,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingLevel::Off => "off",
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::Xhigh => "xhigh",
            ThinkingLevel::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Option<ThinkingLevel> {
        ThinkingLevel::ALL.into_iter().find(|l| l.as_str() == s)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostTier {
    pub input_tokens_above: u64,
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

/// Dollars per million tokens.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<CostTier>,
}

/// One model as the catalog describes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub api: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default = "text_only")]
    pub input: Vec<String>,
    #[serde(default)]
    pub cost: ModelCost,
    #[serde(default = "default_context")]
    pub context_window: u64,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u64,
    /// Level -> provider-native value; `null` means unsupported.
    #[serde(default)]
    pub thinking_level_map: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub compat: Map<String, Value>,
    /// `chat`, `classifier` or `image`. Only chat models run conversations.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn text_only() -> Vec<String> {
    vec!["text".into()]
}

fn default_context() -> u64 {
    128_000
}

fn default_max_tokens() -> u64 {
    16_384
}

impl Model {
    /// `provider/modelId`.
    pub fn key(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }

    pub fn is_chat(&self) -> bool {
        self.kind.as_deref().is_none_or(|k| k == "chat")
    }

    pub fn supports_images(&self) -> bool {
        self.input.iter().any(|i| i == "image")
    }

    pub fn compat_bool(&self, key: &str) -> Option<bool> {
        self.compat.get(key).and_then(Value::as_bool)
    }

    pub fn compat_str(&self, key: &str) -> Option<&str> {
        self.compat.get(key).and_then(Value::as_str)
    }

    /// Levels this model accepts, in order. Non-reasoning models only accept `off`;
    /// `xhigh`/`max` need an explicit mapping; a `null` mapping removes a level.
    pub fn thinking_levels(&self) -> Vec<ThinkingLevel> {
        if !self.reasoning {
            return vec![ThinkingLevel::Off];
        }
        ThinkingLevel::ALL
            .into_iter()
            .filter(|level| match self.thinking_level_map.get(level.as_str()) {
                Some(None) => false,
                Some(Some(_)) => true,
                None => !matches!(level, ThinkingLevel::Xhigh | ThinkingLevel::Max),
            })
            .collect()
    }

    /// The requested level if supported, else the nearest supported level above it, else the
    /// nearest below it.
    pub fn clamp_thinking(&self, level: ThinkingLevel) -> ThinkingLevel {
        let levels = self.thinking_levels();
        if levels.contains(&level) {
            return level;
        }
        levels
            .iter()
            .copied()
            .find(|l| *l > level)
            .or_else(|| levels.iter().copied().rev().find(|l| *l < level))
            .unwrap_or(ThinkingLevel::Off)
    }

    /// The provider-native value for a level (`None` when unmapped).
    pub fn thinking_value(&self, level: ThinkingLevel) -> Option<&str> {
        self.thinking_level_map
            .get(level.as_str())
            .and_then(|v| v.as_deref())
    }

    /// Dollar cost of a usage record, using the tier that applies to its input size.
    pub fn cost_of(&self, usage: &mut pipkin_engine_protocol::Usage, cache_write_1h: u64) {
        let prompt = usage.input + usage.cache_read + usage.cache_write;
        let (input, output, cache_read, cache_write) = self
            .cost
            .tiers
            .iter()
            .filter(|t| prompt > t.input_tokens_above)
            .max_by_key(|t| t.input_tokens_above)
            .map(|t| (t.input, t.output, t.cache_read, t.cache_write))
            .unwrap_or((
                self.cost.input,
                self.cost.output,
                self.cost.cache_read,
                self.cost.cache_write,
            ));
        let per = |tokens: u64, rate: f64| tokens as f64 * rate / 1_000_000.0;
        let short_writes = usage.cache_write.saturating_sub(cache_write_1h);
        usage.cost.input = per(usage.input, input);
        usage.cost.output = per(usage.output, output);
        usage.cost.cache_read = per(usage.cache_read, cache_read);
        usage.cost.cache_write = per(short_writes, cache_write) + per(cache_write_1h, input * 2.0);
        usage.cost.total =
            usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
    }
}

/// The embedded catalog: every model of every built-in provider, in file order.
pub fn builtin_models() -> Vec<Model> {
    let mut out = Vec::new();
    for (file, raw) in data::FILES {
        let parsed: BTreeMap<String, BTreeMap<String, Model>> = match serde_json::from_str(raw) {
            Ok(p) => p,
            Err(e) => {
                log::error!("embedded model data {file} is invalid: {e}");
                continue;
            }
        };
        for (api, models) in parsed {
            for (_, mut model) in models {
                if model.api.is_empty() {
                    model.api = api.clone();
                }
                if model.name.is_empty() {
                    model.name = model.id.clone();
                }
                out.push(model);
            }
        }
    }
    out
}

/// The manifest's structure hash and per-file hashes.
pub fn manifest() -> Value {
    serde_json::from_str(data::MANIFEST).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn embedded_files_match_the_pinned_manifest() {
        let manifest = manifest();
        let files = manifest["files"].as_object().unwrap();
        assert_eq!(files.len(), data::FILES.len());
        for (name, raw) in data::FILES {
            let digest = Sha256::digest(raw.as_bytes());
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(files[*name].as_str(), Some(hex.as_str()), "{name}");
        }
    }

    #[test]
    fn the_catalog_loads_every_provider() {
        let models = builtin_models();
        assert!(models.len() > 1500, "{}", models.len());
        let providers: std::collections::BTreeSet<_> =
            models.iter().map(|m| m.provider.as_str()).collect();
        for p in [
            "anthropic",
            "openai",
            "openai-codex",
            "google",
            "amazon-bedrock",
            "mistral",
        ] {
            assert!(providers.contains(p), "{p}");
        }
    }

    #[test]
    fn thinking_levels_follow_the_reference_rules() {
        let mut m: Model = serde_json::from_value(serde_json::json!({
            "id": "m", "reasoning": true, "thinkingLevelMap": {"minimal": null, "xhigh": "xhigh"}
        }))
        .unwrap();
        use ThinkingLevel::*;
        assert_eq!(m.thinking_levels(), vec![Off, Low, Medium, High, Xhigh]);
        assert_eq!(m.clamp_thinking(Minimal), Low);
        assert_eq!(m.clamp_thinking(Max), Xhigh);
        m.reasoning = false;
        assert_eq!(m.thinking_levels(), vec![Off]);
        assert_eq!(m.clamp_thinking(High), Off);
    }

    #[test]
    fn cost_uses_tiers_and_doubles_long_cache_writes() {
        let m: Model = serde_json::from_value(serde_json::json!({
            "id": "m", "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.1, "cacheWrite": 1.25,
                "tiers": [{"inputTokensAbove": 1000, "input": 10.0, "output": 20.0}]}
        }))
        .unwrap();
        let mut u = pipkin_engine_protocol::Usage {
            input: 500,
            output: 100,
            cache_write: 200,
            ..Default::default()
        };
        m.cost_of(&mut u, 100);
        assert!((u.cost.cache_write - (100.0 * 1.25 + 100.0 * 2.0) / 1e6).abs() < 1e-12);
        let mut big = pipkin_engine_protocol::Usage {
            input: 2000,
            ..Default::default()
        };
        m.cost_of(&mut big, 0);
        assert!((big.cost.input - 2000.0 * 10.0 / 1e6).abs() < 1e-12);
    }
}
