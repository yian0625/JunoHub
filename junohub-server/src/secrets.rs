use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

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
    }
}
