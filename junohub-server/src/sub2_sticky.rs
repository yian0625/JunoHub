use redis::{aio::ConnectionManager, AsyncCommands};
use serde::Serialize;
use sha2::{Digest, Sha256};
use xxhash_rust::xxh64::xxh64;

use crate::error::{AppError, AppResult};

const CLAIM_SESSION_WINDOW_SCRIPT: &str = r#"
local previous = redis.call('GET', KEYS[1])
redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[2])
return previous
"#;

const COMPARE_REFRESH_SESSION_WINDOW_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
if current == false or current ~= ARGV[1] then return 0 end
redis.call('PEXPIRE', KEYS[1], ARGV[2])
return 1
"#;

const COMPARE_DELETE_SESSION_WINDOW_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
if current == false or current ~= ARGV[1] then return 0 end
redis.call('DEL', KEYS[1])
return 1
"#;

#[derive(Clone)]
pub struct StickyStore {
    connection: ConnectionManager,
    sticky_ttl_seconds: i64,
    response_ttl_seconds: i64,
    read_legacy: bool,
    write_legacy: bool,
}

#[derive(Debug, Serialize)]
pub struct StickyProbe {
    pub group_id: i64,
    pub account_id: i64,
    pub current_hash: String,
    pub legacy_hash: String,
    pub session_account_id: i64,
    pub primary_ttl_seconds: i64,
    pub legacy_written: bool,
    pub legacy_ttl_seconds: i64,
    pub response_account_id: i64,
    pub owner_user_id: i64,
    pub owner_api_key_id: i64,
    pub same_user_authorized: bool,
    pub other_user_authorized: bool,
    pub first_window_claim_had_owner: bool,
    pub second_window_claim_returned_first: bool,
    pub stale_window_refresh: bool,
    pub current_window_refresh: bool,
    pub stale_window_delete: bool,
    pub current_window_delete: bool,
    pub residual_keys: i64,
}

impl StickyStore {
    pub fn new(
        connection: ConnectionManager,
        sticky_ttl_seconds: i64,
        response_ttl_seconds: i64,
        read_legacy: bool,
        write_legacy: bool,
    ) -> Self {
        Self {
            connection,
            sticky_ttl_seconds,
            response_ttl_seconds,
            read_legacy,
            write_legacy,
        }
    }

    pub async fn claim_session_window(
        &self,
        group_id: i64,
        session_hash: &str,
        owner: &str,
        ttl_millis: i64,
    ) -> AppResult<Option<String>> {
        let key = session_window_key(group_id, session_hash)?;
        validate_window_owner(owner, ttl_millis)?;
        let mut connection = self.connection.clone();
        let previous: Option<String> = redis::Script::new(CLAIM_SESSION_WINDOW_SCRIPT)
            .key(key)
            .arg(owner.as_bytes())
            .arg(ttl_millis)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(previous)
    }

    pub async fn compare_refresh_session_window(
        &self,
        group_id: i64,
        session_hash: &str,
        owner: &str,
        ttl_millis: i64,
    ) -> AppResult<bool> {
        let key = session_window_key(group_id, session_hash)?;
        validate_window_owner(owner, ttl_millis)?;
        let mut connection = self.connection.clone();
        let refreshed: i64 = redis::Script::new(COMPARE_REFRESH_SESSION_WINDOW_SCRIPT)
            .key(key)
            .arg(owner.as_bytes())
            .arg(ttl_millis)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(refreshed == 1)
    }

    pub async fn compare_delete_session_window(
        &self,
        group_id: i64,
        session_hash: &str,
        owner: &str,
    ) -> AppResult<bool> {
        let key = session_window_key(group_id, session_hash)?;
        validate_window_owner(owner, 1)?;
        let mut connection = self.connection.clone();
        let deleted: i64 = redis::Script::new(COMPARE_DELETE_SESSION_WINDOW_SCRIPT)
            .key(key)
            .arg(owner.as_bytes())
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(deleted == 1)
    }

    pub async fn get_session_account(
        &self,
        group_id: i64,
        current_hash: &str,
        legacy_hash: Option<&str>,
    ) -> AppResult<Option<i64>> {
        let current_key = session_key(group_id, &format!("openai:{}", current_hash.trim()))?;
        if let Some(account_id) = self.get_positive_i64(&current_key).await? {
            return Ok(Some(account_id));
        }
        if self.read_legacy {
            if let Some(legacy_hash) = legacy_hash.filter(|value| !value.trim().is_empty()) {
                let legacy_key = session_key(group_id, &format!("openai:{}", legacy_hash.trim()))?;
                if legacy_key != current_key {
                    return self.get_positive_i64(&legacy_key).await;
                }
            }
        }
        Ok(None)
    }

    pub async fn bind_session_account(
        &self,
        group_id: i64,
        current_hash: &str,
        legacy_hash: Option<&str>,
        account_id: i64,
    ) -> AppResult<()> {
        validate_positive(account_id, "account_id")?;
        let current_key = session_key(group_id, &format!("openai:{}", current_hash.trim()))?;
        let mut connection = self.connection.clone();
        let _: () = connection
            .set_ex(&current_key, account_id, self.sticky_ttl_seconds as u64)
            .await
            .map_err(redis_error)?;
        if self.write_legacy {
            if let Some(legacy_hash) = legacy_hash.filter(|value| !value.trim().is_empty()) {
                let legacy_key = session_key(group_id, &format!("openai:{}", legacy_hash.trim()))?;
                if legacy_key != current_key {
                    let legacy_ttl = self.sticky_ttl_seconds.min(600).max(1);
                    let _: () = connection
                        .set_ex(legacy_key, account_id, legacy_ttl as u64)
                        .await
                        .map_err(redis_error)?;
                }
            }
        }
        Ok(())
    }

    pub async fn refresh_session(
        &self,
        group_id: i64,
        current_hash: &str,
        legacy_hash: Option<&str>,
    ) -> AppResult<()> {
        let current_key = session_key(group_id, &format!("openai:{}", current_hash.trim()))?;
        let mut connection = self.connection.clone();
        let _: bool = connection
            .expire(&current_key, self.sticky_ttl_seconds)
            .await
            .map_err(redis_error)?;
        if self.read_legacy || self.write_legacy {
            if let Some(legacy_hash) = legacy_hash.filter(|value| !value.trim().is_empty()) {
                let legacy_key = session_key(group_id, &format!("openai:{}", legacy_hash.trim()))?;
                if legacy_key != current_key {
                    let _: bool = connection
                        .expire(legacy_key, self.sticky_ttl_seconds.min(600).max(1))
                        .await
                        .map_err(redis_error)?;
                }
            }
        }
        Ok(())
    }

    pub async fn delete_session(
        &self,
        group_id: i64,
        current_hash: &str,
        legacy_hash: Option<&str>,
    ) -> AppResult<()> {
        let current_key = session_key(group_id, &format!("openai:{}", current_hash.trim()))?;
        let mut keys = vec![current_key.clone()];
        if self.read_legacy || self.write_legacy {
            if let Some(legacy_hash) = legacy_hash.filter(|value| !value.trim().is_empty()) {
                let legacy_key = session_key(group_id, &format!("openai:{}", legacy_hash.trim()))?;
                if legacy_key != current_key {
                    keys.push(legacy_key);
                }
            }
        }
        self.delete_keys(&keys).await
    }

    pub async fn bind_response_account(
        &self,
        group_id: i64,
        response_id: &str,
        account_id: i64,
    ) -> AppResult<()> {
        validate_positive(account_id, "account_id")?;
        let key = response_account_key(group_id, response_id)?;
        let mut connection = self.connection.clone();
        let _: () = connection
            .set_ex(key, account_id, self.response_ttl_seconds as u64)
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    pub async fn get_response_account(
        &self,
        group_id: i64,
        response_id: &str,
    ) -> AppResult<Option<i64>> {
        self.get_positive_i64(&response_account_key(group_id, response_id)?)
            .await
    }

    pub async fn bind_response_owner(
        &self,
        group_id: i64,
        response_id: &str,
        user_id: i64,
        api_key_id: i64,
    ) -> AppResult<()> {
        validate_positive(user_id, "user_id")?;
        validate_positive(api_key_id, "api_key_id")?;
        let (user_key, api_key) = response_owner_keys(group_id, response_id)?;
        let mut connection = self.connection.clone();
        let _: () = connection
            .set_ex(user_key, user_id, self.response_ttl_seconds as u64)
            .await
            .map_err(redis_error)?;
        let _: () = connection
            .set_ex(api_key, api_key_id, self.response_ttl_seconds as u64)
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    pub async fn get_response_owner(
        &self,
        group_id: i64,
        response_id: &str,
    ) -> AppResult<Option<(i64, i64)>> {
        let (user_key, api_key) = response_owner_keys(group_id, response_id)?;
        let Some(user_id) = self.get_positive_i64(&user_key).await? else {
            return Ok(None);
        };
        let Some(api_key_id) = self.get_positive_i64(&api_key).await? else {
            return Ok(None);
        };
        Ok(Some((user_id, api_key_id)))
    }

    pub async fn validate_response_owner(
        &self,
        group_id: i64,
        response_id: &str,
        user_id: i64,
        api_key_id: i64,
    ) -> AppResult<bool> {
        if user_id <= 0 || api_key_id <= 0 {
            return Ok(false);
        }
        let Some((owner_user_id, owner_api_key_id)) =
            self.get_response_owner(group_id, response_id).await?
        else {
            return Ok(false);
        };
        Ok(owner_user_id == user_id || (owner_user_id <= 0 && owner_api_key_id == api_key_id))
    }

    pub async fn probe(
        &self,
        group_id: i64,
        account_id: i64,
        user_id: i64,
        api_key_id: i64,
    ) -> AppResult<StickyProbe> {
        let seed = format!("junohub-rust-sticky-probe-{}", uuid::Uuid::new_v4());
        let response_id = format!("resp_{seed}");
        let (current_hash, legacy_hash) = derive_session_hashes(&seed)?;
        let primary_key = session_key(group_id, &format!("openai:{current_hash}"))?;
        let legacy_key = session_key(group_id, &format!("openai:{legacy_hash}"))?;
        let response_key = response_account_key(group_id, &response_id)?;
        let window_hash = format!("wspreempt:{api_key_id}:{current_hash}");
        let window_key = session_window_key(group_id, &window_hash)?;
        let first_window_owner = uuid::Uuid::new_v4().to_string();
        let second_window_owner = uuid::Uuid::new_v4().to_string();
        let (owner_user_key, owner_api_key) = response_owner_keys(group_id, &response_id)?;
        let cleanup_keys = vec![
            primary_key.clone(),
            legacy_key.clone(),
            response_key.clone(),
            owner_user_key.clone(),
            owner_api_key.clone(),
            window_key,
        ];

        let operation = async {
            self.bind_session_account(group_id, &current_hash, Some(&legacy_hash), account_id)
                .await?;
            let session_account_id = self
                .get_session_account(group_id, &current_hash, Some(&legacy_hash))
                .await?
                .unwrap_or_default();
            self.refresh_session(group_id, &current_hash, Some(&legacy_hash))
                .await?;
            let mut connection = self.connection.clone();
            let primary_ttl_seconds: i64 =
                connection.ttl(&primary_key).await.map_err(redis_error)?;
            let legacy_ttl_seconds: i64 = connection.ttl(&legacy_key).await.map_err(redis_error)?;

            self.bind_response_account(group_id, &response_id, account_id)
                .await?;
            self.bind_response_owner(group_id, &response_id, user_id, api_key_id)
                .await?;
            let response_account_id = self
                .get_response_account(group_id, &response_id)
                .await?
                .unwrap_or_default();
            let (owner_user_id, owner_api_key_id) = self
                .get_response_owner(group_id, &response_id)
                .await?
                .unwrap_or_default();
            let same_user_authorized = self
                .validate_response_owner(group_id, &response_id, user_id, api_key_id + 1)
                .await?;
            let other_user_authorized = self
                .validate_response_owner(group_id, &response_id, user_id + 1, api_key_id)
                .await?;
            self.delete_session(group_id, &current_hash, Some(&legacy_hash))
                .await?;

            let first_previous = self
                .claim_session_window(group_id, &window_hash, &first_window_owner, 7_200_000)
                .await?;
            let second_previous = self
                .claim_session_window(group_id, &window_hash, &second_window_owner, 7_200_000)
                .await?;
            let stale_window_refresh = self
                .compare_refresh_session_window(
                    group_id,
                    &window_hash,
                    &first_window_owner,
                    7_200_000,
                )
                .await?;
            let current_window_refresh = self
                .compare_refresh_session_window(
                    group_id,
                    &window_hash,
                    &second_window_owner,
                    7_200_000,
                )
                .await?;
            let stale_window_delete = self
                .compare_delete_session_window(group_id, &window_hash, &first_window_owner)
                .await?;
            let current_window_delete = self
                .compare_delete_session_window(group_id, &window_hash, &second_window_owner)
                .await?;
            AppResult::Ok((
                session_account_id,
                primary_ttl_seconds,
                legacy_ttl_seconds,
                response_account_id,
                owner_user_id,
                owner_api_key_id,
                same_user_authorized,
                other_user_authorized,
                first_previous.is_some(),
                second_previous.as_deref() == Some(first_window_owner.as_str()),
                stale_window_refresh,
                current_window_refresh,
                stale_window_delete,
                current_window_delete,
            ))
        }
        .await;

        let cleanup_result = self.delete_keys(&cleanup_keys).await;
        let values = operation?;
        cleanup_result?;
        let residual_keys = self.count_existing(&cleanup_keys).await?;
        Ok(StickyProbe {
            group_id,
            account_id,
            current_hash,
            legacy_hash,
            session_account_id: values.0,
            primary_ttl_seconds: values.1,
            legacy_written: values.2 > 0,
            legacy_ttl_seconds: values.2,
            response_account_id: values.3,
            owner_user_id: values.4,
            owner_api_key_id: values.5,
            same_user_authorized: values.6,
            other_user_authorized: values.7,
            first_window_claim_had_owner: values.8,
            second_window_claim_returned_first: values.9,
            stale_window_refresh: values.10,
            current_window_refresh: values.11,
            stale_window_delete: values.12,
            current_window_delete: values.13,
            residual_keys,
        })
    }

    async fn get_positive_i64(&self, key: &str) -> AppResult<Option<i64>> {
        let mut connection = self.connection.clone();
        let value: Option<i64> = connection.get(key).await.map_err(redis_error)?;
        Ok(value.filter(|value| *value > 0))
    }

    async fn delete_keys(&self, keys: &[String]) -> AppResult<()> {
        let mut connection = self.connection.clone();
        let _: i64 = connection.del(keys).await.map_err(redis_error)?;
        Ok(())
    }

    async fn count_existing(&self, keys: &[String]) -> AppResult<i64> {
        let mut connection = self.connection.clone();
        let mut count = 0;
        for key in keys {
            let exists: bool = connection.exists(key).await.map_err(redis_error)?;
            count += i64::from(exists);
        }
        Ok(count)
    }
}

pub fn derive_session_hashes(seed: &str) -> AppResult<(String, String)> {
    let normalized = seed.trim();
    if normalized.is_empty() {
        return Err(AppError::BadRequest("session seed 不能为空".into()));
    }
    let current = format!("{:016x}", xxh64(normalized.as_bytes(), 0));
    let legacy = hex::encode(Sha256::digest(normalized.as_bytes()));
    Ok((current, legacy))
}

fn session_window_key(group_id: i64, session_hash: &str) -> AppResult<String> {
    let hash = session_hash.trim();
    if group_id <= 0 || hash.is_empty() || hash.len() > 512 {
        return Err(AppError::BadRequest(
            "Responses session-window key 参数无效".into(),
        ));
    }
    Ok(format!("openai_responses_session_window:{group_id}:{hash}"))
}

fn validate_window_owner(owner: &str, ttl_millis: i64) -> AppResult<()> {
    if owner.trim().is_empty() || owner.len() > 256 || ttl_millis <= 0 {
        return Err(AppError::BadRequest(
            "Responses session-window owner 参数无效".into(),
        ));
    }
    Ok(())
}

fn session_key(group_id: i64, session_hash: &str) -> AppResult<String> {
    let hash = session_hash.trim();
    if group_id < 0 || hash.is_empty() || hash.len() > 256 {
        return Err(AppError::BadRequest("sticky session key 参数无效".into()));
    }
    Ok(format!("sticky_session:{group_id}:{hash}"))
}

fn response_account_key(group_id: i64, response_id: &str) -> AppResult<String> {
    let id = validate_response_id(response_id)?;
    Ok(format!(
        "sticky_session:{group_id}:openai:response:{}",
        hex::encode(Sha256::digest(id.as_bytes()))
    ))
}

fn response_owner_keys(group_id: i64, response_id: &str) -> AppResult<(String, String)> {
    let id = validate_response_id(response_id)?;
    let hash = hex::encode(Sha256::digest(id.as_bytes()));
    Ok((
        format!("sticky_session:{group_id}:openai:http-response-owner:user:{hash}"),
        format!("sticky_session:{group_id}:openai:http-response-owner:key:{hash}"),
    ))
}

fn validate_response_id(response_id: &str) -> AppResult<&str> {
    let id = response_id.trim();
    if id.is_empty() || id.len() > 1024 {
        return Err(AppError::BadRequest("response_id 无效".into()));
    }
    Ok(id)
}

fn validate_positive(value: i64, name: &str) -> AppResult<()> {
    if value <= 0 {
        return Err(AppError::BadRequest(format!("{name} 必须为正数")));
    }
    Ok(())
}

fn redis_error(error: redis::RedisError) -> AppError {
    AppError::Internal(format!("Sub2API Redis sticky store: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_hashes_match_go_vectors() {
        let (current, legacy) = derive_session_hashes("parent-response-id").unwrap();
        assert_eq!(current, "2536daf949307e69");
        assert_eq!(legacy, hex::encode(Sha256::digest(b"parent-response-id")));
        assert_eq!(
            derive_session_hashes("  parent-response-id  ").unwrap(),
            (current, legacy)
        );
    }

    #[test]
    fn response_keys_are_group_isolated_and_do_not_expose_ids() {
        let first = response_account_key(8, "resp_secret").unwrap();
        let second = response_account_key(9, "resp_secret").unwrap();
        assert_ne!(first, second);
        assert!(!first.contains("resp_secret"));
        assert!(first.starts_with("sticky_session:8:openai:response:"));
    }
}
