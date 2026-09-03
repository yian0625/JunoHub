use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::Value;

use crate::{
    error::{AppError, AppResult},
    models::Envelope,
    state::AppState,
};

pub async fn dispatch(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Json(payload): Json<Value>,
) -> AppResult<Json<Envelope<Value>>> {
    let data = match path.trim_matches('/') {
        "users/list" => crate::console::admin_users(&state, &payload).await?,
        "users/update" => crate::console::admin_update_user(&state, &payload).await?,
        "rust-engine/gateway/openai-probe" => {
            let api_key = payload
                .get("api_key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let protocol = payload
                .get("protocol")
                .and_then(Value::as_str)
                .unwrap_or("responses");
            let stream = payload
                .get("stream")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            serde_json::to_value(
                crate::sub2_gateway::probe(&state, api_key, protocol, stream).await?,
            )?
        }
        "rust-engine/models" => {
            let api_key = payload
                .get("api_key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            serde_json::to_value(
                crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                    .model_ids_for_api_key(api_key)
                    .await?,
            )?
        }
        "rust-engine/outbox/audit" => {
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            let watermark = match payload.get("after_id").and_then(Value::as_i64) {
                Some(value) => value.max(0),
                None => {
                    scheduler_cache(&state, connection)
                        .outbox_watermark()
                        .await?
                }
            };
            let events = crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                .outbox_after(watermark, 200)
                .await?;
            serde_json::json!({"watermark":watermark,"events":events})
        }
        "rust-engine/outbox/cleanup-rollback-probe" => serde_json::to_value(
            crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                .verify_outbox_cleanup_rollback()
                .await?,
        )?,
        "rust-engine/outbox/claim-rollback-probe" => serde_json::to_value(
            crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                .verify_outbox_claim_rollback()
                .await?,
        )?,
        "rust-engine/snapshot" => serde_json::to_value(
            crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                .snapshot()
                .await?,
        )?,
        "rust-engine/accounts" => serde_json::to_value(
            crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                .accounts(100)
                .await?,
        )?,
        "rust-engine/groups" => serde_json::to_value(
            crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                .groups(100)
                .await?,
        )?,
        "rust-engine/keys" => serde_json::to_value(
            crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                .api_keys(100)
                .await?,
        )?,
        "rust-engine/account-groups" => {
            let id = payload
                .get("account_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| AppError::BadRequest("account_id 不能为空".into()))?;
            serde_json::to_value(
                crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                    .account_group_ids(id)
                    .await?,
            )?
        }
        "rust-engine/write-probe" => serde_json::to_value(
            crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                .verify_write_rollback()
                .await?,
        )?,
        "rust-engine/sticky/probe" => {
            let positive_id = |name: &str| {
                payload
                    .get(name)
                    .and_then(Value::as_i64)
                    .filter(|id| *id > 0)
                    .ok_or_else(|| AppError::BadRequest(format!("{name} 不能为空")))
            };
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::to_value(
                sticky_store(&state, connection)
                    .probe(
                        positive_id("group_id")?,
                        positive_id("account_id")?,
                        positive_id("user_id")?,
                        positive_id("api_key_id")?,
                    )
                    .await?,
            )?
        }
        "rust-engine/scheduler/watermark" => {
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::json!({"watermark":scheduler_cache(&state, connection).outbox_watermark().await?})
        }
        "rust-engine/scheduler/account" => {
            let id = payload
                .get("account_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| AppError::BadRequest("account_id 不能为空".into()))?;
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::to_value(scheduler_cache(&state, connection).account(id).await?)?
        }
        "rust-engine/scheduler/metadata" => {
            let id = payload
                .get("account_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| AppError::BadRequest("account_id 不能为空".into()))?;
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::to_value(scheduler_cache(&state, connection).metadata(id).await?)?
        }
        "rust-engine/scheduler/full-rebuild-probe" => {
            serde_json::to_value(crate::sub2_worker::rebuild_probe(&state).await?)?
        }
        "rust-engine/scheduler/account-wire-audit" => {
            let account_id = payload
                .get("account_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| AppError::BadRequest("account_id 不能为空".into()))?;
            let wire = crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
                .scheduler_account_wire(account_id)
                .await?
                .ok_or_else(|| AppError::NotFound("调度账户不存在".into()))?;
            let keys = |value: &Value| {
                let mut keys = value
                    .as_object()
                    .map(|object| object.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                keys.sort();
                keys
            };
            serde_json::json!({
                "account_id":account_id,
                "group_ids":wire.group_ids,
                "full_fields":keys(&wire.full),
                "metadata_fields":keys(&wire.metadata),
                "full_credential_fields":keys(&wire.full["Credentials"]),
                "metadata_credential_fields":keys(&wire.metadata["Credentials"]),
                "full_extra_fields":keys(&wire.full["Extra"]),
                "metadata_extra_fields":keys(&wire.metadata["Extra"])
            })
        }
        "rust-engine/scheduler/snapshot-control-probe" => {
            let account_id = payload
                .get("account_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| AppError::BadRequest("account_id 不能为空".into()))?;
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::to_value(
                scheduler_cache(&state, connection)
                    .snapshot_control_probe(account_id)
                    .await?,
            )?
        }
        "rust-engine/scheduler/concurrency-probe" => {
            let positive_id = |name: &str| {
                payload
                    .get(name)
                    .and_then(Value::as_i64)
                    .filter(|id| *id > 0)
                    .ok_or_else(|| AppError::BadRequest(format!("{name} 不能为空")))
            };
            let account_id = positive_id("account_id")?;
            let user_id = positive_id("user_id")?;
            let api_key_id = positive_id("api_key_id")?;
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::to_value(
                scheduler_cache(&state, connection)
                    .concurrency_probe(account_id, user_id, api_key_id)
                    .await?,
            )?
        }
        "rust-engine/scheduler/live-lease-probe" => {
            let positive_id = |name: &str| {
                payload
                    .get(name)
                    .and_then(Value::as_i64)
                    .filter(|id| *id > 0)
                    .ok_or_else(|| AppError::BadRequest(format!("{name} 不能为空")))
            };
            let account_id = positive_id("account_id")?;
            let user_id = positive_id("user_id")?;
            let api_key_id = positive_id("api_key_id")?;
            let user_max = payload
                .get("user_max")
                .and_then(Value::as_i64)
                .filter(|value| *value > 0 && *value <= i64::from(i32::MAX))
                .unwrap_or(30) as i32;
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::to_value(
                scheduler_cache(&state, connection)
                    .live_lease_probe(account_id, user_id, user_max, api_key_id)
                    .await?,
            )?
        }
        "rust-engine/scheduler/ingress-lease-probe" => {
            let api_key_id = payload
                .get("api_key_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| AppError::BadRequest("api_key_id 不能为空".into()))?;
            let max_connections = payload
                .get("max_connections")
                .and_then(Value::as_i64)
                .filter(|value| *value > 0 && *value <= i64::from(i32::MAX))
                .unwrap_or(1) as i32;
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::to_value(
                scheduler_cache(&state, connection)
                    .ingress_lease_probe(api_key_id, max_connections)
                    .await?,
            )?
        }
        "rust-engine/scheduler/slot-probe" => {
            let id = payload
                .get("account_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| AppError::BadRequest("account_id 不能为空".into()))?;
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::to_value(scheduler_cache(&state, connection).slot_probe(id).await?)?
        }
        "rust-engine/scheduler/sticky-select" => {
            let group_id = payload
                .get("group_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| AppError::BadRequest("group_id 不能为空".into()))?;
            let bucket_key = payload
                .get("bucket_key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let platform = payload
                .get("platform")
                .and_then(Value::as_str)
                .unwrap_or("openai")
                .to_owned();
            let excluded_ids = payload
                .get("excluded_ids")
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            let session_seed = payload
                .get("session_seed")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let response_id = payload
                .get("previous_response_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            let sticky = sticky_store(&state, connection.clone());
            let sticky_account_id = if session_seed.trim().is_empty() {
                None
            } else {
                let (current, legacy) = crate::sub2_sticky::derive_session_hashes(session_seed)?;
                sticky
                    .get_session_account(group_id, &current, Some(&legacy))
                    .await?
            };
            let previous_response_account_id = if response_id.trim().is_empty() {
                None
            } else {
                sticky.get_response_account(group_id, response_id).await?
            };
            serde_json::to_value(
                scheduler_cache(&state, connection)
                    .selection_snapshot(crate::sub2_scheduler::SelectionRequest {
                        bucket_key,
                        platform,
                        excluded_ids,
                        sticky_account_id,
                        previous_response_account_id,
                    })
                    .await?,
            )?
        }
        "rust-engine/scheduler/select" => {
            let request: crate::sub2_scheduler::SelectionRequest = serde_json::from_value(payload)?;
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::to_value(
                scheduler_cache(&state, connection)
                    .selection_snapshot(request)
                    .await?,
            )?
        }
        "rust-engine/scheduler/bucket" => {
            let key = payload
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?;
            serde_json::to_value(
                scheduler_cache(&state, connection)
                    .bucket_members(key)
                    .await?,
            )?
        }
        path => {
            return Err(AppError::NotFound(format!(
                "管理员接口不存在: /hub_admin/{path}"
            )))
        }
    };
    Ok(Json(Envelope::ok(data)))
}

fn sticky_store(
    state: &AppState,
    connection: redis::aio::ConnectionManager,
) -> crate::sub2_sticky::StickyStore {
    crate::sub2_sticky::StickyStore::new(
        connection,
        state.config.sub2api_sticky_ttl_seconds,
        state.config.sub2api_response_sticky_ttl_seconds,
        state.config.sub2api_session_hash_read_old_fallback,
        state.config.sub2api_session_hash_dual_write_old,
    )
}

fn scheduler_cache(
    state: &AppState,
    connection: redis::aio::ConnectionManager,
) -> crate::sub2_scheduler::SchedulerCache {
    crate::sub2_scheduler::SchedulerCache::new(
        connection,
        state.config.sub2api_slot_ttl_seconds,
        state.config.sub2api_wait_ttl_seconds,
    )
}
