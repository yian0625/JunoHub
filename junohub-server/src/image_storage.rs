//! Avatar bytes live in OSS; stable application URLs work with private buckets.
use std::{collections::BTreeMap, env, fmt, time::Duration};

use axum::{
    body::Body,
    extract::{Path, State},
    http::header,
    response::Response,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    state::AppState,
    store,
};

const MAX_BYTES: usize = 256 * 1024;
const URL_PREFIX: &str = "/media/avatars/";

#[derive(Clone)]
pub struct OssConfig {
    pub bucket: String,
    pub region: String,
    pub endpoint: String,
    pub prefix: String,
    pub access_key_id: String,
    pub access_key_secret: String,
    pub security_token: String,
}

impl Default for OssConfig {
    fn default() -> Self {
        Self {
            bucket: "juno-app".into(),
            region: String::new(),
            endpoint: String::new(),
            prefix: "JunoHub".into(),
            access_key_id: String::new(),
            access_key_secret: String::new(),
            security_token: String::new(),
        }
    }
}

impl fmt::Debug for OssConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OssConfig")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("prefix", &self.prefix)
            .field("credentials", &"[redacted]")
            .finish()
    }
}

impl OssConfig {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            bucket: env::var("OSS_BUCKET")
                .unwrap_or(defaults.bucket)
                .trim()
                .into(),
            region: env::var("OSS_REGION").unwrap_or_default().trim().into(),
            endpoint: env::var("OSS_ENDPOINT").unwrap_or_default().trim().into(),
            prefix: env::var("OSS_PREFIX")
                .unwrap_or(defaults.prefix)
                .trim_matches('/')
                .into(),
            access_key_id: env::var("OSS_ACCESS_KEY_ID")
                .unwrap_or_default()
                .trim()
                .into(),
            access_key_secret: env::var("OSS_ACCESS_KEY_SECRET")
                .unwrap_or_default()
                .trim()
                .into(),
            security_token: env::var("OSS_SECURITY_TOKEN")
                .unwrap_or_default()
                .trim()
                .into(),
        }
    }

    fn object_url(&self, key: &str) -> AppResult<url::Url> {
        if self.region.is_empty()
            || self.access_key_id.is_empty()
            || self.access_key_secret.is_empty()
        {
            return Err(AppError::ServiceUnavailable(
                "图片存储尚未配置：请设置 OSS_REGION、OSS_ACCESS_KEY_ID 和 OSS_ACCESS_KEY_SECRET"
                    .into(),
            ));
        }
        let safe_part = |value: &str| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        if !safe_part(&self.bucket)
            || !safe_part(&self.region)
            || self
                .prefix
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(AppError::ServiceUnavailable(
                "OSS Bucket、地域或目录配置无效".into(),
            ));
        }
        let endpoint = if self.endpoint.is_empty() {
            format!("https://oss-{}.aliyuncs.com", self.region)
        } else if self.endpoint.contains("://") {
            self.endpoint.clone()
        } else {
            format!("https://{}", self.endpoint)
        };
        let mut url = url::Url::parse(&endpoint)
            .map_err(|_| AppError::ServiceUnavailable("OSS Endpoint 配置无效".into()))?;
        #[cfg(test)]
        if url.scheme() == "http" && url.host_str() == Some("127.0.0.1") {
            url.set_path(&format!("/{}/{key}", self.bucket));
            return Ok(url);
        }
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(AppError::ServiceUnavailable(
                "OSS Endpoint 必须是 HTTPS 地域地址".into(),
            ));
        }
        let host = url.host_str().unwrap_or_default();
        if !host.starts_with("oss-") || !host.ends_with(".aliyuncs.com") {
            return Err(AppError::ServiceUnavailable(
                "OSS Endpoint 必须是阿里云 OSS 地域地址".into(),
            ));
        }
        url.set_host(Some(&format!("{}.{}", self.bucket, host)))
            .map_err(|_| AppError::ServiceUnavailable("OSS Bucket 配置无效".into()))?;
        url.set_path(&format!("/{key}"));
        Ok(url)
    }

    fn canonical_request(
        &self,
        method: &str,
        key: &str,
        headers: &BTreeMap<String, String>,
        additional: &str,
    ) -> String {
        // OSS V4 signs /bucket/key even when the HTTP request uses a bucket hostname.
        let canonical_uri = encode_path(&format!("/{}/{key}", self.bucket));
        let canonical_headers: String = headers
            .iter()
            .map(|(k, v)| format!("{k}:{}\n", v.trim()))
            .collect();
        format!("{method}\n{canonical_uri}\n\n{canonical_headers}\n{additional}\nUNSIGNED-PAYLOAD")
    }

    fn authorization(
        &self,
        method: &str,
        key: &str,
        timestamp: &str,
        headers: &BTreeMap<String, String>,
        additional: &str,
    ) -> String {
        let canonical = self.canonical_request(method, key, headers, additional);
        let date = &timestamp[..8];
        let scope = format!("{date}/{}/oss/aliyun_v4_request", self.region);
        let to_sign = format!(
            "OSS4-HMAC-SHA256\n{timestamp}\n{scope}\n{:x}",
            Sha256::digest(canonical)
        );
        let mut signing_key = format!("aliyun_v4{}", self.access_key_secret).into_bytes();
        for part in [date, &self.region, "oss", "aliyun_v4_request"] {
            signing_key = hmac(&signing_key, part);
        }
        let extra = if additional.is_empty() {
            String::new()
        } else {
            format!("AdditionalHeaders={additional},")
        };
        format!(
            "OSS4-HMAC-SHA256 Credential={}/{scope},{extra}Signature={}",
            self.access_key_id,
            hex::encode(hmac(&signing_key, &to_sign))
        )
    }

    async fn request(
        &self,
        method: reqwest::Method,
        key: &str,
        image: Option<&AvatarImage>,
    ) -> AppResult<reqwest::Response> {
        let url = self.object_url(key)?;
        let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let mut headers = BTreeMap::from([
            ("x-oss-content-sha256".into(), "UNSIGNED-PAYLOAD".into()),
            ("x-oss-date".into(), timestamp.clone()),
        ]);
        if !self.security_token.is_empty() {
            headers.insert("x-oss-security-token".into(), self.security_token.clone());
        }
        if let Some(image) = image {
            headers.insert("content-type".into(), image.content_type.into());
            headers.insert("x-oss-forbid-overwrite".into(), "true".into());
        }
        let auth = self.authorization(method.as_str(), key, &timestamp, &headers, "");
        // Signed credentials must not follow redirects to another object or origin.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| AppError::Internal("无法初始化图片存储连接".into()))?;
        let mut request = client.request(method, url).header("authorization", auth);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        if let Some(image) = image {
            request = request.body(image.bytes.clone());
        }
        let response = request.send().await.map_err(|e| {
            AppError::ServiceUnavailable(
                if e.is_timeout() {
                    "连接阿里云图片存储超时，请重试"
                } else {
                    "无法连接阿里云图片存储，请检查 Endpoint 和网络"
                }
                .into(),
            )
        })?;
        if !response.status().is_success() {
            let status = response.status();
            tracing::warn!(status = status.as_u16(), request_id = ?response.headers().get("x-oss-request-id"), "OSS avatar request failed");
            let detail = match status.as_u16() {
                403 => "请检查 OSS 凭据和 Bucket 读写权限",
                404 => "Bucket 或图片不存在，请检查 OSS 配置",
                301 | 302 | 307 | 308 => "请检查 Bucket 所在地域和 Endpoint",
                _ => "请稍后重试或检查 OSS 配置",
            };
            return Err(AppError::ServiceUnavailable(format!(
                "阿里云图片存储请求失败（HTTP {status}）：{detail}"
            )));
        }
        Ok(response)
    }
}

fn hmac(key: &[u8], value: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(value.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn encode_path(value: &str) -> String {
    let mut result = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            result.push(b as char);
        } else {
            result.push_str(&format!("%{b:02X}"));
        }
    }
    result
}

struct AvatarImage {
    bytes: Vec<u8>,
    content_type: &'static str,
    extension: &'static str,
}

fn decode_avatar(value: &str) -> AppResult<AvatarImage> {
    let (content_type, extension, encoded) = [
        ("image/webp", "webp"),
        ("image/png", "png"),
        ("image/jpeg", "jpg"),
    ]
    .into_iter()
    .find_map(|(mime, ext)| {
        value
            .strip_prefix(&format!("data:{mime};base64,"))
            .map(|encoded| (mime, ext, encoded))
    })
    .ok_or_else(|| AppError::BadRequest("头像必须是 WebP、PNG 或 JPEG 图片".into()))?;
    if encoded.len() > 350_000 {
        return Err(AppError::BadRequest("头像不能超过 256KB".into()));
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| AppError::BadRequest("头像数据无效".into()))?;
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(AppError::BadRequest("头像不能超过 256KB".into()));
    }
    let valid = match content_type {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
        "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        _ => false,
    };
    if !valid {
        return Err(AppError::BadRequest("头像文件内容与图片格式不符".into()));
    }
    Ok(AvatarImage {
        bytes,
        content_type,
        extension,
    })
}

#[cfg(test)]
pub fn validate_avatar(value: &str) -> AppResult<()> {
    if value.is_empty() {
        Ok(())
    } else {
        decode_avatar(value).map(|_| ())
    }
}

#[derive(Clone, Copy)]
pub enum AvatarPurpose {
    User,
    Provider,
}
impl AvatarPurpose {
    fn directory(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Provider => "provider",
        }
    }
}

pub async fn persist_avatar(
    state: &AppState,
    uid: i64,
    purpose: AvatarPurpose,
    value: &str,
) -> AppResult<String> {
    if value.is_empty() {
        return Ok(String::new());
    }
    if let Some(id) = value.strip_prefix(URL_PREFIX) {
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM image_assets WHERE id=? AND user_id=? AND purpose=?)",
        )
        .bind(id)
        .bind(uid)
        .bind(purpose.directory())
        .fetch_one(&state.pool)
        .await?;
        if !owned {
            return Err(AppError::BadRequest("图片不存在或不属于当前账户".into()));
        }
        return Ok(value.into());
    }
    let image = decode_avatar(value)?;
    let id = Uuid::new_v4().to_string();
    let object_key = format!(
        "{}/{}/{uid}/{id}.{}",
        state.config.oss.prefix,
        purpose.directory(),
        image.extension
    );
    state
        .config
        .oss
        .request(reqwest::Method::PUT, &object_key, Some(&image))
        .await?;
    sqlx::query("INSERT INTO image_assets(id,user_id,purpose,object_key,content_type,byte_size,created_at) VALUES(?,?,?,?,?,?,?)")
        .bind(&id).bind(uid).bind(purpose.directory()).bind(&object_key).bind(image.content_type)
        .bind(image.bytes.len() as i64).bind(store::now()).execute(&state.pool).await?;
    Ok(format!("{URL_PREFIX}{id}"))
}

pub async fn migrate_legacy_avatars(state: &AppState) -> AppResult<usize> {
    // Existing deployments without OSS remain readable until storage is configured.
    if state.config.oss.region.is_empty()
        || state.config.oss.access_key_id.is_empty()
        || state.config.oss.access_key_secret.is_empty()
    {
        return Ok(0);
    }
    let rows = sqlx::query("SELECT 'user' AS purpose,id,id AS user_id,avatar_url FROM users WHERE avatar_url LIKE 'data:image/%' UNION ALL SELECT 'provider' AS purpose,id,user_id,avatar_url FROM user_providers WHERE avatar_url LIKE 'data:image/%'")
        .fetch_all(&state.pool).await?;
    let mut migrated = 0;
    for row in rows {
        let uid: i64 = row.get("user_id");
        let id: i64 = row.get("id");
        let purpose: String = row.get("purpose");
        let old: String = row.get("avatar_url");
        let kind = if purpose == "user" {
            AvatarPurpose::User
        } else {
            AvatarPurpose::Provider
        };
        match persist_avatar(state, uid, kind, &old).await {
            Ok(avatar_url) => {
                let statement = if purpose == "user" {
                    "UPDATE users SET avatar_url=? WHERE id=? AND avatar_url=?"
                } else {
                    "UPDATE user_providers SET avatar_url=? WHERE id=? AND avatar_url=?"
                };
                migrated += sqlx::query(statement)
                    .bind(avatar_url)
                    .bind(id)
                    .bind(old)
                    .execute(&state.pool)
                    .await?
                    .rows_affected() as usize;
            }
            Err(error) => {
                tracing::warn!(%purpose, id, %error, "legacy avatar migration failed; original avatar retained")
            }
        }
    }
    Ok(migrated)
}

pub async fn serve_avatar(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    if Uuid::parse_str(&id).is_err() {
        return Err(AppError::NotFound("图片不存在".into()));
    }
    let avatar_url = format!("{URL_PREFIX}{id}");
    // Only saved, currently referenced avatars can be read through opaque image URLs.
    let row = sqlx::query("SELECT object_key,content_type,byte_size FROM image_assets WHERE id=? AND (EXISTS(SELECT 1 FROM users WHERE avatar_url=?) OR EXISTS(SELECT 1 FROM user_providers WHERE avatar_url=?))")
        .bind(&id).bind(&avatar_url).bind(&avatar_url).fetch_optional(&state.pool).await?
        .ok_or_else(|| AppError::NotFound("图片不存在".into()))?;
    let key: String = row.get("object_key");
    let mut response = state
        .config
        .oss
        .request(reqwest::Method::GET, &key, None)
        .await?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AppError::ServiceUnavailable("读取阿里云图片失败，请重试".into()))?
    {
        if bytes.len() + chunk.len() > MAX_BYTES {
            return Err(AppError::ServiceUnavailable(
                "图片存储中的文件大小异常".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as i64 != row.get::<i64, _>("byte_size") {
        return Err(AppError::ServiceUnavailable(
            "图片存储中的文件不完整".into(),
        ));
    }
    Response::builder()
        .header(header::CONTENT_TYPE, row.get::<String, _>("content_type"))
        .header(header::CACHE_CONTROL, "private, max-age=3600")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(bytes))
        .map_err(|_| AppError::Internal("无法返回图片".into()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::{
        body::Bytes,
        http::{HeaderMap, Method, StatusCode},
        response::IntoResponse,
        routing::any,
        Router,
    };
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
    };

    pub const PNG_AVATAR: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII=";
    type Objects = Arc<Mutex<HashMap<String, (String, Vec<u8>)>>>;
    pub struct MockOss {
        pub config: OssConfig,
        pub objects: Objects,
        pub fail: Arc<AtomicBool>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for MockOss {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    pub async fn mock_oss() -> MockOss {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let objects: Objects = Default::default();
        let fail = Arc::new(AtomicBool::new(false));
        let cloud_objects = objects.clone();
        let cloud_fail = fail.clone();
        let app = Router::new().route(
            "/juno-app/{*key}",
            any(
                move |Path(key): Path<String>, method: Method, headers: HeaderMap, bytes: Bytes| {
                    let objects = cloud_objects.clone();
                    let fail = cloud_fail.clone();
                    async move {
                        assert!(headers
                            .get("authorization")
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .starts_with("OSS4-HMAC-SHA256 Credential=test-id/"));
                        assert_eq!(
                            headers.get("x-oss-content-sha256").unwrap(),
                            "UNSIGNED-PAYLOAD"
                        );
                        if fail.load(Ordering::SeqCst) {
                            return StatusCode::FORBIDDEN.into_response();
                        }
                        if method == Method::PUT {
                            assert_eq!(headers.get("x-oss-forbid-overwrite").unwrap(), "true");
                            objects.lock().unwrap().insert(
                                key,
                                (
                                    headers["content-type"].to_str().unwrap().into(),
                                    bytes.to_vec(),
                                ),
                            );
                            StatusCode::OK.into_response()
                        } else if let Some((mime, body)) =
                            objects.lock().unwrap().get(&key).cloned()
                        {
                            ([(header::CONTENT_TYPE, mime)], body).into_response()
                        } else {
                            StatusCode::NOT_FOUND.into_response()
                        }
                    }
                },
            ),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        MockOss {
            config: OssConfig {
                region: "cn-beijing".into(),
                endpoint: format!("http://{address}"),
                access_key_id: "test-id".into(),
                access_key_secret: "test-secret".into(),
                ..Default::default()
            },
            objects,
            fail,
            server,
        }
    }

    #[test]
    fn oss_v4_canonical_request_and_signature() {
        let config = OssConfig {
            bucket: "examplebucket".into(),
            region: "cn-hangzhou".into(),
            access_key_id: "yourAccessKeyId".into(),
            access_key_secret: "yourAccessKeySecret".into(),
            ..Default::default()
        };
        let headers = BTreeMap::from([
            ("content-disposition".into(), "attachment".into()),
            ("content-length".into(), "3".into()),
            ("content-md5".into(), "ICy5YqxZB1uWSwcVLSNLcA==".into()),
            ("content-type".into(), "text/plain".into()),
            ("x-oss-content-sha256".into(), "UNSIGNED-PAYLOAD".into()),
            ("x-oss-date".into(), "20250411T064124Z".into()),
        ]);
        let auth = config.authorization(
            "PUT",
            "exampleobject",
            "20250411T064124Z",
            &headers,
            "content-disposition;content-length",
        );
        let canonical = config.canonical_request(
            "PUT",
            "exampleobject",
            &headers,
            "content-disposition;content-length",
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(canonical)),
            "c46d96390bdbc2d739ac9363293ae9d710b14e48081fcb22cd8ad54b63136eca"
        );
        // Independently derived from the documented illustrative secret using Python hmac.
        assert!(auth.ends_with(
            "Signature=d3694c2dfc5371ee6acd35e88c4871ac95a7ba01d3a2f476768fe61218590097"
        ));
    }

    #[test]
    fn endpoint_and_credentials_are_safe() {
        let mut config = OssConfig {
            region: "cn-beijing".into(),
            access_key_id: "test-id".into(),
            access_key_secret: "secret-value".into(),
            security_token: "secret-token".into(),
            ..Default::default()
        };
        assert_eq!(
            config
                .object_url("JunoHub/user/1/test.png")
                .unwrap()
                .as_str(),
            "https://juno-app.oss-cn-beijing.aliyuncs.com/JunoHub/user/1/test.png"
        );
        assert!(!format!("{config:?}").contains("secret-value"));
        assert!(!format!("{config:?}").contains("secret-token"));
        config.endpoint = "https://evil.example".into();
        assert!(config.object_url("test").is_err());
        config.endpoint.clear();
        config.prefix = "JunoHub/../other".into();
        assert!(config.object_url("test").is_err());
        assert_eq!(
            encode_path("/bucket/中文 test.png"),
            "/bucket/%E4%B8%AD%E6%96%87%20test.png"
        );
    }

    #[tokio::test]
    #[ignore = "requires explicitly configured OSS credentials; uploads and removes temporary test objects"]
    async fn live_oss_round_trip_in_both_directories() {
        let config = OssConfig::from_env();
        let image = decode_avatar(PNG_AVATAR).unwrap();
        for directory in ["user", "provider"] {
            let key = format!("{}/{directory}/verification-{}.png", config.prefix, Uuid::new_v4());
            config.request(reqwest::Method::PUT, &key, Some(&image)).await.unwrap();
            let read = config.request(reqwest::Method::GET, &key, None).await;
            let result = match read {
                Ok(response) => response.bytes().await.map(|bytes| bytes.as_ref() == image.bytes).unwrap_or(false),
                Err(_) => false,
            };
            // This only removes the fresh UUID object created by this test.
            config.request(reqwest::Method::DELETE, &key, None).await.unwrap();
            assert!(result, "OSS did not return the uploaded image bytes");
        }
    }
}
