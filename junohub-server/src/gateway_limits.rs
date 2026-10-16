//! Atomic, cross-instance admission. A rolling 60s window counts admitted requests.
//! Concurrency leases live through streaming and renew until the response is dropped.
use crate::{
    error::{AppError, AppResult},
    state::AppState,
};
use axum::{body::Body, response::Response};
use futures_util::StreamExt;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Clone, Debug, Default, Deserialize, Serialize, FromRow)]
pub struct Limits {
    pub max_concurrency: i64,
    pub rpm: i64,
}
impl Limits {
    pub fn validate(&self) -> AppResult<()> {
        if !(0..=100000).contains(&self.max_concurrency) || !(0..=1000000).contains(&self.rpm) {
            return Err(AppError::BadRequest(
                "并发范围 0–100000，RPM 范围 0–1000000；0 表示不额外限制".into(),
            ));
        }
        Ok(())
    }
}
pub async fn read(state: &AppState, kind: &str, id: i64) -> AppResult<Limits> {
    let table=match kind {"provider"=>"provider_limits","model"=>"model_limits","key"=>"key_limits","user"=>"user_limits",_=>return Err(AppError::BadRequest("资源限制类型无效".into()))};
    Ok(sqlx::query_as(&format!(
        "SELECT max_concurrency,rpm FROM {table} WHERE id=?"
    ))
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or_default())
}
const ACQUIRE: &str = r#"
local time=redis.call('TIME'); local now=time[1]*1000+math.floor(time[2]/1000)
for i=1,#KEYS,2 do
 redis.call('ZREMRANGEBYSCORE',KEYS[i],'-inf',now-120000)
 redis.call('ZREMRANGEBYSCORE',KEYS[i+1],'-inf',now-60000)
 local c=tonumber(ARGV[i+1]); local r=tonumber(ARGV[i+2])
 if c>0 and redis.call('ZCARD',KEYS[i])>=c then return -i end
 if r>0 and redis.call('ZCARD',KEYS[i+1])>=r then return -(#KEYS+i) end
end
for i=1,#KEYS,2 do
 if tonumber(ARGV[i+1])>0 then redis.call('ZADD',KEYS[i],now,ARGV[1]); redis.call('PEXPIRE',KEYS[i],120000) end
 if tonumber(ARGV[i+2])>0 then redis.call('ZADD',KEYS[i+1],now,ARGV[1]); redis.call('PEXPIRE',KEYS[i+1],60000) end
end
return 1
"#;
const RENEW: &str = r#"local t=redis.call('TIME');local n=t[1]*1000+math.floor(t[2]/1000);for _,k in ipairs(KEYS) do if redis.call('ZSCORE',k,ARGV[1]) then redis.call('ZADD',k,n,ARGV[1]);redis.call('PEXPIRE',k,120000) end end return 1"#;
pub struct Lease {
    redis: ConnectionManager,
    keys: Vec<String>,
    token: String,
    heartbeat: tokio::task::JoinHandle<()>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.heartbeat.abort();
        let mut redis = self.redis.clone();
        let keys = self.keys.clone();
        let token = self.token.clone();
        tokio::spawn(async move {
            let mut pipe = redis::pipe();
            for key in keys {
                pipe.cmd("ZREM").arg(key).arg(&token).ignore();
            }
            let _: redis::RedisResult<()> = pipe.query_async(&mut redis).await;
        });
    }
}
pub async fn acquire(state: &AppState, scopes: &[(String, Limits)]) -> AppResult<Option<Lease>> {
    let scopes: Vec<_> = scopes
        .iter()
        .filter(|(_, l)| l.max_concurrency > 0 || l.rpm > 0)
        .collect();
    if scopes.is_empty() {
        return Ok(None);
    }
    let mut redis = state
        .sub2_redis
        .clone()
        .ok_or_else(|| AppError::Internal("限流服务不可用".into()))?;
    let token = uuid::Uuid::new_v4().to_string();
    let script = redis::Script::new(ACQUIRE);
    let mut call = script.prepare_invoke();
    let mut keys = vec![];
    for (scope, l) in &scopes {
        let key = format!("junohub:admission:{scope}:active");
        call.key(&key).key(format!("junohub:admission:{scope}:rpm"));
        if l.max_concurrency > 0 {
            keys.push(key);
        }
    }
    call.arg(&token);
    for (_, l) in &scopes {
        call.arg(l.max_concurrency).arg(l.rpm);
    }
    let accepted: i64 = call
        .invoke_async(&mut redis)
        .await
        .map_err(|_| AppError::Internal("限流服务不可用".into()))?;
    if accepted != 1 {
        let code = accepted.unsigned_abs() as usize;
        let key_count = scopes.len() * 2;
        let rpm = code > key_count;
        let index = (if rpm { code - key_count } else { code }).saturating_sub(1) / 2;
        let name = scopes.get(index).map(|(scope,_)| match scope.split(':').next().unwrap_or("") {
            "user" => "用户总量", "virtual-key" => "访问密钥", "provider" => "供应商总量", "model" => "当前模型", "direct-key" => "上游账户", _ => "请求"
        }).unwrap_or("请求");
        return Err(AppError::TooManyRequests(format!("{name}已达到{}上限，{}。",if rpm {"每分钟请求数"} else {"同时处理请求数"}, if rpm {"请在最近 60 秒窗口释放后重试"} else {"请等待正在进行的请求完成后重试"})));
    }
    let mut heartbeat_redis = redis.clone();
    let heartbeat_keys = keys.clone();
    let heartbeat_token = token.clone();
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let script = redis::Script::new(RENEW);
            let mut call = script.prepare_invoke();
            for k in &heartbeat_keys {
                call.key(k);
            }
            call.arg(&heartbeat_token);
            let _: redis::RedisResult<i64> = call.invoke_async(&mut heartbeat_redis).await;
        }
    });
    Ok(Some(Lease {
        redis,
        keys,
        token,
        heartbeat,
    }))
}
pub fn hold(response: Response, lease: Option<Lease>) -> Response {
    hold_many(response, lease.into_iter().collect())
}
pub fn hold_many(response: Response, leases: Vec<Lease>) -> Response {
    if leases.is_empty() { return response; }
    let (parts, body) = response.into_parts();
    let mut stream = body.into_data_stream();
    Response::from_parts(
        parts,
        Body::from_stream(
            async_stream::stream! {let _leases=leases;while let Some(chunk)=stream.next().await {yield chunk;}},
        ),
    )
}
