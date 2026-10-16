//! Client ownership is separate from upstream credential affinity.
//! Store only hashed IDs, never conversation contents or authentication tokens.
use crate::{
    console::{GatewayKey, UserModelRoute},
    error::{AppError, AppResult},
    state::AppState,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const TTL: i64 = 86400;
fn hash(id: &str) -> AppResult<String> {
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(AppError::BadRequest("资源 ID 格式无效".into()));
    }
    Ok(hex::encode(Sha256::digest(id.as_bytes())))
}

fn references_file(item: &Value) -> bool {
    let has = |pointer| item.pointer(pointer).is_some_and(|value| !value.is_null());
    (matches!(
        item["type"].as_str(),
        Some("input_file" | "input_image" | "file")
    ) && (has("/file_id") || has("/file/file_id")))
        || (item.pointer("/source/type").and_then(Value::as_str) == Some("file")
            && has("/source/file_id"))
}

pub async fn validate(
    state: &AppState,
    key: &GatewayKey,
    model: &UserModelRoute,
    payload: &Value,
) -> AppResult<()> {
    // JunoHub has no ownership-aware Files/Conversations/Vector Stores API yet.
    // Raw IDs from a shared supplier credential must never expose its resources.
    // Inspect protocol-defined resource fields, not arbitrary function schemas,
    // structured outputs or user tool data that happen to use the same names.
    let conversation = &payload["conversation"];
    let mut external = conversation.is_string() || conversation.get("id").is_some();
    if let Some(tools) = payload["tools"].as_array() {
        for tool in tools {
            if tool["type"] == "file_search" {
                external |= tool
                    .get("vector_store_ids")
                    .is_some_and(|ids| ids.as_array().is_none_or(|ids| !ids.is_empty()));
            }
            if tool["type"] == "code_interpreter" {
                let container = &tool["container"];
                external |= container.as_str().is_some_and(|id| id != "auto")
                    || container.get("id").is_some()
                    || container
                        .get("file_ids")
                        .is_some_and(|ids| ids.as_array().is_none_or(|ids| !ids.is_empty()));
            }
        }
    }
    for name in ["input", "messages"] {
        if let Some(items) = payload[name].as_array() {
            for item in items {
                external |= references_file(item);
                if let Some(content) = item["content"].as_array() {
                    external |= content.iter().any(references_file);
                }
            }
        }
    }
    if external {
        crate::observability::security_rejection(
            crate::observability::SecurityReason::UnmanagedResource,
        );
        return Err(AppError::BadRequest("暂不支持引用供应商已有的文件、会话、容器或向量库 ID；文件请使用 URL、Base64 或受支持的 multipart 接口".into()));
    }
    let mut refs = vec![];
    if let Some(value) = payload.get("previous_response_id").filter(|v| !v.is_null()) {
        refs.push((
            "response",
            value
                .as_str()
                .ok_or_else(|| AppError::BadRequest("previous_response_id 必须是文本".into()))?,
        ));
    }
    if let Some(items) = payload.get("input").and_then(Value::as_array) {
        for item in items {
            if item["type"] == "item_reference" {
                refs.push((
                    "item",
                    item["id"]
                        .as_str()
                        .ok_or_else(|| AppError::BadRequest("item_reference 需要 id".into()))?,
                ));
            }
        }
    }
    if refs.len() > 128 {
        return Err(AppError::BadRequest("资源引用数量不能超过 128".into()));
    }
    for (kind, id) in refs {
        let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM gateway_resource_owners WHERE provider_id=? AND model_id=? AND kind=? AND resource_hash=? AND user_id=? AND key_id=? AND expires_at>?)")
            .bind(model.provider_id).bind(model.id).bind(kind).bind(hash(id)?).bind(key.user_id).bind(key.id).bind(crate::store::now())
            .fetch_one(&state.pool).await?;
        if !owned {
            crate::observability::security_rejection(
                crate::observability::SecurityReason::UnownedResource,
            );
            return Err(AppError::Forbidden(
                "此响应或输入项不属于当前访问密钥，或已过期；请重新开始对话".into(),
            ));
        }
    }
    Ok(())
}

async fn remember(
    state: &AppState,
    key: &GatewayKey,
    model: &UserModelRoute,
    ids: Vec<(&str, &str)>,
) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let now = crate::store::now();
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM gateway_resource_owners WHERE rowid IN (SELECT rowid FROM gateway_resource_owners WHERE expires_at<=? LIMIT 512)")
        .bind(now).execute(&mut *tx).await?;
    for (kind, id) in ids {
        let hashed =
            hash(id).map_err(|_| AppError::BadGateway("上游返回了无效的资源 ID".into()))?;
        // An ID collision must never transfer another client's ownership.
        let result = sqlx::query("INSERT INTO gateway_resource_owners(provider_id,model_id,kind,resource_hash,user_id,key_id,expires_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(provider_id,kind,resource_hash) DO UPDATE SET expires_at=excluded.expires_at WHERE model_id=excluded.model_id AND user_id=excluded.user_id AND key_id=excluded.key_id")
            .bind(model.provider_id).bind(model.id).bind(kind).bind(hashed).bind(key.user_id).bind(key.id).bind(now+TTL).execute(&mut *tx).await?;
        if result.rows_affected() == 0 {
            return Err(AppError::BadGateway("上游资源 ID 发生归属冲突".into()));
        }
    }
    tx.commit().await?;
    Ok(())
}

pub async fn remember_json(
    state: &AppState,
    key: &GatewayKey,
    model: &UserModelRoute,
    value: &Value,
) -> AppResult<()> {
    let response = value.get("response").unwrap_or(value);
    let mut ids = vec![];
    if let Some(id) = response
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| id.starts_with("resp_"))
    {
        ids.push(("response", id));
        if let Some(items) = response.get("output").and_then(Value::as_array) {
            if items.len() > 1024 {
                return Err(AppError::BadGateway("上游响应项目过多".into()));
            }
            for item in items {
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    ids.push(("item", id));
                }
            }
        }
    }
    if matches!(
        value["type"].as_str(),
        Some("response.output_item.added" | "response.output_item.done")
    ) {
        if let Some(id) = value.pointer("/item/id").and_then(Value::as_str) {
            ids.push(("item", id));
        }
    }
    remember(state, key, model, ids).await
}

pub async fn remember_sse(
    state: &AppState,
    key: &GatewayKey,
    model: &UserModelRoute,
    frame: &[u8],
) -> AppResult<()> {
    // SSE permits multiple data lines; concatenate them before JSON parsing.
    let data = String::from_utf8_lossy(frame)
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    if let Ok(value) = serde_json::from_str::<Value>(&data) {
        remember_json(state, key, model, &value).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    async fn fixture() -> (AppState, GatewayKey, UserModelRoute) {
        let state = crate::console::tests::state().await;
        sqlx::query("INSERT INTO user_providers(id,user_id,name,provider_type,base_url,encrypted_api_key,created_at,updated_at) VALUES(1,1,'fixture','openai','https://supplier.test','','0','0')").execute(&state.pool).await.unwrap();
        sqlx::query("INSERT INTO user_models(id,user_id,provider_id,alias,upstream_model,created_at,updated_at) VALUES(1,1,1,'fixture','fixture',0,0)").execute(&state.pool).await.unwrap();
        for (id, user) in [(1, 1), (2, 2), (3, 1)] {
            sqlx::query("INSERT INTO virtual_keys(id,user_id,name,key_prefix,key_hash,created_at,updated_at) VALUES(?,?,'fixture','fixture',?,0,0)").bind(id).bind(user).bind(format!("hash-{id}")).execute(&state.pool).await.unwrap();
        }
        let key = GatewayKey {
            id: 1,
            user_id: 1,
            allowed_models: vec![],
            expires_at: 0,
        };
        let model = UserModelRoute {
            id: 1,
            owner_id: 1,
            alias: "fixture".into(),
            upstream_model: "fixture".into(),
            request_path: String::new(),
            request_id: String::new(),
            api_config: Default::default(),
            provider_id: 1,
            input_price_per_million: None,
            output_price_per_million: None,
            billing_config: Default::default(),
        };
        (state, key, model)
    }
    #[tokio::test]
    async fn ownership_is_bound_to_client_user_key_provider_and_route() {
        let (state, key, model) = fixture().await;
        let request = json!({"previous_response_id":"resp_private","input":[{"type":"item_reference","id":"msg_private"}]});
        assert!(validate(&state, &key, &model, &request).await.is_err());
        remember_json(
            &state,
            &key,
            &model,
            &json!({"id":"resp_private","output":[{"id":"msg_private"}]}),
        )
        .await
        .unwrap();
        validate(&state, &key, &model, &request).await.unwrap();
        for other in [
            GatewayKey {
                id: 2,
                user_id: 2,
                ..key.clone()
            },
            GatewayKey {
                id: 3,
                ..key.clone()
            },
        ] {
            assert!(matches!(
                validate(&state, &other, &model, &request).await,
                Err(AppError::Forbidden(_))
            ));
            assert!(
                remember_json(&state, &other, &model, &json!({"id":"resp_private"}))
                    .await
                    .is_err()
            );
        }
        for other in [
            UserModelRoute {
                id: 2,
                ..model.clone()
            },
            UserModelRoute {
                provider_id: 2,
                ..model.clone()
            },
        ] {
            assert!(validate(&state, &key, &other, &request).await.is_err());
        }
        let stored: String = sqlx::query_scalar(
            "SELECT resource_hash FROM gateway_resource_owners WHERE kind='response'",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert!(!stored.contains("resp_private"));
        sqlx::query("UPDATE gateway_resource_owners SET expires_at=0")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(validate(&state, &key, &model, &request).await.is_err());
    }
    #[tokio::test]
    async fn unmanaged_supplier_resources_cannot_be_read_through_shared_credentials() {
        let (state, key, model) = fixture().await;
        for payload in [
            json!({"conversation":"conv_other"}),
            json!({"input":[{"type":"message","content":[{"type":"input_file","file_id":"file_private"}]}]}),
            json!({"tools":[{"type":"file_search","vector_store_ids":["vs_private"]}]}),
            json!({"tools":[{"type":"code_interpreter","container":"cntr_other"}]}),
        ] {
            assert!(validate(&state, &key, &model, &payload).await.is_err());
        }
        validate(&state,&key,&model,&json!({"input":[{"type":"input_file","file_url":"https://example.test/public.pdf"}],"tools":[{"type":"code_interpreter","container":{"type":"auto"}}]})).await.unwrap();
    }
    #[tokio::test]
    async fn user_function_schemas_and_tool_outputs_are_not_supplier_resources() {
        let (state, key, model) = fixture().await;
        validate(&state,&key,&model,&json!({"tools":[{"type":"function","name":"read_file","parameters":{"type":"object","properties":{"file_id":{"type":"string"},"conversation":{"type":"string"},"container":{"type":"object","properties":{"id":{"type":"string"}}}}}}],"input":[{"type":"function_call_output","call_id":"call_fixture","output":{"file_id":"user-value"}}]})).await.unwrap();
    }

    #[tokio::test]
    async fn sse_created_and_output_item_events_register_before_continuation() {
        let (state, key, model) = fixture().await;
        remember_sse(&state,&key,&model,b"event: response.created\r\ndata: {\"type\":\"response.created\",\r\ndata: \"response\":{\"id\":\"resp_stream\"}}").await.unwrap();
        remember_sse(
            &state,
            &key,
            &model,
            b"data: {\"type\":\"response.output_item.added\",\"item\":{\"id\":\"msg_stream\"}}",
        )
        .await
        .unwrap();
        validate(&state,&key,&model,&json!({"previous_response_id":"resp_stream","input":[{"type":"item_reference","id":"msg_stream"}]})).await.unwrap();
        assert!(
            validate(&state, &key, &model, &json!({"previous_response_id":3}))
                .await
                .is_err()
        );
        assert!(validate(
            &state,
            &key,
            &model,
            &json!({"previous_response_id":"../private"})
        )
        .await
        .is_err());
    }
}
