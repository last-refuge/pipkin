//! PKCE (S256) and random state values (reference `auth/oauth/pkce.ts`).

use base64::Engine;
use sha2::{Digest, Sha256};

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).expect("the OS random source is available");
    buf
}

pub fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A PKCE pair: 32 random bytes as base64url verifier, SHA-256 base64url challenge.
#[derive(Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl std::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pkce([redacted])")
    }
}

pub fn pkce() -> Pkce {
    let verifier = b64url(&random_bytes(32));
    let challenge = challenge_for(&verifier);
    Pkce {
        verifier,
        challenge,
    }
}

pub fn challenge_for(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

/// Decode a JWT payload (base64url or base64, padding optional). `None` when malformed.
pub fn jwt_payload(token: &str) -> Option<serde_json::Value> {
    let mut parts = token.split('.');
    let (_, payload, _) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let normalized: String = payload
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(normalized)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s256_challenge_matches_an_independent_implementation() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mJ92K0b7NNkSvW1rLc4f1lPRx0vrbA"),
            "D8tg4KEaQ3VAj_FiMUIUtN08-Jz2hIM1kozsKcESdUI"
        );
        let p = pkce();
        assert_eq!(p.verifier.len(), 43);
        assert_eq!(p.challenge, challenge_for(&p.verifier));
    }

    #[test]
    fn jwt_payloads_decode_leniently() {
        let payload = b64url(br#"{"sub":"x","n":1}"#);
        assert_eq!(jwt_payload(&format!("h.{payload}.s")).unwrap()["sub"], "x");
        assert!(jwt_payload("not-a-jwt").is_none());
        assert!(jwt_payload("a.b.c.d").is_none());
    }
}
