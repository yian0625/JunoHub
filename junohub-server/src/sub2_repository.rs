use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    secrets,
};

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct EngineAccount {
    pub id: i64,
    pub name: String,
    pub notes: Option<String>,
    pub platform: String,
    pub kind: String,
    pub credentials: Value,
    pub extra: Value,
    pub proxy_id: Option<i64>,
    pub proxy_fallback_origin_id: Option<i64>,
    pub concurrency: i32,
    pub load_factor: Option<i32>,
    pub priority: i32,
    pub rate_multiplier: f64,
    pub status: String,
    pub schedulable: bool,
    pub error_message: Option<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub auto_pause_on_expired: bool,
    pub rate_limited_at: Option<DateTime<Utc>>,
    pub rate_limit_reset_at: Option<DateTime<Utc>>,
    pub overload_until: Option<DateTime<Utc>>,
    pub temp_unschedulable_until: Option<DateTime<Utc>>,
    pub temp_unschedulable_reason: Option<String>,
    pub session_window_start: Option<DateTime<Utc>>,
    pub session_window_end: Option<DateTime<Utc>>,
    pub session_window_status: Option<String>,
    pub parent_account_id: Option<i64>,
    pub quota_dimension: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SchedulerAccountWire {
    pub full: Value,
    pub metadata: Value,
    pub group_ids: Vec<i64>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct EngineGroup {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub platform: String,
    pub status: String,
    pub rate_multiplier: f64,
    pub exclusive: bool,
    pub rpm_limit: i32,
    pub model_routing: Option<Value>,
    pub supported_model_scopes: Value,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct EngineKeyRoute {
    pub api_key_id: i64,
    pub user_id: i64,
    pub group_id: i64,
    pub platform: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct EngineApiKey {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub key: String,
    pub group_id: Option<i64>,
    pub status: String,
    pub quota: f64,
    pub quota_used: f64,
    pub rate_limit_5h: f64,
    pub rate_limit_1d: f64,
    pub rate_limit_7d: f64,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct CreateBundleInput {
    pub sub2_user_id: i64,
    pub group_name: String,
    pub account_name: String,
    pub platform: String,
    pub kind: String,
    pub credentials: Value,
    pub extra: Value,
    pub concurrency: i32,
    pub priority: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct CreatedBundle {
    pub group_id: i64,
    pub account_id: i64,
    pub api_key_id: i64,
    pub api_key: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct ManagedAccountSummary {
    pub id: i64,
    pub name: String,
    pub platform: String,
    pub kind: String,
    pub priority: i32,
    pub member_order: i32,
    pub weight: i32,
    pub concurrency: i32,
    pub allowed_models: Vec<String>,
    pub rpm: i32,
    pub health: String,
    pub cooldown_until: Option<i64>,
    pub status: String,
    pub schedulable: bool,
    pub base_url: Option<String>,
    pub has_api_key: bool,
    pub oauth_type: Option<String>,
    pub project_id: Option<String>,
    pub client_id: Option<String>,
    pub has_client_secret: bool,
}

#[derive(Clone, Debug, FromRow)]
struct ManagedAccountRow {
    id: i64,
    name: String,
    platform: String,
    kind: String,
    priority: i32,
    member_order: i32,
    weight: i32,
    concurrency: i32,
    allowed_models: Vec<String>,
    rpm: i32,
    health: String,
    cooldown_until: Option<i64>,
    status: String,
    schedulable: bool,
    credentials: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EngineSnapshot {
    pub accounts: i64,
    pub groups: i64,
    pub api_keys: i64,
    pub account_groups: i64,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct SchedulerOutboxEvent {
    pub id: i64,
    pub event_type: String,
    pub account_id: Option<i64>,
    pub group_id: Option<i64>,
    pub payload: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub has_dedup_key: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OutboxCleanupProbe {
    pub first_lock_acquired: bool,
    pub second_lock_acquired: bool,
    pub lock_released: bool,
    pub old_event_deleted_inside_transaction: bool,
    pub new_event_preserved_inside_transaction: bool,
    pub rolled_back: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OutboxClaimProbe {
    pub event_id: i64,
    pub selected_event_id: i64,
    pub selected_event_type: String,
    pub dedup_released_inside_transaction: bool,
    pub rolled_back: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct WriteProbe {
    pub group_id: i64,
    pub account_id: i64,
    pub api_key_id: i64,
    pub group_linked: bool,
    pub scheduler_events: i64,
    pub outbox_rolled_back: bool,
    pub rolled_back: bool,
    pub before: EngineSnapshot,
    pub after: EngineSnapshot,
}

#[derive(Clone)]
pub struct Sub2Repository {
    pool: PgPool,
}

impl Sub2Repository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn protect_plaintext_credentials(&self, encryption_key: &str) -> AppResult<u64> {
        let rows = sqlx::query_as::<_, (i64, Value)>(
            "SELECT id,credentials FROM accounts WHERE deleted_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut updated = 0;
        for (id, credentials) in rows {
            let protected = secrets::protect_credentials(encryption_key, &credentials)?;
            if protected == credentials {
                continue;
            }
            updated +=
                sqlx::query("UPDATE accounts SET credentials=$1,updated_at=NOW() WHERE id=$2")
                    .bind(protected)
                    .bind(id)
                    .execute(&self.pool)
                    .await?
                    .rows_affected();
        }
        Ok(updated)
    }

    /// 报告“名字像密钥、却仍是明文”的字段，供启动自检使用。
    ///
    /// 只返回账号 id 和**字段名**，不返回值：告警要能进日志，但不能把密钥
    /// 本身写进日志。
    pub async fn audit_plaintext_credentials(&self) -> AppResult<Vec<(i64, Vec<String>)>> {
        let rows = sqlx::query_as::<_, (i64, Value)>(
            "SELECT id,credentials FROM accounts WHERE deleted_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, credentials)| {
                let fields = secrets::unprotected_sensitive_fields(&credentials);
                (!fields.is_empty()).then_some((id, fields))
            })
            .collect())
    }

    pub async fn verify_outbox_cleanup_rollback(&self) -> AppResult<OutboxCleanupProbe> {
        let mut first = self.pool.acquire().await?;
        let mut second = self.pool.acquire().await?;
        let first_lock_acquired: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('scheduler_outbox_cleanup'))")
                .fetch_one(&mut *first)
                .await?;
        let second_lock_acquired: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('scheduler_outbox_cleanup'))")
                .fetch_one(&mut *second)
                .await?;
        let lock_released: bool =
            sqlx::query_scalar("SELECT pg_advisory_unlock(hashtext('scheduler_outbox_cleanup'))")
                .fetch_one(&mut *first)
                .await?;
        drop(first);
        drop(second);

        let mut transaction = self.pool.begin().await?;
        let old_id: i64 = sqlx::query_scalar(
            "INSERT INTO scheduler_outbox(event_type,payload,created_at) VALUES('account_last_used','{}'::jsonb,NOW()-INTERVAL '20 seconds') RETURNING id",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let new_id: i64 = sqlx::query_scalar(
            "INSERT INTO scheduler_outbox(event_type,payload,created_at) VALUES('account_last_used','{}'::jsonb,NOW()) RETURNING id",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let deleted: Vec<i64> = sqlx::query_scalar(
            "WITH doomed AS (SELECT id FROM scheduler_outbox WHERE id<=$1 AND created_at<NOW()-INTERVAL '10 seconds' ORDER BY id ASC LIMIT 5000) DELETE FROM scheduler_outbox o USING doomed d WHERE o.id=d.id RETURNING o.id",
        )
        .bind(new_id)
        .fetch_all(&mut *transaction)
        .await?;
        let new_event_preserved_inside_transaction: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM scheduler_outbox WHERE id=$1)")
                .bind(new_id)
                .fetch_one(&mut *transaction)
                .await?;
        transaction.rollback().await?;
        let residual: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM scheduler_outbox WHERE id IN ($1,$2)")
                .bind(old_id)
                .bind(new_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(OutboxCleanupProbe {
            first_lock_acquired,
            second_lock_acquired,
            lock_released,
            old_event_deleted_inside_transaction: deleted.contains(&old_id),
            new_event_preserved_inside_transaction,
            rolled_back: residual == 0,
        })
    }

    pub async fn claim_outbox(
        &self,
        after_id: i64,
        limit: i64,
    ) -> AppResult<Vec<SchedulerOutboxEvent>> {
        Ok(sqlx::query_as::<_, SchedulerOutboxEvent>(
            "WITH selected AS MATERIALIZED (SELECT id,event_type,account_id,group_id,payload,created_at FROM scheduler_outbox WHERE id>$1 ORDER BY id ASC LIMIT $2 FOR UPDATE), released AS (UPDATE scheduler_outbox AS o SET dedup_key=NULL FROM selected AS s WHERE o.id=s.id AND o.dedup_key IS NOT NULL RETURNING o.id) SELECT s.id,s.event_type,s.account_id,s.group_id,s.payload,s.created_at,false AS has_dedup_key FROM selected AS s CROSS JOIN (SELECT COUNT(*) FROM released) AS release_barrier ORDER BY s.id ASC",
        )
        .bind(after_id.max(0))
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn group_active(&self, group_id: i64) -> AppResult<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM groups WHERE id=$1 AND deleted_at IS NULL AND status='active')",
        )
        .bind(group_id)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn outbox_after(
        &self,
        after_id: i64,
        limit: i64,
    ) -> AppResult<Vec<SchedulerOutboxEvent>> {
        Ok(sqlx::query_as::<_, SchedulerOutboxEvent>(
            "SELECT id,event_type,account_id,group_id,payload,created_at,(dedup_key IS NOT NULL) AS has_dedup_key FROM scheduler_outbox WHERE id>$1 ORDER BY id ASC LIMIT $2",
        )
        .bind(after_id.max(0))
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn verify_outbox_claim_rollback(&self) -> AppResult<OutboxClaimProbe> {
        let mut transaction = self.pool.begin().await?;
        let dedup_key = format!("scheduler_outbox:rust-claim-probe-{}", Uuid::new_v4());
        let event_id: i64 = sqlx::query_scalar(
            "INSERT INTO scheduler_outbox(event_type,group_id,payload,dedup_key) VALUES('group_changed',9223372036854770000,'{}'::jsonb,$1) RETURNING id",
        )
        .bind(&dedup_key)
        .fetch_one(&mut *transaction)
        .await?;
        let selected = sqlx::query_as::<_, SchedulerOutboxEvent>(
            "WITH selected AS MATERIALIZED (SELECT id,event_type,account_id,group_id,payload,created_at FROM scheduler_outbox WHERE id>$1 ORDER BY id ASC LIMIT 1 FOR UPDATE), released AS (UPDATE scheduler_outbox AS o SET dedup_key=NULL FROM selected AS s WHERE o.id=s.id AND o.dedup_key IS NOT NULL RETURNING o.id) SELECT s.id,s.event_type,s.account_id,s.group_id,s.payload,s.created_at,false AS has_dedup_key FROM selected AS s CROSS JOIN (SELECT COUNT(*) FROM released) AS release_barrier ORDER BY s.id ASC",
        )
        .bind(event_id - 1)
        .fetch_one(&mut *transaction)
        .await?;
        let dedup_released_inside_transaction: bool =
            sqlx::query_scalar("SELECT dedup_key IS NULL FROM scheduler_outbox WHERE id=$1")
                .bind(event_id)
                .fetch_one(&mut *transaction)
                .await?;
        transaction.rollback().await?;
        let residual: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM scheduler_outbox WHERE dedup_key=$1 OR id=$2")
                .bind(&dedup_key)
                .bind(event_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(OutboxClaimProbe {
            event_id,
            selected_event_id: selected.id,
            selected_event_type: selected.event_type,
            dedup_released_inside_transaction,
            rolled_back: residual == 0,
        })
    }

    pub async fn snapshot(&self) -> AppResult<EngineSnapshot> {
        let (accounts, groups, api_keys, account_groups) = tokio::try_join!(
            self.count("accounts"),
            self.count("groups"),
            self.count("api_keys"),
            self.count_links(),
        )?;
        Ok(EngineSnapshot {
            accounts,
            groups,
            api_keys,
            account_groups,
        })
    }

    pub async fn update_openai_tokens_cas(
        &self,
        account_id: i64,
        old_refresh_token: &str,
        token_patch: &Value,
        encryption_key: &str,
    ) -> AppResult<bool> {
        let mut transaction = self.pool.begin().await?;
        let current: Option<Value> = sqlx::query_scalar(
            "SELECT credentials FROM accounts WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let matches_refresh_token = current
            .as_ref()
            .map(|credentials| secrets::reveal_credentials(encryption_key, credentials))
            .transpose()?
            .and_then(|credentials| {
                credentials
                    .get("refresh_token")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .is_some_and(|current| current == old_refresh_token);
        if matches_refresh_token {
            let protected_patch = secrets::protect_credentials(encryption_key, token_patch)?;
            sqlx::query(
                "UPDATE accounts SET credentials=COALESCE(credentials,'{}'::jsonb)||$1,updated_at=NOW() WHERE id=$2 AND deleted_at IS NULL",
            )
            .bind(protected_patch)
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
            let group_ids: Vec<i64> = sqlx::query_scalar(
                "SELECT group_id FROM account_groups WHERE account_id=$1 ORDER BY priority,group_id",
            )
            .bind(account_id)
            .fetch_all(&mut *transaction)
            .await?;
            let payload = serde_json::json!({"group_ids":group_ids});
            enqueue_scheduler_event(
                &mut transaction,
                "account_changed",
                Some(account_id),
                None,
                Some(&payload),
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(matches_refresh_token)
    }

    pub async fn touch_account_last_used(&self, account_id: i64) -> AppResult<()> {
        if account_id <= 0 {
            return Ok(());
        }
        let timestamp = chrono::Utc::now().timestamp();
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE accounts SET last_used_at=NOW(),updated_at=NOW() WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        let payload = serde_json::json!({"last_used":{account_id.to_string():timestamp}});
        enqueue_scheduler_event(
            &mut transaction,
            "account_last_used",
            Some(account_id),
            None,
            Some(&payload),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_usage(
        &self,
        engine_api_key: &str,
        account_id: i64,
        requested_model: &str,
        upstream_model: &str,
        input_tokens: i64,
        output_tokens: i64,
        duration_ms: i64,
        stream: bool,
    ) -> AppResult<()> {
        if account_id <= 0 {
            return Ok(());
        }
        let route = self.resolve_engine_key(engine_api_key).await?;
        let requested_model = requested_model.chars().take(100).collect::<String>();
        let upstream_model = upstream_model.chars().take(100).collect::<String>();
        let input_tokens = input_tokens.clamp(0, i32::MAX as i64) as i32;
        let output_tokens = output_tokens.clamp(0, i32::MAX as i64) as i32;
        let duration_ms = duration_ms.clamp(0, i32::MAX as i64) as i32;
        let request_id = uuid::Uuid::new_v4().simple().to_string();
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO usage_logs(user_id,api_key_id,account_id,request_id,model,input_tokens,output_tokens,stream,duration_ms,group_id,requested_model,upstream_model,inbound_endpoint,billing_type,request_type) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'junohub',0,0)",
        )
        .bind(route.user_id)
        .bind(route.api_key_id)
        .bind(account_id)
        .bind(request_id)
        .bind(&upstream_model)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(stream)
        .bind(duration_ms)
        .bind(route.group_id)
        .bind(&requested_model)
        .bind(&upstream_model)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE api_keys SET last_used_at=NOW(),updated_at=NOW() WHERE id=$1 AND deleted_at IS NULL")
            .bind(route.api_key_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn record_account_failure(
        &self,
        account_id: i64,
        status: Option<u16>,
        reason: &str,
        retry_after_seconds: i64,
    ) -> AppResult<()> {
        if account_id <= 0 {
            return Ok(());
        }
        let cooldown = retry_after_seconds.clamp(5, 3600);
        let mut transaction = self.pool.begin().await?;
        match status {
            Some(429) => {
                sqlx::query(
                    "UPDATE accounts SET rate_limited_at=NOW(),rate_limit_reset_at=NOW()+($1*INTERVAL '1 second'),error_message=$2,updated_at=NOW() WHERE id=$3 AND deleted_at IS NULL",
                )
                .bind(cooldown)
                .bind(reason)
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
            }
            Some(code) if code >= 500 => {
                sqlx::query(
                    "UPDATE accounts SET overload_until=NOW()+($1*INTERVAL '1 second'),error_message=$2,updated_at=NOW() WHERE id=$3 AND deleted_at IS NULL",
                )
                .bind(cooldown)
                .bind(reason)
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
            }
            _ => {
                sqlx::query(
                    "UPDATE accounts SET temp_unschedulable_until=NOW()+($1*INTERVAL '1 second'),temp_unschedulable_reason=$2,error_message=$2,updated_at=NOW() WHERE id=$3 AND deleted_at IS NULL",
                )
                .bind(cooldown)
                .bind(reason)
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
            }
        }
        let group_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT group_id FROM account_groups WHERE account_id=$1 ORDER BY group_id",
        )
        .bind(account_id)
        .fetch_all(&mut *transaction)
        .await?;
        enqueue_scheduler_event(
            &mut transaction,
            "account_changed",
            Some(account_id),
            None,
            Some(&serde_json::json!({"group_ids":group_ids})),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn clear_account_failure(&self, account_id: i64) -> AppResult<()> {
        if account_id <= 0 {
            return Ok(());
        }
        sqlx::query(
            "UPDATE accounts SET error_message=NULL,temp_unschedulable_until=NULL,temp_unschedulable_reason=NULL,updated_at=NOW() WHERE id=$1 AND deleted_at IS NULL AND (error_message IS NOT NULL OR temp_unschedulable_until IS NOT NULL)",
        )
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn resolve_engine_key(&self, api_key: &str) -> AppResult<EngineKeyRoute> {
        sqlx::query_as::<_, EngineKeyRoute>(
            "SELECT k.id AS api_key_id,k.user_id,k.group_id,g.platform FROM api_keys k JOIN groups g ON g.id=k.group_id WHERE k.key=$1 AND k.deleted_at IS NULL AND k.status='active' AND (k.expires_at IS NULL OR k.expires_at>NOW()) AND g.deleted_at IS NULL AND g.status='active'",
        )
        .bind(api_key)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AppError::Unauthorized)
    }

    pub async fn account_by_id(&self, account_id: i64) -> AppResult<Option<EngineAccount>> {
        Ok(sqlx::query_as::<_, EngineAccount>(
            "SELECT id,name,notes,platform,type AS kind,credentials,extra,proxy_id,proxy_fallback_origin_id,concurrency,load_factor,priority,rate_multiplier::float8 AS rate_multiplier,status,schedulable,error_message,last_used_at,expires_at,auto_pause_on_expired,rate_limited_at,rate_limit_reset_at,overload_until,temp_unschedulable_until,temp_unschedulable_reason,session_window_start,session_window_end,session_window_status,parent_account_id,quota_dimension,created_at,updated_at FROM accounts WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn model_ids_for_api_key(&self, api_key: &str) -> AppResult<Vec<String>> {
        let group_id: i64 = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT group_id FROM api_keys WHERE key=$1 AND deleted_at IS NULL AND status='active'",
        )
        .bind(api_key)
        .fetch_optional(&self.pool)
        .await?
        .flatten()
        .ok_or_else(|| AppError::NotFound("账户池 API Key 不存在".into()))?;
        let platform: String = sqlx::query_scalar(
            "SELECT platform FROM groups WHERE id=$1 AND deleted_at IS NULL AND status='active'",
        )
        .bind(group_id)
        .fetch_one(&self.pool)
        .await?;
        let credentials: Vec<Value> = sqlx::query_scalar(
            "SELECT a.credentials FROM account_groups ag JOIN accounts a ON a.id=ag.account_id WHERE ag.group_id=$1 AND ag.priority>0 AND a.deleted_at IS NULL AND a.status='active' AND a.schedulable=true ORDER BY ag.priority,a.priority,a.id",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        let mut models = credentials
            .iter()
            .filter_map(|credentials| credentials.get("model_mapping"))
            .filter_map(Value::as_object)
            .flat_map(|mapping| mapping.keys().cloned())
            .filter(|model| !model.trim().is_empty())
            .collect::<Vec<_>>();
        if models.is_empty() {
            models.extend(
                default_model_ids(&platform)
                    .iter()
                    .map(|value| (*value).to_owned()),
            );
        }
        models.sort();
        models.dedup();
        Ok(models)
    }

    pub async fn active_group_ids(&self) -> AppResult<Vec<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM groups WHERE deleted_at IS NULL AND status='active' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn schedulable_account_ids(
        &self,
        group_id: i64,
        platform: &str,
        mixed: bool,
    ) -> AppResult<Vec<i64>> {
        let platforms = if mixed {
            vec![platform.to_owned(), "antigravity".to_owned()]
        } else {
            vec![platform.to_owned()]
        };
        let rows = if group_id > 0 {
            sqlx::query_scalar::<_, i64>(
                "SELECT a.id FROM account_groups ag JOIN accounts a ON a.id=ag.account_id WHERE ag.group_id=$1 AND ag.priority>0 AND a.deleted_at IS NULL AND a.status='active' AND a.schedulable=true AND a.platform=ANY($2) AND (a.temp_unschedulable_until IS NULL OR a.temp_unschedulable_until<=NOW()) AND (a.expires_at IS NULL OR a.expires_at>NOW() OR a.auto_pause_on_expired=false) AND (a.overload_until IS NULL OR a.overload_until<=NOW()) AND (a.rate_limit_reset_at IS NULL OR a.rate_limit_reset_at<=NOW()) AND (a.platform<>'antigravity' OR COALESCE((a.extra->>'mixed_scheduling')::boolean,false)=true OR $3=false) ORDER BY ag.priority,a.priority,a.id",
            )
            .bind(group_id)
            .bind(&platforms)
            .bind(mixed)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT a.id FROM accounts a WHERE a.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM account_groups ag WHERE ag.account_id=a.id) AND a.status='active' AND a.schedulable=true AND a.platform=ANY($1) AND (a.temp_unschedulable_until IS NULL OR a.temp_unschedulable_until<=NOW()) AND (a.expires_at IS NULL OR a.expires_at>NOW() OR a.auto_pause_on_expired=false) AND (a.overload_until IS NULL OR a.overload_until<=NOW()) AND (a.rate_limit_reset_at IS NULL OR a.rate_limit_reset_at<=NOW()) AND (a.platform<>'antigravity' OR COALESCE((a.extra->>'mixed_scheduling')::boolean,false)=true OR $2=false) ORDER BY a.priority,a.id",
            )
            .bind(&platforms)
            .bind(mixed)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(rows)
    }

    pub async fn scheduler_account_wire(
        &self,
        account_id: i64,
    ) -> AppResult<Option<SchedulerAccountWire>> {
        if account_id <= 0 {
            return Ok(None);
        }
        let account = sqlx::query_as::<_, EngineAccount>(
            "SELECT id,name,notes,platform,type AS kind,credentials,extra,proxy_id,proxy_fallback_origin_id,concurrency,load_factor,priority,rate_multiplier::float8 AS rate_multiplier,status,schedulable,error_message,last_used_at,expires_at,auto_pause_on_expired,rate_limited_at,rate_limit_reset_at,overload_until,temp_unschedulable_until,temp_unschedulable_reason,session_window_start,session_window_end,session_window_status,parent_account_id,quota_dimension,created_at,updated_at FROM accounts WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(account) = account else {
            return Ok(None);
        };
        let account_groups: Vec<Value> = sqlx::query_scalar(
            "SELECT jsonb_build_object('AccountID',account_id,'GroupID',group_id,'Priority',priority,'CreatedAt',created_at) FROM account_groups WHERE account_id=$1 ORDER BY priority,group_id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        let group_ids = account_groups
            .iter()
            .filter_map(|group| group.get("GroupID").and_then(Value::as_i64))
            .collect::<Vec<_>>();
        let full = scheduler_account_json(
            &account,
            account.credentials.clone(),
            account.extra.clone(),
            &account_groups,
            &group_ids,
        );
        let metadata = scheduler_account_json(
            &account,
            filter_json_fields(&account.credentials, SCHEDULER_CREDENTIAL_FIELDS),
            filter_json_fields(&account.extra, SCHEDULER_EXTRA_FIELDS),
            &account_groups,
            &group_ids,
        );
        Ok(Some(SchedulerAccountWire {
            full,
            metadata,
            group_ids,
        }))
    }

    pub async fn accounts(&self, limit: i64) -> AppResult<Vec<EngineAccount>> {
        Ok(sqlx::query_as::<_, EngineAccount>(
            "SELECT id,name,notes,platform,type AS kind,credentials,extra,proxy_id,proxy_fallback_origin_id,concurrency,load_factor,priority,rate_multiplier::float8 AS rate_multiplier,status,schedulable,error_message,last_used_at,expires_at,auto_pause_on_expired,rate_limited_at,rate_limit_reset_at,overload_until,temp_unschedulable_until,temp_unschedulable_reason,session_window_start,session_window_end,session_window_status,parent_account_id,quota_dimension,created_at,updated_at FROM accounts WHERE deleted_at IS NULL ORDER BY id LIMIT $1",
        )
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn groups(&self, limit: i64) -> AppResult<Vec<EngineGroup>> {
        Ok(sqlx::query_as::<_, EngineGroup>(
            "SELECT id,name,description,platform,status,rate_multiplier::float8 AS rate_multiplier,is_exclusive AS exclusive,rpm_limit,model_routing,supported_model_scopes FROM groups WHERE deleted_at IS NULL ORDER BY sort_order,id LIMIT $1",
        )
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn api_keys(&self, limit: i64) -> AppResult<Vec<EngineApiKey>> {
        Ok(sqlx::query_as::<_, EngineApiKey>(
            "SELECT id,user_id,name,key,group_id,status,quota::float8 AS quota,quota_used::float8 AS quota_used,rate_limit_5h::float8 AS rate_limit_5h,rate_limit_1d::float8 AS rate_limit_1d,rate_limit_7d::float8 AS rate_limit_7d,expires_at,last_used_at FROM api_keys WHERE deleted_at IS NULL ORDER BY id LIMIT $1",
        )
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sub2_user_id(&self, email: &str) -> AppResult<i64> {
        sqlx::query_scalar("SELECT id FROM users WHERE email=$1 AND deleted_at IS NULL")
            .bind(email)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound("Sub2API 管理用户不存在".into()))
    }

    pub async fn create_bundle(&self, input: CreateBundleInput) -> AppResult<CreatedBundle> {
        if input.sub2_user_id <= 0
            || input.group_name.trim().is_empty()
            || input.account_name.trim().is_empty()
            || input.platform.trim().is_empty()
            || input.kind.trim().is_empty()
            || input.concurrency < 0
        {
            return Err(AppError::BadRequest("账户创建参数无效".into()));
        }
        let mut transaction = self.pool.begin().await?;
        let group_id: i64 = sqlx::query_scalar(
            "INSERT INTO groups(name,description,platform,rate_multiplier,is_exclusive,status,subscription_type,long_context_pricing_enabled) VALUES($1,'Managed by JunoHub Rust engine',$2,1,true,'active','standard',true) RETURNING id",
        )
        .bind(input.group_name.trim())
        .bind(input.platform.trim())
        .fetch_one(&mut *transaction)
        .await?;
        enqueue_scheduler_event(
            &mut transaction,
            "group_changed",
            None,
            Some(group_id),
            None,
        )
        .await?;
        let account_id: i64 = sqlx::query_scalar(
            "INSERT INTO accounts(name,notes,platform,type,credentials,extra,concurrency,priority,status,schedulable,rate_multiplier,auto_pause_on_expired) VALUES($1,'Managed by JunoHub Rust engine',$2,$3,$4,$5,$6,$7,'active',true,1,true) RETURNING id",
        )
        .bind(input.account_name.trim())
        .bind(input.platform.trim())
        .bind(input.kind.trim())
        .bind(input.credentials)
        .bind(input.extra)
        .bind(input.concurrency)
        .bind(input.priority)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO account_groups(account_id,group_id,priority) VALUES($1,$2,$3)")
            .bind(account_id)
            .bind(group_id)
            .bind(input.priority)
            .execute(&mut *transaction)
            .await?;
        let account_payload = serde_json::json!({"group_ids":[group_id]});
        enqueue_scheduler_event(
            &mut transaction,
            "account_changed",
            Some(account_id),
            None,
            Some(&account_payload),
        )
        .await?;
        let api_key = format!("sk-{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let api_key_id: i64 = sqlx::query_scalar(
            "INSERT INTO api_keys(user_id,key,name,group_id,status) VALUES($1,$2,$3,$4,'active') RETURNING id",
        )
        .bind(input.sub2_user_id)
        .bind(&api_key)
        .bind(format!("JunoHub · {}", input.account_name.trim()))
        .bind(group_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(CreatedBundle {
            group_id,
            account_id,
            api_key_id,
            api_key,
        })
    }

    pub async fn add_account_to_group(
        &self,
        group_id: i64,
        input: CreateBundleInput,
    ) -> AppResult<i64> {
        if group_id <= 0
            || input.account_name.trim().is_empty()
            || input.platform.trim().is_empty()
            || input.kind.trim().is_empty()
            || input.concurrency < 0
        {
            return Err(AppError::BadRequest("账户创建参数无效".into()));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
        let duplicate: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts a JOIN account_groups ag ON ag.account_id=a.id WHERE ag.group_id=$1 AND a.deleted_at IS NULL AND lower(trim(a.name))=lower($2) AND a.id<>$3)").bind(group_id).bind(input.account_name.trim()).bind(0_i64).fetch_one(&mut *transaction).await?;
        if duplicate {
            return Err(AppError::BadRequest(
                "此供应商下已有同名账户，请换一个名称".into(),
            ));
        }

        let platform: Option<String> = sqlx::query_scalar(
            "SELECT platform FROM groups WHERE id=$1 AND deleted_at IS NULL AND status='active' FOR UPDATE",
        )
        .bind(group_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if platform.as_deref() != Some(input.platform.trim()) {
            return Err(AppError::BadRequest("账户平台与目标账户池不一致".into()));
        }
        let priority: i32 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(a.priority),0)+1 FROM accounts a JOIN account_groups ag ON ag.account_id=a.id WHERE ag.group_id=$1 AND a.deleted_at IS NULL",
        )
        .bind(group_id)
        .fetch_one(&mut *transaction)
        .await?;
        let account_id: i64 = sqlx::query_scalar(
            "INSERT INTO accounts(name,notes,platform,type,credentials,extra,concurrency,priority,status,schedulable,rate_multiplier,auto_pause_on_expired) VALUES($1,'Managed by JunoHub Rust engine',$2,$3,$4,$5,$6,$7,'active',true,1,true) RETURNING id",
        )
        .bind(input.account_name.trim())
        .bind(input.platform.trim())
        .bind(input.kind.trim())
        .bind(input.credentials)
        .bind(input.extra)
        .bind(input.concurrency)
        .bind(priority)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO account_groups(account_id,group_id,priority) VALUES($1,$2,$3)")
            .bind(account_id)
            .bind(group_id)
            .bind(priority)
            .execute(&mut *transaction)
            .await?;
        let payload = serde_json::json!({"group_ids":[group_id]});
        enqueue_scheduler_event(
            &mut transaction,
            "account_changed",
            Some(account_id),
            None,
            Some(&payload),
        )
        .await?;
        transaction.commit().await?;
        Ok(account_id)
    }

    pub async fn managed_accounts(
        &self,
        group_id: i64,
        encryption_key: &str,
    ) -> AppResult<Vec<ManagedAccountSummary>> {
        if group_id <= 0 {
            return Err(AppError::BadRequest("账户池标识无效".into()));
        }
        let rows = sqlx::query_as::<_, ManagedAccountRow>(
            "SELECT a.id,a.name,a.platform,a.type AS kind,ag.priority,COALESCE((a.extra->>'junohub_order')::integer,0) AS member_order,COALESCE((a.extra->>'junohub_rpm')::integer,0) AS rpm,COALESCE((a.extra->>'junohub_weight')::integer,1) AS weight,CASE WHEN a.load_factor>0 THEN a.load_factor ELSE a.concurrency END AS concurrency,ARRAY(SELECT jsonb_array_elements_text(COALESCE(a.extra->'junohub_models','[]'::jsonb))) AS allowed_models,CASE WHEN a.status<>'active' OR NOT a.schedulable THEN 'disabled' WHEN a.auto_pause_on_expired AND a.expires_at<=NOW() THEN 'expired' WHEN a.rate_limit_reset_at>NOW() THEN 'rate_limited' WHEN GREATEST(a.overload_until,a.temp_unschedulable_until)>NOW() THEN 'cooling' ELSE 'ready' END AS health,EXTRACT(EPOCH FROM GREATEST(a.rate_limit_reset_at,a.overload_until,a.temp_unschedulable_until))::bigint AS cooldown_until,a.status,a.schedulable,a.credentials FROM account_groups ag JOIN accounts a ON a.id=ag.account_id WHERE ag.group_id=$1 AND a.deleted_at IS NULL ORDER BY ag.priority,a.priority,a.id",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let credentials = secrets::reveal_credentials(encryption_key, &row.credentials)?;
                Ok(ManagedAccountSummary {
                    id: row.id,
                    name: row.name,
                    platform: row.platform,
                    kind: row.kind,
                    priority: row.priority,
                    member_order: row.member_order,
                    weight: row.weight,
                    concurrency: row.concurrency,
                    allowed_models: row.allowed_models,
                    rpm: row.rpm,
                    health: row.health,
                    cooldown_until: row.cooldown_until,
                    status: row.status,
                    schedulable: row.schedulable && row.priority > 0,
                    base_url: credentials
                        .get("base_url")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    has_api_key: credentials
                        .get("api_key")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.is_empty()),
                    oauth_type: credentials
                        .get("oauth_type")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    project_id: credentials
                        .get("project_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    client_id: credentials
                        .get("client_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    has_client_secret: credentials
                        .get("client_secret")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.is_empty()),
                })
            })
            .collect()
    }

    pub async fn dispatch_accounts(
        &self,
        group_id: i64,
    ) -> AppResult<Vec<crate::sub2_policy::AccountDispatch>> {
        Ok(sqlx::query_as("SELECT a.id,ag.priority,COALESCE((a.extra->>'junohub_order')::integer,0) AS member_order,COALESCE((a.extra->>'junohub_rpm')::integer,0) AS rpm,COALESCE((a.extra->>'junohub_weight')::integer,1) AS weight,a.concurrency,ARRAY(SELECT jsonb_array_elements_text(COALESCE(a.extra->'junohub_models','[]'::jsonb))) AS allowed_models FROM accounts a JOIN account_groups ag ON ag.account_id=a.id WHERE ag.group_id=$1 AND a.deleted_at IS NULL ORDER BY ag.priority,a.id")
            .bind(group_id).fetch_all(&self.pool).await?)
    }
    pub async fn update_account_dispatch(
        &self,
        group_id: i64,
        accounts: &[crate::sub2_policy::AccountDispatch],
    ) -> AppResult<()> {
        crate::sub2_policy::validate_accounts(accounts)?;
        let mut transaction = self.pool.begin().await?;
        let mut ordered: Vec<_> = accounts.iter().collect();
        ordered.sort_by_key(|a| a.id);
        for account in ordered {
            let result = sqlx::query("UPDATE accounts SET priority=$1,concurrency=$2,load_factor=NULL,extra=jsonb_set(jsonb_set(jsonb_set(COALESCE(extra,'{}'::jsonb),'{junohub_weight}',to_jsonb($3::integer)),'{junohub_models}',$6::jsonb),'{junohub_rpm}',to_jsonb($7::integer)),updated_at=NOW() WHERE id=$4 AND deleted_at IS NULL AND EXISTS(SELECT 1 FROM account_groups WHERE account_id=$4 AND group_id=$5)")
                .bind(account.priority).bind(account.concurrency).bind(account.weight).bind(account.id).bind(group_id).bind(serde_json::json!(account.allowed_models)).bind(account.rpm).execute(&mut *transaction).await?;
            if result.rows_affected() != 1 {
                return Err(AppError::NotFound(
                    "账户不存在或不属于此供应商，未保存任何修改".into(),
                ));
            }
            sqlx::query("UPDATE accounts SET extra=jsonb_set(COALESCE(extra,'{}'::jsonb),'{junohub_order}',to_jsonb($1::integer)) WHERE id=$2").bind(account.member_order).bind(account.id).execute(&mut *transaction).await?;
            sqlx::query(
                "UPDATE account_groups SET priority=$1 WHERE account_id=$2 AND group_id=$3",
            )
            .bind(account.priority)
            .bind(account.id)
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
            enqueue_scheduler_event(
                &mut transaction,
                "account_changed",
                Some(account.id),
                None,
                Some(&serde_json::json!({"group_ids":[group_id]})),
            )
            .await?;
        }
        enqueue_scheduler_event(
            &mut transaction,
            "group_changed",
            None,
            Some(group_id),
            None,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn update_managed_account_priorities(
        &self,
        group_id: i64,
        updates: &[(i64, i32)],
    ) -> AppResult<()> {
        if group_id <= 0
            || updates.is_empty()
            || updates.len() > 100
            || updates
                .iter()
                .any(|(account_id, priority)| *account_id <= 0 || !(1..=999).contains(priority))
        {
            return Err(AppError::BadRequest("账户优先级参数无效".into()));
        }
        let mut transaction = self.pool.begin().await?;
        for &(account_id, priority) in updates {
            let result = sqlx::query(
                "UPDATE accounts SET priority=$1,updated_at=NOW() WHERE id=$2 AND deleted_at IS NULL AND EXISTS(SELECT 1 FROM account_groups WHERE account_id=$2 AND group_id=$3)",
            )
            .bind(priority)
            .bind(account_id)
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() == 0 {
                return Err(AppError::NotFound("账户不存在或不属于该供应商".into()));
            }
            sqlx::query(
                "UPDATE account_groups SET priority=$1 WHERE account_id=$2 AND group_id=$3",
            )
            .bind(priority)
            .bind(account_id)
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
            enqueue_scheduler_event(
                &mut transaction,
                "account_changed",
                Some(account_id),
                None,
                Some(&serde_json::json!({"group_ids":[group_id]})),
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn update_managed_account_config(
        &self,
        group_id: i64,
        account_id: i64,
        name: &str,
        base_url: Option<&str>,
        api_key: Option<&str>,
        encryption_key: &str,
    ) -> AppResult<()> {
        let name = name.trim();
        if group_id <= 0 || account_id <= 0 || name.is_empty() || name.chars().count() > 200 {
            return Err(AppError::BadRequest("账户配置参数无效".into()));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
        let duplicate: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts a JOIN account_groups ag ON ag.account_id=a.id WHERE ag.group_id=$1 AND a.deleted_at IS NULL AND lower(trim(a.name))=lower($2) AND a.id<>$3)").bind(group_id).bind(name).bind(account_id).fetch_one(&mut *transaction).await?;
        if duplicate {
            return Err(AppError::BadRequest(
                "此供应商下已有同名账户，请换一个名称".into(),
            ));
        }

        let row = sqlx::query_as::<_, (String, Value)>(
            "SELECT a.type,a.credentials FROM accounts a JOIN account_groups ag ON ag.account_id=a.id WHERE a.id=$1 AND ag.group_id=$2 AND a.deleted_at IS NULL FOR UPDATE",
        )
        .bind(account_id)
        .bind(group_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| AppError::NotFound("账户不存在或不属于该供应商".into()))?;
        let credentials = if row.0 == "apikey" {
            let base_url = base_url
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| AppError::BadRequest("API Key 账户必须填写 Base URL".into()))?;
            let url = reqwest::Url::parse(base_url)
                .map_err(|_| AppError::BadRequest("Base URL 无效".into()))?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(AppError::BadRequest("Base URL 必须是 HTTP(S) 地址".into()));
            }
            let mut credentials = secrets::reveal_credentials(encryption_key, &row.1)?;
            let object = credentials
                .as_object_mut()
                .ok_or_else(|| AppError::Internal("账户凭证格式无效".into()))?;
            object.insert(
                "base_url".into(),
                Value::String(base_url.trim_end_matches('/').to_owned()),
            );
            if let Some(api_key) = api_key.map(str::trim).filter(|value| !value.is_empty()) {
                if api_key.len() > 8192 {
                    return Err(AppError::BadRequest("API Key 长度无效".into()));
                }
                object.insert("api_key".into(), Value::String(api_key.to_owned()));
            }
            secrets::protect_credentials(encryption_key, &credentials)?
        } else {
            row.1
        };
        sqlx::query(
            "UPDATE accounts SET name=$1,credentials=$2,updated_at=NOW() WHERE id=$3 AND deleted_at IS NULL",
        )
        .bind(name)
        .bind(credentials)
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        enqueue_scheduler_event(
            &mut transaction,
            "account_changed",
            Some(account_id),
            None,
            Some(&serde_json::json!({"group_ids":[group_id]})),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn replace_managed_oauth_credentials(
        &self,
        group_id: i64,
        account_id: i64,
        platform: &str,
        name: &str,
        credentials: Value,
        extra: Value,
    ) -> AppResult<()> {
        let platform = platform.trim();
        let name = name.trim();
        if group_id <= 0
            || account_id <= 0
            || platform.is_empty()
            || name.is_empty()
            || name.chars().count() > 200
        {
            return Err(AppError::BadRequest("OAuth 账户配置参数无效".into()));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
        let duplicate: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts a JOIN account_groups ag ON ag.account_id=a.id WHERE ag.group_id=$1 AND a.deleted_at IS NULL AND lower(trim(a.name))=lower($2) AND a.id<>$3)").bind(group_id).bind(name).bind(account_id).fetch_one(&mut *transaction).await?;
        if duplicate {
            return Err(AppError::BadRequest(
                "此供应商下已有同名账户，请换一个名称".into(),
            ));
        }

        let result = sqlx::query(
            "UPDATE accounts SET name=$1,credentials=$2,extra=COALESCE(extra,'{}'::jsonb) || $3 || jsonb_strip_nulls(jsonb_build_object('junohub_order',extra->'junohub_order','junohub_weight',extra->'junohub_weight','junohub_rpm',extra->'junohub_rpm','junohub_models',extra->'junohub_models')),status='active',schedulable=true,error_message=NULL,rate_limited_at=NULL,rate_limit_reset_at=NULL,overload_until=NULL,temp_unschedulable_until=NULL,temp_unschedulable_reason=NULL,updated_at=NOW() WHERE id=$4 AND platform=$5 AND type='oauth' AND deleted_at IS NULL AND EXISTS(SELECT 1 FROM account_groups WHERE account_id=$4 AND group_id=$6)",
        )
        .bind(name)
        .bind(credentials)
        .bind(extra)
        .bind(account_id)
        .bind(platform)
        .bind(group_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(
                "OAuth 账户不存在或不属于该供应商".into(),
            ));
        }
        enqueue_scheduler_event(
            &mut transaction,
            "account_changed",
            Some(account_id),
            None,
            Some(&serde_json::json!({"group_ids":[group_id]})),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn delete_managed_account(&self, group_id: i64, account_id: i64) -> AppResult<()> {
        let mut transaction = self.pool.begin().await?;
        // Lock the account before unlinking it. Shared accounts remain available to other groups.
        let account: Option<i64> = sqlx::query_scalar("SELECT a.id FROM accounts a WHERE a.id=$1 AND a.deleted_at IS NULL AND EXISTS(SELECT 1 FROM account_groups WHERE account_id=a.id AND group_id=$2) FOR UPDATE")
            .bind(account_id).bind(group_id).fetch_optional(&mut *transaction).await?;
        if account.is_none() {
            return Err(AppError::NotFound("账户不存在或不属于此供应商".into()));
        }
        sqlx::query("DELETE FROM account_groups WHERE account_id=$1 AND group_id=$2")
            .bind(account_id)
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE accounts SET schedulable=false,credentials='{}'::jsonb,deleted_at=NOW(),updated_at=NOW() WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM account_groups WHERE account_id=$1)")
            .bind(account_id).execute(&mut *transaction).await?;
        enqueue_scheduler_event(
            &mut transaction,
            "account_changed",
            Some(account_id),
            None,
            Some(&serde_json::json!({"group_ids":[group_id]})),
        )
        .await?;
        enqueue_scheduler_event(
            &mut transaction,
            "group_changed",
            None,
            Some(group_id),
            None,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn delete_managed_pool(&self, group_id: i64, api_key_id: i64) -> AppResult<()> {
        if group_id <= 0 || api_key_id <= 0 {
            return Err(AppError::BadRequest("账户池资源标识无效".into()));
        }
        let mut transaction = self.pool.begin().await?;
        let account_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT account_id FROM account_groups WHERE group_id=$1 ORDER BY account_id",
        )
        .bind(group_id)
        .fetch_all(&mut *transaction)
        .await?;
        sqlx::query("UPDATE api_keys SET key=$1,deleted_at=NOW(),updated_at=NOW() WHERE id=$2 AND group_id=$3 AND deleted_at IS NULL")
            .bind(format!("__deleted__{}__{}", api_key_id, Utc::now().timestamp_nanos_opt().unwrap_or_default()))
            .bind(api_key_id)
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM account_groups WHERE group_id=$1")
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
        for account_id in account_ids {
            sqlx::query("UPDATE accounts SET schedulable=false,deleted_at=NOW(),updated_at=NOW() WHERE id=$1 AND deleted_at IS NULL")
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
            enqueue_scheduler_event(
                &mut transaction,
                "account_changed",
                Some(account_id),
                None,
                Some(&serde_json::json!({"group_ids":[group_id]})),
            )
            .await?;
        }
        sqlx::query("UPDATE groups SET deleted_at=NOW(),updated_at=NOW() WHERE id=$1 AND deleted_at IS NULL")
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
        enqueue_scheduler_event(
            &mut transaction,
            "group_changed",
            None,
            Some(group_id),
            None,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn delete_bundle(&self, bundle: &CreatedBundle) -> AppResult<()> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE api_keys SET key=$1,deleted_at=NOW(),updated_at=NOW() WHERE id=$2 AND deleted_at IS NULL")
            .bind(format!("__deleted__{}__{}", bundle.api_key_id, Utc::now().timestamp_nanos_opt().unwrap_or_default()))
            .bind(bundle.api_key_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM account_groups WHERE account_id=$1")
            .bind(bundle.account_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE accounts SET schedulable=false,deleted_at=NOW(),updated_at=NOW() WHERE id=$1 AND deleted_at IS NULL")
            .bind(bundle.account_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE groups SET deleted_at=NOW(),updated_at=NOW() WHERE id=$1 AND deleted_at IS NULL")
            .bind(bundle.group_id)
            .execute(&mut *transaction)
            .await?;
        enqueue_scheduler_event(
            &mut transaction,
            "account_changed",
            Some(bundle.account_id),
            None,
            None,
        )
        .await?;
        enqueue_scheduler_event(
            &mut transaction,
            "group_changed",
            None,
            Some(bundle.group_id),
            None,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn verify_write_rollback(&self) -> AppResult<WriteProbe> {
        let before = self.snapshot().await?;
        let mut transaction = self.pool.begin().await?;
        let suffix = Uuid::new_v4().simple().to_string();
        let group_id: i64 = sqlx::query_scalar(
            "INSERT INTO groups(name,description,platform,rate_multiplier,is_exclusive,status,subscription_type,long_context_pricing_enabled) VALUES($1,'Rust compatibility write probe','openai',1,true,'active','standard',true) RETURNING id",
        )
        .bind(format!("rust-write-probe-{suffix}"))
        .fetch_one(&mut *transaction)
        .await?;
        let group_event_key = enqueue_scheduler_event(
            &mut transaction,
            "group_changed",
            None,
            Some(group_id),
            None,
        )
        .await?;
        let account_id: i64 = sqlx::query_scalar(
            "INSERT INTO accounts(name,platform,type,credentials,extra,concurrency,priority,status,schedulable,rate_multiplier) VALUES($1,'openai','apikey',$2,'{}'::jsonb,1,50,'active',true,1) RETURNING id",
        )
        .bind(format!("rust-write-probe-{suffix}"))
        .bind(serde_json::json!({"api_key":"probe-not-a-real-secret","base_url":"https://example.invalid/v1"}))
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO account_groups(account_id,group_id,priority) VALUES($1,$2,50)")
            .bind(account_id)
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
        let account_payload = serde_json::json!({"group_ids":[group_id]});
        let account_event_key = enqueue_scheduler_event(
            &mut transaction,
            "account_changed",
            Some(account_id),
            None,
            Some(&account_payload),
        )
        .await?;
        let user_id: i64 =
            sqlx::query_scalar("SELECT id FROM users WHERE deleted_at IS NULL ORDER BY id LIMIT 1")
                .fetch_one(&mut *transaction)
                .await?;
        let api_key_id: i64 = sqlx::query_scalar(
            "INSERT INTO api_keys(user_id,key,name,group_id,status) VALUES($1,$2,$3,$4,'active') RETURNING id",
        )
        .bind(user_id)
        .bind(format!("sk-rust-probe-{suffix}"))
        .bind("Rust compatibility write probe")
        .bind(group_id)
        .fetch_one(&mut *transaction)
        .await?;
        let group_linked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM account_groups WHERE account_id=$1 AND group_id=$2)",
        )
        .bind(account_id)
        .bind(group_id)
        .fetch_one(&mut *transaction)
        .await?;
        let scheduler_events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM scheduler_outbox WHERE dedup_key IN ($1,$2)")
                .bind(&group_event_key)
                .bind(&account_event_key)
                .fetch_one(&mut *transaction)
                .await?;
        transaction.rollback().await?;
        let residual_events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM scheduler_outbox WHERE dedup_key IN ($1,$2)")
                .bind(&group_event_key)
                .bind(&account_event_key)
                .fetch_one(&self.pool)
                .await?;
        let after = self.snapshot().await?;
        Ok(WriteProbe {
            group_id,
            account_id,
            api_key_id,
            group_linked,
            scheduler_events,
            outbox_rolled_back: residual_events == 0,
            rolled_back: before == after,
            before,
            after,
        })
    }

    pub async fn account_group_ids(&self, account_id: i64) -> AppResult<Vec<i64>> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT group_id FROM account_groups WHERE account_id=$1 ORDER BY priority,group_id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn count(&self, table: &str) -> AppResult<i64> {
        let query = match table {
            "accounts" => "SELECT COUNT(*) FROM accounts WHERE deleted_at IS NULL",
            "groups" => "SELECT COUNT(*) FROM groups WHERE deleted_at IS NULL",
            "api_keys" => "SELECT COUNT(*) FROM api_keys WHERE deleted_at IS NULL",
            _ => unreachable!("fixed table allowlist"),
        };
        Ok(sqlx::query_scalar(query).fetch_one(&self.pool).await?)
    }

    async fn count_links(&self) -> AppResult<i64> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM account_groups")
            .fetch_one(&self.pool)
            .await?)
    }
}

fn default_model_ids(platform: &str) -> &'static [&'static str] {
    match platform {
        "openai" => &[
            "gpt-5.6-sol",
            "gpt-5.6",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.3-codex-spark",
            "codex-auto-review",
            "gpt-5.2",
            "gpt-image-1",
            "gpt-image-1.5",
            "gpt-image-2",
        ],
        "gemini" => &[
            "gemini-2.0-flash",
            "gemini-2.5-flash",
            "gemini-2.5-flash-image",
            "gemini-2.5-pro",
            "gemini-3.5-flash",
            "gemini-3-flash-preview",
            "gemini-3-pro-preview",
            "gemini-3.1-pro-preview",
            "gemini-3.1-flash-image",
        ],
        "anthropic" => &[
            "claude-fable-5",
            "claude-opus-4-5-20251101",
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-opus-4-8",
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-sonnet-4-5-20250929",
            "claude-haiku-4-5-20251001",
        ],
        "grok" => &["grok-code-fast-1", "grok-4", "grok-4-fast", "grok-4-1-fast"],
        "antigravity" => &[
            "gemini-2.5-flash",
            "gemini-2.5-pro",
            "gemini-3-flash-preview",
            "gemini-3-pro-preview",
            "gemini-3.1-pro-preview",
            "claude-sonnet-4-6",
            "claude-opus-4-6",
        ],
        _ => &[],
    }
}

const SCHEDULER_CREDENTIAL_FIELDS: &[&str] = &[
    "model_mapping",
    "compact_model_mapping",
    "api_key",
    "project_id",
    "oauth_type",
    "plan_type",
];

const SCHEDULER_EXTRA_FIELDS: &[&str] = &[
    "quota_limit",
    "quota_used",
    "quota_daily_limit",
    "quota_daily_used",
    "quota_daily_start",
    "quota_daily_reset_mode",
    "quota_daily_reset_hour",
    "quota_weekly_limit",
    "quota_weekly_used",
    "quota_weekly_start",
    "quota_weekly_reset_mode",
    "quota_weekly_reset_day",
    "quota_weekly_reset_hour",
    "quota_reset_timezone",
    "mixed_scheduling",
    "window_cost_limit",
    "window_cost_sticky_reserve",
    "max_sessions",
    "session_idle_timeout_minutes",
    "openai_oauth_responses_websockets_v2_enabled",
    "openai_oauth_responses_websockets_v2_mode",
    "openai_apikey_responses_websockets_v2_enabled",
    "openai_apikey_responses_websockets_v2_mode",
    "responses_websockets_v2_enabled",
    "openai_ws_enabled",
    "openai_ws_force_http",
    "openai_responses_mode",
    "openai_responses_supported",
    "codex_fingerprint_mode",
    "codex_fingerprint_seed",
    "codex_5h_used_percent",
    "codex_7d_used_percent",
    "codex_5h_reset_at",
    "codex_7d_reset_at",
    "codex_5h_reset_after_seconds",
    "codex_7d_reset_after_seconds",
    "codex_usage_updated_at",
    "auto_pause_5h_threshold",
    "auto_pause_7d_threshold",
    "auto_pause_5h_disabled",
    "auto_pause_7d_disabled",
    "model_rate_limits",
    "upstream_billing_probe",
    "grok_media_eligible",
    "grok_billing_snapshot",
];

fn filter_json_fields(value: &Value, allowed: &[&str]) -> Value {
    let Some(source) = value.as_object() else {
        return serde_json::json!({});
    };
    Value::Object(
        allowed
            .iter()
            .filter_map(|key| {
                source
                    .get(*key)
                    .filter(|value| !value.is_null())
                    .map(|value| ((*key).to_owned(), value.clone()))
            })
            .collect(),
    )
}

fn scheduler_account_json(
    account: &EngineAccount,
    credentials: Value,
    extra: Value,
    account_groups: &[Value],
    group_ids: &[i64],
) -> Value {
    serde_json::json!({
        "ID":account.id,
        "Name":account.name,
        "Notes":account.notes,
        "Platform":account.platform,
        "Type":account.kind,
        "Credentials":credentials,
        "Extra":extra,
        "ProxyID":account.proxy_id,
        "ProxyFallbackOriginID":account.proxy_fallback_origin_id,
        "Concurrency":account.concurrency,
        "Priority":account.priority,
        "RateMultiplier":account.rate_multiplier,
        "LoadFactor":account.load_factor,
        "Status":account.status,
        "ErrorMessage":account.error_message,
        "LastUsedAt":account.last_used_at,
        "ExpiresAt":account.expires_at,
        "AutoPauseOnExpired":account.auto_pause_on_expired,
        "CreatedAt":account.created_at,
        "UpdatedAt":account.updated_at,
        "Schedulable":account.schedulable,
        "RateLimitedAt":account.rate_limited_at,
        "RateLimitResetAt":account.rate_limit_reset_at,
        "OverloadUntil":account.overload_until,
        "TempUnschedulableUntil":account.temp_unschedulable_until,
        "TempUnschedulableReason":account.temp_unschedulable_reason.clone().unwrap_or_default(),
        "SessionWindowStart":account.session_window_start,
        "SessionWindowEnd":account.session_window_end,
        "SessionWindowStatus":account.session_window_status.clone().unwrap_or_default(),
        "ParentAccountID":account.parent_account_id,
        "QuotaDimension":account.quota_dimension.clone().unwrap_or_default(),
        "AccountGroups":account_groups,
        "GroupIDs":group_ids,
        "Proxy":null,
        "Groups":null
    })
}

async fn enqueue_scheduler_event(
    transaction: &mut Transaction<'_, Postgres>,
    event_type: &str,
    account_id: Option<i64>,
    group_id: Option<i64>,
    payload: Option<&Value>,
) -> AppResult<String> {
    let payload_bytes = payload.map(serde_json::to_vec).transpose()?;
    let mut hash = Sha256::new();
    hash.update(event_type.as_bytes());
    hash.update([0]);
    if let Some(id) = account_id {
        hash.update(id.to_string().as_bytes());
    }
    hash.update([0]);
    if let Some(id) = group_id {
        hash.update(id.to_string().as_bytes());
    }
    hash.update([0]);
    if let Some(bytes) = &payload_bytes {
        hash.update(bytes);
    }
    let digest = hash.finalize();
    let dedup_key = format!(
        "scheduler_outbox:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    sqlx::query("INSERT INTO scheduler_outbox(event_type,account_id,group_id,payload,dedup_key) VALUES($1,$2,$3,$4,$5) ON CONFLICT (dedup_key) WHERE dedup_key IS NOT NULL DO NOTHING")
        .bind(event_type)
        .bind(account_id)
        .bind(group_id)
        .bind(payload)
        .bind(&dedup_key)
        .execute(&mut **transaction)
        .await?;
    Ok(dedup_key)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore = "requires an isolated PostgreSQL instance via JUNOHUB_TEST_POSTGRES_URL"]
    async fn delete_account_is_scoped_and_preserves_shared_accounts() {
        let url = std::env::var("JUNOHUB_TEST_POSTGRES_URL").expect("isolated test PostgreSQL URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TEMP TABLE accounts(id BIGINT PRIMARY KEY,name TEXT DEFAULT 'fixture',platform TEXT DEFAULT 'openai',type TEXT DEFAULT 'apikey',priority INTEGER DEFAULT 1,concurrency INTEGER DEFAULT 1,load_factor INTEGER,extra JSONB DEFAULT '{}'::jsonb,status TEXT DEFAULT 'active',expires_at TIMESTAMPTZ,auto_pause_on_expired BOOLEAN DEFAULT true,rate_limit_reset_at TIMESTAMPTZ,overload_until TIMESTAMPTZ,temp_unschedulable_until TIMESTAMPTZ,schedulable BOOLEAN DEFAULT true,credentials JSONB DEFAULT '{}'::jsonb,deleted_at TIMESTAMPTZ,updated_at TIMESTAMPTZ); CREATE TEMP TABLE account_groups(account_id BIGINT,group_id BIGINT,priority INTEGER DEFAULT 1); CREATE TEMP TABLE scheduler_outbox(event_type TEXT,account_id BIGINT,group_id BIGINT,payload JSONB,dedup_key TEXT); CREATE UNIQUE INDEX test_dedup ON scheduler_outbox(dedup_key) WHERE dedup_key IS NOT NULL; INSERT INTO accounts(id,credentials) VALUES(11,'{\"token\":\"fixture\"}'),(12,'{}'),(13,'{\"token\":\"shared\"}'); INSERT INTO account_groups(account_id,group_id) VALUES(11,1),(12,1),(13,1),(13,2);")
            .execute(&pool).await.unwrap();
        let repository = super::Sub2Repository::new(pool.clone());
        use crate::sub2_policy::AccountDispatch;
        let updates = vec![
            AccountDispatch {
                member_order: 2,
                id: 11,
                priority: 1,
                weight: 7,
                concurrency: 4,
                rpm: 0,
                allowed_models: vec!["model-a".into()],
            },
            AccountDispatch {
                member_order: 0,
                id: 12,
                priority: 1,
                weight: 3,
                concurrency: 2,
                rpm: 0,
                allowed_models: vec![],
            },
        ];
        repository
            .update_account_dispatch(1, &updates)
            .await
            .unwrap();
        let saved = repository.dispatch_accounts(1).await.unwrap();
        assert_eq!(saved[0].member_order, 2);
        sqlx::query("ALTER TABLE accounts ADD COLUMN error_message TEXT, ADD COLUMN rate_limited_at TIMESTAMPTZ, ADD COLUMN temp_unschedulable_reason TEXT").execute(&pool).await.unwrap();
        sqlx::query("UPDATE accounts SET type='oauth' WHERE id=11")
            .execute(&pool)
            .await
            .unwrap();
        repository
            .replace_managed_oauth_credentials(
                1,
                11,
                "openai",
                "renewed",
                serde_json::json!({"token":"fixture"}),
                serde_json::json!({"junohub_order":999}),
            )
            .await
            .unwrap();
        assert_eq!(
            repository.dispatch_accounts(1).await.unwrap()[0].member_order,
            2
        );
        sqlx::query("UPDATE accounts SET type='apikey' WHERE id=11")
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(saved[0].weight, 7);
        assert_eq!(saved[1].priority, 1);
        assert_eq!(saved[0].concurrency, 4);
        assert_eq!(saved[0].allowed_models, vec!["model-a"]);
        assert!(repository
            .update_managed_account_config(
                1,
                11,
                " FIXTURE ",
                Some("https://example.com/v1"),
                None,
                "fixture-key"
            )
            .await
            .is_err());
        let mut detached = updates[0].clone();
        detached.priority = 0;
        repository
            .update_account_dispatch(1, &[detached])
            .await
            .unwrap();
        assert!(!repository
            .schedulable_account_ids(1, "openai", false)
            .await
            .unwrap()
            .contains(&11));
        assert!(repository
            .managed_accounts(1, "fixture-key")
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == 11 && a.priority == 0));
        repository
            .update_account_dispatch(1, &updates)
            .await
            .unwrap();
        let invalid = vec![
            AccountDispatch {
                member_order: 0,
                id: 11,
                priority: 2,
                weight: 1,
                concurrency: 1,
                rpm: 0,
                allowed_models: vec![],
            },
            AccountDispatch {
                member_order: 0,
                id: 999,
                priority: 2,
                weight: 1,
                concurrency: 1,
                rpm: 0,
                allowed_models: vec![],
            },
        ];
        assert!(repository
            .update_account_dispatch(1, &invalid)
            .await
            .is_err());
        assert_eq!(repository.dispatch_accounts(1).await.unwrap()[0].weight, 7);
        sqlx::query(
            "UPDATE accounts SET rate_limit_reset_at=NOW()+INTERVAL '60 seconds' WHERE id=12",
        )
        .execute(&pool)
        .await
        .unwrap();
        let summaries = repository
            .managed_accounts(1, "fixture-encryption-key")
            .await
            .unwrap();
        assert_eq!(
            summaries.iter().find(|a| a.id == 12).unwrap().health,
            "rate_limited"
        );
        assert_eq!(
            summaries.iter().find(|a| a.id == 11).unwrap().concurrency,
            4
        );
        assert!(repository.delete_managed_account(2, 11).await.is_err());
        repository.delete_managed_account(1, 11).await.unwrap();
        let removed: (bool,bool,bool) = sqlx::query_as("SELECT deleted_at IS NOT NULL,NOT schedulable,credentials='{}'::jsonb FROM accounts WHERE id=11").fetch_one(&pool).await.unwrap();
        assert_eq!(removed, (true, true, true));
        assert!(repository.delete_managed_account(1, 11).await.is_err());
        repository.delete_managed_account(1, 13).await.unwrap();
        let shared: (bool, bool) =
            sqlx::query_as("SELECT deleted_at IS NULL,schedulable FROM accounts WHERE id=13")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(shared, (true, true));
        let remaining: Vec<i64> =
            sqlx::query_scalar("SELECT account_id FROM account_groups WHERE group_id=1")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, vec![12]);
        repository.delete_managed_account(1, 12).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM account_groups WHERE group_id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM scheduler_outbox WHERE event_type='account_changed'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 3);
    }

    use super::*;

    #[test]
    fn openai_default_models_match_v0_1_182() {
        assert_eq!(default_model_ids("openai").len(), 13);
        assert_eq!(default_model_ids("openai")[0], "gpt-5.6-sol");
        assert!(default_model_ids("openai").contains(&"codex-auto-review"));
        assert!(default_model_ids("openai").contains(&"gpt-image-2"));
    }

    #[test]
    fn metadata_credentials_exclude_tokens() {
        let filtered = filter_json_fields(
            &serde_json::json!({"access_token":"secret","refresh_token":"secret","plan_type":"plus"}),
            SCHEDULER_CREDENTIAL_FIELDS,
        );
        assert_eq!(filtered, serde_json::json!({"plan_type":"plus"}));
    }
}
