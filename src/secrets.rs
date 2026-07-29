//! Optional at-rest encryption for connection API keys.
//!
//! When `THINROUTER_SECRETS_KEY` (or `--secrets-key`) is set, `api_key` values
//! are stored as `enc:v1:<base64>`. Without a key, values are stored plaintext
//! (same as before). OS keyring is not required; the env key is the secret.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

const PREFIX: &str = "enc:v1:";

/// Derive a 32-byte AES key from an arbitrary passphrase.
fn derive_key(passphrase: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"thinrouter-secrets-v1:");
    hasher.update(passphrase.as_bytes());
    let dig = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&dig);
    key
}

pub fn encrypt_secret(plaintext: &str, passphrase: Option<&str>) -> String {
    let Some(pass) = passphrase.filter(|p| !p.is_empty()) else {
        return plaintext.to_string();
    };
    if plaintext.is_empty() || plaintext.starts_with(PREFIX) {
        return plaintext.to_string();
    }
    let key = derive_key(pass);
    let cipher = Aes256Gcm::new_from_slice(&key).expect("aes key");
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .expect("encrypt");
    let mut packed = Vec::with_capacity(12 + ciphertext.len());
    packed.extend_from_slice(&nonce_bytes);
    packed.extend_from_slice(&ciphertext);
    format!("{PREFIX}{}", B64.encode(packed))
}

pub fn decrypt_secret(stored: &str, passphrase: Option<&str>) -> String {
    if !stored.starts_with(PREFIX) {
        return stored.to_string();
    }
    let Some(pass) = passphrase.filter(|p| !p.is_empty()) else {
        // encrypted but no key — return as-is (upstream will fail auth)
        return stored.to_string();
    };
    let raw = match B64.decode(stored.trim_start_matches(PREFIX)) {
        Ok(r) => r,
        Err(_) => return stored.to_string(),
    };
    if raw.len() < 13 {
        return stored.to_string();
    }
    let (nonce_bytes, ct) = raw.split_at(12);
    let key = derive_key(pass);
    let cipher = match Aes256Gcm::new_from_slice(&key) {
        Ok(c) => c,
        Err(_) => return stored.to_string(),
    };
    let nonce = Nonce::from_slice(nonce_bytes);
    match cipher.decrypt(nonce, ct) {
        Ok(pt) => String::from_utf8_lossy(&pt).into_owned(),
        Err(_) => stored.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let enc = encrypt_secret("sk-secret-123", Some("pass"));
        assert!(enc.starts_with(PREFIX));
        assert_eq!(decrypt_secret(&enc, Some("pass")), "sk-secret-123");
        assert_ne!(decrypt_secret(&enc, Some("wrong")), "sk-secret-123");
    }

    #[test]
    fn plaintext_passthrough() {
        assert_eq!(encrypt_secret("sk-x", None), "sk-x");
        assert_eq!(decrypt_secret("sk-x", Some("pass")), "sk-x");
    }
}
