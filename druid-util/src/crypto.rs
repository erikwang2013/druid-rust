use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use base64::Engine;
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
    let ciphertext = match cipher.encrypt(&nonce, plain.as_bytes()) {
        Ok(ct) => ct,
        Err(e) => {
            tracing::error!("AES-GCM encrypt failed: {}", e);
            return String::new();
        }
    };
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    base64::engine::general_purpose::STANDARD.encode(&combined)
}

pub fn decrypt(encrypted: &str) -> Option<String> {
    let key = get_key();
    let key = Key::<Aes256Gcm>::from_slice(&key);
    let cipher = Aes256Gcm::new(key);
    let combined = base64::engine::general_purpose::STANDARD
        .decode(encrypted)
        .ok()?;
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

    #[test]
    fn test_encrypt_is_randomized() {
        // 相同明文两次加密结果不同（随机 nonce）
        let a = encrypt("same");
        let b = encrypt("same");
        assert_ne!(a, b);
        assert_eq!(decrypt(&a), Some("same".to_string()));
        assert_eq!(decrypt(&b), Some("same".to_string()));
    }

    #[test]
    fn test_encrypt_unicode() {
        let plain = "密码 123 🚀";
        let decrypted = decrypt(&encrypt(plain)).unwrap();
        assert_eq!(decrypted, plain);
    }

    #[test]
    fn test_decrypt_short_payload() {
        // 合法 base64 但长度 < 12（不足 nonce）→ None
        assert!(decrypt("").is_none());
        assert!(decrypt("aGVsbG8=").is_none()); // "hello"
        assert!(decrypt("AAAAAAAA").is_none());
    }

    #[test]
    fn test_decrypt_tampered_ciphertext() {
        use base64::Engine;
        let encrypted = encrypt("attack at dawn");
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(&encrypted)
            .unwrap();
        // 篡改密文部分（nonce 之后的载荷），GCM 认证必须失败
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let tampered = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert_eq!(decrypt(&tampered), None);
    }

    #[test]
    fn test_decrypt_tampered_nonce() {
        use base64::Engine;
        let encrypted = encrypt("tamper me");
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(&encrypted)
            .unwrap();
        bytes[0] ^= 0x01;
        let tampered = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert_eq!(decrypt(&tampered), None);
    }

    #[test]
    fn test_decrypt_truncated_payload() {
        // 密文被截断（不足 GCM tag）→ 认证失败 → None
        use base64::Engine;
        let encrypted = encrypt("payload-to-truncate");
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(&encrypted)
            .unwrap();
        bytes.truncate(15); // 12 字节 nonce + 3 字节密文
        let truncated = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert_eq!(decrypt(&truncated), None);
    }
}
