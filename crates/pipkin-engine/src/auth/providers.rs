//! Built-in providers' API-key and ambient credential resolution (reference `env-api-keys.ts`,
//! `auth/helpers.ts` `envApiKeyAuth`, `providers/{anthropic,amazon-bedrock,google-vertex,
//! cloudflare-auth}.ts`). Pure functions over a stored key, the credential's `env` and [`Env`];
//! nothing here runs commands or touches the network.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::config::Env;
use crate::provider::Credential;

/// API-key environment variables per provider (reference `env-api-keys.ts:73-127`).
pub fn env_vars(provider: &str) -> Option<&'static [&'static str]> {
    Some(match provider {
        "github-copilot" => &["COPILOT_GITHUB_TOKEN"],
        "anthropic" => &[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
        ],
        "ant-ling" => &["ANT_LING_API_KEY"],
        "qwen-token-plan" | "qwen-token-plan-individual" => &["QWEN_TOKEN_PLAN_API_KEY"],
        "qwen-token-plan-cn" => &["QWEN_TOKEN_PLAN_CN_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "azure-openai-responses" => &["AZURE_OPENAI_API_KEY"],
        "nvidia" => &["NVIDIA_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "google" => &["GEMINI_API_KEY"],
        "google-vertex" => &["GOOGLE_CLOUD_API_KEY"],
        "groq" => &["GROQ_API_KEY"],
        "cerebras" => &["CEREBRAS_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        "typesafe" => &["TYPESAFE_API_KEY"],
        "radius" => &["RADIUS_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "vercel-ai-gateway" => &["AI_GATEWAY_API_KEY"],
        "zai" => &["ZAI_API_KEY"],
        "zai-coding-cn" => &["ZAI_CODING_CN_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "minimax" => &["MINIMAX_API_KEY"],
        "minimax-cn" => &["MINIMAX_CN_API_KEY"],
        "moonshotai" | "moonshotai-cn" => &["MOONSHOT_API_KEY"],
        "huggingface" => &["HF_TOKEN"],
        "fireworks" => &["FIREWORKS_API_KEY"],
        "together" => &["TOGETHER_API_KEY"],
        "baseten" => &["BASETEN_API_KEY"],
        "opencode" | "opencode-go" => &["OPENCODE_API_KEY"],
        "kimi-coding" => &["KIMI_API_KEY"],
        "meta" => &["META_API_KEY"],
        "cloudflare-workers-ai" | "cloudflare-ai-gateway" => &["CLOUDFLARE_API_KEY"],
        "xiaomi" => &["XIAOMI_API_KEY"],
        "xiaomi-token-plan-cn" => &["XIAOMI_TOKEN_PLAN_CN_API_KEY"],
        "xiaomi-token-plan-ams" => &["XIAOMI_TOKEN_PLAN_AMS_API_KEY"],
        "xiaomi-token-plan-sgp" => &["XIAOMI_TOKEN_PLAN_SGP_API_KEY"],
        _ => return None,
    })
}

/// Whether the built-in provider has API-key/ambient auth at all (`openai-codex` is OAuth only).
pub fn has_api_key_auth(provider: &str) -> bool {
    env_vars(provider).is_some() || matches!(provider, "amazon-bedrock")
}

/// What the API-key layer produced.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyAuth {
    Resolved(KeyResolution),
    NotConfigured,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyResolution {
    pub credential: Credential,
    pub headers: BTreeMap<String, Option<String>>,
    pub env: BTreeMap<String, String>,
    pub source: String,
}

impl KeyResolution {
    fn key(key: &str, env: BTreeMap<String, String>, source: &str) -> KeyResolution {
        KeyResolution {
            credential: Credential::ApiKey(key.to_owned()),
            headers: BTreeMap::new(),
            env,
            source: source.to_owned(),
        }
    }

    fn ambient(env: BTreeMap<String, String>, source: &str) -> KeyResolution {
        let mut meta = Map::new();
        meta.insert("source".into(), Value::String(source.to_owned()));
        KeyResolution {
            credential: Credential::Ambient(meta),
            headers: BTreeMap::new(),
            env,
            source: source.to_owned(),
        }
    }
}

/// The stored (or `models.json`) key with its credential `env`. `key` is already resolved.
#[derive(Clone, Debug, Default)]
pub struct KeyInput<'a> {
    pub key: Option<&'a str>,
    pub env: Option<&'a BTreeMap<String, String>>,
}

fn cred_env(input: &KeyInput, name: &str) -> Option<String> {
    input
        .env
        .and_then(|e| e.get(name))
        .filter(|v| !v.is_empty())
        .cloned()
}

fn expand_home(path: &str, env: &Env) -> Option<String> {
    match path.strip_prefix('~') {
        Some(rest) => env.get("HOME").map(|h| format!("{h}{rest}")),
        None => Some(path.to_owned()),
    }
}

/// Resolve built-in provider auth from a stored/configured key, the credential's `env`, and
/// the environment. `None` for providers with no built-in API-key auth.
pub fn resolve_builtin(provider: &str, input: &KeyInput, env: &Env) -> Option<KeyAuth> {
    let owned_env = || input.env.cloned().unwrap_or_default();
    let resolved = |r: KeyResolution| Some(KeyAuth::Resolved(r));
    match provider {
        "anthropic" => {
            if let Some(k) = input.key.filter(|k| !k.is_empty()) {
                return resolved(KeyResolution::key(k, owned_env(), "stored credential"));
            }
            if let Some(token) = env.get("ANTHROPIC_AUTH_TOKEN") {
                return resolved(KeyResolution {
                    credential: Credential::None,
                    headers: BTreeMap::from([(
                        "Authorization".to_owned(),
                        Some(format!("Bearer {token}")),
                    )]),
                    env: BTreeMap::new(),
                    source: "ANTHROPIC_AUTH_TOKEN".into(),
                });
            }
            // Explicit subscription auth takes precedence over API-key billing.
            if let Some(access) = env.get("ANTHROPIC_OAUTH_TOKEN") {
                return resolved(KeyResolution {
                    credential: Credential::OAuth {
                        access,
                        metadata: Map::new(),
                    },
                    headers: BTreeMap::new(),
                    env: BTreeMap::new(),
                    source: "ANTHROPIC_OAUTH_TOKEN".into(),
                });
            }
            if let Some(k) = env.get("ANTHROPIC_API_KEY") {
                return resolved(KeyResolution::key(&k, BTreeMap::new(), "ANTHROPIC_API_KEY"));
            }
            let mut fed = BTreeMap::new();
            for name in [
                "ANTHROPIC_FEDERATION_RULE_ID",
                "ANTHROPIC_ORGANIZATION_ID",
                "ANTHROPIC_IDENTITY_TOKEN_FILE",
            ] {
                fed.insert(name.to_owned(), env.get(name)?);
            }
            for name in ["ANTHROPIC_SERVICE_ACCOUNT_ID", "ANTHROPIC_WORKSPACE_ID"] {
                if let Some(v) = env.get(name) {
                    fed.insert(name.to_owned(), v);
                }
            }
            resolved(KeyResolution::ambient(fed, "workload identity federation"))
        }
        "amazon-bedrock" => {
            if let Some(k) = input.key.filter(|k| !k.is_empty()) {
                return resolved(KeyResolution::key(k, owned_env(), "stored credential"));
            }
            if env.get("AWS_BEARER_TOKEN_BEDROCK").is_some() {
                return resolved(KeyResolution::ambient(
                    BTreeMap::new(),
                    "AWS_BEARER_TOKEN_BEDROCK",
                ));
            }
            if cred_env(input, "AWS_PROFILE").is_some() {
                return resolved(KeyResolution::ambient(owned_env(), "stored credential"));
            }
            if env.get("AWS_PROFILE").is_some() {
                return resolved(KeyResolution::ambient(owned_env(), "AWS_PROFILE"));
            }
            if env.get("AWS_ACCESS_KEY_ID").is_some() && env.get("AWS_SECRET_ACCESS_KEY").is_some()
            {
                return resolved(KeyResolution::ambient(owned_env(), "AWS access keys"));
            }
            if env.get("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI").is_some()
                || env.get("AWS_CONTAINER_CREDENTIALS_FULL_URI").is_some()
            {
                return resolved(KeyResolution::ambient(owned_env(), "ECS task role"));
            }
            if env.get("AWS_WEB_IDENTITY_TOKEN_FILE").is_some() {
                return resolved(KeyResolution::ambient(owned_env(), "web identity token"));
            }
            Some(KeyAuth::NotConfigured)
        }
        "google-vertex" => {
            if let Some(k) = input.key.filter(|k| !k.is_empty()) {
                return resolved(KeyResolution::key(k, BTreeMap::new(), "stored credential"));
            }
            if let Some(k) = env.get("GOOGLE_CLOUD_API_KEY") {
                return resolved(KeyResolution::key(
                    &k,
                    BTreeMap::new(),
                    "GOOGLE_CLOUD_API_KEY",
                ));
            }
            let adc = cred_env(input, "GOOGLE_APPLICATION_CREDENTIALS")
                .or_else(|| env.get("GOOGLE_APPLICATION_CREDENTIALS"))
                .unwrap_or_else(|| "~/.config/gcloud/application_default_credentials.json".into());
            let has_file =
                expand_home(&adc, env).is_some_and(|p| std::path::Path::new(&p).exists());
            let project = cred_env(input, "GOOGLE_CLOUD_PROJECT")
                .or_else(|| env.get("GOOGLE_CLOUD_PROJECT"))
                .or_else(|| env.get("GCLOUD_PROJECT"));
            let location = cred_env(input, "GOOGLE_CLOUD_LOCATION")
                .or_else(|| env.get("GOOGLE_CLOUD_LOCATION"));
            if has_file && project.is_some() && location.is_some() {
                let source = if input.env.is_some() || input.key.is_some() {
                    "stored credential"
                } else {
                    "gcloud application default credentials"
                };
                return resolved(KeyResolution::ambient(owned_env(), source));
            }
            Some(KeyAuth::NotConfigured)
        }
        "cloudflare-workers-ai" | "cloudflare-ai-gateway" => {
            let gateway = provider == "cloudflare-ai-gateway";
            let field = |name: &str| -> Option<String> {
                let from_cred = if name == "CLOUDFLARE_API_KEY" {
                    input.key.map(str::to_owned)
                } else {
                    input.env.and_then(|e| e.get(name)).cloned()
                };
                from_cred.or_else(|| env.get(name))
            };
            let (Some(key), Some(account)) =
                (field("CLOUDFLARE_API_KEY"), field("CLOUDFLARE_ACCOUNT_ID"))
            else {
                return Some(KeyAuth::NotConfigured);
            };
            let gateway_id = if gateway {
                match field("CLOUDFLARE_GATEWAY_ID") {
                    Some(g) => Some(g),
                    None => return Some(KeyAuth::NotConfigured),
                }
            } else {
                None
            };
            if key.is_empty() || account.is_empty() {
                return Some(KeyAuth::NotConfigured);
            }
            let mut out_env = BTreeMap::from([("CLOUDFLARE_ACCOUNT_ID".to_owned(), account)]);
            if let Some(g) = gateway_id {
                out_env.insert("CLOUDFLARE_GATEWAY_ID".into(), g);
            }
            let source = if input.key.is_some() || input.env.is_some() {
                "stored credential"
            } else {
                "CLOUDFLARE_API_KEY"
            };
            if gateway {
                resolved(KeyResolution {
                    credential: Credential::None,
                    headers: BTreeMap::from([
                        (
                            "cf-aig-authorization".to_owned(),
                            Some(format!("Bearer {key}")),
                        ),
                        ("Authorization".to_owned(), None),
                        ("x-api-key".to_owned(), None),
                    ]),
                    env: out_env,
                    source: source.into(),
                })
            } else {
                resolved(KeyResolution::key(&key, out_env, source))
            }
        }
        _ => {
            let vars = env_vars(provider)?;
            if let Some(k) = input.key.filter(|k| !k.is_empty()) {
                return resolved(KeyResolution::key(k, owned_env(), "stored credential"));
            }
            for var in vars {
                if let Some(v) = env.get(var) {
                    return resolved(KeyResolution::key(&v, BTreeMap::new(), var));
                }
            }
            Some(KeyAuth::NotConfigured)
        }
    }
}

/// Resolve for any provider: built-in rules, or a plain key for custom providers.
pub fn resolve_key_auth(provider: &str, input: &KeyInput, env: &Env) -> KeyAuth {
    if let Some(r) = resolve_builtin(provider, input, env) {
        return r;
    }
    match input.key.filter(|k| !k.is_empty()) {
        Some(k) => KeyAuth::Resolved(KeyResolution::key(
            k,
            input.env.cloned().unwrap_or_default(),
            "stored credential",
        )),
        None => KeyAuth::NotConfigured,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> Env {
        Env::isolated(
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn cred(auth: KeyAuth) -> Credential {
        match auth {
            KeyAuth::Resolved(r) => r.credential,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn env_table_and_stored_key_precedence() {
        let e = env(&[("OPENAI_API_KEY", "env-key"), ("MOONSHOT_API_KEY", "m")]);
        let none = KeyInput::default();
        assert_eq!(
            cred(resolve_key_auth("openai", &none, &e)),
            Credential::ApiKey("env-key".into())
        );
        assert_eq!(
            cred(resolve_key_auth("moonshotai-cn", &none, &e)),
            Credential::ApiKey("m".into())
        );
        let stored = KeyInput {
            key: Some("stored"),
            env: None,
        };
        assert_eq!(
            cred(resolve_key_auth("openai", &stored, &e)),
            Credential::ApiKey("stored".into())
        );
        assert_eq!(resolve_key_auth("groq", &none, &e), KeyAuth::NotConfigured);
        assert_eq!(
            resolve_key_auth("openai-codex", &none, &e),
            KeyAuth::NotConfigured
        );
        assert_eq!(
            resolve_key_auth("my-local", &none, &e),
            KeyAuth::NotConfigured
        );
        assert_eq!(
            cred(resolve_key_auth("my-local", &stored, &e)),
            Credential::ApiKey("stored".into())
        );
    }

    #[test]
    fn anthropic_env_rules_and_subscription_token_precedence() {
        let KeyAuth::Resolved(r) = resolve_key_auth(
            "anthropic",
            &KeyInput::default(),
            &env(&[("ANTHROPIC_AUTH_TOKEN", "t"), ("ANTHROPIC_API_KEY", "k")]),
        ) else {
            panic!()
        };
        assert_eq!(r.credential, Credential::None);
        assert_eq!(r.headers["Authorization"].as_deref(), Some("Bearer t"));
        assert_eq!(
            cred(resolve_key_auth(
                "anthropic",
                &KeyInput::default(),
                &env(&[("ANTHROPIC_OAUTH_TOKEN", "o"), ("ANTHROPIC_API_KEY", "k")])
            )),
            Credential::OAuth {
                access: "o".into(),
                metadata: Map::new()
            }
        );
        assert_eq!(
            cred(resolve_key_auth(
                "anthropic",
                &KeyInput::default(),
                &env(&[("ANTHROPIC_API_KEY", "k")])
            )),
            Credential::ApiKey("k".into())
        );
        let fed = env(&[
            ("ANTHROPIC_FEDERATION_RULE_ID", "r"),
            ("ANTHROPIC_ORGANIZATION_ID", "o"),
            ("ANTHROPIC_IDENTITY_TOKEN_FILE", "/f"),
            ("ANTHROPIC_WORKSPACE_ID", "w"),
        ]);
        let KeyAuth::Resolved(r) = resolve_key_auth("anthropic", &KeyInput::default(), &fed) else {
            panic!()
        };
        assert!(matches!(r.credential, Credential::Ambient(_)));
        assert_eq!(r.env.len(), 4);
    }

    #[test]
    fn bedrock_and_vertex_ambient_detection() {
        let none = KeyInput::default();
        assert_eq!(
            resolve_key_auth("amazon-bedrock", &none, &env(&[])),
            KeyAuth::NotConfigured
        );
        assert_eq!(
            resolve_key_auth("amazon-bedrock", &none, &env(&[("AWS_ACCESS_KEY_ID", "a")])),
            KeyAuth::NotConfigured
        );
        for vars in [
            vec![("AWS_PROFILE", "p")],
            vec![("AWS_ACCESS_KEY_ID", "a"), ("AWS_SECRET_ACCESS_KEY", "s")],
            vec![("AWS_BEARER_TOKEN_BEDROCK", "b")],
            vec![("AWS_CONTAINER_CREDENTIALS_FULL_URI", "u")],
            vec![("AWS_WEB_IDENTITY_TOKEN_FILE", "f")],
        ] {
            assert!(matches!(
                cred(resolve_key_auth("amazon-bedrock", &none, &env(&vars))),
                Credential::Ambient(_)
            ));
        }
        let profile = BTreeMap::from([("AWS_PROFILE".to_owned(), "work".to_owned())]);
        let KeyAuth::Resolved(r) = resolve_key_auth(
            "amazon-bedrock",
            &KeyInput {
                key: None,
                env: Some(&profile),
            },
            &env(&[]),
        ) else {
            panic!()
        };
        assert_eq!(r.env, profile);

        let dir = tempfile::tempdir().unwrap();
        let adc = dir.path().join("adc.json");
        std::fs::write(&adc, "{}").unwrap();
        let adc = adc.to_string_lossy().to_string();
        assert_eq!(
            resolve_key_auth(
                "google-vertex",
                &none,
                &env(&[
                    ("GOOGLE_APPLICATION_CREDENTIALS", &adc),
                    ("GOOGLE_CLOUD_PROJECT", "p")
                ])
            ),
            KeyAuth::NotConfigured
        );
        assert!(matches!(
            cred(resolve_key_auth(
                "google-vertex",
                &none,
                &env(&[
                    ("GOOGLE_APPLICATION_CREDENTIALS", &adc),
                    ("GCLOUD_PROJECT", "p"),
                    ("GOOGLE_CLOUD_LOCATION", "us")
                ])
            )),
            Credential::Ambient(_)
        ));
        // The default ADC path is under $HOME of the configured environment only.
        let home = dir.path().to_string_lossy().to_string();
        std::fs::create_dir_all(dir.path().join(".config/gcloud")).unwrap();
        std::fs::write(
            dir.path()
                .join(".config/gcloud/application_default_credentials.json"),
            "{}",
        )
        .unwrap();
        assert!(matches!(
            cred(resolve_key_auth(
                "google-vertex",
                &none,
                &env(&[
                    ("HOME", &home),
                    ("GOOGLE_CLOUD_PROJECT", "p"),
                    ("GOOGLE_CLOUD_LOCATION", "l")
                ])
            )),
            Credential::Ambient(_)
        ));
    }

    #[test]
    fn cloudflare_merges_fields_and_moves_gateway_auth_to_its_header() {
        let e = env(&[
            ("CLOUDFLARE_ACCOUNT_ID", "acct"),
            ("CLOUDFLARE_GATEWAY_ID", "gw"),
        ]);
        let input = KeyInput {
            key: Some("cf"),
            env: None,
        };
        let KeyAuth::Resolved(r) = resolve_key_auth("cloudflare-ai-gateway", &input, &e) else {
            panic!()
        };
        assert_eq!(r.credential, Credential::None);
        assert_eq!(
            r.headers["cf-aig-authorization"].as_deref(),
            Some("Bearer cf")
        );
        assert_eq!(r.headers["Authorization"], None);
        assert_eq!(r.env["CLOUDFLARE_GATEWAY_ID"], "gw");
        let KeyAuth::Resolved(r) = resolve_key_auth("cloudflare-workers-ai", &input, &e) else {
            panic!()
        };
        assert_eq!(r.credential, Credential::ApiKey("cf".into()));
        assert_eq!(
            resolve_key_auth("cloudflare-workers-ai", &input, &env(&[])),
            KeyAuth::NotConfigured
        );
    }
}
