//! `models.json`: custom providers, models and overrides layered over the built-in catalog
//! (reference `coding-agent/core/model-config.ts`, `provider-composer.ts`, `model-runtime.ts`).
//!
//! Layer order per provider: built-in models -> provider `baseUrl`/`compat` -> `models` upserts
//! -> `modelOverrides` (last). Problems never fail loading: a schema error ignores the whole file,
//! a composition error leaves that provider's built-in models untouched; both become warnings.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

use crate::provider::Model;

/// The default Radius gateway (reference `providers/radius-config.ts`).
pub const DEFAULT_RADIUS_GATEWAY: &str = "https://radius.pi.dev";

/// One provider entry, kept as validated raw JSON so unknown model fields survive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderConfig {
    pub name: Option<String>,
    pub base_url: Option<String>,
    /// Config value (literal, `$VAR` template or `!command`).
    pub api_key: Option<String>,
    pub api: Option<String>,
    /// `"oauth": "radius"`.
    pub oauth_radius: bool,
    /// Header name -> config value.
    pub headers: Option<BTreeMap<String, String>>,
    pub compat: Option<Map<String, Value>>,
    pub auth_header: Option<bool>,
    pub models: Vec<Map<String, Value>>,
    pub model_overrides: BTreeMap<String, Map<String, Value>>,
}

impl ProviderConfig {
    /// The Radius gateway origin for an `oauth: radius` provider (`baseUrl` minus `/v1`).
    pub fn radius_gateway(&self) -> Option<String> {
        let base = self.base_url.as_deref()?;
        let trimmed = base.trim_end_matches('/');
        Some(normalize_gateway(
            trimmed.strip_suffix("/v1").unwrap_or(trimmed),
        ))
    }

    /// Per-model headers (`modelOverrides` < `models[]`), as config values.
    pub fn model_headers(&self, model_id: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for source in [
            self.model_overrides.get(model_id),
            self.models
                .iter()
                .find(|m| m.get("id").and_then(Value::as_str) == Some(model_id)),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(Value::Object(h)) = source.get("headers") {
                for (k, v) in h {
                    if let Some(s) = v.as_str() {
                        out.insert(k.clone(), s.to_owned());
                    }
                }
            }
        }
        out
    }
}

/// `https://` added when missing; trailing slashes removed.
pub fn normalize_gateway(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let with_scheme = if lower.starts_with("http://") || lower.starts_with("https://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    with_scheme.trim_end_matches('/').to_owned()
}

/// One load of `models.json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelsConfig {
    pub providers: BTreeMap<String, ProviderConfig>,
    pub error: Option<String>,
}

impl ModelsConfig {
    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.get(id)
    }

    /// Missing file = empty; JSONC and BOM tolerated; invalid = empty plus an error.
    pub fn load(path: &Path) -> ModelsConfig {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ModelsConfig::default(),
            Err(e) => {
                return ModelsConfig::with_error(format!(
                    "Failed to load models.json: {e}\n\nFile: {}",
                    path.display()
                ));
            }
        };
        let stripped = crate::config::strip_json_comments(text.trim_start_matches('\u{feff}'));
        let value: Value = match serde_json::from_str(&stripped) {
            Ok(v) => v,
            Err(e) => {
                return ModelsConfig::with_error(format!(
                    "Failed to parse models.json: {e}\n\nFile: {}",
                    path.display()
                ));
            }
        };
        ModelsConfig::from_value(&value, path)
    }

    fn with_error(error: String) -> ModelsConfig {
        ModelsConfig {
            providers: BTreeMap::new(),
            error: Some(error),
        }
    }

    pub fn from_value(value: &Value, path: &Path) -> ModelsConfig {
        let mut errors = Vec::new();
        validate_root(value, &mut errors);
        if !errors.is_empty() {
            let list: Vec<String> = errors
                .iter()
                .map(|(p, m)| format!("  - {p}: {m}"))
                .collect();
            return ModelsConfig::with_error(format!(
                "Invalid models.json schema:\n{}\n\nFile: {}",
                list.join("\n"),
                path.display()
            ));
        }
        let mut providers = BTreeMap::new();
        if let Some(Value::Object(ps)) = value.get("providers") {
            for (id, p) in ps {
                let Some(p) = p.as_object() else { continue };
                let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_owned);
                providers.insert(
                    id.clone(),
                    ProviderConfig {
                        name: s("name"),
                        base_url: s("baseUrl"),
                        api_key: s("apiKey"),
                        api: s("api"),
                        oauth_radius: p.get("oauth").and_then(Value::as_str) == Some("radius"),
                        headers: p.get("headers").and_then(Value::as_object).map(|h| {
                            h.iter()
                                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                                .collect()
                        }),
                        compat: p.get("compat").and_then(Value::as_object).cloned(),
                        auth_header: p.get("authHeader").and_then(Value::as_bool),
                        models: p
                            .get("models")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(|m| m.as_object().cloned()).collect())
                            .unwrap_or_default(),
                        model_overrides: p
                            .get("modelOverrides")
                            .and_then(Value::as_object)
                            .map(|o| {
                                o.iter()
                                    .filter_map(|(k, v)| Some((k.clone(), v.as_object()?.clone())))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    },
                );
            }
        }
        ModelsConfig {
            providers,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Validation (the reference TypeBox schema, `model-config.ts:84-257`, minus deep compat checks:
// the reference compat union accepts any object whose known keys are well typed).

type Errors = Vec<(String, String)>;

fn err(errors: &mut Errors, path: &str, msg: &str) {
    errors.push((
        if path.is_empty() { "root" } else { path }.to_owned(),
        msg.to_owned(),
    ));
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn check_string(v: Option<&Value>, path: &str, errors: &mut Errors) {
    match v {
        None => {}
        Some(Value::String(s)) if !s.is_empty() => {}
        Some(Value::String(_)) => err(errors, path, "must NOT have fewer than 1 characters"),
        Some(_) => err(errors, path, "must be string"),
    }
}

fn check_type(v: Option<&Value>, path: &str, errors: &mut Errors, kind: &str) {
    let Some(v) = v else { return };
    let ok = match kind {
        "boolean" => v.is_boolean(),
        "number" => v.is_number(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        _ => true,
    };
    if !ok {
        err(errors, path, &format!("must be {kind}"));
    }
}

fn check_string_record(v: Option<&Value>, path: &str, errors: &mut Errors) {
    let Some(v) = v else { return };
    let Some(m) = v.as_object() else {
        return err(errors, path, "must be object");
    };
    for (k, v) in m {
        if !v.is_string() {
            err(errors, &join(path, k), "must be string");
        }
    }
}

const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

fn check_cost(v: Option<&Value>, path: &str, errors: &mut Errors, all_required: bool) {
    let Some(v) = v else { return };
    let Some(m) = v.as_object() else {
        return err(errors, path, "must be object");
    };
    for k in ["input", "output", "cacheRead", "cacheWrite"] {
        match m.get(k) {
            None if all_required => err(errors, &join(path, k), "must have required property"),
            other => check_type(other, &join(path, k), errors, "number"),
        }
    }
    if let Some(t) = m.get("tiers") {
        let Some(tiers) = t.as_array() else {
            return err(errors, &join(path, "tiers"), "must be array");
        };
        for (i, tier) in tiers.iter().enumerate() {
            let p = join(&join(path, "tiers"), &i.to_string());
            let Some(tm) = tier.as_object() else {
                err(errors, &p, "must be object");
                continue;
            };
            for k in [
                "inputTokensAbove",
                "input",
                "output",
                "cacheRead",
                "cacheWrite",
            ] {
                match tm.get(k) {
                    None => err(errors, &join(&p, k), "must have required property"),
                    other => check_type(other, &join(&p, k), errors, "number"),
                }
            }
        }
    }
}

fn check_model_fields(m: &Map<String, Value>, path: &str, errors: &mut Errors, definition: bool) {
    if definition {
        match m.get("id") {
            None => err(errors, &join(path, "id"), "must have required property"),
            other => check_string(other, &join(path, "id"), errors),
        }
        check_string(m.get("api"), &join(path, "api"), errors);
        check_string(m.get("baseUrl"), &join(path, "baseUrl"), errors);
    }
    check_string(m.get("name"), &join(path, "name"), errors);
    check_type(
        m.get("reasoning"),
        &join(path, "reasoning"),
        errors,
        "boolean",
    );
    if let Some(t) = m.get("thinkingLevelMap") {
        match t.as_object() {
            None => err(errors, &join(path, "thinkingLevelMap"), "must be object"),
            Some(t) => {
                for level in LEVELS {
                    if let Some(v) = t.get(level)
                        && !(v.is_string() || v.is_null())
                    {
                        err(
                            errors,
                            &join(&join(path, "thinkingLevelMap"), level),
                            "must be string or null",
                        );
                    }
                }
            }
        }
    }
    if let Some(i) = m.get("input") {
        match i.as_array() {
            None => err(errors, &join(path, "input"), "must be array"),
            Some(a) => {
                for (n, v) in a.iter().enumerate() {
                    if !matches!(v.as_str(), Some("text" | "image")) {
                        err(
                            errors,
                            &join(&join(path, "input"), &n.to_string()),
                            "must be \"text\" or \"image\"",
                        );
                    }
                }
            }
        }
    }
    check_type(
        m.get("inputLimits"),
        &join(path, "inputLimits"),
        errors,
        "object",
    );
    check_cost(m.get("cost"), &join(path, "cost"), errors, definition);
    if let Some(pc) = m.get("promptCache") {
        match pc.as_object() {
            None => err(errors, &join(path, "promptCache"), "must be object"),
            Some(pc) => {
                for k in ["short", "long"] {
                    if let Some(v) = pc.get(k)
                        && !v.as_f64().is_some_and(|n| n > 0.0)
                    {
                        err(
                            errors,
                            &join(&join(path, "promptCache"), k),
                            "must be number > 0",
                        );
                    }
                }
            }
        }
    }
    check_type(
        m.get("contextWindow"),
        &join(path, "contextWindow"),
        errors,
        "number",
    );
    check_type(
        m.get("maxTokens"),
        &join(path, "maxTokens"),
        errors,
        "number",
    );
    check_type(
        m.get("samplingParams"),
        &join(path, "samplingParams"),
        errors,
        "object",
    );
    check_type(
        m.get("samplingParamsByThinkingLevel"),
        &join(path, "samplingParamsByThinkingLevel"),
        errors,
        "object",
    );
    check_string_record(m.get("headers"), &join(path, "headers"), errors);
    check_type(m.get("compat"), &join(path, "compat"), errors, "object");
}

fn validate_root(value: &Value, errors: &mut Errors) {
    let Some(root) = value.as_object() else {
        return err(errors, "", "must be object");
    };
    let Some(providers) = root.get("providers") else {
        return err(errors, "providers", "must have required property");
    };
    let Some(providers) = providers.as_object() else {
        return err(errors, "providers", "must be object");
    };
    for (id, p) in providers {
        let path = join("providers", id);
        let Some(p) = p.as_object() else {
            err(errors, &path, "must be object");
            continue;
        };
        for k in ["name", "baseUrl", "apiKey", "api"] {
            check_string(p.get(k), &join(&path, k), errors);
        }
        if let Some(o) = p.get("oauth")
            && o.as_str() != Some("radius")
        {
            err(
                errors,
                &join(&path, "oauth"),
                "must be equal to constant \"radius\"",
            );
        }
        check_string_record(p.get("headers"), &join(&path, "headers"), errors);
        check_type(p.get("compat"), &join(&path, "compat"), errors, "object");
        check_type(
            p.get("authHeader"),
            &join(&path, "authHeader"),
            errors,
            "boolean",
        );
        if let Some(models) = p.get("models") {
            match models.as_array() {
                None => err(errors, &join(&path, "models"), "must be array"),
                Some(models) => {
                    for (i, m) in models.iter().enumerate() {
                        let mp = join(&join(&path, "models"), &i.to_string());
                        match m.as_object() {
                            None => err(errors, &mp, "must be object"),
                            Some(m) => check_model_fields(m, &mp, errors, true),
                        }
                    }
                }
            }
        }
        if let Some(o) = p.get("modelOverrides") {
            match o.as_object() {
                None => err(errors, &join(&path, "modelOverrides"), "must be object"),
                Some(o) => {
                    for (mid, m) in o {
                        let mp = join(&join(&path, "modelOverrides"), mid);
                        match m.as_object() {
                            None => err(errors, &mp, "must be object"),
                            Some(m) => check_model_fields(m, &mp, errors, false),
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Composition

fn to_obj(model: &Model) -> Map<String, Value> {
    match serde_json::to_value(model) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

fn from_obj(obj: Map<String, Value>) -> Result<Model, String> {
    serde_json::from_value(Value::Object(obj)).map_err(|e| e.to_string())
}

fn shallow_merge(base: Option<&Value>, over: &Value) -> Value {
    let mut out = base.and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(o) = over.as_object() {
        for (k, v) in o {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// Reference `mergeCompat`: shallow merge with nested routing/kwargs objects merged one level.
pub fn merge_compat(
    base: &Map<String, Value>,
    over: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let Some(over) = over else {
        return base.clone();
    };
    let mut merged = base.clone();
    for (k, v) in over {
        merged.insert(k.clone(), v.clone());
    }
    for key in [
        "openRouterRouting",
        "vercelGatewayRouting",
        "chatTemplateKwargs",
        "chatTemplateArgs",
    ] {
        let b = base.get(key).filter(|v| v.is_object());
        let o = over.get(key).filter(|v| v.is_object());
        if b.is_some() || o.is_some() {
            merged.insert(
                key.into(),
                shallow_merge(b, o.unwrap_or(&Value::Object(Map::new()))),
            );
        }
    }
    merged
}

fn is_chat(m: &Model) -> bool {
    m.is_chat()
}

fn find_defaults<'a>(models: &'a [Model], id: &str, api: Option<&str>) -> Option<&'a Model> {
    let chat: Vec<&Model> = models.iter().filter(|m| is_chat(m)).collect();
    chat.iter()
        .find(|m| m.id == id)
        .or_else(|| api.and_then(|a| chat.iter().find(|m| m.api == a)))
        .or_else(|| chat.iter().find(|m| m.api == "openai-completions"))
        .or_else(|| chat.first())
        .copied()
}

fn model_from_json(
    provider: &str,
    def: &Map<String, Value>,
    config: &ProviderConfig,
    defaults: Option<&Model>,
) -> Result<Model, String> {
    let id = def.get("id").and_then(Value::as_str).unwrap_or_default();
    let api = def
        .get("api")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| config.api.clone())
        .or_else(|| defaults.map(|d| d.api.clone()))
        .ok_or_else(|| {
            format!(
                "Provider {provider}, model {id}: no \"api\" specified. Set at provider or model level."
            )
        })?;
    let base_url = def
        .get("baseUrl")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| config.base_url.clone())
        .or_else(|| defaults.map(|d| d.base_url.clone()))
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            format!("Provider {provider}: \"baseUrl\" is required when defining custom models.")
        })?;
    for (field, label) in [
        ("contextWindow", "contextWindow"),
        ("maxTokens", "maxTokens"),
    ] {
        if let Some(n) = def.get(field).and_then(Value::as_f64)
            && n <= 0.0
        {
            return Err(format!("Provider {provider}, model {id}: invalid {label}"));
        }
    }
    let mut obj = Map::new();
    obj.insert("id".into(), Value::String(id.to_owned()));
    obj.insert(
        "name".into(),
        def.get("name")
            .cloned()
            .unwrap_or(Value::String(id.to_owned())),
    );
    obj.insert("api".into(), Value::String(api));
    obj.insert("provider".into(), Value::String(provider.to_owned()));
    obj.insert("baseUrl".into(), Value::String(base_url));
    obj.insert(
        "reasoning".into(),
        def.get("reasoning").cloned().unwrap_or(Value::Bool(false)),
    );
    obj.insert(
        "input".into(),
        def.get("input")
            .cloned()
            .unwrap_or_else(|| serde_json::json!(["text"])),
    );
    obj.insert(
        "cost".into(),
        def.get("cost").cloned().unwrap_or_else(
            || serde_json::json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}),
        ),
    );
    obj.insert(
        "contextWindow".into(),
        def.get("contextWindow")
            .cloned()
            .unwrap_or_else(|| Value::from(128_000)),
    );
    obj.insert(
        "maxTokens".into(),
        def.get("maxTokens")
            .cloned()
            .unwrap_or_else(|| Value::from(16_384)),
    );
    for k in [
        "thinkingLevelMap",
        "inputLimits",
        "promptCache",
        "samplingParams",
        "samplingParamsByThinkingLevel",
    ] {
        if let Some(v) = def.get(k) {
            obj.insert(k.into(), v.clone());
        }
    }
    let compat = merge_compat(
        config.compat.as_ref().unwrap_or(&Map::new()),
        def.get("compat").and_then(Value::as_object),
    );
    obj.insert("compat".into(), Value::Object(compat));
    // Numbers in the file may be floats; the catalog uses integers for limits.
    for k in ["contextWindow", "maxTokens"] {
        if let Some(n) = obj.get(k).and_then(Value::as_f64) {
            obj.insert(k.into(), Value::from(n as u64));
        }
    }
    from_obj(obj).map_err(|e| format!("Provider {provider}, model {id}: {e}"))
}

fn apply_override(model: &Model, o: &Map<String, Value>) -> Result<Model, String> {
    let mut obj = to_obj(model);
    for k in ["name", "reasoning", "input", "contextWindow", "maxTokens"] {
        if let Some(v) = o.get(k) {
            obj.insert(k.into(), v.clone());
        }
    }
    for k in ["contextWindow", "maxTokens"] {
        if let Some(n) = obj.get(k).and_then(Value::as_f64) {
            obj.insert(k.into(), Value::from(n as u64));
        }
    }
    for k in ["thinkingLevelMap", "promptCache", "samplingParams"] {
        if let Some(v) = o.get(k) {
            let merged = shallow_merge(obj.get(k), v);
            obj.insert(k.into(), merged);
        }
    }
    if let Some(Value::Object(cost)) = o.get("cost") {
        let mut merged = obj
            .get("cost")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (k, v) in cost {
            merged.insert(k.clone(), v.clone());
        }
        obj.insert("cost".into(), Value::Object(merged));
    }
    if let Some(Value::Object(by_level)) = o.get("samplingParamsByThinkingLevel") {
        let mut merged = obj
            .get("samplingParamsByThinkingLevel")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for level in LEVELS {
            if let Some(params) = by_level.get(level).filter(|p| p.is_object()) {
                let m = shallow_merge(merged.get(level), params);
                merged.insert(level.into(), m);
            }
        }
        obj.insert(
            "samplingParamsByThinkingLevel".into(),
            Value::Object(merged),
        );
    }
    if let Some(Value::Object(limits)) = o.get("inputLimits") {
        let base = obj
            .get("inputLimits")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut merged = base.clone();
        for (k, v) in limits {
            merged.insert(k.clone(), v.clone());
        }
        if let Some(Value::Object(images)) = limits.get("images") {
            let base_images = base.get("images").and_then(Value::as_object).cloned();
            let mut mi = base_images.clone().unwrap_or_default();
            for (k, v) in images {
                mi.insert(k.clone(), v.clone());
            }
            if let Some(resize) = images.get("resize").filter(|r| r.is_object()) {
                let r = shallow_merge(base_images.as_ref().and_then(|b| b.get("resize")), resize);
                mi.insert("resize".into(), r);
            } else if let Some(r) = base_images.as_ref().and_then(|b| b.get("resize")) {
                mi.insert("resize".into(), r.clone());
            }
            merged.insert("images".into(), Value::Object(mi));
        }
        obj.insert("inputLimits".into(), Value::Object(merged));
    }
    if let Some(Value::Object(c)) = o.get("compat") {
        let merged = merge_compat(&model.compat, Some(c));
        obj.insert("compat".into(), Value::Object(merged));
    }
    from_obj(obj).map_err(|e| format!("Provider {}, model {}: {e}", model.provider, model.id))
}

fn apply_models_json(
    provider: &str,
    base: &[Model],
    config: &ProviderConfig,
) -> Result<Vec<Model>, String> {
    if config.oauth_radius && config.base_url.is_none() {
        return Err(format!(
            "Provider {provider}: \"baseUrl\" is required when \"oauth\" is set."
        ));
    }
    if config.models.is_empty()
        && config.base_url.is_none()
        && config.headers.is_none()
        && config.compat.is_none()
        && config.model_overrides.is_empty()
        && config.api_key.is_none()
        && !config.oauth_radius
        && config.auth_header.is_none()
    {
        return Err(format!(
            "Provider {provider}: must specify \"baseUrl\", \"headers\", \"compat\", \"modelOverrides\", or \"models\"."
        ));
    }
    let mut models: Vec<Model> = base
        .iter()
        .map(|m| {
            let mut m = m.clone();
            if !config.oauth_radius
                && let Some(u) = &config.base_url
            {
                m.base_url = u.clone();
            }
            if is_chat(&m) {
                m.compat = merge_compat(&m.compat, config.compat.as_ref());
            }
            m
        })
        .collect();
    for def in &config.models {
        let id = def.get("id").and_then(Value::as_str).unwrap_or_default();
        let api = def
            .get("api")
            .and_then(Value::as_str)
            .or(config.api.as_deref());
        let defaults = find_defaults(&models, id, api).cloned();
        let model = model_from_json(provider, def, config, defaults.as_ref())?;
        match models.iter().position(|m| is_chat(m) && m.id == id) {
            Some(i) => models[i] = model,
            None => models.push(model),
        }
    }
    Ok(models)
}

fn apply_overrides(models: Vec<Model>, config: &ProviderConfig) -> Result<Vec<Model>, String> {
    models
        .into_iter()
        .map(|m| match config.model_overrides.get(&m.id) {
            Some(o) if is_chat(&m) => apply_override(&m, o),
            _ => Ok(m),
        })
        .collect()
}

/// Models of the Radius provider `id` at `gateway`: the pinned baseline only for the default
/// gateway (reference `providers/radius.ts`).
pub fn radius_baseline(builtin: &[Model], id: &str, gateway: &str) -> Vec<Model> {
    if normalize_gateway(gateway) != normalize_gateway(DEFAULT_RADIUS_GATEWAY) {
        return vec![];
    }
    builtin
        .iter()
        .filter(|m| m.provider == "radius")
        .map(|m| {
            let mut m = m.clone();
            m.provider = id.to_owned();
            m
        })
        .collect()
}

/// Build the catalog: built-in models with `models.json` layered per provider.
pub fn compose(
    builtin: Vec<Model>,
    config: &ModelsConfig,
    warnings: &mut Vec<String>,
) -> Vec<Model> {
    compose_with_radius(
        builtin,
        config,
        warnings,
        &BTreeMap::new(),
        DEFAULT_RADIUS_GATEWAY,
    )
}

pub(super) fn compose_with_radius(
    builtin: Vec<Model>,
    config: &ModelsConfig,
    warnings: &mut Vec<String>,
    radius: &BTreeMap<String, Vec<Model>>,
    default_gateway: &str,
) -> Vec<Model> {
    let mut order: Vec<String> = Vec::new();
    let mut by_provider: BTreeMap<String, Vec<Model>> = BTreeMap::new();
    for m in &builtin {
        if !by_provider.contains_key(&m.provider) {
            order.push(m.provider.clone());
        }
        by_provider
            .entry(m.provider.clone())
            .or_default()
            .push(m.clone());
    }
    for id in config.providers.keys() {
        if !by_provider.contains_key(id) {
            order.push(id.clone());
            by_provider.insert(id.clone(), vec![]);
        }
    }
    let mut out = Vec::new();
    for id in order {
        let mut base = by_provider.remove(&id).unwrap_or_default();
        let pc = config.provider(&id);
        let gateway = pc
            .filter(|pc| pc.oauth_radius)
            .and_then(ProviderConfig::radius_gateway);
        if let Some(gateway) = gateway
            .as_deref()
            .or((id == "radius").then_some(default_gateway))
        {
            base = radius_baseline(&builtin, &id, gateway);
        }
        if let Some(overlay) = radius.get(&id) {
            for model in overlay {
                if let Some(at) = base.iter().position(|m| m.id == model.id) {
                    base[at] = model.clone();
                } else {
                    base.push(model.clone());
                }
            }
        }
        let Some(pc) = pc else {
            out.extend(base);
            continue;
        };
        let composed = apply_models_json(&id, &base, pc).and_then(|m| apply_overrides(m, pc));
        match composed {
            Ok(models) => out.extend(models),
            Err(e) => {
                warnings.push(e);
                out.extend(base);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_model(provider: &str, id: &str, api: &str) -> Model {
        serde_json::from_value(json!({
            "id": id, "name": id, "api": api, "provider": provider,
            "baseUrl": format!("https://{provider}.example/v1"),
            "reasoning": true, "contextWindow": 1000, "maxTokens": 100,
            "compat": {"supportsStore": true, "openRouterRouting": {"only": ["a"]}},
            "headers": {"X-Static": "1"}, "type": "chat"
        }))
        .unwrap()
    }

    fn load(v: Value) -> ModelsConfig {
        ModelsConfig::from_value(&v, Path::new("/cfg/models.json"))
    }

    #[test]
    fn jsonc_files_load_and_missing_files_are_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        assert_eq!(ModelsConfig::load(&path), ModelsConfig::default());
        std::fs::write(
            &path,
            "\u{feff}{ // custom\n \"providers\": {\"local\": {/* x */ \"baseUrl\": \"http://h/v1\", \"api\": \"openai-completions\", \"models\": [{\"id\": \"m\"}]}}}",
        )
        .unwrap();
        let cfg = ModelsConfig::load(&path);
        assert_eq!(cfg.error, None);
        assert_eq!(
            cfg.provider("local").unwrap().base_url.as_deref(),
            Some("http://h/v1")
        );
        std::fs::write(&path, "{nope").unwrap();
        let cfg = ModelsConfig::load(&path);
        assert!(
            cfg.error
                .unwrap()
                .starts_with("Failed to parse models.json")
        );
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn schema_errors_ignore_the_whole_file_with_paths() {
        let cfg = load(json!({"providers": {
            "ok": {"baseUrl": "http://x"},
            "bad": {"baseUrl": 3, "oauth": "other", "models": [{"name": "no id", "input": ["video"]}],
                    "headers": {"A": 1}}
        }}));
        let e = cfg.error.unwrap();
        assert!(e.starts_with("Invalid models.json schema:\n"), "{e}");
        for p in [
            "providers.bad.baseUrl: must be string",
            "providers.bad.oauth",
            "providers.bad.models.0.id: must have required property",
            "providers.bad.models.0.input.0",
            "providers.bad.headers.A: must be string",
        ] {
            assert!(e.contains(p), "{p} in {e}");
        }
        assert!(e.ends_with("File: /cfg/models.json"));
        assert!(cfg.providers.is_empty());
        assert!(
            load(json!({}))
                .error
                .unwrap()
                .contains("providers: must have required")
        );
    }

    #[test]
    fn layers_apply_in_reference_order() {
        let builtin = vec![
            base_model("p", "a", "openai-completions"),
            base_model("p", "b", "anthropic-messages"),
            base_model("q", "z", "openai-responses"),
        ];
        let cfg = load(json!({"providers": {
            "p": {
                "baseUrl": "https://proxy/v1",
                "compat": {"supportsStore": false, "openRouterRouting": {"order": ["x"]}},
                "models": [
                    {"id": "b", "contextWindow": 5000, "compat": {"maxTokensField": "max_tokens"}},
                    {"id": "new", "api": "anthropic-messages", "headers": {"H": "$V"}}
                ],
                "modelOverrides": {
                    "a": {"name": "Renamed", "cost": {"input": 9}, "maxTokens": 7, "headers": {"O": "1"}},
                    "new": {"reasoning": true},
                    "ghost": {"name": "ignored"}
                }
            },
            "custom": {"baseUrl": "http://local:1/v1", "api": "openai-completions", "apiKey": "k",
                "models": [{"id": "m1"}]}
        }}));
        let mut warnings = vec![];
        let models = compose(builtin, &cfg, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        let find = |p: &str, id: &str| {
            models
                .iter()
                .find(|m| m.provider == p && m.id == id)
                .unwrap()
                .clone()
        };
        let a = find("p", "a");
        assert_eq!(a.base_url, "https://proxy/v1");
        assert_eq!(a.name, "Renamed");
        assert_eq!(a.max_tokens, 7);
        assert_eq!(a.cost.input, 9.0);
        assert_eq!(a.headers.get("X-Static").map(String::as_str), Some("1"));
        assert_eq!(a.compat["supportsStore"], json!(false));
        assert_eq!(
            a.compat["openRouterRouting"],
            json!({"only": ["a"], "order": ["x"]})
        );
        // A models[] entry with a built-in id replaces it, defaults from the same-id model.
        let b = find("p", "b");
        assert_eq!(b.api, "anthropic-messages");
        assert_eq!(b.context_window, 5000);
        assert_eq!(b.max_tokens, 16_384);
        assert!(!b.reasoning);
        assert!(b.headers.is_empty());
        assert_eq!(b.compat["maxTokensField"], json!("max_tokens"));
        assert_eq!(b.compat["supportsStore"], json!(false));
        let n = find("p", "new");
        assert!(n.reasoning, "overrides apply after upserts");
        assert_eq!(n.base_url, "https://proxy/v1");
        assert_eq!(n.input, vec!["text"]);
        assert_eq!(n.context_window, 128_000);
        let m1 = find("custom", "m1");
        assert_eq!(
            (m1.api.as_str(), m1.base_url.as_str()),
            ("openai-completions", "http://local:1/v1")
        );
        assert_eq!(m1.name, "m1");
        assert_eq!(find("q", "z").base_url, "https://q.example/v1");
        let pc = cfg.provider("p").unwrap();
        assert_eq!(
            pc.model_headers("a"),
            BTreeMap::from([("O".into(), "1".into())])
        );
        assert_eq!(
            pc.model_headers("new"),
            BTreeMap::from([("H".into(), "$V".into())])
        );
    }

    #[test]
    fn composition_errors_warn_and_keep_built_ins() {
        let builtin = vec![base_model("p", "a", "openai-completions")];
        let cfg = load(json!({"providers": {
            "p": {"baseUrl": "https://new", "models": [{"id": "x", "maxTokens": 0}]},
            "empty": {"name": "Only a name"},
            "nourl": {"api": "openai-completions", "models": [{"id": "m"}]},
            "noapi": {"baseUrl": "http://h", "models": [{"id": "m"}]},
            "radius2": {"oauth": "radius"}
        }}));
        let mut warnings = vec![];
        let models = compose(builtin, &cfg, &mut warnings);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].base_url, "https://p.example/v1");
        let all = warnings.join("\n");
        for w in [
            "Provider p, model x: invalid maxTokens",
            "Provider empty: must specify",
            "Provider nourl: \"baseUrl\" is required when defining custom models.",
            "Provider noapi, model m: no \"api\" specified",
            "Provider radius2: \"baseUrl\" is required when \"oauth\" is set.",
        ] {
            assert!(all.contains(w), "{w} in {all}");
        }
    }

    #[test]
    fn radius_providers_from_models_json() {
        let mut radius = base_model("radius", "balanced", "pi-messages");
        radius.base_url = "https://radius.pi.dev/v1".into();
        let cfg = load(json!({"providers": {
            "team": {"oauth": "radius", "baseUrl": "https://radius.pi.dev/v1/", "name": "Team"},
            "other": {"oauth": "radius", "baseUrl": "gw.example/v1",
                "models": [{"id": "x", "api": "pi-messages"}]}
        }}));
        let mut warnings = vec![];
        let models = compose(vec![radius], &cfg, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        let team: Vec<_> = models.iter().filter(|m| m.provider == "team").collect();
        assert_eq!(team.len(), 1);
        assert_eq!(team[0].base_url, "https://radius.pi.dev/v1");
        let other: Vec<_> = models.iter().filter(|m| m.provider == "other").collect();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].base_url, "gw.example/v1");
        assert_eq!(
            cfg.provider("other").unwrap().radius_gateway().as_deref(),
            Some("https://gw.example")
        );
    }
}
