// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 本地 HTTP 代理数据面（契约 §4.1 / §4.3 / §4.5 / §4.6 / §4.7）。
//!
//! 只处理 **HTTP/1.1**，两种请求形态：`CONNECT`（authority-form，握手成功后是纯字节隧道）
//! 与 absolute-URI（`GET http://host/path`）。规则：
//!
//! - 头区上限 32 KiB（请求与上游响应各一份），超限 → `400` / `502`；
//! - 客户端自带的 `Proxy-Authorization` **全部丢弃**，由中继插**恰好一条**
//!   `Proxy-Authorization: Basic ...`（覆盖而非叠加，契约 §4.5）；
//! - 剥离 hop-by-hop 头（含 `Connection` 点名的扩展头），不重写 `Host`/URI、不加
//!   `X-Forwarded-*` / `Via`；
//! - 首包 10 s、上游连接 10 s、上游协商 10 s、隧道空闲 120 s（双向都无字节才关）。

use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use tokio::io::AsyncRead;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::relay::net;
use crate::relay::pipe::{now_millis, pump, read_some, shutdown_write, write_all};
use crate::relay::secret::Credentials;
use crate::relay::uid::Limiter;
use crate::relay::{ListenerSignals, UpstreamFailure};

/// 请求头区上限（契约 §4.1，硬值）。
pub const MAX_HEAD_BYTES: usize = 32 * 1024;
/// 超时表（契约 §4.6，冻结值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    /// 客户端首包（头区）。
    pub client_head: Duration,
    /// 上游 TCP 连接（含 DNS）。
    pub upstream_connect: Duration,
    /// 上游协商（CONNECT 响应头 / 绝对 URI 响应头）。
    pub upstream_handshake: Duration,
    /// 隧道/转发空闲。
    pub tunnel_idle: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            client_head: Duration::from_secs(10),
            upstream_connect: Duration::from_secs(10),
            upstream_handshake: Duration::from_secs(10),
            tunnel_idle: Duration::from_secs(120),
        }
    }
}

/// 一条上游（含凭据）。`Debug` 不含凭据值。
#[derive(Clone)]
pub struct Upstream {
    /// 代理名（日志用）。
    pub label: String,
    /// 上游主机。
    pub host: String,
    /// 上游端口。
    pub port: u16,
    /// 凭据（`Some` 才注入 `Proxy-Authorization`）。
    pub credentials: Option<Arc<Credentials>>,
}

impl std::fmt::Debug for Upstream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Upstream")
            .field("label", &self.label)
            .field("host", &self.host)
            .field("port", &self.port)
            .field(
                "credentials",
                &self.credentials.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl PartialEq for Upstream {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label
            && self.host == other.host
            && self.port == other.port
            && self.credentials == other.credentials
    }
}

impl Eq for Upstream {}

impl Upstream {
    /// `Proxy-Authorization` 的值；无凭据时 `None`（此时不注入，契约 §4.5 末条）。
    pub fn authorization(&self) -> Option<&str> {
        self.credentials
            .as_ref()
            .and_then(|credentials| std::str::from_utf8(credentials.authorization().expose()).ok())
    }
}

/// 一条连接的上下文（由 listener 提供）。
pub struct ProxyContext {
    /// 上游。
    pub upstream: Arc<Upstream>,
    /// 全局限流器（所有 listener 共享）。
    pub limiter: Arc<Limiter>,
    /// 本 listener 当前连接数。
    pub active: Arc<std::sync::atomic::AtomicUsize>,
    /// 超时表。
    pub timings: Timings,
    /// 运行态信号（绑定失败 / 上游失败）。
    pub signals: ListenerSignals,
    /// `restricted` 端口允许的 uid（含 0）；`None` = 设备级（`all`）。
    pub allowed_uids: Option<Arc<Vec<u32>>>,
    /// `device` / `app`。
    pub scope: &'static str,
    /// 监听端口。
    pub port: u16,
    /// listener 协议（决定 accept 后交给哪个数据面）。
    pub kind: crate::relay::ListenerKind,
    /// 防火墙是否已按 -S 确认生效（accept 期 uid 归属失败时的分层策略依据）。
    pub firewall_enforced: Arc<std::sync::atomic::AtomicBool>,
    /// 限频复核器：uid 未知时重新采样 -S（≤1/s），不得永久信任旧值。
    pub revalidate: Option<Arc<crate::relay::firewall::Revalidator>>,
}

impl ProxyContext {
    /// 构造（测试与 service 共用）。
    // 8 个参数都是 listener 的身份/依赖，拆结构体反而更难读。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        upstream: Arc<Upstream>,
        limiter: Arc<Limiter>,
        active: Arc<std::sync::atomic::AtomicUsize>,
        signals: ListenerSignals,
        allowed_uids: Option<Arc<Vec<u32>>>,
        scope: &'static str,
        port: u16,
        kind: crate::relay::ListenerKind,
        firewall_enforced: Arc<std::sync::atomic::AtomicBool>,
        revalidate: Option<Arc<crate::relay::firewall::Revalidator>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            upstream,
            limiter,
            active,
            timings: Timings::default(),
            signals,
            allowed_uids,
            scope,
            port,
            kind,
            firewall_enforced,
            revalidate,
        })
    }

    /// 记录一次上游失败（egress_relay_upstream_unreachable）。
    pub fn record_upstream_failure(&self, reason: &'static str) {
        self.signals
            .set_upstream_failure(UpstreamFailure::new(reason));
        tracing::warn!(
            proxy = %self.upstream.label,
            port = self.port,
            reason,
            "relay upstream failed"
        );
    }

    /// 上游握手成功 → 清掉运行态告警。
    pub fn clear_upstream_failure(&self) {
        self.signals.clear_upstream_failure();
    }
}

/// 请求头区读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadRead {
    /// 头区（含结尾的空行）。
    pub head: Vec<u8>,
    /// 头区之后已经读到的字节（不能丢，属于隧道/请求体）。
    pub rest: Vec<u8>,
}

/// 头区读取失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadError {
    /// 超过上限（32 KiB）。
    TooLarge,
    /// 对端在读满头区前关闭 / IO 错误。
    Closed,
}

/// 读一个头区（到 `\r\n\r\n` 为止），超限返回 [`HeadError::TooLarge`]。
pub async fn read_head<R>(reader: &mut R, cap: usize) -> Result<HeadRead, HeadError>
where
    R: AsyncRead + Unpin,
{
    let mut buffer: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(index) = find_head_end(&buffer) {
            if index + 4 > cap {
                // 头区本身超过上限（通道里可能有更多字节，但契约只限头区）。
                return Err(HeadError::TooLarge);
            }
            let head = buffer[..index + 4].to_vec();
            let rest = buffer[index + 4..].to_vec();
            return Ok(HeadRead { head, rest });
        }
        if buffer.len() > cap {
            return Err(HeadError::TooLarge);
        }
        let read = read_some(reader, &mut chunk)
            .await
            .map_err(|_| HeadError::Closed)?;
        if read == 0 {
            return Err(HeadError::Closed);
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

/// 请求目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `CONNECT host:port`。
    Connect {
        /// 目标主机。
        host: String,
        /// 目标端口。
        port: u16,
    },
    /// `<METHOD> http://host[:port]/path`。
    Absolute {
        /// 原样保留的绝对 URI（转发时写回请求行）。
        uri: String,
        /// 目标主机。
        host: String,
        /// 目标端口（缺省 80）。
        port: u16,
    },
}

impl Target {
    /// 目标 (host, port)。
    pub fn destination(&self) -> (&str, u16) {
        match self {
            Target::Connect { host, port } => (host, *port),
            Target::Absolute { host, port, .. } => (host, *port),
        }
    }

    /// 是否 CONNECT。
    pub fn is_connect(&self) -> bool {
        matches!(self, Target::Connect { .. })
    }
}

/// 解析后的请求头。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    /// 方法（原样）。
    pub method: String,
    /// 目标。
    pub target: Target,
    /// 头（保持客户端顺序）。
    pub headers: Vec<(String, String)>,
}

impl RequestHead {
    /// 按大小写不敏感读一个头。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// 目标 (host, port)。
    pub fn destination(&self) -> (&str, u16) {
        self.target.destination()
    }
}

/// 请求被拒的原因（全部映射 `400` + close，契约 §4.7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    /// 畸形（缺字段 / 非 3 段请求行 / 头行无冒号 / 非法 authority）。
    Malformed,
    /// 非 HTTP/1.1（含 HTTP/1.0）。
    NotHttp11,
    /// origin-form（`GET /path`）或 `OPTIONS *`。
    OriginForm,
    /// 被拒 scheme（`https://` 直连、`ftp://` 等）。
    UnsupportedScheme,
    /// 缺 `Host`。
    MissingHost,
    /// `Host` 与目标不一致（防请求走私）。
    HostMismatch,
    /// 目标里带用户信息（`user@host`）。
    UserInfo,
}

impl RequestError {
    /// 日志用的原因（不含请求内容）。
    pub fn reason(self) -> &'static str {
        match self {
            RequestError::Malformed => "malformed",
            RequestError::NotHttp11 => "not_http11",
            RequestError::OriginForm => "origin_form",
            RequestError::UnsupportedScheme => "unsupported_scheme",
            RequestError::MissingHost => "missing_host",
            RequestError::HostMismatch => "host_mismatch",
            RequestError::UserInfo => "user_info",
        }
    }
}

/// 解析请求头区（契约 §4.1 的接受/拒绝清单）。
pub fn parse_request(head: &[u8]) -> Result<RequestHead, RequestError> {
    let text = std::str::from_utf8(head).map_err(|_| RequestError::Malformed)?;
    let text = text.strip_suffix("\r\n\r\n").unwrap_or(text);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(RequestError::Malformed)?;
    let mut parts = request_line.split(' ');
    let method = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or(RequestError::Malformed)?;
    let raw_target = parts.next().ok_or(RequestError::Malformed)?;
    let version = parts.next().ok_or(RequestError::Malformed)?;
    if parts.next().is_some() {
        return Err(RequestError::Malformed);
    }
    if !version.eq_ignore_ascii_case("HTTP/1.1") {
        return Err(RequestError::NotHttp11);
    }

    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or(RequestError::Malformed)?;
        let name = name.trim();
        if name.is_empty() || name.chars().any(|c| c == ' ' || c == '\t') {
            return Err(RequestError::Malformed);
        }
        headers.push((name.to_owned(), value.trim().to_owned()));
    }

    let target = if method == "CONNECT" {
        let (host, port) = parse_authority(raw_target)?;
        Target::Connect { host, port }
    } else {
        parse_absolute_uri(raw_target)?
    };

    let (host, port) = target.destination();
    let host_header = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.as_str())
        .ok_or(RequestError::MissingHost)?;
    if !host_header_matches(host_header, host, port) {
        return Err(RequestError::HostMismatch);
    }

    Ok(RequestHead {
        method: method.to_owned(),
        target,
        headers,
    })
}

fn parse_authority(text: &str) -> Result<(String, u16), RequestError> {
    if text.is_empty() || text.contains('@') || text.contains('/') || text.contains(' ') {
        return Err(RequestError::Malformed);
    }
    if let Some(rest) = text.strip_prefix('[') {
        let (host, tail) = rest.split_once(']').ok_or(RequestError::Malformed)?;
        let port = tail
            .strip_prefix(':')
            .ok_or(RequestError::Malformed)?
            .parse::<u16>()
            .map_err(|_| RequestError::Malformed)?;
        if host.is_empty() || port == 0 {
            return Err(RequestError::Malformed);
        }
        return Ok((host.to_owned(), port));
    }
    let (host, port) = text.rsplit_once(':').ok_or(RequestError::Malformed)?;
    let port = port.parse::<u16>().map_err(|_| RequestError::Malformed)?;
    if host.is_empty() || port == 0 || host.contains(':') {
        return Err(RequestError::Malformed);
    }
    Ok((host.to_owned(), port))
}

fn parse_absolute_uri(uri: &str) -> Result<Target, RequestError> {
    if uri.starts_with('/') || uri == "*" {
        return Err(RequestError::OriginForm);
    }
    let lower = uri.to_ascii_lowercase();
    let rest = match lower.strip_prefix("http://") {
        Some(rest) => rest,
        None => {
            return Err(if lower.contains("://") {
                RequestError::UnsupportedScheme
            } else {
                RequestError::OriginForm
            });
        }
    };
    // 用原文切片（lower 与 uri 等长：都是 ASCII 前缀替换，长度一致）。
    let original_rest = &uri[uri.len() - rest.len()..];
    let authority = match original_rest.find(['/', '?', '#']) {
        Some(index) => &original_rest[..index],
        None => original_rest,
    };
    if authority.is_empty() {
        return Err(RequestError::Malformed);
    }
    if authority.contains('@') {
        return Err(RequestError::UserInfo);
    }
    let (host, port) = split_host_port(authority)?;
    Ok(Target::Absolute {
        uri: uri.to_owned(),
        host,
        port,
    })
}

fn split_host_port(authority: &str) -> Result<(String, u16), RequestError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']').ok_or(RequestError::Malformed)?;
        if host.is_empty() {
            return Err(RequestError::Malformed);
        }
        let port = match tail {
            "" => 80,
            other => other
                .strip_prefix(':')
                .ok_or(RequestError::Malformed)?
                .parse::<u16>()
                .map_err(|_| RequestError::Malformed)?,
        };
        return Ok((host.to_owned(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.is_empty() || host.contains(':') {
                return Err(RequestError::Malformed);
            }
            let port = port.parse::<u16>().map_err(|_| RequestError::Malformed)?;
            if port == 0 {
                return Err(RequestError::Malformed);
            }
            Ok((host.to_owned(), port))
        }
        None => Ok((authority.to_owned(), 80)),
    }
}

/// `Host` 头与目标是否一致（host 大小写不敏感；`Host` 带端口时必须相等）。
fn host_header_matches(host_header: &str, host: &str, port: u16) -> bool {
    let Ok((header_host, header_port)) = split_host_port(host_header) else {
        return false;
    };
    if !header_host.eq_ignore_ascii_case(host) {
        return false;
    }
    // Host 显式带端口时才能比较端口；省略端口按目标端口处理。
    let explicit_port = host_header
        .rsplit_once(':')
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()));
    match explicit_port {
        Some(value) => value
            .parse::<u16>()
            .map(|value| value == port)
            .unwrap_or(false),
        None => header_port == port || port == 80,
    }
}

/// 逐跳头（契约 §4.5：两侧都剥离）。
const HOP_BY_HOP: [&str; 7] = [
    "proxy-authorization",
    "proxy-connection",
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "upgrade",
];

fn connection_tokens(headers: &[(String, String)]) -> Vec<String> {
    let mut tokens = Vec::new();
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("connection") {
            continue;
        }
        for token in value.split(',') {
            let token = token.trim().to_ascii_lowercase();
            if !token.is_empty() && !tokens.contains(&token) {
                tokens.push(token);
            }
        }
    }
    tokens
}

fn keep_header(name: &str, tokens: &[String]) -> bool {
    let name = name.to_ascii_lowercase();
    if HOP_BY_HOP.contains(&name.as_str()) {
        return false;
    }
    !tokens.contains(&name)
}

/// `Host` 值：IPv6 字面量加方括号。
pub fn authority_text(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// CONNECT 转发头（契约 §4.3：只发三行头，最小化泄漏面）。
pub fn connect_head(host: &str, port: u16, authorization: Option<&str>) -> Vec<u8> {
    let authority = authority_text(host, port);
    let mut head = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(value) = authorization {
        head.push_str("Proxy-Authorization: ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    head.into_bytes()
}

/// absolute-URI 转发头（契约 §4.5：原始头顺序 + 剥离 hop-by-hop + 末尾注入）。
pub fn absolute_head(request: &RequestHead, authorization: Option<&str>) -> Vec<u8> {
    let uri = match &request.target {
        Target::Absolute { uri, .. } => uri.clone(),
        Target::Connect { host, port } => format!("http://{}", authority_text(host, *port)),
    };
    let tokens = connection_tokens(&request.headers);
    let mut head = Vec::with_capacity(256);
    head.extend_from_slice(format!("{} {} HTTP/1.1\r\n", request.method, uri).as_bytes());
    for (name, value) in &request.headers {
        if !keep_header(name, &tokens) {
            continue;
        }
        head.extend_from_slice(name.as_bytes());
        head.extend_from_slice(b": ");
        head.extend_from_slice(value.as_bytes());
        head.extend_from_slice(b"\r\n");
    }
    head.extend_from_slice(b"Connection: close\r\n");
    if let Some(value) = authorization {
        head.extend_from_slice(b"Proxy-Authorization: ");
        head.extend_from_slice(value.as_bytes());
        head.extend_from_slice(b"\r\n");
    }
    head.extend_from_slice(b"\r\n");
    head
}

/// 响应状态码（`HTTP/1.x NNN ...`）。
pub fn parse_status(head: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(head).ok()?;
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    // 必须已经有完整状态码（`HTTP/1.1` 单独一行不算）
    parts.next()?.parse::<u16>().ok()
}

/// 把上游响应头改写后回传给客户端：剥离 hop-by-hop + `Connection: close`。
pub fn rewrite_response_head(head: &[u8], append_close: bool) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let text = text.strip_suffix("\r\n\r\n").unwrap_or(&text);
    let mut lines = text.split("\r\n");
    let Some(status_line) = lines.next() else {
        return Vec::new();
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let tokens = connection_tokens(&headers);
    let mut out = Vec::with_capacity(head.len());
    out.extend_from_slice(status_line.as_bytes());
    out.extend_from_slice(b"\r\n");
    for (name, value) in &headers {
        if !keep_header(name, &tokens) {
            continue;
        }
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    // CONNECT 2xx 之后是纯字节隧道：**不能**注入 Connection: close（契约 §4.3 原样回传）。
    // 其它情形（absolute-URI / CONNECT 非 2xx）写完就关，显式带 close。
    if append_close {
        out.extend_from_slice(b"Connection: close\r\n");
    }
    out.extend_from_slice(b"\r\n");
    out
}

/// 服务一条客户端连接（accept 循环已经做完限流与 uid 双检）。
pub async fn serve(mut client: TcpStream, context: Arc<ProxyContext>) {
    // 1) 客户端首包（头区）——超时/畸形直接关闭（契约 §4.7：不返回 408）。
    let read = match timeout(
        context.timings.client_head,
        read_head(&mut client, MAX_HEAD_BYTES),
    )
    .await
    {
        Ok(Ok(read)) => read,
        Ok(Err(HeadError::TooLarge)) => {
            tracing::warn!(
                proxy = %context.upstream.label,
                port = context.port,
                reason = "client_head_too_large",
                "relay client rejected"
            );
            let _ = write_status(&mut client, 400, "Bad Request").await;
            return;
        }
        _ => return,
    };
    let request = match parse_request(&read.head) {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(
                proxy = %context.upstream.label,
                port = context.port,
                reason = error.reason(),
                "relay client rejected"
            );
            let _ = write_status(&mut client, 400, "Bad Request").await;
            return;
        }
    };

    // 2) 连**上游代理**（永远不是目标站点；目标写在请求行/CONNECT 里），10 s。
    let (host, port) = request.destination();
    let upstream = match net::connect(
        &context.upstream.host,
        context.upstream.port,
        context.timings.upstream_connect,
    )
    .await
    {
        Ok(stream) => stream,
        Err(reason) => {
            context.record_upstream_failure(reason);
            let _ = write_status(&mut client, 502, "Bad Gateway").await;
            return;
        }
    };

    // 3) 写上游请求头（覆盖式注入）+ 已读到的请求体字节。
    let authorization = context.upstream.authorization();
    let head = if request.target.is_connect() {
        connect_head(host, port, authorization)
    } else {
        absolute_head(&request, authorization)
    };
    let (mut upstream_read, mut upstream_write) = upstream.into_split();
    // 这份 head 里含有上游 Basic/RFC1929 的 base64 副本：写出去之后立刻清零，
    // 不把它留在进程内存里等 GC（F9 low）。
    let mut head = head;
    let written = write_all(&mut upstream_write, &head).await;
    head.iter_mut().for_each(|byte| *byte = 0);
    if written.is_err() {
        context.record_upstream_failure("connect");
        return;
    }
    if !read.rest.is_empty() && write_all(&mut upstream_write, &read.rest).await.is_err() {
        context.record_upstream_failure("connect");
        return;
    }

    // 4) 上游响应头（32 KiB 上限，10 s）。
    let response = match timeout(
        context.timings.upstream_handshake,
        read_head(&mut upstream_read, MAX_HEAD_BYTES),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => {
            context.record_upstream_failure("protocol_error");
            let _ = write_status(&mut client, 502, "Bad Gateway").await;
            return;
        }
        Err(_) => {
            context.record_upstream_failure("timeout");
            let _ = write_status(&mut client, 502, "Bad Gateway").await;
            return;
        }
    };
    let Some(status) = parse_status(&response.head) else {
        context.record_upstream_failure("protocol_error");
        let _ = write_status(&mut client, 502, "Bad Gateway").await;
        return;
    };
    let is_connect = request.target.is_connect();
    let connect_ok = is_connect && (200..300).contains(&status);
    // F3：CONNECT 2xx 之后是隧道，头里**不能**出现 Connection: close；
    // 其余情形（absolute-URI / CONNECT 非 2xx）写完就关，显式带 close。
    let rewritten = rewrite_response_head(&response.head, !connect_ok);
    if rewritten.is_empty() || write_all(&mut client, &rewritten).await.is_err() {
        return;
    }
    if !response.rest.is_empty() && write_all(&mut client, &response.rest).await.is_err() {
        return;
    }
    if connect_ok {
        context.clear_upstream_failure();
    } else if status == 407 {
        context.record_upstream_failure("auth_failed");
    }
    // F2：CONNECT 非 2xx（407/4xx）原样回传后**关闭**，不再进入双向泵，
    // 否则客户端字节会被转发进这条已经失败的 CONNECT 连接（契约 §4.3）。
    if is_connect && !connect_ok {
        let _ = shutdown_write(&mut client).await;
        return;
    }

    // 5) 双向转发（CONNECT 且 2xx 是纯字节隧道；其余是本次请求-响应剩余的字节）。
    //    客户端 → 上游 放在独立任务里，本任务负责 上游 → 客户端（读完即收尾）。
    let (mut client_read, mut client_write) = client.into_split();
    let idle = context.timings.tunnel_idle;
    let last = Arc::new(AtomicU64::new(now_millis()));
    let body_last = Arc::clone(&last);
    let mut body = tokio::spawn(async move {
        let _ = pump(&mut client_read, &mut upstream_write, idle, body_last).await;
    });
    let _ = pump(
        &mut upstream_read,
        &mut client_write,
        idle,
        Arc::clone(&last),
    )
    .await;
    let _ = timeout(idle, &mut body).await;
    body.abort();
}

/// 在协议解析之前拒绝一条连接（超限 / 未授权 uid / 超限流）：写一行状态后关闭。
pub async fn reject(stream: &mut TcpStream, code: u16, reason: &str) {
    let _ = write_status(stream, code, reason).await;
    // 返回后由调用方 drop：TCP 关闭即中止连接（不需要 io-util 的 shutdown）。
}

async fn write_status(client: &mut TcpStream, code: u16, reason: &str) -> io::Result<()> {
    let text =
        format!("HTTP/1.1 {code} {reason}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
    write_all(client, text.as_bytes()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::Ordering;
    use std::task::{Context, Poll};
    use std::time::Instant;

    fn head(text: &str) -> Vec<u8> {
        format!("{text}\r\n\r\n").into_bytes()
    }

    #[test]
    fn parses_connect_and_absolute_forms() {
        let request = parse_request(&head(
            "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n",
        ))
        .unwrap();
        assert_eq!(
            request.target,
            Target::Connect {
                host: "example.com".to_owned(),
                port: 443
            }
        );
        assert_eq!(request.method, "CONNECT");

        let request = parse_request(&head(
            "GET http://example.com/ip?x=1 HTTP/1.1\r\nHost: example.com\r\nUser-Agent: t\r\n",
        ))
        .unwrap();
        assert_eq!(
            request.target,
            Target::Absolute {
                uri: "http://example.com/ip?x=1".to_owned(),
                host: "example.com".to_owned(),
                port: 80
            }
        );
        assert_eq!(request.header("user-agent"), Some("t"));
    }

    #[test]
    fn rejects_contract_cases() {
        let cases: [(&str, RequestError); 7] = [
            (
                "GET /path HTTP/1.1\r\nHost: example.com",
                RequestError::OriginForm,
            ),
            (
                "GET http://example.com/ HTTP/1.0\r\nHost: example.com",
                RequestError::NotHttp11,
            ),
            (
                "OPTIONS * HTTP/1.1\r\nHost: example.com",
                RequestError::OriginForm,
            ),
            (
                "GET https://example.com/ HTTP/1.1\r\nHost: example.com",
                RequestError::UnsupportedScheme,
            ),
            (
                "GET http://example.com/ HTTP/1.1",
                RequestError::MissingHost,
            ),
            (
                "GET http://example.com/ HTTP/1.1\r\nHost: evil.example",
                RequestError::HostMismatch,
            ),
            (
                "CONNECT example.com HTTP/1.1\r\nHost: example.com",
                RequestError::Malformed,
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(parse_request(&head(text)), Err(expected), "text={text}");
        }
        assert_eq!(
            parse_request(&head(
                "GET http://user:pass@example.com/ HTTP/1.1\r\nHost: example.com"
            )),
            Err(RequestError::UserInfo)
        );
    }

    #[test]
    fn connect_head_is_exactly_three_lines() {
        let head = connect_head("203.0.113.10", 8080, Some("Basic dTpw"));
        let text = String::from_utf8(head).unwrap();
        assert_eq!(
            text,
            "CONNECT 203.0.113.10:8080 HTTP/1.1\r\nHost: 203.0.113.10:8080\r\nProxy-Authorization: Basic dTpw\r\n\r\n"
        );
        // 无凭据：两行头，不注入
        let text = String::from_utf8(connect_head("h", 1, None)).unwrap();
        assert!(!text.contains("Proxy-Authorization"), "{text}");
    }

    #[test]
    fn absolute_head_overrides_client_authorization_and_strips_hop_by_hop() {
        let request = parse_request(&head(
            "GET http://example.com/ip HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: Basic bogus\r\nproxy-authorization: Basic bob\r\nConnection: keep-alive, X-Secret\r\nX-Secret: 1\r\nUser-Agent: ok\r\nKeep-Alive: timeout=5\r\n",
        ))
        .unwrap();
        let text = String::from_utf8(absolute_head(&request, Some("Basic cmVhbA=="))).unwrap();
        assert!(
            text.starts_with("GET http://example.com/ip HTTP/1.1\r\n"),
            "{text}"
        );
        assert!(text.contains("User-Agent: ok\r\n"), "{text}");
        assert!(!text.contains("bogus"), "{text}");
        assert!(!text.contains("Basic bob"), "{text}");
        assert!(!text.to_ascii_lowercase().contains("keep-alive"), "{text}");
        assert!(
            !text.contains("X-Secret"),
            "Connection 点名的扩展头也要剥离: {text}"
        );
        assert_eq!(
            text.matches("Proxy-Authorization").count(),
            1,
            "必须恰好一条注入头: {text}"
        );
        assert!(
            text.ends_with("Proxy-Authorization: Basic cmVhbA==\r\n\r\n"),
            "注入头追加在头区末尾: {text}"
        );
        assert!(
            text.contains("Host: example.com\r\n"),
            "不改写 Host: {text}"
        );
    }

    #[test]
    fn absolute_head_without_credentials_injects_nothing() {
        let request = parse_request(&head(
            "GET http://example.com/ HTTP/1.1\r\nHost: example.com",
        ))
        .unwrap();
        let text = String::from_utf8(absolute_head(&request, None)).unwrap();
        assert!(!text.contains("Proxy-Authorization"), "{text}");
        assert!(text.contains("Connection: close\r\n"), "{text}");
    }

    #[test]
    fn response_head_rewrite_strips_and_forces_close() {
        let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\nX-Keep: 1\r\n\r\n";
        let text = String::from_utf8(rewrite_response_head(head, true)).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
        assert!(
            text.contains("Transfer-Encoding: chunked"),
            "不能剥离分帧头: {text}"
        );
        assert!(text.contains("X-Keep: 1"), "{text}");
        assert!(text.contains("Connection: close"), "{text}");
        assert_eq!(text.matches("Connection:").count(), 1, "{text}");
        assert_eq!(parse_status(head), Some(200));
        assert_eq!(parse_status(b"garbage"), None);
    }

    // ---- t17：F1 / F2 / F3 / F4 的边界用例 ----

    /// 构造一个直连 mock 上游的上下文（timings 可调）。
    fn test_context(upstream_port: u16, tunnel_idle: Duration) -> Arc<ProxyContext> {
        let limiter = Limiter::new();
        let signals: crate::relay::ListenerSignals =
            Arc::new(crate::relay::ListenerSignalsInner::default());
        Arc::new(ProxyContext {
            upstream: Arc::new(Upstream {
                label: "test".to_owned(),
                host: "127.0.0.1".to_owned(),
                port: upstream_port,
                credentials: None,
            }),
            limiter,
            active: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            timings: Timings {
                client_head: Duration::from_secs(2),
                upstream_connect: Duration::from_secs(2),
                upstream_handshake: Duration::from_secs(2),
                tunnel_idle,
            },
            signals,
            allowed_uids: None,
            scope: "device",
            port: 17800,
            kind: crate::relay::ListenerKind::Http,
            firewall_enforced: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            revalidate: None,
        })
    }

    #[test]
    fn connect_2xx_does_not_inject_connection_close() {
        let head = b"HTTP/1.1 200 Connection Established\r\nProxy-Agent: x\r\nConnection: keep-alive\r\n\r\n";
        let text = String::from_utf8(rewrite_response_head(head, false)).expect("utf8");
        assert!(
            text.starts_with("HTTP/1.1 200 Connection Established"),
            "{text}"
        );
        assert!(
            !text.contains("Connection: close"),
            "CONNECT 2xx 是隧道，不得注入 Connection: close: {text}"
        );
        assert!(!text.contains("keep-alive"), "hop-by-hop 仍要剥离: {text}");
        assert!(text.contains("Proxy-Agent: x"), "端到端头保留: {text}");
        // 非隧道响应仍显式关连接
        let text = String::from_utf8(rewrite_response_head(head, true)).expect("utf8");
        assert!(text.contains("Connection: close"), "{text}");
    }

    #[tokio::test]
    async fn connect_non_2xx_closes_without_forwarding_client_bytes() {
        // mock 上游：读完 CONNECT 头回 407，之后记录任何多余字节
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind");
        let upstream_port = listener.local_addr().expect("addr").port();
        let extra = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let extra_sink = Arc::clone(&extra);
        let mock = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buffer: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 512];
            while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = read_some(&mut stream, &mut chunk).await.expect("read");
                if read == 0 {
                    return;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
            write_all(
                &mut stream,
                b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n",
            )
            .await
            .expect("407");
            loop {
                match read_some(&mut stream, &mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        extra_sink.fetch_add(read, Ordering::SeqCst);
                    }
                }
            }
        });

        let client_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind");
        let address = client_listener.local_addr().expect("addr");
        let mut client = TcpStream::connect(address).await.expect("connect");
        let (relay_side, _) = client_listener.accept().await.expect("accept");
        let context = test_context(upstream_port, Duration::from_millis(200));
        let serve_task = tokio::spawn(async move { serve(relay_side, context).await });

        write_all(
            &mut client,
            b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
        )
        .await
        .expect("connect request");
        let reply = read_head(&mut client, 4096).await.expect("reply");
        let text = String::from_utf8_lossy(&reply.head).into_owned();
        assert!(text.starts_with("HTTP/1.1 407"), "{text}");
        assert!(
            text.contains("Connection: close"),
            "非 2xx 必须显式关闭: {text}"
        );

        // F2：之后的客户端字节不得再被转发
        let _ = write_all(&mut client, b"should-not-pass").await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            extra.load(Ordering::SeqCst),
            0,
            "CONNECT 非 2xx 之后不得再向上游转发字节"
        );
        let mut sink = [0u8; 16];
        let read =
            tokio::time::timeout(Duration::from_secs(2), read_some(&mut client, &mut sink)).await;
        assert!(
            matches!(read, Ok(Ok(0)) | Ok(Err(_)) | Err(_)),
            "写完 407 后连接必须关闭: {read:?}"
        );
        let _ = tokio::time::timeout(Duration::from_secs(2), serve_task).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), mock).await;
    }

    #[tokio::test]
    async fn idle_covers_write_direction_and_releases_connection() {
        // mock 上游：握手后持续发大块数据；客户端**不读** → 写方向阻塞
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind");
        let upstream_port = listener.local_addr().expect("addr").port();
        let mock = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut chunk = [0u8; 512];
            let mut head: Vec<u8> = Vec::new();
            while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = read_some(&mut stream, &mut chunk).await.expect("read");
                if read == 0 {
                    return;
                }
                head.extend_from_slice(&chunk[..read]);
            }
            let _ = write_all(&mut stream, b"HTTP/1.1 200 Connection Established\r\n\r\n").await;
            // 持续灌数据：客户端不读 → 中继写方向阻塞
            let payload = vec![7u8; 64 * 1024];
            for _ in 0..64 {
                if write_all(&mut stream, &payload).await.is_err() {
                    break;
                }
            }
        });

        let client_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind");
        let address = client_listener.local_addr().expect("addr");
        let mut client = TcpStream::connect(address).await.expect("connect");
        let (relay_side, _) = client_listener.accept().await.expect("accept");
        let idle = Duration::from_millis(200);
        let context = test_context(upstream_port, idle);
        let limiter = Arc::clone(&context.limiter);
        limiter.configure(8, 8);
        let guard = Arc::clone(&limiter)
            .try_acquire(Some(1000))
            .expect("acquire");
        let serve_task = tokio::spawn(async move { serve(relay_side, context).await });

        write_all(
            &mut client,
            b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
        )
        .await
        .expect("connect request");
        // 故意不读：让中继的「上游→客户端」写方向阻塞
        let started = std::time::Instant::now();
        let finished = tokio::time::timeout(Duration::from_secs(5), serve_task).await;
        assert!(finished.is_ok(), "写方向阻塞时连接必须在 idle 内结束（F1）");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "必须靠 idle 收尾，实际 {:?}",
            started.elapsed()
        );
        drop(guard);
        assert_eq!(limiter.active(), 0, "连接结束后并发额度必须归还");
        drop(client);
        let _ = tokio::time::timeout(Duration::from_secs(2), mock).await;
    }

    #[test]
    fn shared_budget_helper_clamps_to_zero() {
        let now = Instant::now();
        assert!(net::remaining_budget(now + Duration::from_secs(10)) <= Duration::from_secs(10));
        assert!(net::remaining_budget(now - Duration::from_secs(1)).is_zero());
    }

    #[tokio::test]
    async fn upstream_budget_is_shared_across_dns_and_connects() {
        // 无法解析的名字：DNS 阶段就该在预算内失败；共享预算不会退化成
        // 「DNS 一份 + 每个地址各一份」（F4 的总量上限）。
        let started = Instant::now();
        let outcome = net::connect("no-such-host.invalid", 80, Duration::from_millis(300)).await;
        assert_eq!(outcome.err(), Some("dns"), "解析失败必须映射 dns");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "共享预算必须生效，实际 {:?}",
            started.elapsed()
        );
    }
    #[tokio::test]
    async fn head_reader_rejects_over_limit() {
        struct Chunked(Vec<u8>, usize);
        impl AsyncRead for Chunked {
            fn poll_read(
                self: Pin<&mut Self>,
                _context: &mut Context<'_>,
                buffer: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                let this = self.get_mut();
                let remaining = this.0.len() - this.1;
                let take = remaining.min(buffer.remaining());
                buffer.put_slice(&this.0[this.1..this.1 + take]);
                this.1 += take;
                Poll::Ready(Ok(()))
            }
        }
        let payload = format!(
            "GET / HTTP/1.1
X: {}

",
            "a".repeat(MAX_HEAD_BYTES)
        );
        let mut reader = Chunked(payload.into_bytes(), 0);
        assert_eq!(
            read_head(&mut reader, MAX_HEAD_BYTES).await.err(),
            Some(HeadError::TooLarge)
        );
    }
}
