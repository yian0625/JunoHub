use crate::{
    error::{AppError, AppResult},
    state::AppState,
    store,
};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

pub async fn read(state: &AppState) -> AppResult<Value> {
    read_pool(&state.pool).await
}
async fn read_pool(pool: &SqlitePool) -> AppResult<Value> {
    let row = sqlx::query("SELECT content,revision,updated_at FROM access_guide WHERE id=1")
        .fetch_one(pool)
        .await?;
    let revision: i64 = row.get("revision");
    let content = if revision == 0 {
        include_str!("access-guide.md").to_owned()
    } else {
        row.get::<String, _>("content")
    };
    Ok(json!({"content":content,"template_content":include_str!("access-guide.md"),"revision":revision,"updated_at":row.get::<i64,_>("updated_at")}))
}
pub async fn save(state: &AppState, payload: &Value) -> AppResult<Value> {
    save_pool(&state.pool, payload).await
}
async fn save_pool(pool: &SqlitePool, payload: &Value) -> AppResult<Value> {
    let content = payload["content"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 256 * 1024)
        .ok_or_else(|| AppError::BadRequest("文档不能为空，且不能超过 256KB".into()))?;
    let revision = payload["revision"]
        .as_i64()
        .filter(|v| *v >= 0)
        .ok_or_else(|| AppError::BadRequest("文档版本无效，请重新加载".into()))?;
    let timestamp = store::now();
    let result = sqlx::query("UPDATE access_guide SET content=?,revision=revision+1,updated_at=? WHERE id=1 AND revision=?")
        .bind(content).bind(timestamp).bind(revision).execute(pool).await?;
    if result.rows_affected() == 0 {
        return Err(AppError::BadRequest(
            "文档已被其他管理员更新，请保留草稿并重新加载最新版本".into(),
        ));
    }
    // Return this save's version, even if another administrator immediately saves again.
    Ok(json!({"content":content,"template_content":include_str!("access-guide.md"),"revision":revision+1,"updated_at":timestamp}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn access_guide_persists_markdown_and_rejects_stale_versions() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE user_models(id INTEGER PRIMARY KEY);")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../migrations/0030_model_request_path_and_guide.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        assert!(read_pool(&pool).await.unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("接入指引"));
        let content = "# 自定义指引\n\n**测试**\n";
        save_pool(&pool, &json!({"content":content,"revision":0}))
            .await
            .unwrap();
        assert_eq!(read_pool(&pool).await.unwrap()["content"], content);
        assert!(read_pool(&pool).await.unwrap()["template_content"].as_str().unwrap().contains("后台会持续查询"));
        assert!(
            save_pool(&pool, &json!({"content":"# 过期草稿","revision":0}))
                .await
                .is_err()
        );
        assert!(save_pool(&pool, &json!({"content":" ","revision":1}))
            .await
            .is_err());
        assert_eq!(read_pool(&pool).await.unwrap()["content"], content);
    }
}
