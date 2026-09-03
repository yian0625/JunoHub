use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

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
    ) -> AppResult<bool> {
        let mut transaction = self.pool.begin().await?;
        let updated: Option<i64> = sqlx::query_scalar(
            "UPDATE accounts SET credentials=COALESCE(credentials,'{}'::jsonb)||$1,updated_at=NOW() WHERE id=$2 AND deleted_at IS NULL AND credentials->>'refresh_token'=$3 RETURNING id",
        )
        .bind(token_patch)
        .bind(account_id)
        .bind(old_refresh_token)
        .fetch_optional(&mut *transaction)
        .await?;
        if updated.is_some() {
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
        Ok(updated.is_some())
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
        let payload =
            serde_json::json!({"last_used":{account_id.to_string():timestamp}});
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
            "SELECT a.credentials FROM account_groups ag JOIN accounts a ON a.id=ag.account_id WHERE ag.group_id=$1 AND a.deleted_at IS NULL AND a.status='active' AND a.schedulable=true ORDER BY ag.priority,a.priority,a.id",
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
                "SELECT a.id FROM account_groups ag JOIN accounts a ON a.id=ag.account_id WHERE ag.group_id=$1 AND a.deleted_at IS NULL AND a.status='active' AND a.schedulable=true AND a.platform=ANY($2) AND (a.temp_unschedulable_until IS NULL OR a.temp_unschedulable_until<=NOW()) AND (a.expires_at IS NULL OR a.expires_at>NOW() OR a.auto_pause_on_expired=false) AND (a.overload_until IS NULL OR a.overload_until<=NOW()) AND (a.rate_limit_reset_at IS NULL OR a.rate_limit_reset_at<=NOW()) AND (a.platform<>'antigravity' OR COALESCE((a.extra->>'mixed_scheduling')::boolean,false)=true OR $3=false) ORDER BY ag.priority,a.priority,a.id",
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
        sqlx::query("INSERT INTO account_groups(account_id,group_id,priority) VALUES($1,$2,50)")
            .bind(account_id)
            .bind(group_id)
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
