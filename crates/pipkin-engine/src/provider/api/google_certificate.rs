//! Bounded Google certificate identity loading and credential-only mutual TLS transport.
use super::{LIMIT, required};
use crate::provider::{ProviderRequest, http};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;
use tokio_rustls::rustls::{
    pki_types::{CertificateDer, pem::PemObject},
    server::ParsedCertificate,
};

pub(super) struct Prepared {
    pub subject: String,
    pub http: reqwest::Client,
}
pub(super) fn environment(req: &ProviderRequest) -> BTreeMap<String, String> {
    [
        "GOOGLE_API_CERTIFICATE_CONFIG",
        "CLOUDSDK_CONFIG",
        "HOME",
        "APPDATA",
    ]
    .into_iter()
    .filter_map(|key| req.env_var(key).map(|v| (key.into(), v)))
    .collect()
}
pub(super) fn validate(config: &Value) -> Result<(), String> {
    // The certificate sub-options, unlike the containing source, use original SDK keys.
    let default = config["use_default_certificate_config"]
        .as_bool()
        .unwrap_or(false);
    let explicit = config["certificate_config_location"]
        .as_str()
        .is_some_and(|s| !s.is_empty());
    if default == explicit {
        return Err(
            "Google certificate source requires exactly one default or explicit config".into(),
        );
    }
    Ok(())
}
fn config_path(config: &Value, env: &BTreeMap<String, String>) -> PathBuf {
    if let Some(path) = config["certificate_config_location"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        return PathBuf::from(path);
    }
    if let Some(path) = env.get("GOOGLE_API_CERTIFICATE_CONFIG") {
        return path.into();
    }
    let base = if let Some(path) = env.get("CLOUDSDK_CONFIG") {
        PathBuf::from(path)
    } else if cfg!(windows) {
        Path::new(env.get("APPDATA").map(String::as_str).unwrap_or("")).join("gcloud")
    } else {
        Path::new(env.get("HOME").map(String::as_str).unwrap_or("")).join(".config/gcloud")
    };
    base.join("certificate_config.json")
}
async fn read(path: &Path) -> Result<Vec<u8>, String> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|_| "Cannot inspect Google certificate source file")?;
    if !meta.is_file() {
        return Err("Google certificate source is not a regular file".into());
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| "Cannot read Google certificate source file")?;
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Cannot read Google certificate source file")?;
    if bytes.len() > LIMIT {
        return Err("Google certificate source file is too large".into());
    }
    Ok(bytes)
}
fn parse_der(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let der = CertificateDer::from_pem_slice(bytes).unwrap_or_else(|_| CertificateDer::from(bytes));
    ParsedCertificate::try_from(&der).map_err(|_| "Invalid Google X.509 certificate")?;
    Ok(der.as_ref().to_vec())
}
fn subject(leaf: &[u8], chain: Option<&[u8]>) -> Result<String, String> {
    let mut certificates = Vec::new();
    if let Some(bytes) = chain {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| "Google certificate trust chain is not UTF-8")?;
        // The SDK ignores non-PEM text and parses each complete certificate block.
        static BLOCK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        for block in BLOCK
            .get_or_init(|| {
                regex::Regex::new(r"-----BEGIN CERTIFICATE-----[^-]+-----END CERTIFICATE-----")
                    .unwrap()
            })
            .find_iter(text)
        {
            certificates.push(parse_der(block.as_str().as_bytes())?);
        }
    }
    match certificates.iter().position(|cert| cert == leaf) {
        None => certificates.insert(0, leaf.to_vec()),
        Some(0) => {}
        Some(_) => return Err("Google certificate leaf must be first in the trust chain".into()),
    }
    serde_json::to_string(
        &certificates
            .iter()
            .map(|c| STANDARD.encode(c))
            .collect::<Vec<_>>(),
    )
    .map_err(|_| "Cannot encode Google certificate chain".into())
}
pub(super) async fn prepare(
    config: &Value,
    env: &BTreeMap<String, String>,
    net: &http::NetSettings,
) -> Result<Prepared, String> {
    validate(config)?;
    let bytes = read(&config_path(config, env)).await?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid Google certificate config JSON")?;
    let workload = &value["cert_configs"]["workload"];
    let cert = read(Path::new(&required(workload, "cert_path")?)).await?;
    let key = read(Path::new(&required(workload, "key_path")?)).await?;
    let leaf = parse_der(&cert)?;
    let chain = if let Some(path) = config["trust_chain_path"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        Some(read(Path::new(path)).await?)
    } else {
        None
    };
    let subject = subject(&leaf, chain.as_deref())?;
    // The certificate file itself carries the TLS chain; trust_chain_path is only the subject.
    let mut pem = if CertificateDer::from_pem_slice(&cert).is_ok() {
        cert
    } else {
        format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            STANDARD.encode(&leaf)
        )
        .into_bytes()
    };
    pem.push(b'\n');
    pem.extend_from_slice(&key);
    let identity = reqwest::Identity::from_pem(&pem)
        .map_err(|_| "Invalid Google certificate private key or identity")?;
    let http = http::client_with_identity(net, identity)?;
    Ok(Prepared { subject, http })
}

#[cfg(test)]
#[path = "google_certificate_tests.rs"]
mod tests;
