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
        path => {
            return Err(AppError::NotFound(format!(
                "管理员接口不存在: /hub_admin/{path}"
            )))
        }
    };
    Ok(Json(Envelope::ok(data)))
}
