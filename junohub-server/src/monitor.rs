//! Passive monitoring: dependency probes and existing traffic, never model generation.
use crate::{
    error::{AppError, AppResult},
    state::AppState,
    store,
};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AlertConfig {
    pub enabled: bool,
    pub window_minutes: u32,
    pub min_requests: u32,
    pub error_rate_percent: f64,
    pub rate_limit_percent: f64,
    pub p95_latency_ms: u64,
    pub unpriced_percent: f64,
}
impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_minutes: 5,
            min_requests: 20,
            error_rate_percent: 20.0,
            rate_limit_percent: 30.0,
            p95_latency_ms: 15000,
            unpriced_percent: 50.0,
        }
    }
}
impl AlertConfig {
    fn validate(&self) -> AppResult<()> {
        if !(1..=60).contains(&self.window_minutes)
            || !(1..=10000).contains(&self.min_requests)
            || !(100..=600000).contains(&self.p95_latency_ms)
            || [
                self.error_rate_percent,
                self.rate_limit_percent,
                self.unpriced_percent,
            ]
            .iter()
            .any(|v| !v.is_finite() || !(1.0..=100.0).contains(v))
        {
            return Err(AppError::BadRequest(
                "告警窗口为 1–60 分钟，最小样本 1–10000，P95 100–600000ms，比例 1–100%".into(),
            ));
        }
        Ok(())
    }
}
pub async fn settings(state: &AppState) -> AppResult<Value> {
    let row = sqlx::query("SELECT * FROM monitor_settings WHERE id=1")
        .fetch_one(&state.pool)
        .await?;
    let config: AlertConfig = serde_json::from_str(row.get("config_json"))
        .map_err(|_| AppError::Internal("告警配置损坏".into()))?;
    config.validate()?;
    Ok(
        json!({"config":config,"revision":row.get::<i64,_>("revision"),"updated_at":row.get::<i64,_>("updated_at")}),
    )
}
pub async fn update_settings(state: &AppState, payload: &Value) -> AppResult<Value> {
    let config: AlertConfig = serde_json::from_value(payload["config"].clone())
        .map_err(|_| AppError::BadRequest("告警配置格式无效".into()))?;
    config.validate()?;
    let revision = payload["revision"]
        .as_i64()
        .filter(|v| *v >= 0)
        .ok_or_else(|| AppError::BadRequest("缺少告警配置版本，请重新读取".into()))?;
    let saved=sqlx::query("UPDATE monitor_settings SET config_json=?,revision=revision+1,updated_at=? WHERE id=1 AND revision=?")
 .bind(serde_json::to_string(&config)?).bind(store::now()).bind(revision).execute(&state.pool).await?;
    if saved.rows_affected() == 0 {
        return Err(AppError::BadRequest(
            "告警配置已被其他管理员修改，请重新加载后保存".into(),
        ));
    }
    settings(state).await
}
#[derive(Clone, Serialize)]
pub struct Component {
    pub name: &'static str,
    pub healthy: bool,
    pub latency_ms: u128,
}
pub async fn dependencies(state: &AppState) -> Vec<Component> {
    let sqlite = async {
        let start = Instant::now();
        let healthy = tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&state.pool),
        )
        .await
        .is_ok_and(|r| r.is_ok_and(|v| v == 1));
        Component {
            name: "sqlite",
            healthy,
            latency_ms: start.elapsed().as_millis(),
        }
    };
    let postgres = async {
        let start = Instant::now();
        let healthy = tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.sub2_pool),
        )
        .await
        .is_ok_and(|r| r.is_ok_and(|v| v == 1));
        Component {
            name: "postgres",
            healthy,
            latency_ms: start.elapsed().as_millis(),
        }
    };
    let redis = async {
        let start = Instant::now();
        let healthy = if let Some(mut connection) = state.sub2_redis.clone() {
            tokio::time::timeout(
                Duration::from_secs(2),
                redis::cmd("PING").query_async::<String>(&mut connection),
            )
            .await
            .is_ok_and(|r| r.is_ok_and(|v| v == "PONG"))
        } else {
            false
        };
        Component {
            name: "redis",
            healthy,
            latency_ms: start.elapsed().as_millis(),
        }
    };
    let (a, b, c) = tokio::join!(sqlite, postgres, redis);
    vec![a, b, c]
}
pub async fn readiness(State(state): State<AppState>) -> Response {
    let components = dependencies(&state).await;
    let healthy = components.iter().all(|c| c.healthy);
    // Public probe deliberately returns no credentials, connection strings or topology.
    (
        if healthy {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({"status":if healthy{"ready"}else{"unavailable"},"service":"junohub-server"})),
    )
        .into_response()
}
pub async fn heartbeat(state: &AppState, name: &str, enabled: bool, success: Option<bool>) {
    let now = store::now();
    let _=sqlx::query("INSERT INTO worker_heartbeats(instance_id,name,enabled,last_tick,last_success,message) VALUES(?,?,?,?,?,?) ON CONFLICT(instance_id,name) DO UPDATE SET enabled=excluded.enabled,last_tick=excluded.last_tick,last_success=CASE WHEN ? THEN excluded.last_success ELSE worker_heartbeats.last_success END,message=CASE WHEN ? THEN excluded.message ELSE worker_heartbeats.message END")
 .bind(crate::observability::instance()).bind(name).bind(enabled).bind(now).bind(if success==Some(true){now}else{0})
 .bind(if success==Some(false){"最近一次执行失败"}else{ "" }).bind(success==Some(true)).bind(success.is_some()).execute(&state.pool).await;
}
async fn workers(state: &AppState) -> AppResult<Value> {
    let rows=sqlx::query("SELECT name,enabled,last_tick,last_success,message FROM worker_heartbeats WHERE instance_id=? ORDER BY name").bind(crate::observability::instance()).fetch_all(&state.pool).await?;
    let now = store::now();
    let mut output = vec![];
    for name in ["monitor", "task_poller", "account_scheduler"] {
        if let Some(row) = rows.iter().find(|r| r.get::<String, _>("name") == name) {
            let enabled = row.get::<bool, _>("enabled");
            let success = row.get::<i64, _>("last_success");
            let tick = row.get::<i64, _>("last_tick");
            let status = if !enabled {
                "disabled"
            } else if now - tick > 120
                || (!row.get::<String, _>("message").is_empty())
                || (success > 0 && now - success > 120)
            {
                "unavailable"
            } else if success == 0 {
                "starting"
            } else {
                "healthy"
            };
            output
                .push(json!({"name":name,"status":status,"last_tick":tick,"last_success":success}));
        } else {
            output.push(json!({"name":name,"status":"starting","last_tick":0,"last_success":0}));
        }
    }
    Ok(json!(output))
}
const AGGREGATES:&str="COUNT(*) AS requests,COALESCE(SUM(CASE WHEN status_code BETWEEN 200 AND 299 AND status_code!=202 THEN 1 ELSE 0 END),0) AS success,COALESCE(SUM(CASE WHEN status_code=202 THEN 1 ELSE 0 END),0) AS pending,COALESCE(SUM(CASE WHEN status_code=499 THEN 1 ELSE 0 END),0) AS cancelled,COALESCE(SUM(CASE WHEN status_code=429 THEN 1 ELSE 0 END),0) AS rate_limited,COALESCE(SUM(CASE WHEN status_code BETWEEN 500 AND 599 THEN 1 ELSE 0 END),0) AS server_errors,COALESCE(SUM(CASE WHEN status_code BETWEEN 200 AND 299 AND status_code!=202 AND cost_usd IS NULL THEN 1 ELSE 0 END),0) AS unpriced,COALESCE(SUM(cost_usd),0.0) AS cost_usd,COALESCE(SUM(input_tokens),0) AS input_tokens,COALESCE(SUM(output_tokens),0) AS output_tokens";
fn aggregate(row: &sqlx::sqlite::SqliteRow) -> Value {
    let total = row.get::<i64, _>("requests");
    let success = row.get::<i64, _>("success");
    let pending = row.get::<i64, _>("pending");
    let cancelled = row.get::<i64, _>("cancelled");
    let completed = total - pending - cancelled;
    json!({"requests":total,"success":success,"failed":completed-success,"pending":pending,"cancelled":cancelled,"success_rate":if completed>0{Some(success as f64*100.0/completed as f64)}else{None},"rate_limited":row.get::<i64,_>("rate_limited"),"server_errors":row.get::<i64,_>("server_errors"),"unpriced":row.get::<i64,_>("unpriced"),"cost_usd":row.get::<f64,_>("cost_usd"),"input_tokens":row.get::<i64,_>("input_tokens"),"output_tokens":row.get::<i64,_>("output_tokens")})
}
async fn traffic(state: &AppState, from: i64, to: i64) -> AppResult<Value> {
    let row = sqlx::query(&format!(
        "SELECT {AGGREGATES} FROM user_request_logs WHERE created_at>=? AND created_at<=?"
    ))
    .bind(from)
    .bind(to)
    .fetch_one(&state.pool)
    .await?;
    let mut result = aggregate(&row);
    let latency=sqlx::query("WITH ranked AS (SELECT duration_ms,ROW_NUMBER() OVER (ORDER BY duration_ms) AS position,COUNT(*) OVER () AS count FROM user_request_logs WHERE created_at>=? AND created_at<=? AND status_code BETWEEN 200 AND 299 AND status_code!=202 AND duration_ms>=0) SELECT MAX(CASE WHEN position=(count*50+99)/100 THEN duration_ms END) AS p50,MAX(CASE WHEN position=(count*95+99)/100 THEN duration_ms END) AS p95,AVG(duration_ms) AS avg FROM ranked")
 .bind(from).bind(to).fetch_one(&state.pool).await?;
    result["p50_ms"] = json!(latency.get::<Option<i64>, _>("p50"));
    result["p95_ms"] = json!(latency.get::<Option<i64>, _>("p95"));
    result["average_ms"] = json!(latency.get::<Option<f64>, _>("avg"));
    Ok(result)
}
async fn providers(state: &AppState) -> AppResult<Value> {
    let rows=sqlx::query("SELECT p.id,p.name,p.enabled,p.provider_type,m.group_id,COUNT(k.id) AS total,COALESCE(SUM(CASE WHEN k.priority>0 AND k.cooldown_until<=? THEN 1 ELSE 0 END),0) AS candidates,COALESCE(SUM(CASE WHEN k.priority>0 AND k.cooldown_until>? THEN 1 ELSE 0 END),0) AS cooling FROM user_providers p LEFT JOIN sub2_managed_pools m ON m.provider_id=p.id LEFT JOIN provider_keys k ON k.provider_id=p.id GROUP BY p.id ORDER BY p.name,p.id").bind(store::now()).bind(store::now()).fetch_all(&state.pool).await?;
    let groups: Vec<i64> = rows
        .iter()
        .filter_map(|r| r.get::<Option<i64>, _>("group_id"))
        .collect();
    let mut managed = HashMap::new();
    let mut managed_available = true;
    if !groups.is_empty() {
        let operation=sqlx::query("SELECT g.group_id,COUNT(*) AS total,COUNT(*) FILTER (WHERE a.status='active' AND a.schedulable AND a.priority>0 AND (a.expires_at IS NULL OR a.expires_at>now() OR NOT a.auto_pause_on_expired) AND (a.rate_limit_reset_at IS NULL OR a.rate_limit_reset_at<=now()) AND (a.overload_until IS NULL OR a.overload_until<=now()) AND (a.temp_unschedulable_until IS NULL OR a.temp_unschedulable_until<=now())) AS candidates FROM account_groups g JOIN accounts a ON a.id=g.account_id AND a.deleted_at IS NULL WHERE g.group_id=ANY($1) GROUP BY g.group_id").bind(&groups).fetch_all(&state.sub2_pool);
        match tokio::time::timeout(Duration::from_secs(2), operation).await {
            Ok(Ok(records)) => {
                for row in records {
                    managed.insert(
                        row.get::<i64, _>("group_id"),
                        (row.get::<i64, _>("total"), row.get::<i64, _>("candidates")),
                    );
                }
            }
            _ => managed_available = false,
        }
    }
    let mut output = vec![];
    for row in rows {
        let engine = row.get::<String, _>("provider_type") == "sub2api";
        let (total, candidates) = if engine {
            managed
                .get(&row.get::<Option<i64>, _>("group_id").unwrap_or_default())
                .copied()
                .unwrap_or((0, 0))
        } else {
            (row.get::<i64, _>("total"), row.get::<i64, _>("candidates"))
        };
        let enabled = row.get::<bool, _>("enabled");
        let known = !engine || managed_available;
        let status = if !enabled {
            "disabled"
        } else if !known {
            "unknown"
        } else if total == 0 {
            "no_accounts"
        } else if candidates == 0 {
            "unavailable"
        } else {
            "eligible"
        };
        output.push(json!({"id":row.get::<i64,_>("id"),"name":row.get::<String,_>("name"),"type":if engine{"account_engine"}else{"api_key"},"status":status,"total_accounts":if known {Some(total)}else{None},"candidate_accounts":if known{Some(candidates)}else{None},"cooling_accounts":if engine {None}else{Some(row.get::<i64,_>("cooling"))}}));
    }
    Ok(json!(output))
}
async fn task_summary(state: &AppState) -> AppResult<Value> {
    let now = store::now();
    let row=sqlx::query("SELECT COUNT(*) AS retained,COALESCE(SUM(CASE WHEN result_json IS NULL AND status IN ('submitting','queued','processing') THEN 1 ELSE 0 END),0) AS pending,COALESCE(SUM(CASE WHEN result_json IS NULL AND status='submitting' AND upstream_task_id='' AND created_at<? THEN 1 ELSE 0 END),0) AS uncertain,COALESCE(SUM(CASE WHEN result_json IS NULL AND created_at<? AND status IN ('queued','processing') THEN 1 ELSE 0 END),0) AS delayed,COALESCE(SUM(CASE WHEN result_json IS NOT NULL AND log_id IS NOT NULL AND settled=0 THEN 1 ELSE 0 END),0) AS unsettled,COALESCE(SUM(CASE WHEN result_json IS NULL AND expires_at<=? THEN 1 ELSE 0 END),0) AS overdue FROM gateway_tasks")
 .bind(now-120).bind(now-900).bind(now).fetch_one(&state.pool).await?;
    Ok(
        json!({"retained":row.get::<i64,_>("retained"),"pending":row.get::<i64,_>("pending"),"uncertain":row.get::<i64,_>("uncertain"),"delayed":row.get::<i64,_>("delayed"),"unsettled":row.get::<i64,_>("unsettled"),"overdue":row.get::<i64,_>("overdue")}),
    )
}
#[derive(Clone)]
struct Alert {
    fingerprint: String,
    rule: &'static str,
    severity: &'static str,
    title: String,
    detail: String,
}
fn alert(
    rule: &'static str,
    scope: &str,
    severity: &'static str,
    title: impl Into<String>,
    detail: impl Into<String>,
) -> Alert {
    Alert {
        fingerprint: format!("{rule}:{scope}"),
        rule,
        severity,
        title: title.into(),
        detail: detail.into(),
    }
}
fn evaluate(snapshot: &Value, config: &AlertConfig) -> Vec<Alert> {
    let mut out = vec![];
    for component in snapshot["components"].as_array().into_iter().flatten() {
        if component["healthy"] == false {
            let name = component["name"].as_str().unwrap_or("dependency");
            out.push(alert(
                "dependency",
                name,
                "critical",
                format!("{name} 依赖不可用"),
                "连接检查失败或超时，请检查服务、网络和连接配置。",
            ));
        }
    }
    for worker in snapshot["workers"].as_array().into_iter().flatten() {
        if worker["status"] == "unavailable" {
            let name = worker["name"].as_str().unwrap_or("worker");
            out.push(alert(
                "worker",
                name,
                "critical",
                format!("{name} 后台任务异常"),
                "后台任务执行失败或心跳超过两分钟未更新。",
            ));
        }
    }
    for provider in snapshot["providers"].as_array().into_iter().flatten() {
        if matches!(
            provider["status"].as_str(),
            Some("no_accounts" | "unavailable" | "unknown")
        ) {
            out.push(alert(
                "provider_accounts",
                &provider["id"].to_string(),
                "warning",
                format!(
                    "{} {}",
                    provider["name"].as_str().unwrap_or("供应商"),
                    if provider["status"] == "unknown" {
                        "调度状态无法确认"
                    } else {
                        "没有可参与调度的账户"
                    }
                ),
                "请检查账户启用、分组、有效期和冷却状态。",
            ));
        }
    }
    let tasks = &snapshot["tasks"];
    for (key, title) in [
        ("uncertain", "存在未获提交确认的生成任务"),
        ("delayed", "存在等待超过 15 分钟的生成任务"),
        ("overdue", "存在超过有效期尚未处理的生成任务"),
    ] {
        if tasks[key].as_i64().unwrap_or(0) > 0 {
            out.push(alert(
                "task",
                key,
                "warning",
                title,
                format!("{} 个任务；查询前核对上游记录，避免重复生成。", tasks[key]),
            ));
        }
    }
    if tasks["unsettled"].as_i64().unwrap_or(0) >= 10 {
        out.push(alert(
            "settlement",
            "all",
            "critical",
            "任务结算积压",
            "至少 10 个已完成任务尚未完成账务结算。",
        ));
    }
    if !config.enabled {
        return out;
    }
    let t = &snapshot["traffic"];
    let count = t["requests"].as_i64().unwrap_or(0);
    let completed =
        count - t["pending"].as_i64().unwrap_or(0) - t["cancelled"].as_i64().unwrap_or(0);
    if completed >= config.min_requests as i64 {
        let error = t["failed"].as_f64().unwrap_or(0.0) * 100.0 / completed as f64;
        if error >= config.error_rate_percent {
            out.push(alert(
                "error_rate",
                "all",
                "warning",
                "调用失败率升高",
                format!(
                    "最近 {} 分钟，{} 个已完成请求的失败率为 {:.1}%。",
                    config.window_minutes, completed, error
                ),
            ));
        }
        let rate = t["rate_limited"].as_f64().unwrap_or(0.0) * 100.0 / completed as f64;
        if rate >= config.rate_limit_percent {
            out.push(alert(
                "rate_limit",
                "all",
                "warning",
                "请求限流比例升高",
                format!(
                    "最近 {} 分钟，429 比例为 {:.1}%。",
                    config.window_minutes, rate
                ),
            ));
        }
    }
    let success = t["success"].as_i64().unwrap_or(0);
    if success >= config.min_requests as i64 {
        if t["p95_ms"]
            .as_u64()
            .is_some_and(|v| v >= config.p95_latency_ms)
        {
            out.push(alert(
                "latency",
                "all",
                "warning",
                "成功请求延迟升高",
                format!(
                    "最近 {} 分钟的 P95 总耗时为 {}ms。",
                    config.window_minutes, t["p95_ms"]
                ),
            ));
        }
        let ratio = t["unpriced"].as_f64().unwrap_or(0.0) * 100.0 / success as f64;
        if ratio >= config.unpriced_percent {
            out.push(alert(
                "billing",
                "all",
                "warning",
                "成功请求未计价比例升高",
                format!(
                    "最近 {} 分钟，{:.1}% 的成功请求未计价，请核对价格配置与实际用量字段。",
                    config.window_minutes, ratio
                ),
            ));
        }
    }
    out
}
async fn reconcile(state: &AppState, current: &[Alert]) -> AppResult<()> {
    let now = store::now();
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    for item in current {
        sqlx::query("INSERT INTO monitor_alerts(fingerprint,rule,severity,title,detail,opened_at,last_seen_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(fingerprint) WHERE resolved_at=0 DO UPDATE SET severity=excluded.severity,title=excluded.title,detail=excluded.detail,last_seen_at=excluded.last_seen_at,occurrences=monitor_alerts.occurrences+1")
 .bind(&item.fingerprint).bind(item.rule).bind(item.severity).bind(&item.title).bind(&item.detail).bind(now).bind(now).execute(&mut *tx).await?;
    }
    let fingerprints: HashSet<&str> = current.iter().map(|a| a.fingerprint.as_str()).collect();
    let active: Vec<(i64, String)> =
        sqlx::query_as("SELECT id,fingerprint FROM monitor_alerts WHERE resolved_at=0")
            .fetch_all(&mut *tx)
            .await?;
    for (id, fingerprint) in active {
        if !fingerprints.contains(fingerprint.as_str()) {
            sqlx::query("UPDATE monitor_alerts SET resolved_at=? WHERE id=? AND resolved_at=0")
                .bind(now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
pub async fn run_once(state: &AppState) -> AppResult<()> {
    let config: AlertConfig = serde_json::from_value(settings(state).await?["config"].clone())?;
    let now = store::now();
    let (checks, workers, providers, tasks, traffic) = tokio::join!(
        dependencies(state),
        workers(state),
        providers(state),
        task_summary(state),
        traffic(state, now - config.window_minutes as i64 * 60, now)
    );
    let snapshot = json!({"components":checks,"workers":workers?,"providers":providers?,"tasks":tasks?,"traffic":traffic?,"checked_at":now});
    // Do not mark a monitor cycle healthy until its previous data collection succeeded.
    reconcile(state, &evaluate(&snapshot, &config)).await?;
    sqlx::query("INSERT INTO monitor_snapshots(id,data_json,updated_at) VALUES(1,?,?) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json,updated_at=excluded.updated_at")
 .bind(snapshot.to_string()).bind(now).execute(&state.pool).await?;
    Ok(())
}
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            heartbeat(&state, "monitor", true, None).await;
            let result = run_once(&state).await;
            heartbeat(&state, "monitor", true, Some(result.is_ok())).await;
            if result.is_err() {
                tracing::warn!("monitor data collection failed");
            }
        }
    })
}
/// 单批删除上限：一次删太多会长时间占住 SQLite 的写锁。
const RETENTION_BATCH: i64 = 5000;
/// 每张表单轮最多清理的批次数，避免异常数据把一轮拖成无限循环。
const RETENTION_MAX_BATCHES: usize = 40;

/// 定期清理只增不减的运营数据。
///
/// `user_request_logs`、`operation_audit`、`monitor_alerts` 都会随请求量
/// 无限增长，而业务库是单个 SQLite 文件：不清理的话，备份会越来越慢、
/// 监控查询会越来越慢，最终磁盘写满。
///
/// 清理的是逐条明细；`virtual_keys` 上的累计请求数、token 与费用是累加值，
/// 不受影响，所以账单总额不会因为清理而改变。
pub fn spawn_retention(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(6 * 3600));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // 冷启动时让迁移和引导先跑完，别和它们抢数据库。
        tokio::time::sleep(Duration::from_secs(300)).await;
        loop {
            tick.tick().await;
            match run_retention(&state.pool, state.config.log_retention_days).await {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "pruned expired operational rows"),
                Err(error) => tracing::warn!(%error, "operational data retention failed"),
            }
        }
    })
}

pub(crate) async fn run_retention(pool: &sqlx::SqlitePool, days: i64) -> AppResult<i64> {
    let now = store::now();
    let mut removed: i64;

    // 已过期的资源归属与早已停跳的旧实例心跳：无需等待保留策略。
    removed = sqlx::query("DELETE FROM gateway_resource_owners WHERE expires_at<?")
        .bind(now)
        .execute(pool)
        .await?
        .rows_affected() as i64;
    removed += sqlx::query("DELETE FROM worker_heartbeats WHERE last_tick<?")
        .bind(now - 7 * 86400)
        .execute(pool)
        .await?
        .rows_affected() as i64;

    if days <= 0 {
        return Ok(removed);
    }
    let cutoff = now - days * 86400;

    // 表名与列名是代码里的固定常量，不来自外部输入。
    for (table, column) in [
        ("user_request_logs", "created_at"),
        ("operation_audit", "created_at"),
        ("monitor_alerts", "opened_at"),
    ] {
        let sql = format!(
            "DELETE FROM {table} WHERE id IN (SELECT id FROM {table} WHERE {column}<? ORDER BY {column} LIMIT {RETENTION_BATCH})"
        );
        for _ in 0..RETENTION_MAX_BATCHES {
            let deleted = sqlx::query(&sql)
                .bind(cutoff)
                .execute(pool)
                .await?
                .rows_affected() as i64;
            removed += deleted;
            if deleted < RETENTION_BATCH {
                break;
            }
        }
    }
    Ok(removed)
}

fn pagination(payload: &Value) -> (i64, i64) {
    (
        payload["page"].as_i64().unwrap_or(1).clamp(1, 1_000_000),
        payload["page_size"].as_i64().unwrap_or(20).clamp(1, 100),
    )
}
pub async fn overview(state: &AppState, payload: &Value) -> AppResult<Value> {
    let hours = payload["hours"].as_i64().unwrap_or(24);
    if !matches!(hours, 1 | 24 | 168) {
        return Err(AppError::BadRequest(
            "监控范围支持 1 小时、24 小时或 7 天".into(),
        ));
    }
    let now = store::now();
    let from = now - hours * 3600;
    let step = hours * 3600 / 48;
    let summary = traffic(state, from, now).await?;
    let rows=sqlx::query("SELECT CAST((created_at-?)/? AS INTEGER) AS bucket,COUNT(*) AS requests,SUM(CASE WHEN status_code NOT BETWEEN 200 AND 299 AND status_code!=499 THEN 1 ELSE 0 END) AS failed FROM user_request_logs WHERE created_at>=? AND created_at<=? GROUP BY bucket ORDER BY bucket")
 .bind(from).bind(step).bind(from).bind(now).fetch_all(&state.pool).await?;
    let mut buckets = HashMap::new();
    for row in rows {
        buckets.insert(
            row.get::<i64, _>("bucket"),
            (row.get::<i64, _>("requests"), row.get::<i64, _>("failed")),
        );
    }
    let timeline: Vec<Value> = (0..=48)
        .map(|i| {
            let (requests, failed) = buckets.get(&i).copied().unwrap_or_default();
            json!({"timestamp":from+i*step,"requests":requests,"failed":failed})
        })
        .collect();
    let rows=sqlx::query(&format!("SELECT model_alias,{AGGREGATES} FROM user_request_logs WHERE created_at>=? AND created_at<=? GROUP BY model_alias ORDER BY requests DESC LIMIT 20")).bind(from).bind(now).fetch_all(&state.pool).await?;
    let models: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut value = aggregate(r);
            value["name"] = json!(r.get::<String, _>("model_alias"));
            value
        })
        .collect();
    let row = sqlx::query("SELECT data_json,updated_at FROM monitor_snapshots WHERE id=1")
        .fetch_optional(&state.pool)
        .await?;
    let snapshot = if let Some(row) = row {
        let mut value: Value = serde_json::from_str(row.get("data_json"))?;
        value["stale"] = json!(now - row.get::<i64, _>("updated_at") > 90);
        value
    } else {
        json!({"components":[],"providers":[],"workers":[],"tasks":{},"checked_at":0,"stale":true})
    };
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM monitor_alerts WHERE resolved_at=0")
        .fetch_one(&state.pool)
        .await?;
    Ok(
        json!({"summary":summary,"timeline":timeline,"models":models,"snapshot":snapshot,"runtime":crate::observability::runtime(),"active_alerts":active,"from":from,"to":now,"hours":hours,"generated_at":now}),
    )
}
pub async fn alerts(state: &AppState, payload: &Value) -> AppResult<Value> {
    let scope = payload["scope"].as_str().unwrap_or("active");
    if !matches!(scope, "active" | "history") {
        return Err(AppError::BadRequest("告警范围无效".into()));
    }
    let (page, size) = pagination(payload);
    let active = scope == "active";
    let rows=sqlx::query("SELECT a.*,COALESCE(NULLIF(u.display_name,''),u.email,'') AS acknowledged_name FROM monitor_alerts a LEFT JOIN users u ON u.id=a.acknowledged_by WHERE (?=0 OR resolved_at=0) ORDER BY opened_at DESC,a.id DESC LIMIT ? OFFSET ?").bind(active).bind(size).bind((page-1)*size).fetch_all(&state.pool).await?;
    let list:Vec<Value>=rows.iter().map(|r|json!({"id":r.get::<i64,_>("id"),"rule":r.get::<String,_>("rule"),"severity":r.get::<String,_>("severity"),"title":r.get::<String,_>("title"),"detail":r.get::<String,_>("detail"),"opened_at":r.get::<i64,_>("opened_at"),"last_seen_at":r.get::<i64,_>("last_seen_at"),"resolved_at":r.get::<i64,_>("resolved_at"),"acknowledged_at":r.get::<i64,_>("acknowledged_at"),"acknowledged_name":r.get::<String,_>("acknowledged_name"),"occurrences":r.get::<i64,_>("occurrences")})).collect();
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM monitor_alerts WHERE (?=0 OR resolved_at=0)")
            .bind(active)
            .fetch_one(&state.pool)
            .await?;
    Ok(json!({"list":list,"total":total,"page":page}))
}
pub async fn acknowledge(state: &AppState, actor: i64, payload: &Value) -> AppResult<Value> {
    let id = payload["id"]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or_else(|| AppError::BadRequest("告警标识无效".into()))?;
    let result=sqlx::query("UPDATE monitor_alerts SET acknowledged_at=CASE WHEN acknowledged_at=0 THEN ? ELSE acknowledged_at END,acknowledged_by=CASE WHEN acknowledged_at=0 THEN ? ELSE acknowledged_by END WHERE id=? AND resolved_at=0").bind(store::now()).bind(actor).bind(id).execute(&state.pool).await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("告警不存在或已恢复".into()));
    }
    Ok(json!({"acknowledged":true}))
}
pub async fn tasks(state: &AppState, payload: &Value) -> AppResult<Value> {
    let status = payload["status"].as_str().unwrap_or("");
    if !matches!(
        status,
        "" | "submitting" | "queued" | "processing" | "succeeded" | "failed" | "submit_failed"
    ) {
        return Err(AppError::BadRequest("任务状态无效".into()));
    }
    let (page, size) = pagination(payload);
    let rows=sqlx::query("SELECT t.id,t.model_alias,t.status,t.created_at,t.expires_at,t.poll_after,t.poll_failures,t.settled,t.upstream_task_id!='' AS can_recheck,t.result_json IS NULL AS incomplete,t.log_id,t.poll_lease_until,l.request_id,COALESCE(NULLIF(l.key_name,''),k.name,'已删除密钥') AS key_name,COALESCE(NULLIF(l.provider_name,''),p.name,'已删除供应商') AS provider_name,COALESCE(NULLIF(u.display_name,''),u.email,'已删除用户') AS owner_name FROM gateway_tasks t LEFT JOIN user_request_logs l ON l.id=t.log_id LEFT JOIN users u ON u.id=t.user_id LEFT JOIN virtual_keys k ON k.id=t.virtual_key_id LEFT JOIN user_providers p ON p.id=t.provider_id WHERE (?='' OR t.status=?) ORDER BY t.created_at DESC,t.id LIMIT ? OFFSET ?")
 .bind(status).bind(status).bind(size).bind((page-1)*size).fetch_all(&state.pool).await?;
    let list:Vec<Value>=rows.iter().map(|r|json!({"id":r.get::<String,_>("id"),"model":r.get::<String,_>("model_alias"),"status":r.get::<String,_>("status"),"created_at":r.get::<i64,_>("created_at"),"expires_at":r.get::<i64,_>("expires_at"),"poll_after":r.get::<i64,_>("poll_after"),"poll_failures":r.get::<i64,_>("poll_failures"),"settled":r.get::<bool,_>("settled"),"request_id":r.get::<Option<String>,_>("request_id").unwrap_or_default(),"can_recheck":r.get::<bool,_>("can_recheck")&&r.get::<bool,_>("incomplete")&&r.get::<Option<i64>,_>("log_id").is_some()&&r.get::<i64,_>("poll_lease_until")<=store::now()&&r.get::<i64,_>("expires_at")>store::now(),"key_name":r.get::<String,_>("key_name"),"provider_name":r.get::<String,_>("provider_name"),"owner_name":r.get::<String,_>("owner_name")})).collect();
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM gateway_tasks WHERE (?='' OR status=?)")
            .bind(status)
            .bind(status)
            .fetch_one(&state.pool)
            .await?;
    Ok(json!({"list":list,"total":total,"page":page}))
}
pub async fn recheck_task(state: &AppState, payload: &Value) -> AppResult<Value> {
    let id = payload["id"]
        .as_str()
        .filter(|v| v.len() < 100)
        .ok_or_else(|| AppError::BadRequest("任务标识无效".into()))?;
    let result=sqlx::query("UPDATE gateway_tasks SET poll_after=0 WHERE id=? AND result_json IS NULL AND upstream_task_id!='' AND log_id IS NOT NULL AND expires_at>? AND poll_lease_until<=?").bind(id).bind(store::now()).bind(store::now()).execute(&state.pool).await?;
    if result.rows_affected() == 0 {
        return Err(AppError::BadRequest(
            "任务已完成、正在查询、已过期或提交未获确认，不能再次查询".into(),
        ));
    }
    Ok(json!({"scheduled":true}))
}
pub async fn audit_list(state: &AppState, payload: &Value) -> AppResult<Value> {
    let (page, size) = pagination(payload);
    let rows=sqlx::query("SELECT a.action,a.request_id,a.status_code,a.created_at,COALESCE(NULLIF(u.display_name,''),u.email,'已删除用户') AS actor_name FROM operation_audit a LEFT JOIN users u ON u.id=a.actor_id ORDER BY a.id DESC LIMIT ? OFFSET ?").bind(size).bind((page-1)*size).fetch_all(&state.pool).await?;
    let list:Vec<Value>=rows.iter().map(|r|json!({"action":r.get::<String,_>("action"),"request_id":r.get::<String,_>("request_id"),"status_code":r.get::<i64,_>("status_code"),"created_at":r.get::<i64,_>("created_at"),"actor_name":r.get::<String,_>("actor_name")})).collect();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operation_audit")
        .fetch_one(&state.pool)
        .await?;
    Ok(json!({"list":list,"total":total,"page":page}))
}
pub async fn metrics_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    use hmac::{Hmac, Mac};
    let configured = std::env::var("JUNOHUB_METRICS_TOKEN").unwrap_or_default();
    if !configured.is_empty() {
        let token = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default();
        let mut expected =
            Hmac::<sha2::Sha256>::new_from_slice(b"JunoHub metrics token comparison").unwrap();
        expected.update(configured.as_bytes());
        let mut actual =
            Hmac::<sha2::Sha256>::new_from_slice(b"JunoHub metrics token comparison").unwrap();
        actual.update(token.as_bytes());
        if actual
            .verify_slice(&expected.finalize().into_bytes())
            .is_ok()
        {
            return Ok(next.run(request).await);
        }
    }
    crate::auth::require_admin(State(state), request, next).await
}
pub async fn metrics(State(state): State<AppState>) -> AppResult<Response> {
    use std::fmt::Write;
    let mut output = crate::observability::prometheus();
    if let Some(row) = sqlx::query("SELECT data_json,updated_at FROM monitor_snapshots WHERE id=1")
        .fetch_optional(&state.pool)
        .await?
    {
        let snapshot: Value = serde_json::from_str(row.get("data_json"))?;
        let _=writeln!(output,"# TYPE junohub_monitor_last_success_timestamp_seconds gauge\njunohub_monitor_last_success_timestamp_seconds {}",row.get::<i64,_>("updated_at"));
        output.push_str("# HELP junohub_dependency_up Last passive dependency probe result.\n# TYPE junohub_dependency_up gauge\n");
        for c in snapshot["components"].as_array().into_iter().flatten() {
            let name = c["name"].as_str().unwrap_or("unknown");
            if matches!(name, "sqlite" | "postgres" | "redis") {
                let _ = writeln!(
                    output,
                    "junohub_dependency_up{{dependency=\"{name}\"}} {}",
                    i32::from(c["healthy"] == true)
                );
            }
        }
        output.push_str("# HELP junohub_worker_up Last worker heartbeat health.\n# TYPE junohub_worker_up gauge\n");
        for w in snapshot["workers"].as_array().into_iter().flatten() {
            let name = w["name"].as_str().unwrap_or("unknown");
            if matches!(name, "monitor" | "account_scheduler" | "task_poller")
                && w["status"] != "disabled"
            {
                let _ = writeln!(
                    output,
                    "junohub_worker_up{{worker=\"{name}\"}} {}",
                    i32::from(w["status"] == "healthy")
                );
            }
        }
        output.push_str(
            "# HELP junohub_tasks Persistent background task counts.\n# TYPE junohub_tasks gauge\n",
        );
        for key in ["pending", "uncertain", "unsettled", "overdue"] {
            let _ = writeln!(
                output,
                "junohub_tasks{{state=\"{key}\"}} {}",
                snapshot["tasks"][key].as_i64().unwrap_or(0)
            );
        }
    }
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM monitor_alerts WHERE resolved_at=0")
        .fetch_one(&state.pool)
        .await?;
    let _ = writeln!(
        output,
        "# TYPE junohub_active_alerts gauge\njunohub_active_alerts {active}"
    );
    Ok((
        [
            ("content-type", "text/plain; version=0.0.4; charset=utf-8"),
            ("cache-control", "private, no-store"),
        ],
        output,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::tests::state;
    #[tokio::test]
    async fn aggregation_excludes_pending_cancellation_and_missing_samples() {
        let state = state().await;
        let now = store::now();
        let empty = overview(&state, &json!({"hours":1})).await.unwrap();
        assert_eq!(empty["summary"]["success_rate"], Value::Null);
        assert_eq!(empty["summary"]["p95_ms"], Value::Null);
        for (status, ms, cost) in [
            (200, 100, Some(0.04)),
            (200, 500, None),
            (502, 900, None),
            (429, 900, None),
            (202, 300, None),
            (499, 10000, None),
        ] {
            sqlx::query("INSERT INTO user_request_logs(user_id,virtual_key_id,model_alias,status_code,duration_ms,created_at,cost_usd) VALUES(1,1,'test-model',?,?,?,?)").bind(status).bind(ms).bind(now).bind(cost).execute(&state.pool).await.unwrap();
        }
        let data = overview(&state, &json!({"hours":1})).await.unwrap();
        let s = &data["summary"];
        assert_eq!(s["requests"], 6);
        assert_eq!(s["success"], 2);
        assert_eq!(s["failed"], 2);
        assert_eq!(s["pending"], 1);
        assert_eq!(s["cancelled"], 1);
        assert_eq!(s["success_rate"], 50.0);
        assert_eq!(s["p50_ms"], 100);
        assert_eq!(s["p95_ms"], 500);
        assert_eq!(s["cost_usd"], 0.04);
        assert_eq!(s["unpriced"], 1);
        assert_eq!(
            data["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["requests"].as_i64().unwrap())
                .sum::<i64>(),
            6
        );
        assert!(overview(&state, &json!({"hours":100000})).await.is_err());
    }
    #[tokio::test]
    async fn alerts_dedupe_acknowledge_recover_and_refire_with_versioned_rules() {
        let state = state().await;
        let config = AlertConfig {
            min_requests: 2,
            ..Default::default()
        };
        let saved = update_settings(&state, &json!({"revision":0,"config":config}))
            .await
            .unwrap();
        assert_eq!(saved["revision"], 1);
        assert!(
            update_settings(&state, &json!({"revision":0,"config":config}))
                .await
                .is_err()
        );
        assert!(
            update_settings(&state, &json!({"revision":1,"config":{"window_minutes":0}}))
                .await
                .is_err()
        );
        let snapshot = json!({"components":[{"name":"redis","healthy":false}],"workers":[],"providers":[],"tasks":{},"traffic":{"requests":4,"failed":2,"success":2,"p95_ms":20000,"pending":0,"cancelled":0,"rate_limited":2,"unpriced":2}});
        let evaluated = evaluate(&snapshot, &config);
        assert_eq!(evaluated.len(), 5);
        reconcile(&state, &evaluated).await.unwrap();
        reconcile(&state, &evaluated).await.unwrap();
        let active = alerts(&state, &json!({})).await.unwrap();
        assert_eq!(active["total"], 5);
        assert_eq!(active["list"][0]["occurrences"], 2);
        let id = active["list"][0]["id"].as_i64().unwrap();
        acknowledge(&state, 1, &json!({"id":id})).await.unwrap();
        reconcile(&state, &evaluated).await.unwrap();
        assert!(alerts(&state, &json!({})).await.unwrap()["list"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == id && v["acknowledged_at"].as_i64().unwrap() > 0));
        reconcile(&state, &[]).await.unwrap();
        assert_eq!(alerts(&state, &json!({})).await.unwrap()["total"], 0);
        assert!(acknowledge(&state, 1, &json!({"id":id})).await.is_err());
        reconcile(&state, &evaluated[..1]).await.unwrap();
        assert_eq!(
            alerts(&state, &json!({"scope":"history"})).await.unwrap()["total"],
            6
        );
        let mut insufficient = snapshot.clone();
        insufficient["traffic"] =
            json!({"requests":1,"failed":1,"success":0,"pending":0,"cancelled":0});
        assert_eq!(evaluate(&insufficient, &config).len(), 1);
        let mut disabled = config;
        disabled.enabled = false;
        assert_eq!(evaluate(&snapshot, &disabled).len(), 1);
    }
    #[tokio::test]
    async fn worker_failure_does_not_reset_on_tick_and_readiness_really_probes_dependencies() {
        let state = state().await;
        heartbeat(&state, "task_poller", true, Some(true)).await;
        heartbeat(&state, "task_poller", true, Some(false)).await;
        heartbeat(&state, "task_poller", true, None).await;
        let list = workers(&state).await.unwrap();
        assert_eq!(list[1]["status"], "unavailable");
        heartbeat(&state, "task_poller", true, Some(true)).await;
        assert_eq!(workers(&state).await.unwrap()[1]["status"], "healthy");
        let response = readiness(State(state)).await;
        assert_eq!(response.status(), 503);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("postgres"));
    }
    #[tokio::test]
    async fn admin_permissions_metrics_auth_trace_and_audit_never_expose_secrets() {
        use axum::{
            body::to_bytes,
            middleware,
            routing::{get, post},
            Router,
        };
        let state = state().await;
        sqlx::query("UPDATE users SET role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let admin =
            crate::auth::issue_user_token(&state, 1, "one@example.test".into(), "admin".into())
                .unwrap();
        let user =
            crate::auth::issue_user_token(&state, 2, "two@example.test".into(), "user".into())
                .unwrap();
        let routes = Router::new()
            .route("/console/{*path}", post(crate::console::dispatch))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                crate::observability::audit,
            ))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                crate::auth::require_user,
            ));
        let metric_routes = Router::new()
            .route("/metrics", get(metrics))
            .route_layer(middleware::from_fn_with_state(state.clone(), metrics_auth));
        let app = routes
            .merge(metric_routes)
            .with_state(state.clone())
            .layer(middleware::from_fn(crate::observability::trace));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client.get(format!("{url}/metrics")).send().await.unwrap();
        assert_eq!(response.status(), 401);
        let response = client
            .get(format!("{url}/metrics"))
            .bearer_auth(&user)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        let response = client
            .get(format!("{url}/metrics"))
            .bearer_auth(&admin)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let text = response.text().await.unwrap();
        assert!(text.contains("junohub_http_duration_seconds_bucket"));
        assert!(!text.contains(&admin));
        for path in [
            "overview",
            "settings",
            "alerts",
            "tasks",
            "audit",
            "alerts/acknowledge",
            "tasks/recheck",
            "settings/update",
        ] {
            let response = client
                .post(format!("{url}/console/monitor/{path}"))
                .bearer_auth(&user)
                .json(&json!({"id":1}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 403, "{path}");
        }
        let response = client
            .post(format!("{url}/console/monitor/settings/update"))
            .bearer_auth(&admin)
            .header("x-request-id", "attacker-secret")
            .json(&json!({"revision":0,"config":AlertConfig::default()}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let id = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        response.bytes().await.unwrap();
        let audit = audit_list(&state, &json!({})).await.unwrap();
        assert_eq!(audit["total"], 4);
        assert_eq!(audit["list"][0]["request_id"], id);
        assert!(!audit.to_string().contains("attacker-secret"));
        assert!(!audit.to_string().contains(&admin));
        let exported = metrics(State(state)).await.unwrap();
        let bytes = to_bytes(exported.into_body(), 100000).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("junohub_active_alerts"));
        server.abort();
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    async fn pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect");
        sqlx::migrate!().run(&pool).await.expect("migrate");
        pool
    }

    async fn insert_log(pool: &sqlx::SqlitePool, created_at: i64) {
        sqlx::query("INSERT INTO user_request_logs(user_id,virtual_key_id,key_name,model_alias,upstream_model,provider_type,provider_id,provider_name,status_code,duration_ms,error_message,input_tokens,output_tokens,total_tokens,created_at,request_id) VALUES(1,1,'k','m','u','openai',1,'p',200,1,'',1,1,2,?,'')")
            .bind(created_at)
            .execute(pool)
            .await
            .expect("insert log");
    }

    async fn count_logs(pool: &sqlx::SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM user_request_logs")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn retention_prunes_only_rows_past_the_window() {
        let pool = pool().await;
        let now = store::now();
        for age_days in [1, 10, 100, 400] {
            insert_log(&pool, now - age_days * 86400).await;
        }
        assert_eq!(count_logs(&pool).await, 4);

        // 90 天窗口：只应删掉 100 天和 400 天前的两条。
        let removed = run_retention(&pool, 90).await.unwrap();
        assert_eq!(removed, 2);
        assert_eq!(count_logs(&pool).await, 2);
        let oldest: i64 = sqlx::query_scalar("SELECT MIN(created_at) FROM user_request_logs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(now - oldest <= 10 * 86400);
    }

    #[tokio::test]
    async fn zero_retention_keeps_history_and_expired_resources_still_go() {
        let pool = pool().await;
        let now = store::now();
        insert_log(&pool, now - 4000 * 86400).await;
        sqlx::query("INSERT INTO gateway_resource_owners(provider_id,model_id,kind,resource_hash,user_id,key_id,expires_at) VALUES(1,1,'response','hash',1,1,?)")
            .bind(now - 10)
            .execute(&pool)
            .await
            .ok();

        let removed = run_retention(&pool, 0).await.unwrap();
        assert_eq!(count_logs(&pool).await, 1, "0 表示永久保留日志");
        assert_eq!(removed, 0);
    }
}
