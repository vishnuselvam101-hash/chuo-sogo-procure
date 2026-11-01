//! Second factor for the admin console: TOTP (RFC 6238, SHA-1, 6 digits, 30 s — what common
//! authenticator apps use) and encryption of the per-user secrets at rest (AES-256-GCM with
//! a key held outside the database, `PROCURE_SECRET_KEY`).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;

pub const STEP_SECONDS: i64 = 30;
pub const DIGITS: u32 = 6;
const SECRET_BYTES: usize = 20;
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn new_secret() -> Vec<u8> {
    let mut s = vec![0u8; SECRET_BYTES];
    rand::rng().fill_bytes(&mut s);
    s
}

/// RFC 4648 base32 without padding (what authenticator apps expect).
pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in bytes {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// The code for time step `step`.
pub fn code_at(secret: &[u8], step: i64) -> u32 {
    let Ok(mut mac) = <Hmac<Sha1> as Mac>::new_from_slice(secret) else {
        return u32::MAX; // unreachable: HMAC accepts any key length; never a valid code
    };
    mac.update(&step.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let offset = usize::from(h[h.len() - 1] & 0x0f);
    let bin = (u32::from(h[offset] & 0x7f) << 24)
        | (u32::from(h[offset + 1]) << 16)
        | (u32::from(h[offset + 2]) << 8)
        | u32::from(h[offset + 3]);
    bin % 10u32.pow(DIGITS)
}

/// Checks `code` against the current step ±1 (clock drift) and only accepts steps newer than
/// `last_step` (no replay). Returns the matched step.
pub fn verify(secret: &[u8], code: &str, unix_now: i64, last_step: Option<i64>) -> Option<i64> {
    let code = code.trim();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let wanted: u32 = code.parse().ok()?;
    let now = unix_now.div_euclid(STEP_SECONDS);
    (now - 1..=now + 1)
        .filter(|s| last_step.is_none_or(|l| *s > l))
        .find(|s| constant_time_eq(code_at(secret, *s), wanted))
}

fn constant_time_eq(a: u32, b: u32) -> bool {
    (a ^ b) == 0
}

/// Decodes RFC 4648 base32 (no padding, case-insensitive). `None` on invalid input.
pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in s.trim().bytes() {
        let v = BASE32.iter().position(|b| *b == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

pub fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    let enc = |s: &str| -> String {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~@".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    };
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        enc(issuer),
        enc(account),
        base32(secret),
        enc(issuer)
    )
}

/// Encrypts small secrets for storage: output is nonce (12 bytes) || ciphertext+tag.
pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl SecretBox {
    /// `hex_key`: 64 hex characters (32 bytes).
    pub fn from_hex(hex_key: &str) -> Option<Self> {
        let key = hex::decode(hex_key.trim()).ok()?;
        if key.len() != 32 {
            return None;
        }
        Aes256Gcm::new_from_slice(&key)
            .ok()
            .map(|cipher| SecretBox { cipher })
    }

    pub fn seal(&self, plain: &[u8]) -> Option<Vec<u8>> {
        let mut nonce = [0u8; 12];
        rand::rng().fill_bytes(&mut nonce);
        let ct = self.cipher.encrypt(Nonce::from_slice(&nonce), plain).ok()?;
        let mut out = nonce.to_vec();
        out.extend(ct);
        Some(out)
    }

    pub fn open(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        if sealed.len() < 12 + 16 {
            return None;
        }
        let (nonce, ct) = sealed.split_at(12);
        self.cipher.decrypt(Nonce::from_slice(nonce), ct).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_test_vectors() {
        // RFC 6238 appendix B, SHA-1 secret "12345678901234567890" (8-digit codes there; our
        // 6-digit codes are the same values mod 10^6).
        let secret = b"12345678901234567890";
        for (t, eight) in [
            (59, 94_287_082),
            (1_111_111_109, 7_081_804),
            (1_234_567_890, 89_005_924),
            (2_000_000_000, 69_279_037),
        ] {
            assert_eq!(
                code_at(secret, t / STEP_SECONDS),
                eight % 1_000_000,
                "t={t}"
            );
        }
    }

    #[test]
    fn verification_window_and_replay() {
        let secret = b"12345678901234567890";
        let now = 1_234_567_890;
        let step = now / STEP_SECONDS;
        let code = format!("{:06}", code_at(secret, step));
        assert_eq!(verify(secret, &code, now, None), Some(step));
        // Same code again is refused (replay).
        assert_eq!(verify(secret, &code, now, Some(step)), None);
        // Previous step accepted for clock drift, two steps back refused.
        let prev = format!("{:06}", code_at(secret, step - 1));
        assert_eq!(verify(secret, &prev, now, None), Some(step - 1));
        let old = format!("{:06}", code_at(secret, step - 2));
        assert_eq!(verify(secret, &old, now, None), None);
        for bad in ["", "12345", "1234567", "abcdef", "12 345"] {
            assert_eq!(verify(secret, bad, now, None), None, "{bad}");
        }
    }

    #[test]
    fn base32_matches_rfc4648() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32(b"f"), "MY");
        assert_eq!(base32(&new_secret()).len(), 32);
    }

    #[test]
    fn secrets_round_trip_and_tampering_is_detected() {
        let b = SecretBox::from_hex(&"11".repeat(32)).unwrap();
        let sealed = b.seal(b"secret").unwrap();
        assert_eq!(b.open(&sealed).unwrap(), b"secret");
        let mut bad = sealed.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(b.open(&bad).is_none());
        let other = SecretBox::from_hex(&"22".repeat(32)).unwrap();
        assert!(other.open(&sealed).is_none());
        assert!(SecretBox::from_hex("abcd").is_none());
    }

    #[test]
    fn otpauth_uri_is_well_formed() {
        let uri = otpauth_uri("Chuo-Sogo Procure", "root@chuo-sogo.test", b"foobar");
        assert_eq!(
            uri,
            "otpauth://totp/Chuo-Sogo%20Procure:root@chuo-sogo.test?secret=MZXW6YTBOI&issuer=Chuo-Sogo%20Procure&algorithm=SHA1&digits=6&period=30"
        );
    }
}
