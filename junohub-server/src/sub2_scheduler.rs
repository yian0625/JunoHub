use chrono::{DateTime, Utc};
use redis::{aio::ConnectionManager, AsyncCommands};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{cmp::Ordering, collections::HashSet};

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SchedulerAccountPayload {
    #[serde(rename = "ID")]
    pub id: i64,
    pub name: String,
    pub platform: String,
    #[serde(rename = "Type")]
    pub kind: String,
    pub concurrency: i32,
    pub priority: i32,
    #[serde(default)]
    pub load_factor: Option<i32>,
    #[serde(default = "default_rate_multiplier")]
    pub rate_multiplier: f64,
    pub status: String,
    pub schedulable: bool,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub auto_pause_on_expired: bool,
    #[serde(default)]
    pub rate_limit_reset_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub overload_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub temp_unschedulable_until: Option<DateTime<Utc>>,
    #[serde(default, rename = "GroupIDs")]
    pub group_ids: Vec<i64>,
    #[serde(default)]
    pub credentials: Value,
    #[serde(default)]
    pub extra: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct SchedulerAccountSnapshot {
    pub id: i64,
    pub name: String,
    pub platform: String,
    pub kind: String,
    pub concurrency: i32,
    pub effective_load_factor: i32,
    pub priority: i32,
    pub rate_multiplier: f64,
    pub status: String,
    pub schedulable: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub auto_pause_on_expired: bool,
    pub rate_limit_reset_at: Option<DateTime<Utc>>,
    pub overload_until: Option<DateTime<Utc>>,
    pub temp_unschedulable_until: Option<DateTime<Utc>>,
    pub group_ids: Vec<i64>,
    pub credential_fields: Vec<String>,
    pub extra_fields: Vec<String>,
}

impl From<SchedulerAccountPayload> for SchedulerAccountSnapshot {
    fn from(value: SchedulerAccountPayload) -> Self {
        let mut credential_fields = object_keys(&value.credentials);
        let mut extra_fields = object_keys(&value.extra);
        credential_fields.sort();
        extra_fields.sort();
        Self {
            id: value.id,
            name: value.name,
            platform: value.platform,
            kind: value.kind,
            concurrency: value.concurrency,
            effective_load_factor: value
                .load_factor
                .filter(|factor| *factor > 0)
                .unwrap_or_else(|| value.concurrency.max(1)),
            priority: value.priority,
            rate_multiplier: value.rate_multiplier,
            status: value.status,
            schedulable: value.schedulable,
            expires_at: value.expires_at,
            auto_pause_on_expired: value.auto_pause_on_expired,
            rate_limit_reset_at: value.rate_limit_reset_at,
            overload_until: value.overload_until,
            temp_unschedulable_until: value.temp_unschedulable_until,
            group_ids: value.group_ids,
            credential_fields,
            extra_fields,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SlotProbe {
    pub account_id: i64,
    pub slot_ttl_seconds: i64,
    pub acquired: bool,
    pub request_id: String,
    pub load_before: AccountLoad,
    pub load_during: AccountLoad,
    pub load_after: AccountLoad,
}

const UPDATE_LAST_USED_SCRIPT: &str = r#"
local candidate = tonumber(ARGV[1])
if candidate == nil then return redis.error_reply('invalid last_used value') end
if redis.call('EXISTS', KEYS[1]) == 1 then
    local current = tonumber(redis.call('GET', KEYS[2]))
    if current == nil or candidate > current then redis.call('SET', KEYS[2], ARGV[1]); return 1 end
end
return 0
"#;

const CAPTURE_BUCKET_TOKEN_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[2]) == 1 then return -1 end
local currentEpoch = redis.call('GET', KEYS[1])
if currentEpoch == false then redis.call('SET', KEYS[1], '1'); return 1 end
local parsedEpoch = tonumber(currentEpoch)
if parsedEpoch == nil or parsedEpoch < 1 then return -2 end
return parsedEpoch
"#;

const ALLOCATE_SNAPSHOT_VERSION_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[2]) == 1 then return -1 end
local currentEpoch = tonumber(redis.call('GET', KEYS[1]))
local expectedEpoch = tonumber(ARGV[1])
if currentEpoch == nil or expectedEpoch == nil or currentEpoch ~= expectedEpoch then return -2 end
return redis.call('INCR', KEYS[3])
"#;

const ACTIVATE_SNAPSHOT_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[6]) == 1 then redis.call('DEL', KEYS[4]); return -1 end
local currentEpoch = tonumber(redis.call('GET', KEYS[5]))
local expectedEpoch = tonumber(ARGV[5])
if currentEpoch == nil or expectedEpoch == nil or currentEpoch ~= expectedEpoch then
    redis.call('DEL', KEYS[4]); return -2
end
local currentActive = redis.call('GET', KEYS[1])
local newVersion = tonumber(ARGV[1])
if currentActive ~= false then
    local curVersion = tonumber(currentActive)
    if curVersion and newVersion < curVersion then redis.call('DEL', KEYS[4]); return 0 end
end
redis.call('SET', KEYS[1], ARGV[1])
redis.call('SET', KEYS[2], '1')
redis.call('SADD', KEYS[3], ARGV[2])
if currentActive ~= false and currentActive ~= ARGV[1] then
    redis.call('EXPIRE', ARGV[3] .. currentActive, tonumber(ARGV[4]))
end
return 1
"#;

const RETIRE_BUCKET_SCRIPT: &str = r#"
local retired = redis.call('GET', KEYS[2])
local currentEpoch = tonumber(redis.call('GET', KEYS[1])) or 0
if retired == false then
    currentEpoch = currentEpoch + 1
    if currentEpoch < 1 then currentEpoch = 1 end
    redis.call('SET', KEYS[1], tostring(currentEpoch))
    redis.call('SET', KEYS[2], tostring(currentEpoch))
elseif currentEpoch < 1 then
    currentEpoch = tonumber(retired) or 1
    redis.call('SET', KEYS[1], tostring(currentEpoch))
end
redis.call('SREM', KEYS[3], ARGV[1])
local currentActive = redis.call('GET', KEYS[5])
if currentActive ~= false then redis.call('EXPIRE', ARGV[2] .. currentActive, tonumber(ARGV[3])) end
redis.call('DEL', KEYS[4], KEYS[5])
return currentEpoch
"#;

const REOPEN_BUCKET_SCRIPT: &str = r#"
local currentEpochRaw = redis.call('GET', KEYS[1])
local currentEpoch = tonumber(currentEpochRaw)
local retiredEpochRaw = redis.call('GET', KEYS[2])
if retiredEpochRaw == false then
    if currentEpochRaw == false then redis.call('SET', KEYS[1], '1'); return 1 end
    if currentEpoch == nil or currentEpoch < 1 then return -2 end
    return currentEpoch
end
local retiredEpoch = tonumber(retiredEpochRaw)
if retiredEpoch == nil or retiredEpoch < 1 then return -2 end
if currentEpoch == nil or currentEpoch < retiredEpoch then currentEpoch = retiredEpoch end
redis.call('SET', KEYS[1], tostring(currentEpoch))
redis.call('DEL', KEYS[2])
redis.call('SREM', KEYS[3], ARGV[1])
local currentActive = redis.call('GET', KEYS[5])
if currentActive ~= false then redis.call('EXPIRE', ARGV[2] .. currentActive, tonumber(ARGV[3])) end
redis.call('DEL', KEYS[4], KEYS[5])
return currentEpoch
"#;

const RELEASE_OWNED_LOCK_SCRIPT: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) end
return 0
"#;

const ACQUIRE_ACCOUNT_SLOT_SCRIPT: &str = r#"
redis.replicate_commands()
local key = KEYS[1]
local liveKey = KEYS[2]
local maxConcurrency = tonumber(ARGV[1])
local ttl = tonumber(ARGV[2])
local requestID = ARGV[3]
local timeResult = redis.call('TIME')
local now = tonumber(timeResult[1])
local expireBefore = now - ttl
redis.call('ZREMRANGEBYSCORE', key, '-inf', expireBefore)
redis.call('ZREMRANGEBYSCORE', liveKey, '-inf', now - 60)
local exists = redis.call('ZSCORE', key, requestID)
if exists ~= false then
    redis.call('ZADD', key, now, requestID)
    redis.call('EXPIRE', key, ttl)
    return {1, now}
end
local count = redis.call('ZCARD', key) + redis.call('ZCARD', liveKey)
if count < maxConcurrency then
    redis.call('ZADD', key, now, requestID)
    redis.call('EXPIRE', key, ttl)
    return {1, now}
end
return {0, now}
"#;

const TRACK_SLOT_SCRIPT: &str = r#"
redis.replicate_commands()
local key = KEYS[1]
local ttl = tonumber(ARGV[1])
local requestID = ARGV[2]
local now = tonumber(redis.call('TIME')[1])
local expireBefore = now - ttl
redis.call('ZREMRANGEBYSCORE', key, '-inf', expireBefore)
redis.call('ZADD', key, now, requestID)
redis.call('EXPIRE', key, ttl)
return 1
"#;

const INCREMENT_WAIT_SCRIPT: &str = r#"
redis.replicate_commands()
local current = redis.call('GET', KEYS[1])
if current == false then current = 0 else current = tonumber(current) end
local now = tonumber(redis.call('TIME')[1])
if current >= tonumber(ARGV[1]) then return {0, now} end
redis.call('INCR', KEYS[1])
redis.call('EXPIRE', KEYS[1], ARGV[2])
return {1, now}
"#;

const DECREMENT_WAIT_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
if current ~= false and tonumber(current) > 0 then redis.call('DECR', KEYS[1]) end
return 1
"#;

const ACQUIRE_LIVE_LEASE_SCRIPT: &str = r#"
redis.replicate_commands()
local accountRegular = KEYS[1]
local accountLive = KEYS[2]
local userRegular = KEYS[3]
local userLive = KEYS[4]
local apiLive = KEYS[5]
local accountMax = tonumber(ARGV[1])
local userMax = tonumber(ARGV[2])
local ttl = tonumber(ARGV[3])
local leaseID = ARGV[4]
local replacing = tonumber(ARGV[5])
local now = tonumber(redis.call('TIME')[1])
local liveExpireBefore = now - ttl
redis.call('ZREMRANGEBYSCORE', accountLive, '-inf', liveExpireBefore)
redis.call('ZREMRANGEBYSCORE', userLive, '-inf', liveExpireBefore)
redis.call('ZREMRANGEBYSCORE', apiLive, '-inf', liveExpireBefore)
if redis.call('ZSCORE', accountLive, leaseID) ~= false then return 1 end
local accountCount = redis.call('ZCARD', accountRegular) + redis.call('ZCARD', accountLive)
local userCount = redis.call('ZCARD', userRegular) + redis.call('ZCARD', userLive)
local allowance = 0
if replacing == 1 then allowance = 1 end
if accountMax > 0 and accountCount >= accountMax + allowance then return 0 end
if userMax > 0 and userCount >= userMax + allowance then return 0 end
redis.call('ZADD', accountLive, now, leaseID)
redis.call('ZADD', userLive, now, leaseID)
redis.call('ZADD', apiLive, now, leaseID)
redis.call('EXPIRE', accountLive, ttl)
redis.call('EXPIRE', userLive, ttl)
redis.call('EXPIRE', apiLive, ttl)
return 1
"#;

const REFRESH_LIVE_LEASE_SCRIPT: &str = r#"
redis.replicate_commands()
local ttl = tonumber(ARGV[1])
local leaseID = ARGV[2]
local now = tonumber(redis.call('TIME')[1])
local expireBefore = now - ttl
for _, key in ipairs(KEYS) do
    redis.call('ZREMRANGEBYSCORE', key, '-inf', expireBefore)
    if redis.call('ZSCORE', key, leaseID) == false then return 0 end
end
for _, key in ipairs(KEYS) do
    redis.call('ZADD', key, now, leaseID)
    redis.call('EXPIRE', key, ttl)
end
return 1
"#;

const ACQUIRE_INGRESS_LEASE_SCRIPT: &str = r#"
redis.replicate_commands()
local key = KEYS[1]
local maxConnections = tonumber(ARGV[1])
local ttl = tonumber(ARGV[2])
local leaseID = ARGV[3]
local now = tonumber(redis.call('TIME')[1])
local expireBefore = now - ttl
redis.call('ZREMRANGEBYSCORE', key, '-inf', expireBefore)
if redis.call('ZSCORE', key, leaseID) ~= false then
    redis.call('ZADD', key, now, leaseID)
    redis.call('EXPIRE', key, ttl)
    return 1
end
if redis.call('ZCARD', key) < maxConnections then
    redis.call('ZADD', key, now, leaseID)
    redis.call('EXPIRE', key, ttl)
    return 1
end
return 0
"#;

const REFRESH_INGRESS_LEASE_SCRIPT: &str = r#"
redis.replicate_commands()
local key = KEYS[1]
local ttl = tonumber(ARGV[1])
local leaseID = ARGV[2]
local now = tonumber(redis.call('TIME')[1])
local expireBefore = now - ttl
redis.call('ZREMRANGEBYSCORE', key, '-inf', expireBefore)
if redis.call('ZSCORE', key, leaseID) == false then return 0 end
redis.call('ZADD', key, now, leaseID)
redis.call('EXPIRE', key, ttl)
return 1
"#;

#[derive(Clone, Debug, Serialize)]
pub struct ConcurrencyProbe {
    pub account_id: i64,
    pub user_id: i64,
    pub api_key_id: i64,
    pub slot_ttl_seconds: i64,
    pub wait_ttl_seconds: i64,
    pub user_slot_acquired: bool,
    pub api_key_tracked: bool,
    pub user_wait_incremented: bool,
    pub account_wait_incremented: bool,
    pub user_slot_residual: bool,
    pub api_key_slot_residual: bool,
    pub user_wait_before: i64,
    pub user_wait_after: i64,
    pub account_wait_before: i64,
    pub account_wait_after: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LiveLeaseProbe {
    pub account_id: i64,
    pub user_id: i64,
    pub api_key_id: i64,
    pub lease_id: String,
    pub acquired: bool,
    pub refreshed: bool,
    pub refresh_after_release: bool,
    pub residual_members: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct IngressLeaseProbe {
    pub api_key_id: i64,
    pub lease_id: String,
    pub acquired: bool,
    pub refreshed: bool,
    pub refresh_after_release: bool,
    pub residual_members: i64,
}

#[derive(Clone, Debug)]
pub struct SchedulerBucket {
    pub group_id: i64,
    pub platform: String,
    pub mode: String,
}

impl SchedulerBucket {
    fn name(&self) -> AppResult<String> {
        if self.group_id < 0
            || self.platform.is_empty()
            || self.mode.is_empty()
            || self.platform.contains(':')
            || self.mode.contains(':')
        {
            return Err(AppError::BadRequest("scheduler bucket 参数无效".into()));
        }
        Ok(format!("{}:{}:{}", self.group_id, self.platform, self.mode))
    }

    fn control_key(&self, prefix: &str) -> AppResult<String> {
        Ok(format!("{prefix}{}", self.name()?))
    }

    fn snapshot_prefix(&self) -> AppResult<String> {
        Ok(format!(
            "sched:{}:{}:{}:v",
            self.group_id, self.platform, self.mode
        ))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SnapshotControlProbe {
    pub bucket: String,
    pub initial_epoch: i64,
    pub allocated_version: i64,
    pub activated: bool,
    pub ready: bool,
    pub active_version: i64,
    pub snapshot_account_ids: Vec<i64>,
    pub retired_epoch: i64,
    pub capture_while_retired: i64,
    pub old_snapshot_grace_ttl: i64,
    pub reopened_epoch: i64,
    pub stale_writer_fenced: bool,
    pub bucket_lock_acquired: bool,
    pub bucket_lock_second_acquire: bool,
    pub wrong_owner_release: bool,
    pub owned_release: bool,
    pub lifecycle_lock_acquired: bool,
    pub lifecycle_wrong_owner_release: bool,
    pub lifecycle_owned_release: bool,
    pub residual_keys: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountLoad {
    pub current_concurrency: i64,
    pub waiting_count: i64,
    pub load_rate: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RankedCandidate {
    pub account_id: i64,
    pub priority: i32,
    pub score: f64,
    pub load: AccountLoad,
}

#[derive(Clone, Debug, Serialize)]
pub struct RejectedCandidate {
    pub account_id: i64,
    pub reason: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelectionSnapshot {
    pub bucket_key: String,
    pub pool_count: usize,
    pub eligible_count: usize,
    pub selected_layer: &'static str,
    pub selected_account_id: Option<i64>,
    pub ranked: Vec<RankedCandidate>,
    pub rejected: Vec<RejectedCandidate>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SelectionRequest {
    pub bucket_key: String,
    #[serde(default = "default_platform")]
    pub platform: String,
    #[serde(default)]
    pub excluded_ids: Vec<i64>,
    #[serde(default)]
    pub sticky_account_id: Option<i64>,
    #[serde(default)]
    pub previous_response_account_id: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BucketMember {
    pub account_id: i64,
    pub score: f64,
}

#[derive(Clone)]
pub struct SchedulerCache {
    connection: ConnectionManager,
    slot_ttl_seconds: i64,
    wait_ttl_seconds: i64,
}

impl SchedulerCache {
    pub fn new(
        connection: ConnectionManager,
        slot_ttl_seconds: i64,
        wait_ttl_seconds: i64,
    ) -> Self {
        Self {
            connection,
            slot_ttl_seconds,
            wait_ttl_seconds,
        }
    }

    pub async fn account(&self, id: i64) -> AppResult<Option<SchedulerAccountSnapshot>> {
        let mut connection = self.connection.clone();
        let payload: Option<String> = connection
            .get(format!("sched:acc:{id}"))
            .await
            .map_err(redis_error)?;
        payload
            .map(|payload| {
                serde_json::from_str::<SchedulerAccountPayload>(&payload)
                    .map(SchedulerAccountSnapshot::from)
                    .map_err(AppError::from)
            })
            .transpose()
    }

    pub async fn metadata(&self, id: i64) -> AppResult<Option<SchedulerAccountSnapshot>> {
        let mut connection = self.connection.clone();
        let payload: Option<String> = connection
            .get(format!("sched:meta:{id}"))
            .await
            .map_err(redis_error)?;
        payload
            .map(|payload| {
                serde_json::from_str::<SchedulerAccountPayload>(&payload)
                    .map(SchedulerAccountSnapshot::from)
                    .map_err(AppError::from)
            })
            .transpose()
    }

    pub async fn set_outbox_watermark(&self, watermark: i64) -> AppResult<()> {
        if watermark < 0 {
            return Err(AppError::BadRequest("outbox watermark 无效".into()));
        }
        let mut connection = self.connection.clone();
        let _: () = connection
            .set("sched:outbox:watermark", watermark)
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    pub async fn update_last_used(&self, account_id: i64, unix_seconds: i64) -> AppResult<bool> {
        if account_id <= 0 || unix_seconds <= 0 {
            return Ok(false);
        }
        let mut connection = self.connection.clone();
        let updated: i64 = redis::Script::new(UPDATE_LAST_USED_SCRIPT)
            .key(format!("sched:acc:{account_id}"))
            .key(format!("sched:acc:last_used:{account_id}"))
            .arg(unix_seconds.saturating_mul(1000))
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(updated == 1)
    }

    pub async fn outbox_watermark(&self) -> AppResult<i64> {
        let mut connection = self.connection.clone();
        let value: Option<i64> = connection
            .get("sched:outbox:watermark")
            .await
            .map_err(redis_error)?;
        Ok(value.unwrap_or_default())
    }

    pub async fn set_account_wire(
        &self,
        account_id: i64,
        wire: &crate::sub2_repository::SchedulerAccountWire,
    ) -> AppResult<()> {
        if account_id <= 0 {
            return Err(AppError::BadRequest("account_id 无效".into()));
        }
        let full = serde_json::to_string(&wire.full)?;
        let metadata = serde_json::to_string(&wire.metadata)?;
        let mut connection = self.connection.clone();
        let _: () = redis::pipe()
            .set(format!("sched:acc:{account_id}"), full)
            .set(format!("sched:meta:{account_id}"), metadata)
            .query_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    pub async fn delete_account_wire(&self, account_id: i64) -> AppResult<()> {
        if account_id <= 0 {
            return Ok(());
        }
        let mut connection = self.connection.clone();
        let _: i64 = connection
            .del(&[
                format!("sched:acc:{account_id}"),
                format!("sched:meta:{account_id}"),
                format!("sched:acc:last_used:{account_id}"),
            ])
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    pub async fn publish_snapshot(
        &self,
        bucket: &SchedulerBucket,
        account_ids: &[i64],
    ) -> AppResult<()> {
        let epoch = self.capture_bucket_token(bucket).await?;
        if epoch < 1 {
            return Err(AppError::Internal(format!(
                "scheduler bucket publish fenced: {} result={epoch}",
                bucket.name()?
            )));
        }
        let lock_key = bucket.control_key("sched:lock:")?;
        let owner = format!("junohub-snapshot-{}", uuid::Uuid::new_v4());
        if !self.try_owned_lock(&lock_key, &owner, 30_000).await? {
            return Err(AppError::Internal(format!(
                "scheduler bucket rebuild busy: {}",
                bucket.name()?
            )));
        }
        let operation = async {
            let version = self.allocate_snapshot_version(bucket, epoch).await?;
            if version < 1 {
                return Err(AppError::Internal(format!(
                    "scheduler snapshot allocation fenced: {} result={version}",
                    bucket.name()?
                )));
            }
            let snapshot_key = format!("{}{version}", bucket.snapshot_prefix()?);
            let mut connection = self.connection.clone();
            let _: i64 = connection.del(&snapshot_key).await.map_err(redis_error)?;
            if !account_ids.is_empty() {
                let mut pipe = redis::pipe();
                for (score, account_id) in account_ids.iter().enumerate() {
                    pipe.zadd(&snapshot_key, *account_id, score as i64);
                }
                let _: () = pipe
                    .query_async(&mut connection)
                    .await
                    .map_err(redis_error)?;
            }
            let activated = self.activate_snapshot(bucket, epoch, version).await?;
            if activated != 1 {
                return Err(AppError::Internal(format!(
                    "scheduler snapshot activation fenced: {} result={activated}",
                    bucket.name()?
                )));
            }
            Ok(())
        }
        .await;
        let release = self.release_owned_lock(&lock_key, &owner).await;
        operation?;
        if !release? {
            return Err(AppError::Internal(
                "scheduler bucket lock ownership lost".into(),
            ));
        }
        Ok(())
    }

    pub async fn snapshot_control_probe(&self, account_id: i64) -> AppResult<SnapshotControlProbe> {
        if account_id <= 0 {
            return Err(AppError::BadRequest("account_id 不能为空".into()));
        }
        let nonce = uuid::Uuid::new_v4();
        let group_id = 8_000_000_000 + (nonce.as_u128() % 1_000_000_000) as i64;
        let bucket = SchedulerBucket {
            group_id,
            platform: "junohub-probe".into(),
            mode: nonce.simple().to_string(),
        };
        let bucket_name = bucket.name()?;
        let prefix = bucket.snapshot_prefix()?;
        let epoch_key = bucket.control_key("sched:epoch:")?;
        let retired_key = bucket.control_key("sched:retired:")?;
        let version_key = bucket.control_key("sched:ver:")?;
        let active_key = bucket.control_key("sched:active:")?;
        let ready_key = bucket.control_key("sched:ready:")?;
        let lock_key = bucket.control_key("sched:lock:")?;
        let lifecycle_key = format!("sched:group:lifecycle-lock:{group_id}");
        let owner = format!("junohub-{nonce}");
        let wrong_owner = format!("wrong-{nonce}");
        let mut cleanup_keys = vec![
            epoch_key.clone(),
            retired_key.clone(),
            version_key.clone(),
            active_key.clone(),
            ready_key.clone(),
            lock_key.clone(),
            lifecycle_key.clone(),
        ];

        let operation = async {
            let bucket_lock_acquired = self.try_owned_lock(&lock_key, &owner, 30_000).await?;
            let bucket_lock_second_acquire =
                self.try_owned_lock(&lock_key, &wrong_owner, 30_000).await?;
            let wrong_owner_release = self.release_owned_lock(&lock_key, &wrong_owner).await?;
            let owned_release = self.release_owned_lock(&lock_key, &owner).await?;

            let lifecycle_lock_acquired =
                self.try_owned_lock(&lifecycle_key, &owner, 30_000).await?;
            let lifecycle_wrong_owner_release = self
                .release_owned_lock(&lifecycle_key, &wrong_owner)
                .await?;
            let lifecycle_owned_release = self.release_owned_lock(&lifecycle_key, &owner).await?;

            let initial_epoch = self.capture_bucket_token(&bucket).await?;
            let allocated_version = self
                .allocate_snapshot_version(&bucket, initial_epoch)
                .await?;
            let snapshot_key = format!("{prefix}{allocated_version}");
            cleanup_keys.push(snapshot_key.clone());
            let mut connection = self.connection.clone();
            let _: i64 = connection
                .zadd(&snapshot_key, account_id, 0)
                .await
                .map_err(redis_error)?;
            let activate_result = self
                .activate_snapshot(&bucket, initial_epoch, allocated_version)
                .await?;
            let ready: Option<String> = connection.get(&ready_key).await.map_err(redis_error)?;
            let active: Option<i64> = connection.get(&active_key).await.map_err(redis_error)?;
            let snapshot_account_ids: Vec<i64> = connection
                .zrange(&snapshot_key, 0, -1)
                .await
                .map_err(redis_error)?;

            let retired_epoch = self.retire_bucket(&bucket).await?;
            let capture_while_retired = self.capture_bucket_token(&bucket).await?;
            let old_snapshot_grace_ttl: i64 =
                connection.ttl(&snapshot_key).await.map_err(redis_error)?;
            let reopened_epoch = self.reopen_bucket(&bucket).await?;
            let stale_allocate = self
                .allocate_snapshot_version(&bucket, initial_epoch)
                .await?;

            AppResult::Ok((
                initial_epoch,
                allocated_version,
                activate_result,
                ready.as_deref() == Some("1"),
                active.unwrap_or_default(),
                snapshot_account_ids,
                retired_epoch,
                capture_while_retired,
                old_snapshot_grace_ttl,
                reopened_epoch,
                stale_allocate == -2,
                bucket_lock_acquired,
                bucket_lock_second_acquire,
                wrong_owner_release,
                owned_release,
                lifecycle_lock_acquired,
                lifecycle_wrong_owner_release,
                lifecycle_owned_release,
            ))
        }
        .await;

        let cleanup_result = self.cleanup_bucket_probe(&bucket, &cleanup_keys).await;
        let values = operation?;
        cleanup_result?;
        let residual_keys = self.count_existing_keys(&cleanup_keys).await?;
        Ok(SnapshotControlProbe {
            bucket: bucket_name,
            initial_epoch: values.0,
            allocated_version: values.1,
            activated: values.2 == 1,
            ready: values.3,
            active_version: values.4,
            snapshot_account_ids: values.5,
            retired_epoch: values.6,
            capture_while_retired: values.7,
            old_snapshot_grace_ttl: values.8,
            reopened_epoch: values.9,
            stale_writer_fenced: values.10,
            bucket_lock_acquired: values.11,
            bucket_lock_second_acquire: values.12,
            wrong_owner_release: values.13,
            owned_release: values.14,
            lifecycle_lock_acquired: values.15,
            lifecycle_wrong_owner_release: values.16,
            lifecycle_owned_release: values.17,
            residual_keys,
        })
    }

    pub async fn capture_bucket_token(&self, bucket: &SchedulerBucket) -> AppResult<i64> {
        let mut connection = self.connection.clone();
        redis::Script::new(CAPTURE_BUCKET_TOKEN_SCRIPT)
            .key(bucket.control_key("sched:epoch:")?)
            .key(bucket.control_key("sched:retired:")?)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)
    }

    pub async fn allocate_snapshot_version(
        &self,
        bucket: &SchedulerBucket,
        epoch: i64,
    ) -> AppResult<i64> {
        let mut connection = self.connection.clone();
        redis::Script::new(ALLOCATE_SNAPSHOT_VERSION_SCRIPT)
            .key(bucket.control_key("sched:epoch:")?)
            .key(bucket.control_key("sched:retired:")?)
            .key(bucket.control_key("sched:ver:")?)
            .arg(epoch)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)
    }

    pub async fn activate_snapshot(
        &self,
        bucket: &SchedulerBucket,
        epoch: i64,
        version: i64,
    ) -> AppResult<i64> {
        let active_key = bucket.control_key("sched:active:")?;
        let ready_key = bucket.control_key("sched:ready:")?;
        let snapshot_prefix = bucket.snapshot_prefix()?;
        let snapshot_key = format!("{snapshot_prefix}{version}");
        let bucket_name = bucket.name()?;
        let mut connection = self.connection.clone();
        redis::Script::new(ACTIVATE_SNAPSHOT_SCRIPT)
            .key(active_key)
            .key(ready_key)
            .key("sched:buckets")
            .key(snapshot_key)
            .key(bucket.control_key("sched:epoch:")?)
            .key(bucket.control_key("sched:retired:")?)
            .arg(version)
            .arg(bucket_name)
            .arg(snapshot_prefix)
            .arg(60)
            .arg(epoch)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)
    }

    pub async fn retire_bucket(&self, bucket: &SchedulerBucket) -> AppResult<i64> {
        self.retire_or_reopen(bucket, true).await
    }

    pub async fn reopen_bucket(&self, bucket: &SchedulerBucket) -> AppResult<i64> {
        self.retire_or_reopen(bucket, false).await
    }

    async fn retire_or_reopen(&self, bucket: &SchedulerBucket, retire: bool) -> AppResult<i64> {
        let mut connection = self.connection.clone();
        let script = if retire {
            RETIRE_BUCKET_SCRIPT
        } else {
            REOPEN_BUCKET_SCRIPT
        };
        redis::Script::new(script)
            .key(bucket.control_key("sched:epoch:")?)
            .key(bucket.control_key("sched:retired:")?)
            .key("sched:buckets")
            .key(bucket.control_key("sched:ready:")?)
            .key(bucket.control_key("sched:active:")?)
            .arg(bucket.name()?)
            .arg(bucket.snapshot_prefix()?)
            .arg(60)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)
    }

    async fn try_owned_lock(&self, key: &str, owner: &str, ttl_millis: i64) -> AppResult<bool> {
        let mut connection = self.connection.clone();
        let result: Option<String> = redis::cmd("SET")
            .arg(key)
            .arg(owner)
            .arg("PX")
            .arg(ttl_millis)
            .arg("NX")
            .query_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(result.as_deref() == Some("OK"))
    }

    async fn release_owned_lock(&self, key: &str, owner: &str) -> AppResult<bool> {
        let mut connection = self.connection.clone();
        let released: i64 = redis::Script::new(RELEASE_OWNED_LOCK_SCRIPT)
            .key(key)
            .arg(owner)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(released == 1)
    }

    async fn cleanup_bucket_probe(
        &self,
        bucket: &SchedulerBucket,
        keys: &[String],
    ) -> AppResult<()> {
        let mut connection = self.connection.clone();
        let _: i64 = connection.del(keys).await.map_err(redis_error)?;
        let _: i64 = connection
            .srem("sched:buckets", bucket.name()?)
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    async fn count_existing_keys(&self, keys: &[String]) -> AppResult<i64> {
        let mut connection = self.connection.clone();
        let mut count = 0;
        for key in keys {
            let exists: bool = connection.exists(key).await.map_err(redis_error)?;
            count += i64::from(exists);
        }
        Ok(count)
    }

    pub async fn concurrency_probe(
        &self,
        account_id: i64,
        user_id: i64,
        api_key_id: i64,
    ) -> AppResult<ConcurrencyProbe> {
        if account_id <= 0 || user_id <= 0 || api_key_id <= 0 {
            return Err(AppError::BadRequest("并发探针 ID 无效".into()));
        }
        let request_id = format!("junohub-rust-concurrency-probe-{}", uuid::Uuid::new_v4());
        let user_wait_before = self.wait_count("concurrency:wait", user_id).await?;
        let account_wait_before = self.wait_count("wait:account", account_id).await?;

        let user_slot_acquired = self
            .acquire_user_slot(user_id, i32::MAX, &request_id)
            .await?;
        if let Err(error) = self.track_api_key_slot(api_key_id, &request_id).await {
            if user_slot_acquired {
                let _ = self.release_user_slot(user_id, &request_id).await;
            }
            return Err(error);
        }
        let api_key_tracked = true;
        let user_wait_incremented = match self
            .increment_wait(
                "concurrency:wait",
                "concurrency:user:active_index",
                user_id,
                i32::MAX,
            )
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = self.release_api_key_slot(api_key_id, &request_id).await;
                if user_slot_acquired {
                    let _ = self.release_user_slot(user_id, &request_id).await;
                }
                return Err(error);
            }
        };
        let account_wait_incremented = match self
            .increment_wait(
                "wait:account",
                "concurrency:account:active_index",
                account_id,
                i32::MAX,
            )
            .await
        {
            Ok(value) => value,
            Err(error) => {
                if user_wait_incremented {
                    let _ = self
                        .decrement_wait("concurrency:wait", "user", user_id)
                        .await;
                }
                let _ = self.release_api_key_slot(api_key_id, &request_id).await;
                if user_slot_acquired {
                    let _ = self.release_user_slot(user_id, &request_id).await;
                }
                return Err(error);
            }
        };

        if account_wait_incremented {
            self.decrement_wait("wait:account", "account", account_id)
                .await?;
        }
        if user_wait_incremented {
            self.decrement_wait("concurrency:wait", "user", user_id)
                .await?;
        }
        self.release_api_key_slot(api_key_id, &request_id).await?;
        if user_slot_acquired {
            self.release_user_slot(user_id, &request_id).await?;
        }

        let mut connection = self.connection.clone();
        let user_slot_residual: Option<f64> = connection
            .zscore(format!("concurrency:user:{user_id}"), &request_id)
            .await
            .map_err(redis_error)?;
        let api_key_slot_residual: Option<f64> = connection
            .zscore(format!("concurrency:api_key:{api_key_id}"), &request_id)
            .await
            .map_err(redis_error)?;
        Ok(ConcurrencyProbe {
            account_id,
            user_id,
            api_key_id,
            slot_ttl_seconds: self.slot_ttl_seconds,
            wait_ttl_seconds: self.wait_ttl_seconds,
            user_slot_acquired,
            api_key_tracked,
            user_wait_incremented,
            account_wait_incremented,
            user_slot_residual: user_slot_residual.is_some(),
            api_key_slot_residual: api_key_slot_residual.is_some(),
            user_wait_before,
            user_wait_after: self.wait_count("concurrency:wait", user_id).await?,
            account_wait_before,
            account_wait_after: self.wait_count("wait:account", account_id).await?,
        })
    }

    pub async fn acquire_user_slot(
        &self,
        user_id: i64,
        max_concurrency: i32,
        request_id: &str,
    ) -> AppResult<bool> {
        if user_id <= 0 || max_concurrency <= 0 || request_id.is_empty() {
            return Err(AppError::BadRequest("用户并发槽位参数无效".into()));
        }
        let mut connection = self.connection.clone();
        let result: Vec<i64> = redis::Script::new(ACQUIRE_ACCOUNT_SLOT_SCRIPT)
            .key(format!("concurrency:user:{user_id}"))
            .key(format!("concurrency:live:user:{user_id}"))
            .arg(max_concurrency)
            .arg(self.slot_ttl_seconds)
            .arg(request_id)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        let acquired = result.first() == Some(&1);
        if acquired {
            if let Some(now) = result.get(1) {
                let _: Result<i64, _> = connection
                    .zadd(
                        "concurrency:user:active_index",
                        user_id,
                        now + self.slot_ttl_seconds,
                    )
                    .await;
            }
        }
        Ok(acquired)
    }

    pub async fn release_user_slot(&self, user_id: i64, request_id: &str) -> AppResult<()> {
        if user_id <= 0 || request_id.is_empty() {
            return Err(AppError::BadRequest("用户并发槽位参数无效".into()));
        }
        let mut connection = self.connection.clone();
        let _: i64 = connection
            .zrem(format!("concurrency:user:{user_id}"), request_id)
            .await
            .map_err(redis_error)?;
        self.refresh_active_index("user", user_id).await
    }

    pub async fn track_api_key_slot(&self, api_key_id: i64, request_id: &str) -> AppResult<()> {
        if api_key_id <= 0 || request_id.is_empty() {
            return Err(AppError::BadRequest("API Key 并发统计参数无效".into()));
        }
        let mut connection = self.connection.clone();
        let _: i64 = redis::Script::new(TRACK_SLOT_SCRIPT)
            .key(format!("concurrency:api_key:{api_key_id}"))
            .arg(self.slot_ttl_seconds)
            .arg(request_id)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    pub async fn release_api_key_slot(&self, api_key_id: i64, request_id: &str) -> AppResult<()> {
        if api_key_id <= 0 || request_id.is_empty() {
            return Err(AppError::BadRequest("API Key 并发统计参数无效".into()));
        }
        let mut connection = self.connection.clone();
        let _: i64 = connection
            .zrem(format!("concurrency:api_key:{api_key_id}"), request_id)
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    async fn increment_wait(
        &self,
        key_prefix: &str,
        index_key: &str,
        id: i64,
        max_wait: i32,
    ) -> AppResult<bool> {
        if id <= 0 || max_wait <= 0 {
            return Err(AppError::BadRequest("等待队列参数无效".into()));
        }
        let mut connection = self.connection.clone();
        let result: Vec<i64> = redis::Script::new(INCREMENT_WAIT_SCRIPT)
            .key(format!("{key_prefix}:{id}"))
            .arg(max_wait)
            .arg(self.wait_ttl_seconds)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        let incremented = result.first() == Some(&1);
        if incremented {
            if let Some(now) = result.get(1) {
                let _: Result<i64, _> = connection
                    .zadd(index_key, id, now + self.wait_ttl_seconds)
                    .await;
            }
        }
        Ok(incremented)
    }

    async fn decrement_wait(&self, key_prefix: &str, scope: &str, id: i64) -> AppResult<()> {
        let mut connection = self.connection.clone();
        let _: i64 = redis::Script::new(DECREMENT_WAIT_SCRIPT)
            .key(format!("{key_prefix}:{id}"))
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        self.refresh_active_index(scope, id).await
    }

    async fn wait_count(&self, key_prefix: &str, id: i64) -> AppResult<i64> {
        let mut connection = self.connection.clone();
        let value: Option<i64> = connection
            .get(format!("{key_prefix}:{id}"))
            .await
            .map_err(redis_error)?;
        Ok(value.unwrap_or_default().max(0))
    }

    async fn refresh_active_index(&self, scope: &str, id: i64) -> AppResult<()> {
        let (slot_key, wait_key, index_key) = match scope {
            "account" => (
                format!("concurrency:account:{id}"),
                format!("wait:account:{id}"),
                "concurrency:account:active_index",
            ),
            "user" => (
                format!("concurrency:user:{id}"),
                format!("concurrency:wait:{id}"),
                "concurrency:user:active_index",
            ),
            _ => return Err(AppError::BadRequest("并发索引 scope 无效".into())),
        };
        let mut connection = self.connection.clone();
        let time: Vec<String> = redis::cmd("TIME")
            .query_async(&mut connection)
            .await
            .map_err(redis_error)?;
        let now = time
            .first()
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| AppError::Internal("Redis TIME 响应无效".into()))?;
        let _: i64 = connection
            .zrembyscore(&slot_key, "-inf", now - self.slot_ttl_seconds)
            .await
            .map_err(redis_error)?;
        let slot_count: i64 = connection.zcard(slot_key).await.map_err(redis_error)?;
        let wait_count: Option<i64> = connection.get(wait_key).await.map_err(redis_error)?;
        let wait_count = wait_count.unwrap_or_default().max(0);
        if slot_count == 0 && wait_count == 0 {
            let _: i64 = connection.zrem(index_key, id).await.map_err(redis_error)?;
        } else {
            let ttl = if slot_count > 0 && wait_count > 0 {
                self.slot_ttl_seconds.max(self.wait_ttl_seconds)
            } else if slot_count > 0 {
                self.slot_ttl_seconds
            } else {
                self.wait_ttl_seconds
            };
            let _: i64 = connection
                .zadd(index_key, id, now + ttl)
                .await
                .map_err(redis_error)?;
        }
        Ok(())
    }

    pub async fn live_lease_probe(
        &self,
        account_id: i64,
        user_id: i64,
        user_max: i32,
        api_key_id: i64,
    ) -> AppResult<LiveLeaseProbe> {
        let account = self
            .account(account_id)
            .await?
            .ok_or_else(|| AppError::NotFound("调度账户不存在".into()))?;
        let lease_id = format!("junohub-rust-live-probe-{}", uuid::Uuid::new_v4());
        let acquired = self
            .acquire_live_lease(
                account_id,
                account.effective_load_factor,
                user_id,
                user_max,
                api_key_id,
                &lease_id,
                false,
            )
            .await?;
        let refreshed_result = if acquired {
            self.refresh_live_lease(account_id, user_id, api_key_id, &lease_id)
                .await
        } else {
            Ok(false)
        };
        let release_result = if acquired {
            self.release_live_lease(account_id, user_id, api_key_id, &lease_id)
                .await
        } else {
            Ok(())
        };
        let refreshed = refreshed_result?;
        release_result?;
        let refresh_after_release = self
            .refresh_live_lease(account_id, user_id, api_key_id, &lease_id)
            .await?;
        let residual_members = self
            .live_lease_member_count(account_id, user_id, api_key_id, &lease_id)
            .await?;
        Ok(LiveLeaseProbe {
            account_id,
            user_id,
            api_key_id,
            lease_id,
            acquired,
            refreshed,
            refresh_after_release,
            residual_members,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn acquire_live_lease(
        &self,
        account_id: i64,
        account_max: i32,
        user_id: i64,
        user_max: i32,
        api_key_id: i64,
        lease_id: &str,
        replacing_regular_slots: bool,
    ) -> AppResult<bool> {
        validate_live_lease_ids(account_id, user_id, api_key_id, lease_id)?;
        let mut connection = self.connection.clone();
        let acquired: i64 = redis::Script::new(ACQUIRE_LIVE_LEASE_SCRIPT)
            .key(format!("concurrency:account:{account_id}"))
            .key(format!("concurrency:live:account:{account_id}"))
            .key(format!("concurrency:user:{user_id}"))
            .key(format!("concurrency:live:user:{user_id}"))
            .key(format!("concurrency:live:api_key:{api_key_id}"))
            .arg(account_max)
            .arg(user_max)
            .arg(60)
            .arg(lease_id)
            .arg(i32::from(replacing_regular_slots))
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(acquired == 1)
    }

    pub async fn refresh_live_lease(
        &self,
        account_id: i64,
        user_id: i64,
        api_key_id: i64,
        lease_id: &str,
    ) -> AppResult<bool> {
        validate_live_lease_ids(account_id, user_id, api_key_id, lease_id)?;
        let mut connection = self.connection.clone();
        let refreshed: i64 = redis::Script::new(REFRESH_LIVE_LEASE_SCRIPT)
            .key(format!("concurrency:live:account:{account_id}"))
            .key(format!("concurrency:live:user:{user_id}"))
            .key(format!("concurrency:live:api_key:{api_key_id}"))
            .arg(60)
            .arg(lease_id)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(refreshed == 1)
    }

    pub async fn release_live_lease(
        &self,
        account_id: i64,
        user_id: i64,
        api_key_id: i64,
        lease_id: &str,
    ) -> AppResult<()> {
        validate_live_lease_ids(account_id, user_id, api_key_id, lease_id)?;
        let mut connection = self.connection.clone();
        let _: () = redis::pipe()
            .atomic()
            .zrem(format!("concurrency:live:account:{account_id}"), lease_id)
            .zrem(format!("concurrency:live:user:{user_id}"), lease_id)
            .zrem(format!("concurrency:live:api_key:{api_key_id}"), lease_id)
            .query_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    async fn live_lease_member_count(
        &self,
        account_id: i64,
        user_id: i64,
        api_key_id: i64,
        lease_id: &str,
    ) -> AppResult<i64> {
        let mut connection = self.connection.clone();
        let mut count = 0;
        for key in [
            format!("concurrency:live:account:{account_id}"),
            format!("concurrency:live:user:{user_id}"),
            format!("concurrency:live:api_key:{api_key_id}"),
        ] {
            let score: Option<f64> = connection
                .zscore(key, lease_id)
                .await
                .map_err(redis_error)?;
            count += i64::from(score.is_some());
        }
        Ok(count)
    }

    pub async fn ingress_lease_probe(
        &self,
        api_key_id: i64,
        max_connections: i32,
    ) -> AppResult<IngressLeaseProbe> {
        let lease_id = format!("junohub-rust-ingress-probe-{}", uuid::Uuid::new_v4());
        let acquired = self
            .acquire_ingress_lease(api_key_id, max_connections, &lease_id)
            .await?;
        let refreshed_result = if acquired {
            self.refresh_ingress_lease(api_key_id, &lease_id).await
        } else {
            Ok(false)
        };
        let release_result = if acquired {
            self.release_ingress_lease(api_key_id, &lease_id).await
        } else {
            Ok(())
        };
        let refreshed = refreshed_result?;
        release_result?;
        let refresh_after_release = self.refresh_ingress_lease(api_key_id, &lease_id).await?;
        let mut connection = self.connection.clone();
        let residual: Option<f64> = connection
            .zscore(
                format!("concurrency:openai_ws_ingress:api_key:{api_key_id}"),
                &lease_id,
            )
            .await
            .map_err(redis_error)?;
        Ok(IngressLeaseProbe {
            api_key_id,
            lease_id,
            acquired,
            refreshed,
            refresh_after_release,
            residual_members: i64::from(residual.is_some()),
        })
    }

    pub async fn acquire_ingress_lease(
        &self,
        api_key_id: i64,
        max_connections: i32,
        lease_id: &str,
    ) -> AppResult<bool> {
        validate_ingress_lease(api_key_id, max_connections, lease_id)?;
        let mut connection = self.connection.clone();
        let acquired: i64 = redis::Script::new(ACQUIRE_INGRESS_LEASE_SCRIPT)
            .key(format!(
                "concurrency:openai_ws_ingress:api_key:{api_key_id}"
            ))
            .arg(max_connections)
            .arg(60)
            .arg(lease_id)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(acquired == 1)
    }

    pub async fn refresh_ingress_lease(&self, api_key_id: i64, lease_id: &str) -> AppResult<bool> {
        validate_ingress_lease(api_key_id, 1, lease_id)?;
        let mut connection = self.connection.clone();
        let refreshed: i64 = redis::Script::new(REFRESH_INGRESS_LEASE_SCRIPT)
            .key(format!(
                "concurrency:openai_ws_ingress:api_key:{api_key_id}"
            ))
            .arg(60)
            .arg(lease_id)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(refreshed == 1)
    }

    pub async fn release_ingress_lease(&self, api_key_id: i64, lease_id: &str) -> AppResult<()> {
        validate_ingress_lease(api_key_id, 1, lease_id)?;
        let mut connection = self.connection.clone();
        let _: i64 = connection
            .zrem(
                format!("concurrency:openai_ws_ingress:api_key:{api_key_id}"),
                lease_id,
            )
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    pub async fn slot_probe(&self, account_id: i64) -> AppResult<SlotProbe> {
        let account = self
            .account(account_id)
            .await?
            .ok_or_else(|| AppError::NotFound("调度账户不存在".into()))?;
        let max_concurrency = account.effective_load_factor;
        let load_before = self.account_load(account_id, max_concurrency).await?;
        let request_id = format!("junohub-rust-probe-{}", uuid::Uuid::new_v4());
        let acquired = self
            .acquire_account_slot(account_id, max_concurrency, &request_id)
            .await?;
        let load_during = self.account_load(account_id, max_concurrency).await;
        let release_result = if acquired {
            self.release_account_slot(account_id, &request_id).await
        } else {
            Ok(())
        };
        let load_during = load_during?;
        release_result?;
        let load_after = self.account_load(account_id, max_concurrency).await?;
        Ok(SlotProbe {
            account_id,
            slot_ttl_seconds: self.slot_ttl_seconds,
            acquired,
            request_id,
            load_before,
            load_during,
            load_after,
        })
    }

    pub async fn acquire_account_slot(
        &self,
        account_id: i64,
        max_concurrency: i32,
        request_id: &str,
    ) -> AppResult<bool> {
        if account_id <= 0 || max_concurrency <= 0 || request_id.is_empty() {
            return Err(AppError::BadRequest("并发槽位参数无效".into()));
        }
        let mut connection = self.connection.clone();
        let result: Vec<i64> = redis::Script::new(ACQUIRE_ACCOUNT_SLOT_SCRIPT)
            .key(format!("concurrency:account:{account_id}"))
            .key(format!("concurrency:live:account:{account_id}"))
            .arg(max_concurrency)
            .arg(self.slot_ttl_seconds)
            .arg(request_id)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        let acquired = result.first() == Some(&1);
        if acquired {
            if let Some(now) = result.get(1) {
                let _: Result<i64, _> = connection
                    .zadd(
                        "concurrency:account:active_index",
                        account_id,
                        now + self.slot_ttl_seconds,
                    )
                    .await;
            }
        }
        Ok(acquired)
    }

    pub async fn release_account_slot(&self, account_id: i64, request_id: &str) -> AppResult<()> {
        if account_id <= 0 || request_id.is_empty() {
            return Err(AppError::BadRequest("并发槽位参数无效".into()));
        }
        let mut connection = self.connection.clone();
        let slot_key = format!("concurrency:account:{account_id}");
        let _: i64 = connection
            .zrem(&slot_key, request_id)
            .await
            .map_err(redis_error)?;

        let time: Vec<String> = redis::cmd("TIME")
            .query_async(&mut connection)
            .await
            .map_err(redis_error)?;
        let now = time
            .first()
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| AppError::Internal("Redis TIME 响应无效".into()))?;
        let _: i64 = connection
            .zrembyscore(&slot_key, "-inf", now - self.slot_ttl_seconds)
            .await
            .map_err(redis_error)?;
        let slot_count: i64 = connection.zcard(&slot_key).await.map_err(redis_error)?;
        let waiting: Option<i64> = connection
            .get(format!("wait:account:{account_id}"))
            .await
            .map_err(redis_error)?;
        if slot_count == 0 && waiting.unwrap_or_default() <= 0 {
            let _: i64 = connection
                .zrem("concurrency:account:active_index", account_id)
                .await
                .map_err(redis_error)?;
        }
        Ok(())
    }

    pub async fn active_version(&self, bucket: &SchedulerBucket) -> AppResult<Option<i64>> {
        let mut connection = self.connection.clone();
        let version: Option<i64> = connection
            .get(bucket.control_key("sched:active:")?)
            .await
            .map_err(redis_error)?;
        Ok(version)
    }

    pub async fn selection_snapshot(
        &self,
        request: SelectionRequest,
    ) -> AppResult<SelectionSnapshot> {
        let members = self.bucket_members(&request.bucket_key).await?;
        let excluded: HashSet<i64> = request.excluded_ids.into_iter().collect();
        let now = Utc::now();
        let mut ranked = Vec::new();
        let mut rejected = Vec::new();

        for member in &members {
            let Some(account) = self.account(member.account_id).await? else {
                rejected.push(RejectedCandidate {
                    account_id: member.account_id,
                    reason: "snapshot_missing",
                });
                continue;
            };
            if let Some(reason) = eligibility_reason(&account, &request.platform, &excluded, now) {
                rejected.push(RejectedCandidate {
                    account_id: account.id,
                    reason,
                });
                continue;
            }
            let load = self
                .account_load(account.id, account.effective_load_factor)
                .await?;
            ranked.push((account, load));
        }

        let min_priority = ranked.iter().map(|(a, _)| a.priority).min().unwrap_or(0);
        let max_priority = ranked.iter().map(|(a, _)| a.priority).max().unwrap_or(0);
        let max_waiting = ranked
            .iter()
            .map(|(_, load)| load.waiting_count)
            .max()
            .unwrap_or(1)
            .max(1);
        let mut ranked: Vec<RankedCandidate> = ranked
            .into_iter()
            .map(|(account, load)| {
                let priority_factor = if max_priority > min_priority {
                    1.0 - f64::from(account.priority - min_priority)
                        / f64::from(max_priority - min_priority)
                } else {
                    1.0
                };
                let load_factor = 1.0 - (load.load_rate as f64 / 100.0).clamp(0.0, 1.0);
                let queue_factor =
                    1.0 - (load.waiting_count as f64 / max_waiting as f64).clamp(0.0, 1.0);
                // Go defaults: priority=1, load=1, queue=.7, error=.8, TTFT=.5.
                // Runtime error/TTFT samples are process-local, so neutral values are used here.
                let score = priority_factor + load_factor + 0.7 * queue_factor + 0.8 + 0.25;
                RankedCandidate {
                    account_id: account.id,
                    priority: account.priority,
                    score,
                    load,
                }
            })
            .collect();
        ranked.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.priority.cmp(&right.priority))
                .then_with(|| left.load.load_rate.cmp(&right.load.load_rate))
                .then_with(|| left.load.waiting_count.cmp(&right.load.waiting_count))
                .then_with(|| left.account_id.cmp(&right.account_id))
        });

        let selected_layer = if promote_candidate(&mut ranked, request.previous_response_account_id)
        {
            "previous_response_id"
        } else if promote_candidate(&mut ranked, request.sticky_account_id) {
            "session_hash"
        } else {
            "load_balance"
        };
        let selected_account_id = ranked.first().map(|candidate| candidate.account_id);

        Ok(SelectionSnapshot {
            bucket_key: request.bucket_key,
            pool_count: members.len(),
            eligible_count: ranked.len(),
            selected_layer,
            selected_account_id,
            ranked,
            rejected,
        })
    }

    async fn account_load(&self, id: i64, max_concurrency: i32) -> AppResult<AccountLoad> {
        let mut connection = self.connection.clone();
        let time: Vec<String> = redis::cmd("TIME")
            .query_async(&mut connection)
            .await
            .map_err(redis_error)?;
        let now = time
            .first()
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| AppError::Internal("Redis TIME 响应无效".into()))?;
        let regular: i64 = connection
            .zcount(
                format!("concurrency:account:{id}"),
                format!("({}", now - self.slot_ttl_seconds),
                "+inf",
            )
            .await
            .map_err(redis_error)?;
        let live: i64 = connection
            .zcount(
                format!("concurrency:live:account:{id}"),
                format!("({}", now - 60),
                "+inf",
            )
            .await
            .map_err(redis_error)?;
        let waiting: Option<i64> = connection
            .get(format!("wait:account:{id}"))
            .await
            .map_err(redis_error)?;
        let current_concurrency = regular + live;
        let waiting_count = waiting.unwrap_or_default().max(0);
        let load_rate = if max_concurrency > 0 {
            (current_concurrency + waiting_count) * 100 / i64::from(max_concurrency)
        } else {
            0
        };
        Ok(AccountLoad {
            current_concurrency,
            waiting_count,
            load_rate,
        })
    }

    pub async fn bucket_members(&self, key: &str) -> AppResult<Vec<BucketMember>> {
        if !key.starts_with("sched:") || key.len() > 200 {
            return Err(AppError::BadRequest("调度 bucket key 无效".into()));
        }
        let mut connection = self.connection.clone();
        let values: Vec<(String, f64)> = connection
            .zrange_withscores(key, 0, -1)
            .await
            .map_err(redis_error)?;
        Ok(values
            .into_iter()
            .filter_map(|(id, score)| {
                Some(BucketMember {
                    account_id: id.parse().ok()?,
                    score,
                })
            })
            .collect())
    }
}

fn promote_candidate(candidates: &mut Vec<RankedCandidate>, account_id: Option<i64>) -> bool {
    let Some(account_id) = account_id.filter(|id| *id > 0) else {
        return false;
    };
    let Some(index) = candidates
        .iter()
        .position(|candidate| candidate.account_id == account_id)
    else {
        return false;
    };
    if index > 0 {
        let candidate = candidates.remove(index);
        candidates.insert(0, candidate);
    }
    true
}

fn validate_live_lease_ids(
    account_id: i64,
    user_id: i64,
    api_key_id: i64,
    lease_id: &str,
) -> AppResult<()> {
    if account_id <= 0 || user_id <= 0 || api_key_id <= 0 || lease_id.is_empty() {
        return Err(AppError::BadRequest("live lease 参数无效".into()));
    }
    Ok(())
}

fn validate_ingress_lease(api_key_id: i64, max_connections: i32, lease_id: &str) -> AppResult<()> {
    if api_key_id <= 0 || max_connections <= 0 || lease_id.is_empty() {
        return Err(AppError::BadRequest(
            "WebSocket ingress lease 参数无效".into(),
        ));
    }
    Ok(())
}

fn default_rate_multiplier() -> f64 {
    1.0
}

fn default_platform() -> String {
    "openai".to_owned()
}

fn eligibility_reason(
    account: &SchedulerAccountSnapshot,
    platform: &str,
    excluded: &HashSet<i64>,
    now: DateTime<Utc>,
) -> Option<&'static str> {
    if excluded.contains(&account.id) {
        return Some("excluded");
    }
    if account.status != "active" || !account.schedulable {
        return Some("not_schedulable");
    }
    if account.platform != platform {
        return Some("platform_mismatch");
    }
    if account.auto_pause_on_expired && account.expires_at.is_some_and(|until| now >= until) {
        return Some("expired");
    }
    if account.overload_until.is_some_and(|until| now < until) {
        return Some("overloaded");
    }
    if account.rate_limit_reset_at.is_some_and(|until| now < until) {
        return Some("rate_limited");
    }
    if account
        .temp_unschedulable_until
        .is_some_and(|until| now < until)
    {
        return Some("temporarily_unschedulable");
    }
    None
}

fn object_keys(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|value| value.keys().cloned().collect())
        .unwrap_or_default()
}

fn redis_error(error: redis::RedisError) -> AppError {
    AppError::Internal(format!("Sub2API Redis: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_payload_matches_go_pascal_case_wire_format() {
        let payload = serde_json::json!({
            "ID":1,
            "Name":"Codex",
            "Platform":"openai",
            "Type":"oauth",
            "Concurrency":3,
            "Priority":50,
            "RateMultiplier":1.0,
            "Status":"active",
            "Schedulable":true,
            "GroupIDs":[8],
            "Credentials":{"plan_type":"plus"},
            "Extra":{"codex_7d_used_percent":42}
        });
        let account: SchedulerAccountPayload = serde_json::from_value(payload).unwrap();
        let snapshot = SchedulerAccountSnapshot::from(account);
        assert_eq!(snapshot.id, 1);
        assert_eq!(snapshot.kind, "oauth");
        assert_eq!(snapshot.group_ids, vec![8]);
        assert_eq!(snapshot.credential_fields, vec!["plan_type"]);
        assert_eq!(snapshot.extra_fields, vec!["codex_7d_used_percent"]);
        assert_eq!(snapshot.effective_load_factor, 3);
    }

    #[test]
    fn sticky_candidate_promotion_only_uses_eligible_accounts() {
        let load = || AccountLoad {
            current_concurrency: 0,
            waiting_count: 0,
            load_rate: 0,
        };
        let mut candidates = vec![
            RankedCandidate {
                account_id: 1,
                priority: 1,
                score: 3.0,
                load: load(),
            },
            RankedCandidate {
                account_id: 2,
                priority: 2,
                score: 2.0,
                load: load(),
            },
        ];
        assert!(promote_candidate(&mut candidates, Some(2)));
        assert_eq!(candidates[0].account_id, 2);
        assert!(!promote_candidate(&mut candidates, Some(99)));
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn eligibility_matches_go_time_gates() {
        let now = Utc::now();
        let base = SchedulerAccountSnapshot {
            id: 1,
            name: "Codex".into(),
            platform: "openai".into(),
            kind: "oauth".into(),
            concurrency: 3,
            effective_load_factor: 3,
            priority: 50,
            rate_multiplier: 1.0,
            status: "active".into(),
            schedulable: true,
            expires_at: None,
            auto_pause_on_expired: false,
            rate_limit_reset_at: None,
            overload_until: None,
            temp_unschedulable_until: None,
            group_ids: vec![8],
            credential_fields: vec![],
            extra_fields: vec![],
        };
        assert_eq!(
            eligibility_reason(&base, "openai", &HashSet::new(), now),
            None
        );

        let mut limited = base.clone();
        limited.rate_limit_reset_at = Some(now + chrono::Duration::minutes(1));
        assert_eq!(
            eligibility_reason(&limited, "openai", &HashSet::new(), now),
            Some("rate_limited")
        );

        let mut expired_without_auto_pause = base;
        expired_without_auto_pause.expires_at = Some(now - chrono::Duration::minutes(1));
        assert_eq!(
            eligibility_reason(&expired_without_auto_pause, "openai", &HashSet::new(), now),
            None
        );
        expired_without_auto_pause.auto_pause_on_expired = true;
        assert_eq!(
            eligibility_reason(&expired_without_auto_pause, "openai", &HashSet::new(), now),
            Some("expired")
        );
    }
}
