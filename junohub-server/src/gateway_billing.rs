use std::str::FromStr;

use rust_decimal::{prelude::ToPrimitive, Decimal};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    #[default]
    Token,
    Image,
    Second,
    Character,
    Request,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub unit: Unit,
    pub price_per_unit: Option<f64>,
    pub quantity_pointer: String,
}

impl Config {
    pub fn stored(stored: &str) -> AppResult<Self> {
        let config: Self = serde_json::from_str(stored)
            .map_err(|_| AppError::BadRequest("模型计费配置无效".into()))?;
        config.validate()?;
        Ok(config)
    }
    pub fn from_payload(payload: &Value, previous: &str) -> AppResult<Self> {
        match payload.get("billing_config") {
            Some(value) => Self::stored(&value.to_string()),
            None => Self::stored(previous),
        }
    }
    pub fn validate_api(&self, api: &crate::model_api::ApiType) -> AppResult<()> {
        if *api == crate::model_api::ApiType::Embeddings && self.unit != Unit::Token {
            return Err(AppError::BadRequest("向量化模型按输入 Token 计费".into()));
        }
        Ok(())
    }
    fn validate(&self) -> AppResult<()> {
        if self
            .price_per_unit
            .is_some_and(|p| !p.is_finite() || !(0.0..=1_000_000_000.0).contains(&p))
        {
            return Err(AppError::BadRequest(
                "计费单价须为 0–1000000000 之间的数字".into(),
            ));
        }
        if self.price_per_unit.is_some_and(|p| decimal(p).is_none()) {
            return Err(AppError::BadRequest("计费单价精度最多为 28 位小数".into()));
        }
        let pointer = &self.quantity_pointer;
        if !pointer.is_empty()
            && (!pointer.starts_with('/')
                || pointer.len() > 256
                || pointer.chars().any(char::is_control)
                || pointer
                    .split('~')
                    .skip(1)
                    .any(|part| !part.starts_with(['0', '1'])))
        {
            return Err(AppError::BadRequest(
                "用量字段须为 JSON Pointer，例如 /usage/duration_seconds".into(),
            ));
        }
        if matches!(self.unit, Unit::Second | Unit::Character) && pointer.is_empty() {
            return Err(AppError::BadRequest(
                "按秒或字符计费需要填写供应商响应中的实际用量字段".into(),
            ));
        }
        if matches!(self.unit, Unit::Token | Unit::Request) && !pointer.is_empty() {
            return Err(AppError::BadRequest(
                "Token 或按次计费无需配置用量字段".into(),
            ));
        }
        if self.unit == Unit::Token && self.price_per_unit.is_some() {
            return Err(AppError::BadRequest(
                "Token 价格请使用输入和输出单价".into(),
            ));
        }
        Ok(())
    }
}

pub struct Bill {
    pub cost: Option<f64>,
    pub details: Value,
}

fn decimal(value: f64) -> Option<Decimal> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Decimal::from_str(&value.to_string())
        .ok()
        .filter(|parsed| value == 0.0 || !parsed.is_zero())
}

pub(crate) fn price_is_supported(value: f64) -> bool {
    (0.0..=1_000_000_000.0).contains(&value) && decimal(value).is_some()
}

fn actual_quantity(value: &Value) -> Option<Decimal> {
    let raw = value.as_number()?.to_string();
    Decimal::from_str(&raw)
        .or_else(|_| Decimal::from_scientific(&raw))
        .ok()
        .filter(|q| *q >= Decimal::ZERO && (!q.is_zero() || value.as_f64() == Some(0.0)))
}

/// Both synchronous responses and completed tasks use the same price snapshot.
/// Media quantities come only from successful actual results, never request n,
/// input text length or a guessed duration. Missing facts remain unpriced.
#[allow(clippy::too_many_arguments)]
pub fn calculate(
    config: &Config,
    input_price: Option<f64>,
    output_price: Option<f64>,
    input: i64,
    output: i64,
    complete: bool,
    response: Option<&Value>,
    success: bool,
) -> Bill {
    let unknown = |reason| Bill {
        cost: None,
        details: json!({"unit":config.unit,"unpriced_reason":reason}),
    };
    if config.unit == Unit::Token {
        if !complete {
            return unknown("missing_usage");
        }
        let Some(ip) = (if input == 0 { Some(0.0) } else { input_price }).and_then(decimal) else {
            return unknown("missing_price");
        };
        let Some(op) = (if output == 0 { Some(0.0) } else { output_price }).and_then(decimal)
        else {
            return unknown("missing_price");
        };
        let Some(cost) = Decimal::from(input)
            .checked_mul(ip)
            .and_then(|x| {
                Decimal::from(output)
                    .checked_mul(op)
                    .and_then(|y| x.checked_add(y))
            })
            .and_then(|amount| amount.checked_div(Decimal::from(1_000_000)))
        else {
            return unknown("amount_out_of_range");
        };
        if cost.is_zero() && ((input > 0 && !ip.is_zero()) || (output > 0 && !op.is_zero())) {
            return unknown("amount_out_of_range");
        }
        return Bill {
            cost: cost.to_f64(),
            details: json!({"unit":"token","input":input,"output":output,"amount_usd":cost.normalize().to_string()}),
        };
    }
    if !success || response.is_some_and(|v| v.get("error").is_some_and(|e| !e.is_null())) {
        return unknown("no_successful_result");
    }
    let quantity = if config.unit == Unit::Request {
        Some(Decimal::ONE)
    } else if !config.quantity_pointer.is_empty() {
        response
            .and_then(|v| {
                v.pointer(&config.quantity_pointer).or_else(|| {
                    v.get("result")
                        .and_then(|v| v.pointer(&config.quantity_pointer))
                })
            })
            .and_then(actual_quantity)
    } else if config.unit == Unit::Image {
        response
            .and_then(|v| {
                v.get("data").or_else(|| v.get("images")).or_else(|| {
                    v.get("result")
                        .and_then(|v| v.get("data").or_else(|| v.get("images")))
                })
            })
            .and_then(Value::as_array)
            .filter(|images| {
                !images.is_empty()
                    && images.iter().all(|image| {
                        ["url", "b64_json"].iter().any(|field| {
                            image
                                .get(*field)
                                .and_then(Value::as_str)
                                .is_some_and(|s| !s.trim().is_empty())
                        })
                    })
            })
            .map(|images| Decimal::from(images.len() as u64))
    } else {
        None
    };
    let Some(quantity) = quantity else {
        return unknown("missing_quantity");
    };
    if matches!(config.unit, Unit::Image | Unit::Character) && quantity.fract() != Decimal::ZERO {
        return unknown("invalid_quantity");
    };
    let Some(price) = config.price_per_unit.and_then(decimal) else {
        return unknown("missing_price");
    };
    let Some(cost) = quantity.checked_mul(price) else {
        return unknown("amount_out_of_range");
    };
    if cost.is_zero() && !quantity.is_zero() && !price.is_zero() {
        return unknown("amount_out_of_range");
    }
    Bill {
        cost: cost.to_f64(),
        details: json!({"unit":config.unit,"quantity":quantity.normalize().to_string(),"price_per_unit":price.normalize().to_string(),"amount_usd":cost.normalize().to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uses_actual_images_and_explicit_quantities() {
        let config = Config {
            unit: Unit::Image,
            price_per_unit: Some(0.04),
            ..Default::default()
        };
        let response =
            json!({"n":100,"data":[{"url":"https://example.test/a"},{"b64_json":"fixture"}]});
        assert_eq!(
            calculate(&config, None, None, 0, 0, false, Some(&response), true).cost,
            Some(0.08)
        );
        for value in [
            json!({"n":100}),
            json!({"data":[{}]}),
            json!({"error":"failed"}),
        ] {
            assert!(
                calculate(&config, None, None, 0, 0, false, Some(&value), true)
                    .cost
                    .is_none()
            );
        }
        assert!(
            calculate(&config, None, None, 0, 0, false, Some(&response), false)
                .cost
                .is_none()
        );
        for (unit, pointer, value, cost) in [
            (
                Unit::Second,
                "/usage/seconds",
                json!({"usage":{"seconds":2.5}}),
                0.1,
            ),
            (
                Unit::Character,
                "/usage/characters",
                json!({"usage":{"characters":100}}),
                4.0,
            ),
        ] {
            let cfg = Config {
                unit,
                quantity_pointer: pointer.into(),
                ..config.clone()
            };
            assert_eq!(
                calculate(&cfg, None, None, 0, 0, false, Some(&value), true).cost,
                Some(cost)
            );
            assert!(calculate(
                &cfg,
                None,
                None,
                0,
                0,
                false,
                Some(&json!({"usage":{}})),
                true
            )
            .cost
            .is_none());
        }
        let cfg = Config {
            unit: Unit::Request,
            ..config
        };
        assert_eq!(
            calculate(&cfg, None, None, 0, 0, false, None, true).cost,
            Some(0.04)
        );
        assert!(calculate(&cfg, None, None, 0, 0, false, None, false)
            .cost
            .is_none());
    }
    #[test]
    fn decimal_prices_free_and_missing_are_distinct() {
        let cfg = Config {
            unit: Unit::Image,
            price_per_unit: Some(0.1),
            ..Default::default()
        };
        let images = json!({"data":[{"url":"a"},{"url":"b"},{"url":"c"}]});
        let bill = calculate(&cfg, None, None, 0, 0, false, Some(&images), true);
        assert_eq!(bill.cost, Some(0.3));
        assert_eq!(bill.details["amount_usd"], "0.3");
        for (price, cost) in [(None, None), (Some(0.0), Some(0.0))] {
            assert_eq!(
                calculate(
                    &Config {
                        price_per_unit: price,
                        ..cfg.clone()
                    },
                    None,
                    None,
                    0,
                    0,
                    false,
                    Some(&images),
                    true
                )
                .cost,
                cost
            );
        }
        let bill = calculate(
            &Config::default(),
            Some(2.5),
            Some(10.0),
            1000,
            500,
            true,
            None,
            true,
        );
        assert_eq!(bill.cost, Some(0.0075));
        assert!(calculate(
            &Config::default(),
            Some(2.5),
            Some(10.0),
            1000,
            0,
            false,
            None,
            true
        )
        .cost
        .is_none());
        assert_eq!(
            calculate(&Config::default(), None, None, 0, 0, true, None, true).cost,
            Some(0.0)
        );
        let cfg = Config {
            unit: Unit::Character,
            price_per_unit: Some(1.0),
            quantity_pointer: "/count".into(),
        };
        let bill = calculate(
            &cfg,
            None,
            None,
            0,
            0,
            false,
            Some(&json!({"count":9007199254740993_u64})),
            true,
        );
        assert_eq!(bill.details["quantity"], "9007199254740993");
        assert_eq!(bill.details["amount_usd"], "9007199254740993");
        let bill = calculate(
            &Config::default(),
            Some(1e-28),
            None,
            1,
            0,
            true,
            None,
            true,
        );
        assert!(bill.cost.is_none());
        assert_eq!(bill.details["unpriced_reason"], "amount_out_of_range");
    }
    #[test]
    fn rejects_unsafe_or_incomplete_configuration() {
        for value in [
            json!({"unit":"second"}),
            json!({"unit":"image","price_per_unit":-1}),
            json!({"unit":"image","quantity_pointer":"usage/n"}),
            json!({"unit":"image","quantity_pointer":"/~broken"}),
            json!({"unit":"request","quantity_pointer":"/n"}),
            json!({"unit":"image","price_per_unit":1e-29}),
            json!({"unit":"unknown"}),
            json!({"extra":1}),
        ] {
            assert!(Config::stored(&value.to_string()).is_err());
        }
    }
}
