//! Centralized REST client for OpenLess sync.
//!
//! UI code calls Tauri commands; request paths, Bearer auth, and backend error
//! handling stay in this module.

use std::time::Duration;

use chrono::Utc;
use reqwest::{Client, Method, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

const REQUEST_TIMEOUT_SECS: u64 = 15;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncApiError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
    pub retryable: bool,
}

impl SyncApiError {
    pub(crate) fn local(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            status: None,
            retryable,
        }
    }

    fn http(status: StatusCode, code: String, message: String) -> Self {
        Self {
            code,
            message,
            status: Some(status.as_u16()),
            retryable: status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error(),
        }
    }
}

impl std::fmt::Display for SyncApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(status) = self.status {
            write!(f, "{}: {} (HTTP {status})", self.code, self.message)
        } else {
            write!(f, "{}: {}", self.code, self.message)
        }
    }
}

impl std::error::Error for SyncApiError {}

#[derive(Debug, Clone)]
pub struct SyncApiClient {
    base_url: String,
    access_token: Option<String>,
    client: Client,
}

impl SyncApiClient {
    pub fn new(
        base_url: impl AsRef<str>,
        access_token: Option<String>,
    ) -> Result<Self, SyncApiError> {
        let base_url = normalize_base_url(base_url.as_ref())?;
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .map_err(|err| {
                SyncApiError::local(
                    "sync_client_unavailable",
                    format!("同步客户端初始化失败：{err}"),
                    false,
                )
            })?;
        Ok(Self {
            base_url,
            access_token,
            client,
        })
    }

    pub async fn request_email_code(
        &self,
        email: String,
    ) -> Result<SyncEmailCodeRequestResult, SyncApiError> {
        let response: EmailCodeRequestResponse = self
            .send_json(
                Method::POST,
                "/auth/email-code/request",
                Some(json!({ "email": email })),
                false,
            )
            .await?;
        Ok(SyncEmailCodeRequestResult {
            ok: response.ok,
            expires_in: response.expires_in,
        })
    }

    pub async fn verify_email_code(
        &self,
        email: String,
        code: String,
        device: SyncDeviceInfo,
    ) -> Result<SyncEmailCodeVerifyResult, SyncApiError> {
        self.send_json(
            Method::POST,
            "/auth/email-code/verify",
            Some(json!({
                "email": email,
                "code": code,
                "device": {
                    "id": device.id,
                    "name": device.name,
                    "platform": device.platform,
                },
            })),
            false,
        )
        .await
    }

    pub async fn refresh_token(
        &self,
        refresh_token: String,
    ) -> Result<SyncTokenRefreshResult, SyncApiError> {
        self.send_json(
            Method::POST,
            "/auth/token/refresh",
            Some(json!({ "refresh_token": refresh_token })),
            false,
        )
        .await
    }

    pub async fn pull(&self, cursor: Option<&str>) -> Result<SyncPullResult, SyncApiError> {
        let cursor = cursor.unwrap_or("").trim();
        let path = if cursor.is_empty() {
            "/sync/pull".to_string()
        } else {
            format!("/sync/pull?cursor={cursor}")
        };
        let response: BackendPullResponse = self.send_json(Method::GET, &path, None, true).await?;
        Ok(response.into())
    }

    pub async fn push(
        &self,
        device_id: String,
        changes: SyncPushChanges,
    ) -> Result<SyncPushResult, SyncApiError> {
        let response: BackendPushResponse = self
            .send_json(
                Method::POST,
                "/sync/push",
                Some(json!({
                    "device_id": device_id,
                    "client_time": Utc::now().to_rfc3339(),
                    "prompts": changes.prompts,
                    "provider_configs": changes.provider_configs,
                    "history_items": changes.history_items,
                    "dictionary_entries": changes.dictionary_entries,
                    "correction_rules": changes.correction_rules,
                    "vocab_presets": changes.vocab_presets,
                })),
                true,
            )
            .await?;
        Ok(SyncPushResult {
            ok: response.ok,
            cursor: response.cursor,
            conflicts_resolved: response.conflicts_resolved,
        })
    }

    pub async fn logout_device(
        &self,
        device_id: String,
        refresh_token: String,
    ) -> Result<SyncOkResult, SyncApiError> {
        self.send_json(
            Method::POST,
            "/sync/logout-device",
            Some(json!({
                "device_id": device_id,
                "refresh_token": refresh_token,
            })),
            false,
        )
        .await
    }

    pub async fn clear_cloud_data(&self) -> Result<SyncClearCloudDataResult, SyncApiError> {
        let response: BackendClearCloudDataResponse = self
            .send_json(Method::DELETE, "/account/data", None, true)
            .await?;
        Ok(SyncClearCloudDataResult {
            ok: response.ok,
            deleted_at: response.deleted_at,
        })
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        requires_auth: bool,
    ) -> Result<T, SyncApiError> {
        let url = endpoint_url(&self.base_url, path)?;
        let mut request = self.client.request(method, url);
        if requires_auth {
            let token = self
                .access_token
                .as_deref()
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .ok_or_else(|| {
                    SyncApiError::local("sync_login_required", "请先登录同步账号", false)
                })?;
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request.send().await.map_err(|err| {
            SyncApiError::local(
                "sync_network_failed",
                format!("同步服务连接失败：{err}"),
                err.is_timeout() || err.is_connect(),
            )
        })?;
        let status = response.status();
        let text = response.text().await.map_err(|err| {
            SyncApiError::local(
                "sync_response_failed",
                format!("同步服务响应读取失败：{err}"),
                true,
            )
        })?;

        if !status.is_success() {
            return Err(parse_error_response(status, &text));
        }

        serde_json::from_str(&text).map_err(|err| {
            SyncApiError::local(
                "sync_invalid_response",
                format!("同步服务返回了无法识别的数据：{err}"),
                false,
            )
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncEmailCodeRequestResult {
    pub ok: bool,
    pub expires_in: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncDeviceInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncUserInfo {
    pub id: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncEmailCodeVerifyResult {
    #[serde(alias = "access_token")]
    pub access_token: String,
    #[serde(alias = "refresh_token")]
    pub refresh_token: Option<String>,
    #[serde(default, alias = "access_token_expires_at")]
    pub access_token_expires_at: Option<String>,
    #[serde(default, alias = "refresh_token_expires_at")]
    pub refresh_token_expires_at: Option<String>,
    pub user: SyncUserInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncTokenRefreshResult {
    #[serde(alias = "access_token")]
    pub access_token: String,
    #[serde(alias = "refresh_token")]
    pub refresh_token: Option<String>,
    #[serde(default, alias = "access_token_expires_at")]
    pub access_token_expires_at: Option<String>,
    #[serde(default, alias = "refresh_token_expires_at")]
    pub refresh_token_expires_at: Option<String>,
    pub user: SyncUserInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncLoginResult {
    pub user: SyncUserInfo,
    pub account_email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncPushChanges {
    pub prompts: Vec<Value>,
    pub provider_configs: Vec<Value>,
    pub history_items: Vec<Value>,
    pub dictionary_entries: Vec<Value>,
    pub correction_rules: Vec<Value>,
    pub vocab_presets: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncPullResult {
    pub cursor: String,
    pub server_time: String,
    pub prompts: Vec<Value>,
    pub provider_configs: Vec<Value>,
    pub history_items: Vec<Value>,
    pub dictionary_entries: Vec<Value>,
    pub correction_rules: Vec<Value>,
    pub vocab_presets: Vec<Value>,
}

impl Default for SyncPullResult {
    fn default() -> Self {
        Self {
            cursor: String::new(),
            server_time: String::new(),
            prompts: Vec::new(),
            provider_configs: Vec::new(),
            history_items: Vec::new(),
            dictionary_entries: Vec::new(),
            correction_rules: Vec::new(),
            vocab_presets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncPushResult {
    pub ok: bool,
    pub cursor: String,
    pub conflicts_resolved: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncOkResult {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncClearCloudDataResult {
    pub ok: bool,
    pub deleted_at: String,
}

#[derive(Debug, Deserialize)]
struct EmailCodeRequestResponse {
    ok: bool,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct BackendPushResponse {
    ok: bool,
    cursor: String,
    #[serde(rename = "conflicts_resolved")]
    conflicts_resolved: usize,
}

#[derive(Debug, Deserialize)]
struct BackendPullResponse {
    cursor: String,
    server_time: String,
    #[serde(default)]
    prompts: Vec<Value>,
    #[serde(default)]
    provider_configs: Vec<Value>,
    #[serde(default)]
    history_items: Vec<Value>,
    #[serde(default)]
    dictionary_entries: Vec<Value>,
    #[serde(default)]
    correction_rules: Vec<Value>,
    #[serde(default)]
    vocab_presets: Vec<Value>,
}

impl From<BackendPullResponse> for SyncPullResult {
    fn from(value: BackendPullResponse) -> Self {
        Self {
            cursor: value.cursor,
            server_time: value.server_time,
            prompts: value.prompts,
            provider_configs: value.provider_configs,
            history_items: value.history_items,
            dictionary_entries: value.dictionary_entries,
            correction_rules: value.correction_rules,
            vocab_presets: value.vocab_presets,
        }
    }
}

#[derive(Debug, Deserialize)]
struct BackendClearCloudDataResponse {
    ok: bool,
    deleted_at: String,
}

#[derive(Debug, Deserialize)]
struct BackendErrorResponse {
    error: Option<BackendError>,
}

#[derive(Debug, Deserialize)]
struct BackendError {
    code: String,
    message: String,
}

pub fn normalize_base_url(value: &str) -> Result<String, SyncApiError> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(SyncApiError::local(
            "sync_server_url_required",
            "请先填写同步服务地址",
            false,
        ));
    }
    let url = reqwest::Url::parse(trimmed).map_err(|err| {
        SyncApiError::local(
            "sync_server_url_invalid",
            format!("同步服务地址无效：{err}"),
            false,
        )
    })?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(SyncApiError::local(
            "sync_server_url_invalid",
            "同步服务地址必须以 http:// 或 https:// 开头",
            false,
        ));
    }
    if url.host_str().is_none() {
        return Err(SyncApiError::local(
            "sync_server_url_invalid",
            "同步服务地址缺少主机名",
            false,
        ));
    }
    Ok(trimmed.to_string())
}

fn endpoint_url(base_url: &str, path: &str) -> Result<String, SyncApiError> {
    let joined = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    reqwest::Url::parse(&joined).map_err(|err| {
        SyncApiError::local(
            "sync_endpoint_invalid",
            format!("同步接口地址无效：{err}"),
            false,
        )
    })?;
    Ok(joined)
}

fn parse_error_response(status: StatusCode, text: &str) -> SyncApiError {
    if let Ok(parsed) = serde_json::from_str::<BackendErrorResponse>(text) {
        if let Some(error) = parsed.error {
            return SyncApiError::http(status, error.code, error.message);
        }
    }
    SyncApiError::http(
        status,
        format!("sync_http_{}", status.as_u16()),
        if text.trim().is_empty() {
            format!("同步服务返回 HTTP {}", status.as_u16())
        } else {
            text.trim().to_string()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn normalize_base_url_rejects_empty_or_non_http() {
        assert!(normalize_base_url("").is_err());
        assert!(normalize_base_url("file:///tmp/sync").is_err());
        assert_eq!(
            normalize_base_url(" http://127.0.0.1:8080/ ").unwrap(),
            "http://127.0.0.1:8080"
        );
    }

    #[tokio::test]
    async fn sync_client_sends_login_pull_push_logout_and_clear_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let expectations = vec![
                (
                    "POST /auth/email-code/request HTTP/1.1",
                    None,
                    r#"{"ok":true,"expires_in":300}"#,
                ),
                (
                    "POST /auth/email-code/verify HTTP/1.1",
                    None,
                    r#"{"access_token":"tok","refresh_token":"ref","user":{"id":"user-1","email":"u@example.com"}}"#,
                ),
                (
                    "GET /sync/pull?cursor=7 HTTP/1.1",
                    Some("Authorization: Bearer tok"),
                    r#"{"cursor":"8","server_time":"2026-05-20T00:00:00Z","prompts":[{"id":"pack-1","baseMode":"light"}],"provider_configs":[],"history_items":[],"dictionary_entries":[],"correction_rules":[],"vocab_presets":[]}"#,
                ),
                (
                    "POST /auth/token/refresh HTTP/1.1",
                    None,
                    r#"{"access_token":"tok-2","refresh_token":"ref-2","access_token_expires_at":"2026-05-20T01:00:00Z","refresh_token_expires_at":"2026-06-20T00:00:00Z","user":{"id":"user-1","email":"u@example.com"}}"#,
                ),
                (
                    "POST /sync/push HTTP/1.1",
                    Some("Authorization: Bearer tok"),
                    r#"{"ok":true,"cursor":"9","conflicts_resolved":0}"#,
                ),
                ("POST /sync/logout-device HTTP/1.1", None, r#"{"ok":true}"#),
                (
                    "DELETE /account/data HTTP/1.1",
                    Some("Authorization: Bearer tok"),
                    r#"{"ok":true,"deleted_at":"2026-05-20T00:00:01Z"}"#,
                ),
            ];

            for (request_line, required_header, body) in expectations {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                assert!(
                    request.starts_with(request_line),
                    "request did not start with {request_line:?}: {request}"
                );
                if let Some(header) = required_header {
                    assert!(request.contains(header), "missing {header}: {request}");
                }
                if request_line.starts_with("POST /sync/push") {
                    assert!(request.contains(r#""device_id":"device-1""#));
                    assert!(request.contains(r#""prompts":[{"id":"pack-1"}]"#));
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let base_url = format!("http://{addr}");
        let public_client = SyncApiClient::new(&base_url, None).unwrap();
        let email = public_client
            .request_email_code("u@example.com".into())
            .await
            .unwrap();
        assert_eq!(email.expires_in, 300);

        let login = public_client
            .verify_email_code(
                "u@example.com".into(),
                "123456".into(),
                SyncDeviceInfo {
                    id: "device-1".into(),
                    name: "Mac".into(),
                    platform: "macos".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(login.access_token, "tok");

        let authed_client = SyncApiClient::new(&base_url, Some(login.access_token)).unwrap();
        let pulled = authed_client.pull(Some("7")).await.unwrap();
        assert_eq!(pulled.cursor, "8");
        assert_eq!(pulled.prompts[0]["baseMode"], "light");

        let refreshed = public_client.refresh_token("ref".into()).await.unwrap();
        assert_eq!(refreshed.access_token, "tok-2");
        assert_eq!(refreshed.refresh_token.as_deref(), Some("ref-2"));

        let pushed = authed_client
            .push(
                "device-1".into(),
                SyncPushChanges {
                    prompts: vec![json!({ "id": "pack-1" })],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(pushed.cursor, "9");

        assert!(
            public_client
                .logout_device("device-1".into(), "ref-2".into())
                .await
                .unwrap()
                .ok
        );
        assert!(authed_client.clear_cloud_data().await.unwrap().ok);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn sync_client_maps_backend_error_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            let body =
                r#"{"error":{"code":"rate_limited","message":"too many email code requests"}}"#;
            let response = format!(
                "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let client = SyncApiClient::new(format!("http://{addr}"), None).unwrap();
        let error = client
            .request_email_code("u@example.com".into())
            .await
            .unwrap_err();
        assert_eq!(error.code, "rate_limited");
        assert_eq!(error.status, Some(429));
        assert!(error.retryable);
        server.join().unwrap();
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut buf = [0u8; 8192];
        let mut request = Vec::new();
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                let text = String::from_utf8_lossy(&request);
                let content_length = text
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .or_else(|| {
                        text.lines()
                            .find_map(|line| line.strip_prefix("Content-Length:"))
                    })
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let header_len = request
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|pos| pos + 4)
                    .unwrap_or(request.len());
                if request.len() >= header_len + content_length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }
}
