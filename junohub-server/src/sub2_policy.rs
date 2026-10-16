use crate::{
    error::{AppError, AppResult},
    state::AppState,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Continuation is an ownership constraint, never a soft scheduling preference.
pub fn pin_response_candidates(ids: &mut Vec<i64>, bound: Option<i64>) -> AppResult<()> {
    let bound = bound
        .ok_or_else(|| AppError::BadRequest("无法确定续接响应对应的账户，请重新开始对话".into()))?;
    ids.retain(|id| *id == bound);
    if ids.is_empty() {
        return Err(AppError::BadRequest(
            "续接响应的原账户当前不可用，不能切换到其他账户".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, FromRow)]
pub struct DispatchPolicy {
    pub strategy: String,
    pub max_attempts: i64,
    pub sticky_sessions: bool,
    #[serde(default = "default_timeout")]
    pub request_timeout_seconds: i64,
    #[serde(default = "default_cooldown")]
    pub failure_cooldown_seconds: i64,
}
fn default_timeout() -> i64 {
    120
}
fn default_cooldown() -> i64 {
    30
}

impl Default for DispatchPolicy {
    fn default() -> Self {
        Self {
            strategy: "balanced".into(),
            max_attempts: 3,
            sticky_sessions: true,
            request_timeout_seconds: 120,
            failure_cooldown_seconds: 30,
        }
    }
}
impl DispatchPolicy {
    pub fn validate(&self) -> AppResult<()> {
        if !matches!(
            self.strategy.as_str(),
            "balanced" | "priority" | "round_robin" | "weighted"
        ) || !(1..=100).contains(&self.max_attempts)
            || !(5..=600).contains(&self.request_timeout_seconds)
            || !(5..=3600).contains(&self.failure_cooldown_seconds)
        {
            return Err(AppError::BadRequest(
                "策略参数无效：尝试次数 1–100，响应期限 5–600 秒，故障冷却 5–3600 秒".into(),
            ));
        }
        Ok(())
    }
}
pub async fn for_provider(state: &AppState, provider_id: i64) -> AppResult<DispatchPolicy> {
    Ok(sqlx::query_as::<_,DispatchPolicy>("SELECT strategy,max_attempts,sticky_sessions,request_timeout_seconds,failure_cooldown_seconds FROM provider_dispatch_policy WHERE provider_id=?").bind(provider_id).fetch_optional(&state.pool).await?.unwrap_or_default())
}
pub async fn for_group(state: &AppState, group_id: i64) -> AppResult<DispatchPolicy> {
    let provider: Option<i64> =
        sqlx::query_scalar("SELECT provider_id FROM sub2_managed_pools WHERE group_id=?")
            .bind(group_id)
            .fetch_optional(&state.pool)
            .await?;
    match provider {
        Some(id) => for_provider(state, id).await,
        None => Ok(DispatchPolicy::default()),
    }
}
pub fn order_candidates(ids: &mut [i64], rotation: usize, preferred: Option<i64>) {
    if !ids.is_empty() {
        let length = ids.len();
        ids.rotate_left(rotation % length);
    }
    if let Some(index) = preferred.and_then(|id| ids.iter().position(|candidate| *candidate == id))
    {
        ids[..=index].rotate_right(1);
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, FromRow)]
pub struct AccountDispatch {
    pub id: i64,
    pub priority: i32,
    #[serde(default)]
    pub member_order: i32,
    pub weight: i32,
    pub concurrency: i32,
    #[serde(default)]
    pub rpm: i32,
    #[serde(default)]
    pub allowed_models: Vec<String>,
}
pub fn validate_accounts(accounts: &[AccountDispatch]) -> AppResult<()> {
    let mut ids = std::collections::HashSet::new();
    if accounts.is_empty()
        || accounts.len() > 100
        || accounts.iter().any(|a| {
            a.id <= 0
                || !ids.insert(a.id)
                || !(0..=999).contains(&a.priority)
                || !(0..=1000).contains(&a.member_order)
                || !(1..=1000).contains(&a.weight)
                || !(1..=1000).contains(&a.concurrency)
                || !(0..=1000000).contains(&a.rpm)
                || a.allowed_models.len() > 100
                || a.allowed_models
                    .iter()
                    .any(|m| m.trim().is_empty() || m.len() > 200)
        })
    {
        return Err(AppError::BadRequest(
            "账户调度参数无效：分组 1–999（0 表示未分组），权重和并发上限 1–1000，账户不得重复"
                .into(),
        ));
    }
    Ok(())
}
// Keep tiers strict and randomize only within a tier; generate the retry order once.
pub fn rank_accounts(
    accounts: &[AccountDispatch],
    strategy: &str,
    rotation: usize,
    snapshot: &[i64],
) -> Vec<i64> {
    use rand::Rng;
    let mut tiers = std::collections::BTreeMap::<i32, Vec<&AccountDispatch>>::new();
    for account in accounts.iter().filter(|a| a.priority > 0) {
        tiers.entry(account.priority).or_default().push(account);
    }
    let mut result = Vec::new();
    for (_, mut tier) in tiers {
        tier.sort_by_key(|a| (a.member_order, a.id));
        if strategy == "weighted" {
            let mut rng = rand::thread_rng();
            while !tier.is_empty() {
                let total: i64 = tier.iter().map(|a| i64::from(a.weight.max(1))).sum();
                let mut pick = rng.gen_range(0..total);
                let index = tier
                    .iter()
                    .position(|a| {
                        pick -= i64::from(a.weight.max(1));
                        pick < 0
                    })
                    .unwrap();
                result.push(tier.remove(index).id);
            }
        } else {
            if strategy == "balanced" {
                tier.sort_by_key(|a| {
                    snapshot
                        .iter()
                        .position(|id| *id == a.id)
                        .unwrap_or(usize::MAX)
                });
            }
            let mut ids: Vec<i64> = tier.iter().map(|a| a.id).collect();
            if strategy == "round_robin" {
                order_candidates(&mut ids, rotation, None);
            }
            result.extend(ids);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_continuation_is_pinned_not_promoted() {
        let mut ids = vec![1, 2, 3];
        pin_response_candidates(&mut ids, Some(3)).unwrap();
        assert_eq!(ids, vec![3]);
        // A retry receives only the bound account; excluding it cannot expose another one.
        ids.retain(|id| *id != 3);
        assert!(pin_response_candidates(&mut ids, Some(3)).is_err());
        assert!(pin_response_candidates(&mut vec![1, 2], Some(3)).is_err());
        assert!(pin_response_candidates(&mut vec![1, 2], None).is_err());
    }

    #[test]
    fn group_order_and_member_order_are_independent() {
        let rows = vec![
            AccountDispatch {
                id: 1,
                priority: 1,
                member_order: 2,
                weight: 1,
                concurrency: 1,
                rpm: 0,
                allowed_models: vec![],
            },
            AccountDispatch {
                id: 2,
                priority: 1,
                member_order: 1,
                weight: 1,
                concurrency: 1,
                rpm: 0,
                allowed_models: vec![],
            },
            AccountDispatch {
                id: 3,
                priority: 2,
                member_order: 0,
                weight: 1000,
                concurrency: 1,
                rpm: 0,
                allowed_models: vec![],
            },
            AccountDispatch {
                id: 4,
                priority: 0,
                member_order: 0,
                weight: 1,
                concurrency: 1,
                rpm: 0,
                allowed_models: vec![],
            },
        ];
        validate_accounts(&rows).unwrap();
        assert_eq!(rank_accounts(&rows, "priority", 0, &[]), vec![2, 1, 3]);
        assert_eq!(rank_accounts(&rows, "round_robin", 0, &[]), vec![2, 1, 3]);
        assert_eq!(rank_accounts(&rows, "round_robin", 1, &[]), vec![1, 2, 3]);
        assert_eq!(
            rank_accounts(&rows, "balanced", 0, &[3, 1, 2]),
            vec![1, 2, 3]
        );
        for _ in 0..100 {
            assert_eq!(rank_accounts(&rows, "weighted", 0, &[])[2], 3);
        }
        let mut reordered = rows.clone();
        reordered[2].priority = 1;
        reordered[0].priority = 2;
        reordered[1].priority = 2;
        assert_eq!(rank_accounts(&reordered, "priority", 0, &[]), vec![3, 2, 1]);
    }

    #[tokio::test]
    async fn policy_migration_preserves_existing_preferences() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE user_providers(id INTEGER PRIMARY KEY); INSERT INTO user_providers VALUES(1);").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../migrations/0009_provider_dispatch_policy.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO provider_dispatch_policy VALUES(1,'round_robin',5,0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../migrations/0010_dispatch_controls.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let policy: DispatchPolicy=sqlx::query_as("SELECT strategy,max_attempts,sticky_sessions,request_timeout_seconds,failure_cooldown_seconds FROM provider_dispatch_policy WHERE provider_id=1").fetch_one(&pool).await.unwrap();
        assert_eq!(policy.strategy, "round_robin");
        assert_eq!(policy.max_attempts, 5);
        assert!(!policy.sticky_sessions);
        assert_eq!(policy.request_timeout_seconds, 120);
    }

    #[test]
    fn weighted_selection_respects_tiers_and_proportions() {
        let accounts = vec![
            AccountDispatch {
                member_order: 0,
                id: 1,
                priority: 1,
                weight: 7,
                concurrency: 2,
                rpm: 0,
                allowed_models: vec![],
            },
            AccountDispatch {
                member_order: 0,
                id: 2,
                priority: 1,
                weight: 3,
                concurrency: 2,
                rpm: 0,
                allowed_models: vec![],
            },
            AccountDispatch {
                member_order: 0,
                id: 3,
                priority: 2,
                weight: 1000,
                concurrency: 2,
                rpm: 0,
                allowed_models: vec![],
            },
        ];
        let mut first = 0;
        for _ in 0..10000 {
            let ranked = rank_accounts(&accounts, "weighted", 0, &[]);
            if ranked[0] == 1 {
                first += 1;
            }
            assert_eq!(ranked[2], 3);
            assert_ne!(ranked[0], ranked[1]);
        }
        assert!((6600..7400).contains(&first));
        assert_eq!(
            rank_accounts(&accounts, "round_robin", 1, &[]),
            vec![2, 1, 3]
        );
        assert_eq!(
            rank_accounts(&accounts, "balanced", 0, &[3, 2, 1]),
            vec![2, 1, 3]
        );
        assert!(validate_accounts(&accounts).is_ok());
        let mut invalid = accounts.clone();
        invalid[1].id = 1;
        assert!(validate_accounts(&invalid).is_err());
        invalid = accounts;
        invalid[1].weight = 0;
        assert!(validate_accounts(&invalid).is_err());
    }
    #[test]
    fn rotation_and_continuation() {
        let mut ids = [11, 22, 33];
        order_candidates(&mut ids, 1, None);
        assert_eq!(ids, [22, 33, 11]);
        order_candidates(&mut ids, 0, Some(11));
        assert_eq!(ids, [11, 22, 33]);
        order_candidates(&mut [], 9, None);
    }
    #[test]
    fn validates_strategy_and_retry_bounds() {
        let mut policy = DispatchPolicy::default();
        assert!(policy.validate().is_ok());
        policy.strategy = "unknown".into();
        assert!(policy.validate().is_err());
        policy.strategy = "round_robin".into();
        policy.max_attempts = 0;
        assert!(policy.validate().is_err());
        policy.max_attempts = 1;
        assert!(policy.validate().is_ok());
    }
}
