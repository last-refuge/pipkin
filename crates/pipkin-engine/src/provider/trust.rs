//! Additional verified server roots, shared by HTTP, credential exchange and WebSockets.
use std::io::Read;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio_rustls::rustls::{
    ClientConfig, RootCertStore,
    pki_types::{CertificateDer, pem::PemObject},
};

const MAX_BUNDLE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct ExtraRoots {
    certificates: Vec<Vec<u8>>,
    warning: Option<String>,
}
impl std::fmt::Debug for ExtraRoots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtraRoots")
            .field("fingerprint", &self.fingerprint())
            .field("warning", &self.warning)
            .finish()
    }
}
impl ExtraRoots {
    /// `NODE_EXTRA_CA_CERTS` is retained as a migration-compatible configuration name.
    /// Unlike Node's startup snapshot, native refreshes this at the next network snapshot.
    pub fn from_env(env: &crate::config::Env) -> Self {
        let Some(path) = env.get("NODE_EXTRA_CA_CERTS") else {
            return Self::default();
        };
        let mut roots = Self::default();
        let read = || -> Result<Vec<u8>, &'static str> {
            let metadata =
                std::fs::metadata(&path).map_err(|_| "cannot inspect certificate file")?;
            if !metadata.is_file() {
                return Err("certificate bundle is not a regular file");
            }
            if metadata.len() > MAX_BUNDLE_BYTES {
                return Err("certificate bundle exceeds 1 MiB");
            }
            let file = std::fs::File::open(&path).map_err(|_| "cannot read certificate file")?;
            let mut bytes = Vec::new();
            file.take(MAX_BUNDLE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "cannot read certificate file")?;
            if bytes.len() as u64 > MAX_BUNDLE_BYTES {
                return Err("certificate bundle exceeds 1 MiB");
            }
            Ok(bytes)
        };
        match read() {
            Ok(bytes) => {
                let mut check = RootCertStore::empty();
                for certificate in CertificateDer::pem_slice_iter(&bytes) {
                    match certificate {
                        Ok(cert) if check.add(cert.clone()).is_ok() => {
                            roots.certificates.push(cert.to_vec())
                        }
                        _ => {
                            roots.warning = Some("invalid certificate; only preceding valid certificates were loaded".into());
                            break;
                        }
                    }
                }
            }
            Err(message) => roots.warning = Some(message.into()),
        }
        roots
    }

    pub fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        for cert in &self.certificates {
            digest.update((cert.len() as u64).to_be_bytes());
            digest.update(cert);
        }
        format!("{:x}", digest.finalize())
    }

    pub(crate) fn configure_http(
        &self,
        mut builder: reqwest::ClientBuilder,
    ) -> reqwest::ClientBuilder {
        if let Some(warning) = &self.warning {
            log::warn!("NODE_EXTRA_CA_CERTS: {warning}");
        }
        for cert in &self.certificates {
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_der(cert).expect("validated certificate"),
            );
        }
        builder
    }

    pub(crate) fn tls_config(&self) -> Arc<ClientConfig> {
        let mut roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for cert in &self.certificates {
            roots
                .add(CertificateDer::from(cert.clone()))
                .expect("validated certificate");
        }
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    }
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod tests;
