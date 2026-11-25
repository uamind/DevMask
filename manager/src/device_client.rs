// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 设备端 HTTP 客户端：baseUrl 归一化、请求构造、错误分类与探测。

use std::time::{Duration, Instant};

use reqwest::{Client, Method, StatusCode, Url, header, redirect::Policy};
use serde_json::{Value, json};

/// 请求设备时使用的 User-Agent。
pub const USER_AGENT: &str = "device-faker-manager/0.1.0";
/// 到设备的连接超时。
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// baseUrl 校验错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum BaseUrlError {
    #[error("不是合法 URL：{0}")]
    Parse(String),
    #[error("只支持 http / https，当前为 {0}")]
    Scheme(String),
    #[error("缺少主机名")]
    Host,
    #[error("不允许在地址里携带用户名或密码")]
    Credentials,
    #[error("地址不允许带查询参数或片段")]
    QueryOrFragment,
}

/// 归一化设备地址：小写 scheme/host、去默认端口、去尾部斜杠，允许路径前缀。
pub fn normalize_base_url(raw: &str) -> Result<String, BaseUrlError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(BaseUrlError::Parse("空地址".to_string()));
    }
    let mut url = Url::parse(raw).map_err(|error| BaseUrlError::Parse(error.to_string()))?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(BaseUrlError::Scheme(url.scheme().to_string()));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(BaseUrlError::Host);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(BaseUrlError::Credentials);
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(BaseUrlError::QueryOrFragment);
    }

    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// 拼接设备 API 地址。
pub fn api_url(base_url: &str, path: &str) -> String {
    let suffix = path.trim_start_matches('/');
    if suffix.is_empty() {
        format!("{base_url}/api/v1")
    } else {
        format!("{base_url}/api/v1/{suffix}")
    }
}

/// 不可达原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineReason {
    Connect,
    Dns,
    Timeout,
}

impl OfflineReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Dns => "dns",
            Self::Timeout => "timeout",
        }
    }
}

/// 一次设备调用的错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum DeviceCallError {
    #[error("设备不可达（{reason:?}）")]
    Offline { reason: OfflineReason },
    #[error("设备返回 HTTP {status}（{code}）")]
    Http {
        status: u16,
        code: String,
        message: String,
        details: Value,
    },
    #[error("设备响应无法解析：{detail}")]
    Decode { detail: String },
    #[error("设备响应超过上限（{limit} 字节）")]
    TooLarge { limit: usize },
}

impl DeviceCallError {
    /// 映射到 Manager 结果码。
    pub fn manager_code(&self) -> &'static str {
        match self {
            Self::Offline { .. } => "device_offline",
            Self::Http { status, .. } if *status == 401 || *status == 403 => "forbidden_scope",
            Self::Http { .. } | Self::Decode { .. } | Self::TooLarge { .. } => "device_error",
        }
    }

    /// 面向用户的短说明（不包含设备响应体）。
    pub fn summary(&self) -> String {
        match self {
            Self::Offline { reason } => match reason {
                OfflineReason::Connect => "连接失败".to_string(),
                OfflineReason::Dns => "域名解析失败".to_string(),
                OfflineReason::Timeout => "请求超时".to_string(),
            },
            Self::Http { status, code, .. } => format!("设备返回 HTTP {status}（{code}）"),
            Self::Decode { .. } => "设备响应无法解析".to_string(),
            Self::TooLarge { limit } => format!("设备响应超过 {limit} 字节上限"),
        }
    }

    /// HTTP 状态码（非 HTTP 错误为 None）。
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            Self::Offline { .. } | Self::Decode { .. } | Self::TooLarge { .. } => None,
        }
    }

    /// 设备错误信封里的 details 对象（仅 HTTP 错误）。
    pub fn upstream_details(&self) -> Option<&Value> {
        match self {
            Self::Http { details, .. } => Some(details),
            Self::Offline { .. } | Self::Decode { .. } | Self::TooLarge { .. } => None,
        }
    }

    /// 错误详情（不含设备响应体与 Token）。
    pub fn details(&self) -> Value {
        match self {
            Self::Offline { reason } => json!({ "reason": reason.as_str() }),
            Self::Http { status, code, .. } => {
                json!({ "upstreamStatus": status, "upstreamCode": code })
            }
            Self::Decode { .. } => json!({ "reason": "invalid_response" }),
            Self::TooLarge { limit } => {
                json!({ "reason": "response_too_large", "limitBytes": limit })
            }
        }
    }
}

/// 探测失败（添加 / 修改设备时）。
#[derive(Debug, Clone)]
pub struct ProbeFailure {
    pub reason: &'static str,
    pub upstream: Option<Value>,
    pub message: String,
}

impl From<DeviceCallError> for ProbeFailure {
    fn from(error: DeviceCallError) -> Self {
        match &error {
            DeviceCallError::Offline { reason } => Self {
                reason: reason.as_str(),
                upstream: None,
                message: error.summary(),
            },
            DeviceCallError::Http { status, code, .. } => {
                let reason = match *status {
                    401 => "unauthorized",
                    403 => "forbidden_scope",
                    _ => "upstream_status",
                };
                Self {
                    reason,
                    upstream: Some(json!({ "status": status, "code": code })),
                    message: error.summary(),
                }
            }
            DeviceCallError::Decode { .. } | DeviceCallError::TooLarge { .. } => Self {
                reason: "invalid_response",
                upstream: None,
                message: error.summary(),
            },
        }
    }
}

/// 探测结果：设备 status + overview + 往返耗时。
#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub status: Value,
    pub overview: Value,
    pub latency_ms: i64,
}

/// 设备 HTTP 客户端（克隆成本低，内部共享连接池）。
#[derive(Clone)]
pub struct DeviceHttp {
    client: Client,
}

/// 设备响应体上限（所有读接口与代理共用，8 MiB）。
pub const DEVICE_RESPONSE_LIMIT: usize = 8 * 1024 * 1024;
/// 代理响应体上限。
pub const PROXY_RESPONSE_LIMIT: usize = DEVICE_RESPONSE_LIMIT;

/// 单设备代理的响应。
#[derive(Debug, Clone)]
pub struct ProxyResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub revision: Option<String>,
    pub etag: Option<String>,
    pub retry_after: Option<String>,
    pub upstream_request_id: Option<String>,
    pub body: Vec<u8>,
}

impl DeviceHttp {
    pub fn new() -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(Policy::none())
            .no_proxy()
            .user_agent(USER_AGENT)
            .build()
            .expect("构建 reqwest 客户端失败");
        Self { client }
    }

    #[allow(clippy::too_many_arguments)]
    async fn request_json_with_status(
        &self,
        method: Method,
        base_url: &str,
        token: &str,
        path: &str,
        timeout: Duration,
        request_id: Option<&str>,
        extra_headers: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<(u16, Value), DeviceCallError> {
        let url = api_url(base_url, path);
        let mut request = self
            .client
            .request(method, &url)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::ACCEPT, "application/json")
            .timeout(timeout);
        if let Some(request_id) = request_id {
            request = request.header("x-request-id", request_id);
        }
        for (name, value) in extra_headers {
            let header_name = header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                DeviceCallError::Decode {
                    detail: format!("非法请求头名：{name}"),
                }
            })?;
            let header_value =
                header::HeaderValue::from_str(value).map_err(|_| DeviceCallError::Decode {
                    detail: format!("非法请求头值：{name}"),
                })?;
            request = request.header(header_name, header_value);
        }
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request.send().await.map_err(classify_send_error)?;
        let (status, _, bytes) = read_limited_response(response, DEVICE_RESPONSE_LIMIT).await?;

        if !status.is_success() {
            return Err(parse_http_error(status, &bytes));
        }
        if bytes.is_empty() {
            return Ok((status.as_u16(), Value::Null));
        }

        let value = serde_json::from_slice(&bytes).map_err(|error| DeviceCallError::Decode {
            detail: error.to_string(),
        })?;
        Ok((status.as_u16(), value))
    }

    #[allow(clippy::too_many_arguments)]
    async fn request_json(
        &self,
        method: Method,
        base_url: &str,
        token: &str,
        path: &str,
        timeout: Duration,
        request_id: Option<&str>,
        extra_headers: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value, DeviceCallError> {
        self.request_json_with_status(
            method,
            base_url,
            token,
            path,
            timeout,
            request_id,
            extra_headers,
            body,
        )
        .await
        .map(|(_, value)| value)
    }

    async fn get_json(
        &self,
        base_url: &str,
        token: &str,
        path: &str,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<Value, DeviceCallError> {
        self.request_json(
            Method::GET,
            base_url,
            token,
            path,
            timeout,
            request_id,
            &[],
            None,
        )
        .await
    }

    /// GET /api/v1/config/revision
    pub async fn get_revision(
        &self,
        base_url: &str,
        token: &str,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<Value, DeviceCallError> {
        self.get_json(base_url, token, "config/revision", timeout, request_id)
            .await
    }

    /// GET /api/v1/templates/{name}
    pub async fn get_template(
        &self,
        base_url: &str,
        token: &str,
        name: &str,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<Value, DeviceCallError> {
        self.get_json(
            base_url,
            token,
            &format!("templates/{name}"),
            timeout,
            request_id,
        )
        .await
    }

    /// PUT /api/v1/templates/{name}（If-Match: *，整体替换）
    pub async fn put_template(
        &self,
        base_url: &str,
        token: &str,
        name: &str,
        config: &Value,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<(u16, Value), DeviceCallError> {
        self.request_json_with_status(
            Method::PUT,
            base_url,
            token,
            &format!("templates/{name}"),
            timeout,
            request_id,
            &[("If-Match", "*".to_string())],
            Some(config),
        )
        .await
    }

    /// PUT /api/v1/apps/{pkg}，绑定模板（If-Match 为设备当前 revision）
    #[allow(clippy::too_many_arguments)]
    pub async fn bind_app(
        &self,
        base_url: &str,
        token: &str,
        package: &str,
        template: &str,
        revision: &str,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<(u16, Value), DeviceCallError> {
        let body = serde_json::json!({
            "template": template,
            "detachFromTemplates": true,
        });
        self.request_json_with_status(
            Method::PUT,
            base_url,
            token,
            &format!("apps/{package}"),
            timeout,
            request_id,
            &[("If-Match", format!("\"{revision}\""))],
            Some(&body),
        )
        .await
    }

    /// POST /api/v1/apps/{pkg}/restart
    pub async fn restart_app(
        &self,
        base_url: &str,
        token: &str,
        package: &str,
        idempotency_key: &str,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<(u16, Value), DeviceCallError> {
        let body = serde_json::json!({ "forceStop": true, "launch": false });
        self.request_json_with_status(
            Method::POST,
            base_url,
            token,
            &format!("apps/{package}/restart"),
            timeout,
            request_id,
            &[("Idempotency-Key", idempotency_key.to_string())],
            Some(&body),
        )
        .await
    }

    /// GET /api/v1/egress（设备端出口现状）。
    pub async fn get_egress(
        &self,
        base_url: &str,
        token: &str,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<Value, DeviceCallError> {
        self.get_json(base_url, token, "egress", timeout, request_id)
            .await
    }

    /// PUT /api/v1/egress；If-Match 为设备端 egress revision（"*" 表示无条件写）。
    #[allow(clippy::too_many_arguments)]
    pub async fn put_egress(
        &self,
        base_url: &str,
        token: &str,
        body: &Value,
        if_match: &str,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<(u16, Value), DeviceCallError> {
        self.request_json_with_status(
            Method::PUT,
            base_url,
            token,
            "egress",
            timeout,
            request_id,
            &[("If-Match", if_match.to_string())],
            Some(body),
        )
        .await
    }

    /// POST /api/v1/egress/probe（经设备当前链路探测出口 IP）。
    pub async fn probe_egress(
        &self,
        base_url: &str,
        token: &str,
        body: Option<&Value>,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<Value, DeviceCallError> {
        self.request_json(
            Method::POST,
            base_url,
            token,
            "egress/probe",
            timeout,
            request_id,
            &[],
            body,
        )
        .await
    }

    /// 透传一次设备 /api/v1 请求；保留设备状态码与响应体。
    #[allow(clippy::too_many_arguments)]
    pub async fn proxy(
        &self,
        base_url: &str,
        token: &str,
        method: Method,
        path: &str,
        query: Option<&str>,
        extra_headers: &[(String, String)],
        body: Option<Vec<u8>>,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<ProxyResponse, DeviceCallError> {
        let mut url = api_url(base_url, path);
        if let Some(query) = query
            && !query.is_empty()
        {
            url.push('?');
            url.push_str(query);
        }

        let mut request = self
            .client
            .request(method, &url)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::ACCEPT, "application/json")
            .timeout(timeout);
        if let Some(request_id) = request_id {
            request = request.header("x-request-id", request_id);
        }
        for (name, value) in extra_headers {
            let header_name = header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                DeviceCallError::Decode {
                    detail: format!("非法请求头名：{name}"),
                }
            })?;
            let header_value =
                header::HeaderValue::from_str(value).map_err(|_| DeviceCallError::Decode {
                    detail: format!("非法请求头值：{name}"),
                })?;
            request = request.header(header_name, header_value);
        }
        if let Some(body) = body {
            request = request.body(body);
        }

        let response = request.send().await.map_err(classify_send_error)?;
        let (status, headers, bytes) =
            read_limited_response(response, PROXY_RESPONSE_LIMIT).await?;

        Ok(ProxyResponse {
            status: status.as_u16(),
            content_type: header_value(&headers, "content-type"),
            revision: header_value(&headers, "x-df-revision"),
            etag: header_value(&headers, "etag"),
            retry_after: header_value(&headers, "retry-after"),
            upstream_request_id: header_value(&headers, "x-request-id"),
            body: bytes,
        })
    }

    /// 只拉取 overview（detail / overview 端点使用）。
    pub async fn fetch_overview(
        &self,
        base_url: &str,
        token: &str,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<Value, DeviceCallError> {
        let overview = self
            .get_json(base_url, token, "overview", timeout, request_id)
            .await?;
        ensure_overview(&overview)?;
        Ok(overview)
    }

    /// 探测 status + overview（添加 / 修改 / 同步设备使用）。
    pub async fn probe(
        &self,
        base_url: &str,
        token: &str,
        timeout: Duration,
        request_id: Option<&str>,
    ) -> Result<ProbeOutcome, ProbeFailure> {
        let started = Instant::now();

        let status = self
            .get_json(base_url, token, "status", timeout, request_id)
            .await
            .map_err(ProbeFailure::from)?;

        let overview = self
            .get_json(base_url, token, "overview", timeout, request_id)
            .await
            .map_err(ProbeFailure::from)?;

        ensure_overview(&overview).map_err(ProbeFailure::from)?;

        Ok(ProbeOutcome {
            status,
            overview,
            latency_ms: started.elapsed().as_millis().min(i64::MAX as u128) as i64,
        })
    }
}

impl Default for DeviceHttp {
    fn default() -> Self {
        Self::new()
    }
}

/// 逐 chunk 读取设备响应，超过上限立即中止（不依赖 Content-Length）。
async fn read_limited_response(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<(StatusCode, header::HeaderMap, Vec<u8>), DeviceCallError> {
    let status = response.status();
    let headers = response.headers().clone();
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(DeviceCallError::TooLarge { limit });
    }

    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len().saturating_add(chunk.len()) > limit {
                    return Err(DeviceCallError::TooLarge { limit });
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => return Err(classify_send_error(error)),
        }
    }
    Ok((status, headers, body))
}

fn header_value(headers: &header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn ensure_overview(overview: &Value) -> Result<(), DeviceCallError> {
    if overview.get("device").is_some_and(Value::is_object) {
        Ok(())
    } else {
        Err(DeviceCallError::Decode {
            detail: "overview 缺少 device 对象".to_string(),
        })
    }
}

fn classify_send_error(error: reqwest::Error) -> DeviceCallError {
    if error.is_timeout() {
        return DeviceCallError::Offline {
            reason: OfflineReason::Timeout,
        };
    }
    if error.is_connect() {
        let text = error.to_string().to_ascii_lowercase();
        let reason = if text.contains("dns") || text.contains("resolve") {
            OfflineReason::Dns
        } else {
            OfflineReason::Connect
        };
        return DeviceCallError::Offline { reason };
    }
    DeviceCallError::Offline {
        reason: OfflineReason::Connect,
    }
}

fn parse_http_error(status: StatusCode, bytes: &[u8]) -> DeviceCallError {
    let parsed: Option<Value> = serde_json::from_slice(bytes).ok();
    if let Some(error) = parsed.as_ref().and_then(|value| value.get("error")) {
        return DeviceCallError::Http {
            status: status.as_u16(),
            code: error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("device_error")
                .to_string(),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("设备返回错误")
                .to_string(),
            details: error.get("details").cloned().unwrap_or(Value::Null),
        };
    }

    DeviceCallError::Http {
        status: status.as_u16(),
        code: "device_error".to_string(),
        message: format!("设备返回 HTTP {}", status.as_u16()),
        details: Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_base_urls() {
        assert_eq!(
            normalize_base_url("http://192.168.30.2:8686/").unwrap(),
            "http://192.168.30.2:8686"
        );
        assert_eq!(
            normalize_base_url("  HTTP://Example.COM:80/df/  ").unwrap(),
            "http://example.com/df"
        );
        assert_eq!(
            normalize_base_url("https://host:8686").unwrap(),
            "https://host:8686"
        );
        assert_eq!(
            normalize_base_url("http://[::1]:8686").unwrap(),
            "http://[::1]:8686"
        );
    }

    #[test]
    fn rejects_invalid_base_urls() {
        for raw in [
            "",
            "not a url",
            "ftp://host:8686",
            "file:///etc/passwd",
            "http://user:pass@host:8686",
            "http://host:8686/?a=1",
            "http://host:8686/#frag",
            "http://",
        ] {
            assert!(normalize_base_url(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn joins_api_paths() {
        assert_eq!(
            api_url("http://host:8686", "status"),
            "http://host:8686/api/v1/status"
        );
        assert_eq!(
            api_url("http://host:8686/df", "/overview"),
            "http://host:8686/df/api/v1/overview"
        );
    }

    #[test]
    fn maps_probe_failures() {
        let failure = ProbeFailure::from(DeviceCallError::Http {
            status: 401,
            code: "unauthorized".to_string(),
            message: "bad token".to_string(),
            details: Value::Null,
        });
        assert_eq!(failure.reason, "unauthorized");

        let failure = ProbeFailure::from(DeviceCallError::Offline {
            reason: OfflineReason::Timeout,
        });
        assert_eq!(failure.reason, "timeout");

        let failure = ProbeFailure::from(DeviceCallError::Http {
            status: 500,
            code: "internal_error".to_string(),
            message: "boom".to_string(),
            details: Value::Null,
        });
        assert_eq!(failure.reason, "upstream_status");
        assert_eq!(failure.upstream.unwrap()["status"], 500);
    }

    #[test]
    fn rejects_overview_without_device() {
        assert!(ensure_overview(&json!({ "revision": "x" })).is_err());
        assert!(ensure_overview(&json!({ "device": {} })).is_ok());
    }
}
