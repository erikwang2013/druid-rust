use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use zeroize::Zeroizing;

fn get_key() -> Zeroizing<Vec<u8>> {
    static KEY: std::sync::OnceLock<Zeroizing<Vec<u8>>> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        if let Ok(env_key) = std::env::var("DRUID_CONFIG_KEY") {
            let mut key = env_key.into_bytes();
            key.resize(32, 0);
            Zeroizing::new(key)
        } else {
            tracing::warn!("DRUID_CONFIG_KEY not set, using random per-process key (passwords cannot be shared across processes)");
            Zeroizing::new(Aes256Gcm::generate_key(OsRng).to_vec())
        }
    })
    .clone()
}

pub fn encrypt(plain: &str) -> String {
    let key = get_key();
    let key = Key::<Aes256Gcm>::from_slice(&key);
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plain.as_bytes())
        .expect("AES-GCM encrypt failed");
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    base64_encode(&combined)
}

pub fn decrypt(encrypted: &str) -> Option<String> {
    let key = get_key();
    let key = Key::<Aes256Gcm>::from_slice(&key);
    let cipher = Aes256Gcm::new(key);
    let combined = base64_decode(encrypted)?;
    if combined.len() < 12 {
        return None;
    }
    let nonce = Nonce::from_slice(&combined[..12]);
    let ciphertext = &combined[12..];
    cipher
        .decrypt(nonce, ciphertext)
        .ok()
        .and_then(|v| String::from_utf8(v).ok())
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((n >> 18) & 0x3f) as usize] as char);
        result.push(CHARS[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((n >> 6) & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(n & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut result = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in s.chars() {
        let val = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            result.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let plain = "my_secret_password";
        let encrypted = encrypt(plain);
        assert_ne!(encrypted, plain);
        let decrypted = decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plain);
    }

    #[test]
    fn test_encrypt_empty() {
        let encrypted = encrypt("");
        let decrypted = decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_decrypt_invalid() {
        assert!(decrypt("!!!invalid!!!").is_none());
    }
}
