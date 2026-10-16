use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// 账户引擎托管供应商的 base_url 标记值。
///
/// 托管供应商的请求走进程内的账户引擎（`sub2_gateway::forward`），这个字段
/// **不参与路由**，只是一个标记。曾经它被写成部署拓扑（容器名 + 端口），
/// 换个部署名就失效，还得写一次性迁移去修历史数据 —— 现在只存一个与部署无关的
/// 标记值。它也不是合法的 http(s) URL，因此永远不会被当作真实上游地址使用。
pub const MANAGED_PROVIDER_BASE_URL: &str = "managed:account-engine";

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
