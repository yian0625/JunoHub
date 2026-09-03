#![allow(dead_code)] // Removed when the Rust engine repositories take production traffic.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Anthropic,
    Openai,
    Gemini,
    Antigravity,
    Grok,
    Kimi,
    Zhipu,
    Deepseek,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountKind {
    Oauth,
    SetupToken,
    ApiKey,
    ServiceAccount,
    Bedrock,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Active,
    Inactive,
    Error,
    RateLimited,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountConfig {
    pub name: String,
    pub platform: Platform,
    pub kind: AccountKind,
    pub concurrency: u32,
    pub load_factor: Option<u32>,
    pub priority: i32,
    pub rate_multiplier: f64,
    pub schedulable: bool,
    pub status: AccountStatus,
}

impl AccountConfig {
    pub fn validate(&self) -> AppResult<()> {
        if self.name.trim().is_empty() || self.name.chars().count() > 100 {
            return Err(AppError::BadRequest(
                "账户名称长度必须为 1 到 100 个字符".into(),
            ));
        }
        if self.concurrency > 10_000 {
            return Err(AppError::BadRequest("账户并发不能超过 10000".into()));
        }
        if self
            .load_factor
            .is_some_and(|value| value == 0 || value > 10_000)
        {
            return Err(AppError::BadRequest("负载系数必须为 1 到 10000".into()));
        }
        if !self.rate_multiplier.is_finite() || self.rate_multiplier < 0.0 {
            return Err(AppError::BadRequest("计费倍率必须是非负数".into()));
        }
        Ok(())
    }

    pub fn can_schedule(&self) -> bool {
        self.status == AccountStatus::Active && self.schedulable && self.concurrency > 0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupConfig {
    pub name: String,
    pub platform: Platform,
    pub rate_multiplier: f64,
    pub exclusive: bool,
    pub rpm_limit: u32,
    pub allowed_models: Vec<String>,
}

impl GroupConfig {
    pub fn validate(&self) -> AppResult<()> {
        if self.name.trim().is_empty() || self.name.chars().count() > 100 {
            return Err(AppError::BadRequest(
                "分组名称长度必须为 1 到 100 个字符".into(),
            ));
        }
        if !self.rate_multiplier.is_finite() || self.rate_multiplier < 0.0 {
            return Err(AppError::BadRequest("分组倍率必须是非负数".into()));
        }
        if self.allowed_models.iter().any(|model| {
            let model = model.trim();
            model.is_empty() || model.len() > 200
        }) {
            return Err(AppError::BadRequest("模型名称无效".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiKeyPolicy {
    pub name: String,
    pub quota: f64,
    pub rate_limit_5h: f64,
    pub rate_limit_1d: f64,
    pub rate_limit_7d: f64,
    pub ip_whitelist: Vec<String>,
    pub ip_blacklist: Vec<String>,
}

impl ApiKeyPolicy {
    pub fn validate(&self) -> AppResult<()> {
        if self.name.trim().is_empty() || self.name.chars().count() > 100 {
            return Err(AppError::BadRequest(
                "API Key 名称长度必须为 1 到 100 个字符".into(),
            ));
        }
        for value in [
            self.quota,
            self.rate_limit_5h,
            self.rate_limit_1d,
            self.rate_limit_7d,
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(AppError::BadRequest("额度和限流必须是非负数".into()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> AccountConfig {
        AccountConfig {
            name: "OpenAI Codex".into(),
            platform: Platform::Openai,
            kind: AccountKind::Oauth,
            concurrency: 3,
            load_factor: Some(1),
            priority: 50,
            rate_multiplier: 1.0,
            schedulable: true,
            status: AccountStatus::Active,
        }
    }

    #[test]
    fn active_account_with_capacity_is_schedulable() {
        let mut value = account();
        value.validate().unwrap();
        assert!(value.can_schedule());
        value.status = AccountStatus::RateLimited;
        assert!(!value.can_schedule());
        value.status = AccountStatus::Active;
        value.concurrency = 0;
        assert!(!value.can_schedule());
    }

    #[test]
    fn account_validation_rejects_invalid_limits() {
        let mut value = account();
        value.rate_multiplier = f64::NAN;
        assert!(value.validate().is_err());
        value.rate_multiplier = 1.0;
        value.load_factor = Some(0);
        assert!(value.validate().is_err());
    }

    #[test]
    fn group_and_key_policies_validate_user_input() {
        let group = GroupConfig {
            name: "Personal OpenAI".into(),
            platform: Platform::Openai,
            rate_multiplier: 1.0,
            exclusive: true,
            rpm_limit: 0,
            allowed_models: vec!["gpt-5.4".into()],
        };
        group.validate().unwrap();
        let key = ApiKeyPolicy {
            name: "JunoHub".into(),
            quota: 0.0,
            rate_limit_5h: 0.0,
            rate_limit_1d: 0.0,
            rate_limit_7d: 0.0,
            ip_whitelist: vec![],
            ip_blacklist: vec![],
        };
        key.validate().unwrap();
    }
}
