use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

const ENCRYPTED_VALUE_PREFIX: &str = "enc:v1:";
/// 命中这些名字的字段会被加密，名字按小写比较。
///
/// 这是**正向**清单：只有列出来的字段才加密，其余一律按明文处理。上游
/// 哪天新增一个凭据字段名，它就会悄悄以明文落库。`unprotected_sensitive_fields`
/// 负责把这种情况在启动阶段变成一条显式告警，两者要一起看。
const CREDENTIAL_SECRET_FIELDS: &[&str] = &[
    "access_token",
    "refresh_token",
    "id_token",
    "api_key",
    "apikey",
    "client_secret",
    "api_secret",
    "session_key",
    "session_token",
    "cookie",
    "password",
    "access_key",
    "secret_key",
    "private_key",
    "auth_token",
    "authorization",
    "bearer",
];

/// 名字里出现这些片段的字段按“疑似密钥”对待，用于启动自检
/// （`api_keys`、`codex_token`、`session_cookie` 都能命中）。
/// 刻意不含 `auth` 这种太宽的片段：`oauth_type`、`plan_type` 不是密钥，
/// 误报会把真正需要看的告警淹掉。
const SENSITIVE_NAME_MARKERS: &[&str] = &[
    "token",
    "secret",
    "cookie",
    "password",
    "apikey",
    "api_key",
    "credential",
    "bearer",
    "authorization",
];

pub fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|value| value.to_string())
        .map_err(|error| AppError::Internal(error.to_string()))
}

pub fn verify_password(password: &str, encoded: &str) -> bool {
    PasswordHash::new(encoded).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

fn cipher(master_key: &str) -> Aes256Gcm {
    let key = Sha256::digest(master_key.as_bytes());
    Aes256Gcm::new_from_slice(&key).expect("SHA-256 always returns a 32-byte key")
}

pub fn encrypt(master_key: &str, plaintext: &str) -> AppResult<String> {
    use aes_gcm::aead::rand_core::RngCore;
    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let encrypted = cipher(master_key)
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_bytes())
        .map_err(|_| AppError::Internal("加密供应商密钥失败".into()))?;
    let mut output = nonce_bytes.to_vec();
    output.extend(encrypted);
    Ok(STANDARD.encode(output))
}

pub fn decrypt(master_key: &str, encoded: &str) -> AppResult<String> {
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| AppError::Internal("供应商密钥格式无效".into()))?;
    if bytes.len() < 13 {
        return Err(AppError::Internal("供应商密钥格式无效".into()));
    }
    let plaintext = cipher(master_key)
        .decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..])
        .map_err(|_| AppError::Internal("无法解密供应商密钥".into()))?;
    String::from_utf8(plaintext).map_err(|_| AppError::Internal("供应商密钥格式无效".into()))
}

pub fn protect_credentials(master_key: &str, credentials: &Value) -> AppResult<Value> {
    let mut protected = credentials.clone();
    protect_value(master_key, &mut protected)?;
    Ok(protected)
}

pub fn reveal_credentials(master_key: &str, credentials: &Value) -> AppResult<Value> {
    let mut revealed = credentials.clone();
    reveal_value(master_key, &mut revealed)?;
    Ok(revealed)
}

fn is_protected_field(name: &str) -> bool {
    CREDENTIAL_SECRET_FIELDS.contains(&name.to_ascii_lowercase().as_str())
}

fn protect_value(master_key: &str, value: &mut Value) -> AppResult<()> {
    match value {
        Value::Object(object) => {
            for (name, value) in object {
                if is_protected_field(name) {
                    if let Some(plaintext) = value.as_str() {
                        if !plaintext.is_empty() && !plaintext.starts_with(ENCRYPTED_VALUE_PREFIX) {
                            *value = Value::String(format!(
                                "{ENCRYPTED_VALUE_PREFIX}{}",
                                encrypt(master_key, plaintext)?
                            ));
                        }
                    }
                } else {
                    protect_value(master_key, value)?;
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                protect_value(master_key, value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// 找出凭据里“看着像密钥、却仍是明文”的字段名。
///
/// 加密走的是正向清单，上游一旦启用新的字段名，新密钥会明文落库且不报任何错。
/// 启动时跑一遍这个检测，把沉默的泄漏变成一条看得见的告警。只返回**字段名**，
/// 不返回值，避免把密钥本身写进日志。
pub fn unprotected_sensitive_fields(credentials: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect_unprotected(credentials, &mut found);
    found.sort();
    found.dedup();
    found
}

fn is_sensitive_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "key"
        || name.ends_with("_key")
        || name.ends_with("_token")
        || SENSITIVE_NAME_MARKERS
            .iter()
            .any(|marker| name.contains(marker))
}

fn collect_unprotected(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (name, value) in object {
                if is_sensitive_name(name) {
                    if !is_protected_field(name) && has_plaintext_secret(value) {
                        found.push(name.clone());
                    }
                    // 疑似密钥的字段不再往下钻：里面的内容已经被名字覆盖了。
                    continue;
                }
                collect_unprotected(value, found);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_unprotected(value, found);
            }
        }
        _ => {}
    }
}

fn has_plaintext_secret(value: &Value) -> bool {
    match value {
        Value::String(text) => is_plaintext(text),
        Value::Array(items) => items
            .iter()
            .any(|item| item.as_str().is_some_and(is_plaintext)),
        _ => false,
    }
}

fn is_plaintext(text: &str) -> bool {
    !text.is_empty() && !text.starts_with(ENCRYPTED_VALUE_PREFIX)
}

fn reveal_value(master_key: &str, value: &mut Value) -> AppResult<()> {
    match value {
        Value::String(encoded) if encoded.starts_with(ENCRYPTED_VALUE_PREFIX) => {
            *value = Value::String(decrypt(
                master_key,
                encoded.trim_start_matches(ENCRYPTED_VALUE_PREFIX),
            )?);
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                reveal_value(master_key, value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reveal_value(master_key, value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_and_secret_round_trip() {
        let password = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &password));
        assert!(!verify_password("wrong", &password));
        let encrypted = encrypt("master", "sk-secret").unwrap();
        assert_ne!(encrypted, "sk-secret");
        assert_eq!(decrypt("master", &encrypted).unwrap(), "sk-secret");

        let credentials = serde_json::json!({
            "access_token":"access",
            "refresh_token":"refresh",
            "client_id":"public-client",
            "nested":{"api_key":"key"}
        });
        let protected = protect_credentials("master", &credentials).unwrap();
        assert_ne!(protected["access_token"], "access");
        assert_eq!(protected["client_id"], "public-client");
        assert_ne!(protected["nested"]["api_key"], "key");
        assert_eq!(
            reveal_credentials("master", &protected).unwrap(),
            credentials
        );
        assert_eq!(
            protect_credentials("master", &protected).unwrap(),
            protected
        );
    }

    #[test]
    fn field_names_are_matched_case_insensitively_and_cover_common_variants() {
        let credentials = serde_json::json!({
            "API_KEY": "upper",
            "Access_Token": "mixed",
            "secret_key": "variant",
            "authorization": "Bearer value",
        });
        let protected = protect_credentials("master", &credentials).unwrap();
        for field in ["API_KEY", "Access_Token", "secret_key", "authorization"] {
            assert_ne!(protected[field], credentials[field], "{field} 应当被加密");
            assert_eq!(
                reveal_credentials("master", &protected).unwrap()[field],
                credentials[field]
            );
        }
    }

    #[test]
    fn audit_flags_unknown_secret_names_and_ignores_public_fields() {
        let credentials = serde_json::json!({
            // 在清单内，加密后不再算问题。
            "api_key": "sk-encrypted-elsewhere",
            // 不在清单内、但名字像密钥 —— 应当被点名。
            "codex_bearer": "plain",
            "some_credential": "plain",
            "vendor_token": ["plain", ""],
            // 公开字段与非密钥字段不应误报。
            "base_url": "https://example.test",
            "project_id": "proj-1",
            "oauth_type": "oauth",
            "plan_type": "plus",
            "client_id": "public-client",
            "model_mapping": {"a": "b"},
            "empty_token": "",
            // 空的疑似字段没内容，不必告警。
            "no_value_secret": null,
        });
        assert_eq!(
            unprotected_sensitive_fields(&credentials),
            vec![
                "codex_bearer".to_string(),
                "some_credential".to_string(),
                "vendor_token".to_string(),
            ]
        );

        // 加密过的值不再触发告警。
        let protected = protect_credentials("master", &credentials).unwrap();
        assert_eq!(
            unprotected_sensitive_fields(&protected),
            vec![
                "codex_bearer".to_string(),
                "some_credential".to_string(),
                "vendor_token".to_string(),
            ]
        );
        assert!(unprotected_sensitive_fields(&serde_json::json!({
            "api_key": "enc:v1:abc",
            "base_url": "https://example.test",
        }))
        .is_empty());
    }
}
