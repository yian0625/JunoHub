//! Bounded multipart forwarding. Files stay in memory and never enter logs or OSS.
use crate::error::{AppError, AppResult};
use axum::http::{header, HeaderMap, Method};
use bytes::Bytes;
use serde_json::{json, Value};

pub const MAX_BODY: usize = 32 * 1024 * 1024;
struct Field {
    name: String,
    filename: Option<String>,
    headers: HeaderMap,
    bytes: Bytes,
}
pub struct Upload {
    fields: Vec<Field>,
    pub alias: String,
    streaming: bool,
}
pub fn is_multipart(headers: &HeaderMap) -> bool {
    headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("multipart/form-data"))
}
pub fn check_endpoint(method: &Method, path: &str) -> AppResult<()> {
    if *method != Method::POST || !matches!(path, "/v1/images/edits" | "/v1/audio/transcriptions" | "/v1/audio/translations") {
        return Err(AppError::BadRequest("文件请求请使用 POST /v1/images/edits、/v1/audio/transcriptions 或 /v1/audio/translations".into()));
    }
    Ok(())
}
impl Upload {
    pub async fn parse(headers: &HeaderMap, body: Bytes) -> AppResult<Self> {
        if body.len() > MAX_BODY { return Err(AppError::PayloadTooLarge("请求体不能超过 32MiB（包含所有文件和表单字段）".into())); }
        if headers.get(header::CONTENT_ENCODING).is_some_and(|value| value != "identity") {
            return Err(AppError::BadRequest("文件请求暂不支持压缩 Content-Encoding".into()));
        }
        let content_type = headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok())
            .ok_or_else(|| AppError::BadRequest("文件请求缺少有效 Content-Type".into()))?;
        let boundary = multer::parse_boundary(content_type)
            .map_err(|_| AppError::BadRequest("multipart/form-data 缺少有效 boundary".into()))?;
        if boundary.is_empty() || boundary.len() > 70 {
            return Err(AppError::BadRequest("multipart boundary 必须为 1–70 字节".into()));
        }
        let stream = futures_util::stream::once(async move { Ok::<_, std::convert::Infallible>(body) });
        let mut parser = multer::Multipart::new(stream, boundary);
        let mut fields = Vec::new();
        let mut alias = None;
        let mut streaming = None;
        let mut files = 0;
        while let Some(field) = parser.next_field().await.map_err(|_| malformed())? {
            if fields.len() >= 64 { return Err(AppError::BadRequest("文件请求最多包含 64 个表单项".into())); }
            let name = field.name().filter(|name| !name.is_empty() && name.len() <= 1024)
                .ok_or_else(|| AppError::BadRequest("文件请求包含缺失或过长的字段名称".into()))?.to_owned();
            let filename = field.file_name().map(str::to_owned);
            let mut part_headers = field.headers().clone();
            part_headers.remove(header::CONTENT_DISPOSITION);
            part_headers.remove(header::CONTENT_LENGTH);
            let bytes = field.bytes().await.map_err(|_| malformed())?;
            if name == "model" {
                if alias.is_some() || filename.is_some() || bytes.len() > 200 {
                    return Err(AppError::BadRequest("文件请求必须包含唯一的文本 model 字段，最长 200 字节".into()));
                }
                let model = std::str::from_utf8(&bytes).map_err(|_| AppError::BadRequest("model 必须为 UTF-8 文本".into()))?.trim();
                if model.is_empty() { return Err(AppError::BadRequest("model 不能为空".into())); }
                alias = Some(model.to_owned());
            } else if name == "stream" {
                if streaming.is_some() || filename.is_some() {
                    return Err(AppError::BadRequest("stream 必须是唯一的文本字段".into()));
                }
                streaming = Some(match bytes.as_ref() {
                    b"true" => true,
                    b"false" => false,
                    _ => return Err(AppError::BadRequest("stream 必须为 true 或 false".into())),
                });
            }
            if filename.is_some() {
                if bytes.is_empty() { return Err(AppError::BadRequest("上传文件不能为空".into())); }
                files += 1;
            } else if bytes.len() > 1024 * 1024 {
                return Err(AppError::BadRequest("单个文本表单字段不能超过 1MiB".into()));
            }
            fields.push(Field { name, filename, headers: part_headers, bytes });
        }
        let alias = alias.ok_or_else(|| AppError::BadRequest("文件请求缺少 model 字段，请填写 JunoHub 模型别名".into()))?;
        if files == 0 { return Err(AppError::BadRequest("文件请求必须包含至少一个非空文件".into())); }
        Ok(Self { fields, alias, streaming: streaming.unwrap_or(false) })
    }
    /// Only routing metadata is represented as JSON; uploaded bytes are not converted.
    pub fn metadata(&self) -> Value { json!({"model":self.alias,"stream":self.streaming}) }
    pub fn form(&self, upstream_model: &str) -> reqwest::multipart::Form {
        let mut form = reqwest::multipart::Form::new();
        for field in &self.fields {
            let bytes = if field.name == "model" { Bytes::from(upstream_model.to_owned()) } else { field.bytes.clone() };
            let len = bytes.len() as u64;
            let mut part = reqwest::multipart::Part::stream_with_length(bytes, len).headers(field.headers.clone());
            if let Some(filename) = &field.filename { part = part.file_name(filename.clone()); }
            form = form.part(field.name.clone(), part);
        }
        form
    }
}
fn malformed() -> AppError { AppError::BadRequest("文件表单不完整或格式无效，请重新选择文件后提交".into()) }

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn body(parts: &[(&str, Option<&str>, &[u8])]) -> (HeaderMap, Bytes) {
        let mut data = Vec::new();
        for (name, filename, bytes) in parts {
            data.extend_from_slice(format!("--fixture-boundary\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes());
            if let Some(filename) = filename { data.extend_from_slice(format!("; filename=\"{filename}\"\r\nContent-Type: application/octet-stream").as_bytes()); }
            data.extend_from_slice(b"\r\n\r\n"); data.extend_from_slice(bytes); data.extend_from_slice(b"\r\n");
        }
        data.extend_from_slice(b"--fixture-boundary--\r\n");
        let mut headers = HeaderMap::new(); headers.insert(header::CONTENT_TYPE, "multipart/form-data; boundary=fixture-boundary".parse().unwrap());
        (headers, Bytes::from(data))
    }
    #[tokio::test]
    async fn validates_model_files_and_ambiguous_forms() {
        let valid = [("model",None,b"call-alias".as_slice()),("image[]",Some("a.png"),b"\0\xffimage".as_slice()),("image[]",Some("b.png"),b"second".as_slice()),("prompt",None,b"edit image".as_slice())];
        let (headers, bytes) = body(&valid);
        let upload = Upload::parse(&headers,bytes).await.unwrap();
        assert_eq!(upload.alias,"call-alias"); assert_eq!(upload.fields.len(),4);
        assert_eq!(upload.fields[1].bytes.as_ref(),b"\0\xffimage");
        assert_eq!(upload.fields[1].headers[header::CONTENT_TYPE],"application/octet-stream");
        for parts in [vec![valid[1]],vec![valid[0]],vec![valid[0],valid[0],valid[1]],vec![("model",Some("file"),b"alias".as_slice()),valid[1]],vec![("model",None,b"\xff".as_slice()),valid[1]],vec![valid[0],("file",Some("empty"),b"".as_slice())],vec![valid[0],valid[1],("stream",None,b"yes".as_slice())]] {
            let (headers,bytes)=body(&parts); assert!(Upload::parse(&headers,bytes).await.is_err());
        }
        let (headers,bytes)=body(&valid);
        assert!(Upload::parse(&headers,bytes.slice(..bytes.len()-25)).await.is_err());
        assert!(check_endpoint(&Method::POST,"/v1/chat/completions").is_err());
        assert!(check_endpoint(&Method::GET,"/v1/images/edits").is_err());
    }
    #[tokio::test]
    async fn rejects_oversize_and_missing_boundaries() {
        let (mut headers,_)=body(&[]);
        assert!(matches!(Upload::parse(&headers,Bytes::from(vec![0;MAX_BODY+1])).await,Err(AppError::PayloadTooLarge(_))));
        headers.insert(header::CONTENT_TYPE,"multipart/form-data".parse().unwrap());
        assert!(Upload::parse(&headers,Bytes::new()).await.is_err());
    }
}
