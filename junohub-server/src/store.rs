use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn hash_key(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_hash_is_stable_and_not_plaintext() {
        let hash = hash_key("sk-juno-example");
        assert_eq!(hash, hash_key("sk-juno-example"));
        assert_ne!(hash, "sk-juno-example");
    }
}
