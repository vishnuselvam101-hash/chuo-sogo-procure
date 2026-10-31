//! Password hashing (argon2id) and opaque random tokens (stored only as SHA-256).

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const MIN_PASSWORD_CHARS: usize = 12;
pub const MAX_PASSWORD_BYTES: usize = 256;

#[derive(Debug, thiserror::Error)]
#[error("password hashing failed")]
pub struct HashError;

pub fn validate_password(password: &str) -> Result<(), &'static str> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err("password_too_short");
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err("password_too_long");
    }
    Ok(())
}

pub fn hash_password(password: &str) -> Result<String, HashError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| HashError)
}

pub fn verify_password(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc)
        .and_then(|h| Argon2::default().verify_password(password.as_bytes(), &h))
        .is_ok()
}

/// Burn the same time as a real verification so unknown emails are not distinguishable.
pub fn dummy_verify(password: &str) {
    static DUMMY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    if let Some(phc) = DUMMY.get_or_init(|| hash_password("dummy-password-for-timing").ok()) {
        let _ = verify_password(password, phc);
    }
}

/// Runs argon2 off the async runtime, bounded by `permits` so a login flood cannot
/// exhaust CPU/memory. `phc = None` (unknown/inactive user) burns equal time and returns false.
pub async fn verify_password_blocking(
    permits: &tokio::sync::Semaphore,
    password: String,
    phc: Option<String>,
) -> bool {
    let Ok(_permit) = permits.acquire().await else {
        return false;
    };
    tokio::task::spawn_blocking(move || match phc {
        Some(h) => verify_password(&password, &h),
        None => {
            dummy_verify(&password);
            false
        }
    })
    .await
    .unwrap_or(false)
}

pub async fn hash_password_blocking(
    permits: &tokio::sync::Semaphore,
    password: String,
) -> Result<String, HashError> {
    let _permit = permits.acquire().await.map_err(|_| HashError)?;
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| HashError)?
}

/// 256-bit random token, hex encoded for transport. Returns (token, sha256(token)).
pub fn new_token() -> (String, Vec<u8>) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let hash = token_hash(&token);
    (token, hash)
}

pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash_password("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &h));
        assert!(!verify_password("wrong horse battery", &h));
        assert!(!verify_password("x", "not-a-phc-string"));
    }

    #[test]
    fn password_policy() {
        assert_eq!(validate_password("short"), Err("password_too_short"));
        assert_eq!(validate_password("１２３４５６７８９０１２"), Ok(())); // counts chars, not bytes
        assert_eq!(
            validate_password(&"a".repeat(300)),
            Err("password_too_long")
        );
    }

    #[test]
    fn tokens_are_unique_and_hashed() {
        let (a, ha) = new_token();
        let (b, _) = new_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert_eq!(ha, token_hash(&a));
        assert_eq!(ha.len(), 32);
    }
}
