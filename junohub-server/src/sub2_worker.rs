use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::{
    error::AppResult,
    state::AppState,
    sub2_repository::{SchedulerOutboxEvent, Sub2Repository},
    sub2_scheduler::{SchedulerBucket, SchedulerCache},
};

const PLATFORMS: [&str; 8] = [
    "anthropic",
    "gemini",
    "openai",
    "antigravity",
    "grok",
    "kimi",
    "zhipu",
    "deepseek",
];

#[derive(Debug, Serialize)]
pub struct SchedulerRebuildProbe {
    pub redis_database: i64,
    pub active_group_count: usize,
    pub registered_bucket_count: i64,
    pub openai_forced_ready: bool,
    pub openai_forced_account_ids: Vec<i64>,
    pub account_payload_present: bool,
    pub residual_keys: i64,
}

pub async fn rebuild_probe(state: &AppState) -> AppResult<SchedulerRebuildProbe> {
    let mut url = url::Url::parse(&state.config.sub2api_redis_url)
        .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
    url.set_path("/15");
    let client = redis::Client::open(url.as_str())
        .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
    let mut connection = redis::aio::ConnectionManager::new(client)
        .await
        .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
    let initial_size: i64 = redis::cmd("DBSIZE")
        .query_async(&mut connection)
        .await
        .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
    if initial_size != 0 {
        return Err(crate::error::AppError::Internal(
            "Redis DB 15 非空，拒绝运行隔离重建探针".into(),
        ));
    }
    let active_group_count = Sub2Repository::new(state.sub2_pool.clone())
        .active_group_ids()
        .await?
        .len();
    let worker = SchedulerWorker {
        repository: Sub2Repository::new(state.sub2_pool.clone()),
        cache: SchedulerCache::new(
            connection.clone(),
            state.config.sub2api_slot_ttl_seconds,
            state.config.sub2api_wait_ttl_seconds,
        ),
    };
    let operation = async {
        worker.full_rebuild().await?;
        let active: Option<i64> =
            redis::AsyncCommands::get(&mut connection, "sched:active:8:openai:forced")
                .await
                .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
        let openai_forced_account_ids = if let Some(version) = active {
            redis::AsyncCommands::zrange(
                &mut connection,
                format!("sched:8:openai:forced:v{version}"),
                0,
                -1,
            )
            .await
            .map_err(|error| crate::error::AppError::Internal(error.to_string()))?
        } else {
            Vec::new()
        };
        let ready: Option<String> =
            redis::AsyncCommands::get(&mut connection, "sched:ready:8:openai:forced")
                .await
                .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
        let account_payload_present: bool =
            redis::AsyncCommands::exists(&mut connection, "sched:acc:1")
                .await
                .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
        let registered_bucket_count: i64 =
            redis::AsyncCommands::scard(&mut connection, "sched:buckets")
                .await
                .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
        AppResult::Ok((
            registered_bucket_count,
            ready.as_deref() == Some("1"),
            openai_forced_account_ids,
            account_payload_present,
        ))
    }
    .await;
    let _: () = redis::cmd("FLUSHDB")
        .query_async(&mut connection)
        .await
        .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
    let residual_keys: i64 = redis::cmd("DBSIZE")
        .query_async(&mut connection)
        .await
        .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
    let values = operation?;
    Ok(SchedulerRebuildProbe {
        redis_database: 15,
        active_group_count,
        registered_bucket_count: values.0,
        openai_forced_ready: values.1,
        openai_forced_account_ids: values.2,
        account_payload_present: values.3,
        residual_keys,
    })
}

pub fn spawn(state: AppState) -> Option<JoinHandle<()>> {
    if !state.config.scheduler_worker_enabled {
        info!("Rust scheduler worker disabled while Go compatibility worker is active");
        return None;
    }
    Some(tokio::spawn(async move {
        let worker = SchedulerWorker::new(state.clone());
        loop {
            crate::monitor::heartbeat(&state,"account_scheduler",true,None).await;
            let result=worker.poll().await;
            crate::monitor::heartbeat(&state,"account_scheduler",true,Some(result.is_ok())).await;
            if let Err(error)=result {error!(error = %error, "Rust scheduler outbox poll failed");}
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }))
}

struct SchedulerWorker {
    repository: Sub2Repository,
    cache: SchedulerCache,
}

impl SchedulerWorker {
    fn new(state: AppState) -> Self {
        Self {
            repository: Sub2Repository::new(state.sub2_pool),
            cache: SchedulerCache::new(
                state.sub2_redis.expect("scheduler worker requires Redis"),
                state.config.sub2api_slot_ttl_seconds,
                state.config.sub2api_wait_ttl_seconds,
            ),
        }
    }

    async fn poll(&self) -> AppResult<()> {
        let watermark = self.cache.outbox_watermark().await?;
        let events = self.repository.claim_outbox(watermark, 200).await?;
        if events.is_empty() {
            return Ok(());
        }
        for event in &events {
            self.handle(event).await?;
        }
        self.cache
            .set_outbox_watermark(events.last().map(|event| event.id).unwrap_or(watermark))
            .await
    }

    async fn handle(&self, event: &SchedulerOutboxEvent) -> AppResult<()> {
        match event.event_type.as_str() {
            "account_last_used" => self.handle_last_used(event.payload.as_ref()).await,
            "account_changed" | "account_groups_changed" => {
                self.handle_account(event.account_id, event.payload.as_ref())
                    .await
            }
            "account_bulk_changed" => self.handle_bulk(event.payload.as_ref()).await,
            "group_changed" => {
                if let Some(group_id) = event.group_id.filter(|id| *id > 0) {
                    self.rebuild_group(group_id).await?;
                }
                Ok(())
            }
            "full_rebuild" => self.full_rebuild().await,
            _ => Ok(()),
        }
    }

    async fn handle_last_used(&self, payload: Option<&Value>) -> AppResult<()> {
        let Some(last_used) = payload
            .and_then(|payload| payload.get("last_used"))
            .and_then(Value::as_object)
        else {
            return Ok(());
        };
        for (account_id, timestamp) in last_used {
            if let (Ok(account_id), Some(timestamp)) =
                (account_id.parse::<i64>(), timestamp.as_i64())
            {
                self.cache.update_last_used(account_id, timestamp).await?;
            }
        }
        Ok(())
    }

    async fn handle_account(
        &self,
        account_id: Option<i64>,
        payload: Option<&Value>,
    ) -> AppResult<()> {
        let Some(account_id) = account_id.filter(|id| *id > 0) else {
            return Ok(());
        };
        let mut group_ids = payload_group_ids(payload);
        match self.repository.scheduler_account_wire(account_id).await? {
            Some(wire) => {
                group_ids.extend(wire.group_ids.iter().copied());
                self.cache.set_account_wire(account_id, &wire).await?;
            }
            None => self.cache.delete_account_wire(account_id).await?,
        }
        group_ids.sort_unstable();
        group_ids.dedup();
        for group_id in group_ids.into_iter().filter(|id| *id > 0) {
            self.rebuild_group(group_id).await?;
        }
        Ok(())
    }

    async fn handle_bulk(&self, payload: Option<&Value>) -> AppResult<()> {
        let account_ids = payload
            .and_then(|payload| payload.get("account_ids"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .filter(|id| *id > 0)
            .collect::<Vec<_>>();
        let mut group_ids = payload_group_ids(payload);
        for account_id in account_ids {
            if let Some(wire) = self.repository.scheduler_account_wire(account_id).await? {
                group_ids.extend(wire.group_ids.iter().copied());
                self.cache.set_account_wire(account_id, &wire).await?;
            } else {
                self.cache.delete_account_wire(account_id).await?;
            }
        }
        group_ids.sort_unstable();
        group_ids.dedup();
        for group_id in group_ids.into_iter().filter(|id| *id > 0) {
            self.rebuild_group(group_id).await?;
        }
        Ok(())
    }

    async fn full_rebuild(&self) -> AppResult<()> {
        self.rebuild_group(0).await?;
        for group_id in self.repository.active_group_ids().await? {
            self.rebuild_group(group_id).await?;
        }
        Ok(())
    }

    async fn rebuild_group(&self, group_id: i64) -> AppResult<()> {
        let active = group_id == 0 || self.repository.group_active(group_id).await?;
        for platform in PLATFORMS {
            for &mode in canonical_modes(platform) {
                let bucket = SchedulerBucket {
                    group_id,
                    platform: platform.to_owned(),
                    mode: mode.to_owned(),
                };
                if !active {
                    self.cache.retire_bucket(&bucket).await?;
                    continue;
                }
                self.cache.reopen_bucket(&bucket).await?;
                let account_ids = self
                    .repository
                    .schedulable_account_ids(group_id, platform, mode == "mixed")
                    .await?;
                for account_id in &account_ids {
                    if let Some(wire) = self.repository.scheduler_account_wire(*account_id).await? {
                        self.cache.set_account_wire(*account_id, &wire).await?;
                    }
                }
                self.cache.publish_snapshot(&bucket, &account_ids).await?;
            }
        }
        Ok(())
    }
}

fn canonical_modes(platform: &str) -> &'static [&'static str] {
    if matches!(platform, "anthropic" | "gemini") {
        &["single", "forced", "mixed"]
    } else {
        &["single", "forced"]
    }
}

fn payload_group_ids(payload: Option<&Value>) -> Vec<i64> {
    payload
        .and_then(|payload| payload.get("group_ids"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .filter(|id| *id > 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_bucket_modes_match_go() {
        assert_eq!(canonical_modes("openai"), ["single", "forced"]);
        assert_eq!(canonical_modes("anthropic"), ["single", "forced", "mixed"]);
        assert_eq!(PLATFORMS.len(), 8);
    }

    #[test]
    fn payload_group_ids_ignores_invalid_values() {
        let payload = serde_json::json!({"group_ids":[8,0,-1,"bad",9]});
        assert_eq!(payload_group_ids(Some(&payload)), vec![8, 9]);
    }
}
