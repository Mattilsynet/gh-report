//! HMAC-SHA256 webhook signature verification.
//!
//! Validates GitHub webhook signatures using constant-time comparison
//! via [`hmac::Mac::verify_slice`] (AD5). Never compares hex strings
//! with `==` — that would leak timing information.

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;

/// Decode a hex string into bytes.
///
/// Returns `None` if the string contains non-ASCII bytes, has odd length,
/// or contains non-hex characters.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.is_ascii() || !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Verify a GitHub webhook HMAC-SHA256 signature.
///
/// # Arguments
///
/// * `secret` — The webhook secret (arbitrary string; raw UTF-8 bytes
///   are the HMAC key).
/// * `body` — The raw request body bytes.
/// * `signature_header` — The value of the `X-Hub-Signature-256` header,
///   expected to be `sha256=<hex-encoded-mac>`.
///
/// # Returns
///
/// `true` if the signature is valid, `false` otherwise (wrong prefix,
/// invalid hex, or HMAC mismatch).
///
/// # Panics
///
/// Panics if the HMAC implementation rejects the key, which cannot
/// happen because HMAC-SHA256 accepts keys of any length.
#[must_use]
pub fn verify_signature(secret: &SecretString, body: &[u8], signature_header: &str) -> bool {
    let Some(hex_str) = signature_header.strip_prefix("sha256=") else {
        return false;
    };

    let Some(expected) = hex_decode(hex_str) else {
        return false;
    };

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.expose_secret().as_bytes())
        .expect("HMAC accepts any key size");
    mac.update(body);

    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode bytes as a lowercase hex string.
    fn hex_encode(bytes: &[u8]) -> String {
        use std::fmt::Write;
        bytes
            .iter()
            .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
                let _ = write!(s, "{b:02x}");
                s
            })
    }

    /// Helper: compute the correct HMAC-SHA256 signature for a body.
    fn compute_signature(secret: &str, body: &[u8]) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
        mac.update(body);
        let result = mac.finalize();
        format!("sha256={}", hex_encode(&result.into_bytes()))
    }

    #[test]
    fn hmac_valid_signature() {
        let secret = SecretString::from("test-secret".to_string());
        let body = b"hello world";
        let sig = compute_signature("test-secret", body);
        assert!(verify_signature(&secret, body, &sig));
    }

    #[test]
    fn hmac_invalid_signature() {
        let secret = SecretString::from("test-secret".to_string());
        let body = b"hello world";
        let sig = compute_signature("test-secret", b"tampered body");
        assert!(!verify_signature(&secret, body, &sig));
    }

    #[test]
    fn hmac_missing_prefix() {
        let secret = SecretString::from("test-secret".to_string());
        let body = b"hello world";
        assert!(!verify_signature(&secret, body, "abcdef1234567890"));
    }

    #[test]
    fn hmac_wrong_algorithm() {
        let secret = SecretString::from("test-secret".to_string());
        let body = b"hello world";
        let sig = compute_signature("test-secret", body);
        let sha1_sig = sig.replace("sha256=", "sha1=");
        assert!(!verify_signature(&secret, body, &sha1_sig));
    }

    #[test]
    fn hmac_empty_body() {
        let secret = SecretString::from("test-secret".to_string());
        let body = b"";
        let sig = compute_signature("test-secret", body);
        assert!(verify_signature(&secret, body, &sig));
    }

    #[test]
    fn hmac_invalid_hex() {
        let secret = SecretString::from("test-secret".to_string());
        let body = b"hello";
        assert!(!verify_signature(&secret, body, "sha256=ZZZZ"));
    }

    #[test]
    fn hmac_wrong_secret() {
        let secret = SecretString::from("correct-secret".to_string());
        let body = b"hello world";
        let sig = compute_signature("wrong-secret", body);
        assert!(!verify_signature(&secret, body, &sig));
    }

    #[test]
    fn hex_decode_empty_string() {
        assert_eq!(hex_decode(""), Some(vec![]));
    }

    #[test]
    fn hex_decode_valid_lowercase() {
        assert_eq!(hex_decode("deadbeef"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
    }

    #[test]
    fn hex_decode_valid_uppercase() {
        assert_eq!(hex_decode("DEADBEEF"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
    }

    #[test]
    fn hex_decode_odd_length_returns_none() {
        assert_eq!(hex_decode("abc"), None);
    }

    #[test]
    fn hex_decode_invalid_chars_returns_none() {
        assert_eq!(hex_decode("zzzz"), None);
    }

    #[test]
    fn hex_decode_multibyte_utf8_returns_none() {
        assert_eq!(hex_decode("café"), None);
    }

    #[test]
    fn hex_decode_emoji_returns_none() {
        assert_eq!(hex_decode("😀😀"), None);
    }
}
