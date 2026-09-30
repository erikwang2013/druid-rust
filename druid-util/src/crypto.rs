use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use base64::Engine;
use zeroize::{Zeroize, Zeroizing};

/// 加密/解密错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// 密钥配置错误（DRUID_CONFIG_KEY 长度或编码非法）
    Key(String),
    /// 加密失败（如明文超出 GCM 长度上限）
    Encrypt(String),
    /// 密文非法：base64 解码失败、长度不足、GCM 认证失败或明文非 UTF-8
    Invalid(String),
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoError::Key(m) => write!(f, "crypto key error: {m}"),
            CryptoError::Encrypt(m) => write!(f, "encrypt error: {m}"),
            CryptoError::Invalid(m) => write!(f, "invalid ciphertext: {m}"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// 解析 DRUID_CONFIG_KEY：必须是 32 字节，hex（64 字符）或 base64（43/44 字符）编码
///
/// 长度不符一律返回 None，绝不截断/补零：
/// - 补零会把弱口令直接当成密钥，且这里没有 KDF/盐/迭代，拿到密文即可高速爆破；
/// - 截断会让轮换后仍落在相同的前缀密钥上，运维误以为已轮换。
///
/// 待办：若将来要支持口令，应在解析前用 HKDF-SHA256 派生（固定 salt + info="druid-config-v1"），
/// 当前仓库无 hkdf/sha2 依赖，未实现。
fn parse_key(value: &str) -> Option<Zeroizing<Vec<u8>>> {
    let v = value.trim();
    let raw = if v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()) {
        (0..v.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&v[i..i + 2], 16).ok())
            .collect::<Option<Vec<u8>>>()?
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(v)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(v))
            .ok()?
    };
    if raw.len() == 32 {
        Some(Zeroizing::new(raw))
    } else {
        None
    }
}

fn get_key() -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    static KEY: std::sync::OnceLock<Result<Zeroizing<Vec<u8>>, CryptoError>> =
        std::sync::OnceLock::new();
    KEY.get_or_init(|| match std::env::var("DRUID_CONFIG_KEY") {
        Ok(value) => parse_key(&value).ok_or_else(|| {
            CryptoError::Key(format!(
                "DRUID_CONFIG_KEY must decode to exactly 32 bytes \
                 (hex: 64 chars, base64: 43-44 chars), got {} chars",
                value.trim().len()
            ))
        }),
        Err(_) => {
            tracing::warn!("DRUID_CONFIG_KEY not set, using random per-process key (passwords cannot be shared across processes)");
            Ok(Zeroizing::new(Aes256Gcm::generate_key(OsRng).to_vec()))
        }
    })
    .clone()
}

/// AES-256-GCM 加密，返回 base64(nonce || 密文+tag)
pub fn encrypt(plain: &str) -> Result<String, CryptoError> {
    let key = get_key()?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plain.as_bytes())
        .map_err(|e| CryptoError::Encrypt(format!("AES-GCM encrypt failed: {e}")))?;
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(&combined))
}

/// AES-256-GCM 解密（base64(nonce || 密文+tag)），明文以 `Zeroizing<String>` 返回
pub fn decrypt(encrypted: &str) -> Result<Zeroizing<String>, CryptoError> {
    let key = get_key()?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let combined = base64::engine::general_purpose::STANDARD
        .decode(encrypted)
        .map_err(|e| CryptoError::Invalid(format!("invalid base64: {e}")))?;
    if combined.len() < 12 {
        return Err(CryptoError::Invalid("payload shorter than nonce".into()));
    }
    let nonce = Nonce::from_slice(&combined[..12]);
    let plaintext = cipher
        .decrypt(nonce, &combined[12..])
        .map_err(|_| CryptoError::Invalid("AES-GCM authentication failed".into()))?;
    // 明文清零：成功路径把同一块堆内存的所有权移交给 Zeroizing<String>（Drop 时清零）；
    // 失败路径先从 FromUtf8Error 取回原始字节主动清零，避免明文残留在已释放的堆块里
    match String::from_utf8(plaintext) {
        Ok(s) => Ok(Zeroizing::new(s)),
        Err(e) => {
            let mut bytes = e.into_bytes();
            bytes.zeroize();
            Err(CryptoError::Invalid("plaintext is not valid UTF-8".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_parse_accepts_hex_and_base64() {
        let hex32 = "a".repeat(64);
        let k = parse_key(&hex32).unwrap();
        assert_eq!(k.len(), 32);
        assert_eq!(k[0], 0xaa);

        let base64_44 = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        assert_eq!(parse_key(&base64_44).unwrap().as_slice(), &[7u8; 32][..]);
        // 无 padding 的 base64 同样接受
        let no_pad = base64::engine::general_purpose::STANDARD_NO_PAD.encode([7u8; 32]);
        assert_eq!(parse_key(&no_pad).unwrap().as_slice(), &[7u8; 32][..]);
        // 两侧空白（如 $(cat key.txt) 带来的换行）容忍
        assert!(parse_key(&format!("  {hex32}\n")).is_some());
    }

    #[test]
    fn test_key_parse_rejects_non_32_bytes() {
        assert!(parse_key("").is_none());
        assert!(parse_key("hunter2").is_none()); // 短口令
        assert!(parse_key("0123456789abcdef0123456789abcdef").is_none()); // 32 字符口令 ≠ 32 字节密钥
                                                                          // 超长值：旧实现静默截断成相同前缀密钥，现在两个都必须被拒绝
        assert!(parse_key(&format!("k{}", "x".repeat(100))).is_none());
        assert!(parse_key(&format!("k{}", "y".repeat(100))).is_none());
        // 合法编码但解码长度不是 32 字节
        let short = base64::engine::general_purpose::STANDARD.encode([1u8; 16]);
        assert!(parse_key(&short).is_none());
        let long = base64::engine::general_purpose::STANDARD.encode([1u8; 64]);
        assert!(parse_key(&long).is_none());
        let short_hex = "ab".repeat(16); // 32 字符 hex → 16 字节
        assert!(parse_key(&short_hex).is_none());
    }

    #[test]
    fn test_aes_zeroize_feature_enabled() {
        // 轮密钥清零的 Drop 实现由 aes/zeroize feature 决定，feature 掉了这里编译失败
        fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
        assert_zeroize_on_drop::<aes::Aes256>();
    }

    #[test]
    fn test_encrypt_decrypt() {
        let plain = "my_secret_password";
        let encrypted = encrypt(plain).unwrap();
        assert_ne!(encrypted, plain);
        assert_eq!(decrypt(&encrypted).unwrap().as_str(), plain);
    }

    #[test]
    fn test_encrypt_empty() {
        let encrypted = encrypt("").unwrap();
        assert_eq!(decrypt(&encrypted).unwrap().as_str(), "");
    }

    #[test]
    fn test_decrypt_invalid() {
        assert!(decrypt("!!!invalid!!!").is_err());
    }

    #[test]
    fn test_encrypt_is_randomized() {
        // 相同明文两次加密结果不同（随机 nonce）
        let a = encrypt("same").unwrap();
        let b = encrypt("same").unwrap();
        assert_ne!(a, b);
        assert_eq!(decrypt(&a).unwrap().as_str(), "same");
        assert_eq!(decrypt(&b).unwrap().as_str(), "same");
    }

    #[test]
    fn test_encrypt_unicode() {
        let plain = "密码 123 🚀";
        assert_eq!(decrypt(&encrypt(plain).unwrap()).unwrap().as_str(), plain);
    }

    #[test]
    fn test_decrypt_short_payload() {
        // 合法 base64 但长度 < 12（不足 nonce）→ Err
        assert!(decrypt("").is_err());
        assert!(decrypt("aGVsbG8=").is_err()); // "hello"
        assert!(decrypt("AAAAAAAA").is_err());
    }

    #[test]
    fn test_decrypt_tampered_ciphertext() {
        let encrypted = encrypt("attack at dawn").unwrap();
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(&encrypted)
            .unwrap();
        // 篡改密文部分（nonce 之后的载荷），GCM 认证必须失败
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let tampered = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert!(decrypt(&tampered).is_err());
    }

    #[test]
    fn test_decrypt_tampered_nonce() {
        let encrypted = encrypt("tamper me").unwrap();
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(&encrypted)
            .unwrap();
        bytes[0] ^= 0x01;
        let tampered = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert!(decrypt(&tampered).is_err());
    }

    #[test]
    fn test_decrypt_truncated_payload() {
        // 密文被截断（不足 GCM tag）→ 认证失败 → Err
        let encrypted = encrypt("payload-to-truncate").unwrap();
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(&encrypted)
            .unwrap();
        bytes.truncate(15); // 12 字节 nonce + 3 字节密文
        let truncated = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert!(decrypt(&truncated).is_err());
    }

    #[test]
    fn test_decrypt_invalid_utf8_returns_err() {
        // 用当前密钥直接加密一段非法 UTF-8 字节，覆盖 from_utf8 失败（明文清零）路径
        let key = get_key().unwrap();
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let mut combined = nonce.to_vec();
        combined.extend_from_slice(&cipher.encrypt(&nonce, &[0xffu8, 0xfe, 0xfd][..]).unwrap());
        let encoded = base64::engine::general_purpose::STANDARD.encode(&combined);
        assert!(matches!(decrypt(&encoded), Err(CryptoError::Invalid(_))));
    }
}
