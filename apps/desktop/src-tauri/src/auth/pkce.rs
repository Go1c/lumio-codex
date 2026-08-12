//! PKCE (RFC 7636) parameters for the browser login flow.
//!
//! The desktop app is a public OAuth client: it holds no client secret, so the
//! authorization code is bound to a one-time verifier that never leaves this
//! process. The control plane rejects anything but `S256`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::TryRngCore;
use sha2::{Digest, Sha256};

/// Number of random bytes behind a code verifier. 32 bytes base64url-encode to
/// 43 characters, the shortest length RFC 7636 allows.
const VERIFIER_BYTES: usize = 32;

/// A verifier/challenge pair plus the CSRF `state` for one login attempt.
#[derive(Clone)]
pub struct Pkce {
    verifier: String,
    challenge: String,
    state: String,
}

impl Pkce {
    /// Generate a fresh pair from the OS random source.
    pub fn generate() -> Result<Self, String> {
        let verifier = random_urlsafe(VERIFIER_BYTES)?;
        let challenge = challenge_for(&verifier);
        let state = random_urlsafe(16)?;
        Ok(Self {
            verifier,
            challenge,
            state,
        })
    }

    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    pub fn state(&self) -> &str {
        &self.state
    }
}

/// Redacted on purpose: the verifier is as sensitive as the resulting token.
impl std::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pkce([REDACTED])")
    }
}

/// `BASE64URL(SHA256(verifier))` without padding.
pub fn challenge_for(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn random_urlsafe(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng
        .try_fill_bytes(&mut buf)
        .map_err(|e| format!("无法生成随机数：{e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 appendix B reference vector.
    #[test]
    fn challenge_matches_rfc7636_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_uses_the_unreserved_charset_and_legal_length() {
        let pkce = Pkce::generate().expect("generate");
        assert_eq!(pkce.verifier().len(), 43);
        assert!((43..=128).contains(&pkce.verifier().len()));
        assert!(
            pkce.verifier()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
        );
    }

    #[test]
    fn challenge_is_derived_from_its_own_verifier() {
        let pkce = Pkce::generate().expect("generate");
        assert_eq!(pkce.challenge(), challenge_for(pkce.verifier()));
        // The control plane rejects challenges shorter than 43 characters.
        assert_eq!(pkce.challenge().len(), 43);
    }

    #[test]
    fn each_attempt_gets_distinct_secrets() {
        let a = Pkce::generate().expect("generate");
        let b = Pkce::generate().expect("generate");
        assert_ne!(a.verifier(), b.verifier());
        assert_ne!(a.state(), b.state());
    }

    #[test]
    fn debug_never_leaks_the_verifier() {
        let pkce = Pkce::generate().expect("generate");
        let rendered = format!("{pkce:?}");
        assert!(!rendered.contains(pkce.verifier()));
        assert_eq!(rendered, "Pkce([REDACTED])");
    }
}
