// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 出口 IP 探测：最小 HTTP/1.1 客户端（契约 §4）。
//!
//! 设备上没有 curl/wget（也不该为了一次探测引入外部二进制），因此这里用 tokio 的
//! TcpStream 手写：
//!
//! - 直连：直接连 probe URL 的 host:port，发 origin-form 请求；
//! - HTTP 代理：连代理，发 absolute-form 请求（目标 URL 原样进请求行）；
//! - SOCKS5 代理：先做 RFC 1928 握手（可选用户名/密码认证），再在隧道里发 origin-form。
//!
//! 限制（契约 §4.2 冻结）：只接受 http://、不跟随重定向、响应体上限 4 KiB、
//! 默认 5 s 超时；失败只映射六个 reason，不回落直连。

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::egress::file::ProxyType;
use crate::egress::{MAX_PROBE_TIMEOUT_SECONDS, MAX_PROBE_URL_LEN};
use crate::util;

/// 响应体上限（防大响应拖死探测）。
pub const MAX_BODY: usize = 4 * 1024;
/// 响应头上限。
const MAX_HEADER: usize = 16 * 1024;
/// 探测请求的 User-Agent。
pub const USER_AGENT: &str = concat!("device_faker/", env!("CARGO_PKG_VERSION"));

/// 解析后的 probe URL（只支持 http://）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeUrl {
    /// 原始 URL（请求行里用）。
    pub raw: String,
    /// 主机（不含方括号）。
    pub host: String,
    /// 端口（缺省 80）。
    pub port: u16,
    /// 路径 + 查询（缺省 /）。
    pub path: String,
}

/// URL 解析失败（映射 422 invalid_probe_url）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    /// 超过 512 字符。
    TooLong,
    /// 不是 http:// 开头。
    NotHttp,
    /// 含 user:pass@。
    UserInfo,
    /// host 为空。
    MissingHost,
    /// 端口非法。
    BadPort,
}

impl UrlError {
    /// 人类可读说明（不带原值，避免把凭据带进错误信息）。
    pub fn message(self) -> &'static str {
        match self {
            UrlError::TooLong => "探测 URL 超过 512 字符",
            UrlError::NotHttp => "探测 URL 必须以 http:// 开头（不引入 TLS 栈）",
            UrlError::UserInfo => "探测 URL 不得包含用户信息（user:pass@）",
            UrlError::MissingHost => "探测 URL 缺少主机名",
            UrlError::BadPort => "探测 URL 的端口必须在 1..=65535",
        }
    }
}

/// 解析并校验 probe URL（契约 §4.3）。
pub fn parse_url(raw: &str) -> Result<ProbeUrl, UrlError> {
    let trimmed = raw.trim();
    if trimmed.chars().count() > MAX_PROBE_URL_LEN {
        return Err(UrlError::TooLong);
    }
    let rest = trimmed.strip_prefix("http://").ok_or(UrlError::NotHttp)?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], rest[index..].to_owned()),
        None => (rest, "/".to_owned()),
    };
    if authority.is_empty() {
        return Err(UrlError::MissingHost);
    }
    if authority.contains('@') {
        return Err(UrlError::UserInfo);
    }
    let (host, port) = split_authority(authority)?;
    Ok(ProbeUrl {
        raw: trimmed.to_owned(),
        host,
        port,
        path,
    })
}

fn split_authority(authority: &str) -> Result<(String, u16), UrlError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']').ok_or(UrlError::MissingHost)?;
        if host.is_empty() {
            return Err(UrlError::MissingHost);
        }
        let port = match tail {
            "" => 80,
            other => parse_port(other.strip_prefix(':').ok_or(UrlError::BadPort)?)?,
        };
        return Ok((host.to_owned(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.is_empty() {
                return Err(UrlError::MissingHost);
            }
            Ok((host.to_owned(), parse_port(port)?))
        }
        None => Ok((authority.to_owned(), 80)),
    }
}

fn parse_port(raw: &str) -> Result<u16, UrlError> {
    let port = raw.parse::<u16>().map_err(|_| UrlError::BadPort)?;
    if port == 0 {
        return Err(UrlError::BadPort);
    }
    Ok(port)
}

/// 本次探测走的链路。
///
/// Debug 手写脱敏：用户名/密码只表示「是否设置」，值不出现（契约 §7.1）。
#[derive(Clone, PartialEq, Eq)]
pub enum Channel {
    /// 直连。
    Direct,
    /// 经**设备内中继**（`via="relay"`）：连 `127.0.0.1:<local_port>`，**不带凭据**
    /// （凭据由中继注入；探测侧再带一份只会掩盖「中继没带凭据」的真实故障，契约 §3.5）。
    Relay {
        /// 代理名（响应里的 `proxy` 字段）。
        name: String,
        /// 本地中继端口。
        port: u16,
    },
    /// 经代理（含凭据，凭据只用于本次握手）。
    Proxy {
        /// 代理名（响应里的 proxy 字段）。
        name: String,
        /// 协议。
        kind: ProxyType,
        /// 主机。
        host: String,
        /// 端口。
        port: u16,
        /// 用户名（可选）。
        username: Option<String>,
        /// 密码（可选）。
        password: Option<String>,
    },
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Channel::Direct => formatter.write_str("Channel::Direct"),
            Channel::Relay { name, port } => formatter
                .debug_struct("Channel::Relay")
                .field("name", name)
                .field("port", port)
                .finish(),
            Channel::Proxy {
                name,
                kind,
                host,
                port,
                username,
                password,
            } => formatter
                .debug_struct("Channel::Proxy")
                .field("name", name)
                .field("kind", kind)
                .field("host", host)
                .field("port", port)
                .field("username", &username.as_ref().map(|_| "<redacted>"))
                .field("password", &password.as_ref().map(|_| "<redacted>"))
                .finish(),
        }
    }
}

impl Channel {
    /// 代理名（直连为 None）。
    pub fn name(&self) -> Option<&str> {
        match self {
            Channel::Direct => None,
            Channel::Relay { name, .. } | Channel::Proxy { name, .. } => Some(name),
        }
    }

    /// 线上 via 字段（`via="relay"` 恒回 `"relay"`，契约 §3.5 第 3 条）。
    pub fn via(&self) -> &'static str {
        match self {
            Channel::Direct => "direct",
            Channel::Relay { .. } => "relay",
            Channel::Proxy { .. } => "proxy",
        }
    }

    /// 本次探测使用的本地中继端口（非 `relay` 为 `None`）。
    pub fn relay_port(&self) -> Option<u16> {
        match self {
            Channel::Relay { port, .. } => Some(*port),
            _ => None,
        }
    }
}

/// 探测成功。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSuccess {
    /// 出口 IP。
    pub exit_ip: IpAddr,
    /// 从发请求到读完响应体的毫秒数。
    pub latency_ms: u64,
    /// 请求发出时刻（RFC 3339 UTC）。
    pub checked_at: String,
    /// probe URL 返回的 HTTP 状态码。
    pub upstream_status: u16,
}

/// 探测失败（只映射契约 §4.4 的六个 reason）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    /// TCP 连接失败/被拒。
    Connect,
    /// 域名解析失败。
    Dns,
    /// 超时（含读响应体）。
    Timeout,
    /// 疑似 TLS / 误配 https。
    Tls,
    /// 收到非 2xx 响应。
    HttpStatus(u16),
    /// 2xx 但响应体不可解析或超过 4 KiB。
    InvalidResponse,
    /// `via="relay"`：连本地中继端口被拒 / 握手失败（契约 §3.5 第 4 条）。
    RelayConnect,
    /// `via="relay"`：端口在端口表里但 listener 没在监听（只由 API 层产生）。
    RelayNotListening,
}

impl ProbeFailure {
    /// details.reason 的六个枚举值之一。
    pub fn reason(self) -> &'static str {
        match self {
            ProbeFailure::Connect => "connect",
            ProbeFailure::Dns => "dns",
            ProbeFailure::Timeout => "timeout",
            ProbeFailure::Tls => "tls",
            ProbeFailure::HttpStatus(_) => "http_status",
            ProbeFailure::InvalidResponse => "invalid_response",
            ProbeFailure::RelayConnect => "relay_connect",
            ProbeFailure::RelayNotListening => "relay_not_listening",
        }
    }

    /// 是否触发 fail-open（契约 §3.5：只有前四个）。
    pub fn triggers_fail_open(self) -> bool {
        matches!(
            self,
            ProbeFailure::Connect | ProbeFailure::Dns | ProbeFailure::Timeout | ProbeFailure::Tls
        )
    }

    /// 已拿到的上游状态码（连接失败时为 None）。
    pub fn upstream_status(self) -> Option<u16> {
        match self {
            ProbeFailure::HttpStatus(status) => Some(status),
            _ => None,
        }
    }
}

/// 经指定链路探测出口 IP。
pub async fn probe(
    url: &ProbeUrl,
    channel: &Channel,
    timeout_seconds: u64,
) -> Result<ProbeSuccess, ProbeFailure> {
    let budget = Duration::from_secs(timeout_seconds.clamp(1, MAX_PROBE_TIMEOUT_SECONDS));
    let checked_at = util::now_rfc3339();
    let started = Instant::now();
    let result = timeout(budget, exchange(url, channel)).await;
    match result {
        Err(_) => Err(ProbeFailure::Timeout),
        Ok(Err(failure)) => Err(failure),
        Ok(Ok((status, ip))) => {
            if !(200..300).contains(&status) {
                return Err(ProbeFailure::HttpStatus(status));
            }
            Ok(ProbeSuccess {
                exit_ip: ip,
                latency_ms: started.elapsed().as_millis() as u64,
                checked_at,
                upstream_status: status,
            })
        }
    }
}

async fn exchange(url: &ProbeUrl, channel: &Channel) -> Result<(u16, IpAddr), ProbeFailure> {
    let stream = connect(channel, url).await?;
    let request = request_text(url, channel);
    write_all(&stream, request.as_bytes())
        .await
        .map_err(|_| ProbeFailure::Connect)?;
    let (status, body) = read_response(&stream).await?;
    // 先判状态码：非 2xx 的响应体没有解析义务（reason 必须是 http_status，不是 invalid_response）
    if !(200..300).contains(&status) {
        return Err(ProbeFailure::HttpStatus(status));
    }
    let exit_ip = extract_ip(&body).ok_or(ProbeFailure::InvalidResponse)?;
    Ok((status, exit_ip))
}

async fn connect(channel: &Channel, url: &ProbeUrl) -> Result<TcpStream, ProbeFailure> {
    match channel {
        Channel::Direct => connect_host(&url.host, url.port).await,
        Channel::Relay { port, .. } => TcpStream::connect(SocketAddr::new(
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            *port,
        ))
        .await
        .map_err(|_| ProbeFailure::RelayConnect),
        Channel::Proxy {
            kind,
            host,
            port,
            username,
            password,
            ..
        } => {
            let stream = connect_host(host, *port).await?;
            if *kind == ProxyType::Socks5 {
                socks5_handshake(&stream, url, username.as_deref(), password.as_deref()).await?;
            }
            Ok(stream)
        }
    }
}

/// 连接主机：IP 字面量直接连；域名取全部解析结果逐个尝试（避免只连到不可达的 IPv6）。
async fn connect_host(host: &str, port: u16) -> Result<TcpStream, ProbeFailure> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return TcpStream::connect(SocketAddr::new(ip, port))
            .await
            .map_err(|_| ProbeFailure::Connect);
    }
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| ProbeFailure::Dns)?
        .collect();
    if addresses.is_empty() {
        return Err(ProbeFailure::Dns);
    }
    for address in addresses {
        if let Ok(stream) = TcpStream::connect(address).await {
            return Ok(stream);
        }
    }
    Err(ProbeFailure::Connect)
}

fn request_text(url: &ProbeUrl, channel: &Channel) -> String {
    // HTTP 代理用 absolute-form；直连与 SOCKS5 隧道内用 origin-form。
    // （只支持 http 目标，因此不需要 CONNECT：CONNECT 是 https 隧道的做法。）
    let target = match channel {
        // 中继的本地监听就是 HTTP 代理：absolute-form。
        Channel::Relay { .. } => url.raw.clone(),
        Channel::Proxy {
            kind: ProxyType::Http,
            ..
        } => url.raw.clone(),
        _ => url.path.clone(),
    };
    let host = &url.host;
    format!(
        "GET {target} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {USER_AGENT}\r\nAccept: text/plain, application/json\r\nConnection: close\r\n\r\n"
    )
}

async fn read_response(stream: &TcpStream) -> Result<(u16, Vec<u8>), ProbeFailure> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        if let Some(index) = find_header_end(&buffer) {
            break index;
        }
        if buffer.len() > MAX_HEADER {
            return Err(ProbeFailure::InvalidResponse);
        }
        let read = read_some(stream, &mut chunk)
            .await
            .map_err(|_| ProbeFailure::Connect)?;
        if read == 0 {
            return Err(ProbeFailure::InvalidResponse);
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    if buffer.first() == Some(&0x16) {
        // 对端回了 TLS 记录，说明探测目标其实是 https
        return Err(ProbeFailure::Tls);
    }
    let header = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let status = parse_status(&header).ok_or(ProbeFailure::InvalidResponse)?;
    let content_length = parse_content_length(&header);
    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < content_length.unwrap_or(usize::MAX) {
        if body.len() > MAX_BODY {
            break;
        }
        let read = read_some(stream, &mut chunk)
            .await
            .map_err(|_| ProbeFailure::Connect)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    if body.len() > MAX_BODY {
        return Err(ProbeFailure::InvalidResponse);
    }
    Ok((status, body))
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_status(header: &str) -> Option<u16> {
    let line = header.lines().next()?;
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse::<u16>().ok()
}

fn parse_content_length(header: &str) -> Option<usize> {
    header.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    })
}

/// 按契约 §4.3 的顺序解析响应体：IP 字面量 → JSON 的 ip/origin/query。
pub fn extract_ip(body: &[u8]) -> Option<IpAddr> {
    let text = std::str::from_utf8(body).ok()?.trim();
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Some(ip);
    }
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    for key in ["ip", "origin", "query"] {
        if let Some(candidate) = object.get(key).and_then(serde_json::Value::as_str)
            && let Ok(ip) = candidate.trim().parse::<IpAddr>()
        {
            return Some(ip);
        }
    }
    None
}

async fn socks5_handshake(
    stream: &TcpStream,
    url: &ProbeUrl,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<(), ProbeFailure> {
    let with_auth = username.is_some() || password.is_some();
    let mut greeting = vec![0x05u8];
    if with_auth {
        greeting.extend_from_slice(&[0x02, 0x00, 0x02]);
    } else {
        greeting.extend_from_slice(&[0x01, 0x00]);
    }
    write_all(stream, &greeting)
        .await
        .map_err(|_| ProbeFailure::Connect)?;
    let mut reply = [0u8; 2];
    read_exact(stream, &mut reply)
        .await
        .map_err(|_| ProbeFailure::Connect)?;
    if reply[0] != 0x05 {
        return Err(ProbeFailure::InvalidResponse);
    }
    match reply[1] {
        0x00 => {}
        0x02 => {
            let user = username.unwrap_or("");
            let pass = password.unwrap_or("");
            if user.len() > 255 || pass.len() > 255 {
                return Err(ProbeFailure::InvalidResponse);
            }
            let mut auth = vec![0x01u8, user.len() as u8];
            auth.extend_from_slice(user.as_bytes());
            auth.push(pass.len() as u8);
            auth.extend_from_slice(pass.as_bytes());
            write_all(stream, &auth)
                .await
                .map_err(|_| ProbeFailure::Connect)?;
            let mut answer = [0u8; 2];
            read_exact(stream, &mut answer)
                .await
                .map_err(|_| ProbeFailure::Connect)?;
            if answer[1] != 0x00 {
                return Err(ProbeFailure::Connect);
            }
        }
        _ => return Err(ProbeFailure::Connect),
    }

    let mut request = vec![0x05u8, 0x01, 0x00];
    match url.host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            request.push(0x01);
            request.extend_from_slice(&v4.octets());
        }
        Ok(IpAddr::V6(v6)) => {
            request.push(0x04);
            request.extend_from_slice(&v6.octets());
        }
        Err(_) => {
            let bytes = url.host.as_bytes();
            if bytes.len() > 255 {
                return Err(ProbeFailure::InvalidResponse);
            }
            request.push(0x03);
            request.push(bytes.len() as u8);
            request.extend_from_slice(bytes);
        }
    }
    request.extend_from_slice(&url.port.to_be_bytes());
    write_all(stream, &request)
        .await
        .map_err(|_| ProbeFailure::Connect)?;

    let mut head = [0u8; 4];
    read_exact(stream, &mut head)
        .await
        .map_err(|_| ProbeFailure::Connect)?;
    if head[0] != 0x05 || head[1] != 0x00 {
        return Err(ProbeFailure::Connect);
    }
    let address_len = match head[3] {
        0x01 => 4usize,
        0x04 => 16usize,
        0x03 => {
            let mut len = [0u8; 1];
            read_exact(stream, &mut len)
                .await
                .map_err(|_| ProbeFailure::Connect)?;
            len[0] as usize
        }
        _ => return Err(ProbeFailure::InvalidResponse),
    };
    let mut rest = vec![0u8; address_len + 2];
    read_exact(stream, &mut rest)
        .await
        .map_err(|_| ProbeFailure::Connect)?;
    Ok(())
}

// 最小异步 IO：只用 TcpStream 的读写就绪通知 + try_read/try_write（不需要 tokio 的 io-util feature）。
async fn write_all(stream: &TcpStream, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        stream.writable().await?;
        match stream.try_write(data) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
            Ok(written) => data = &data[written..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn read_some(stream: &TcpStream, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        stream.readable().await?;
        match stream.try_read(buffer) {
            Ok(read) => return Ok(read),
            // 就绪通知可能被同进程的其它读者「偷走」：回到等待，不当作错误
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
    }
}

async fn read_exact(stream: &TcpStream, buffer: &mut [u8]) -> io::Result<()> {
    let mut filled = 0usize;
    while filled < buffer.len() {
        let read = read_some(stream, &mut buffer[filled..]).await?;
        if read == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        filled += read;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_accepts_http_only() {
        let url = parse_url("http://checkip.amazonaws.com/").unwrap();
        assert_eq!(url.host, "checkip.amazonaws.com");
        assert_eq!(url.port, 80);
        assert_eq!(url.path, "/");
        let url = parse_url("http://127.0.0.1:8080/ip?x=1").unwrap();
        assert_eq!(url.port, 8080);
        assert_eq!(url.path, "/ip?x=1");
        let url = parse_url("http://[::1]:9/").unwrap();
        assert_eq!(url.host, "::1");
        assert_eq!(url.port, 9);
        assert_eq!(parse_url("http://example.com").unwrap().path, "/");

        assert_eq!(parse_url("https://example.com/"), Err(UrlError::NotHttp));
        assert_eq!(
            parse_url("http://user:pass@example.com/"),
            Err(UrlError::UserInfo)
        );
        assert_eq!(parse_url("http:///path"), Err(UrlError::MissingHost));
        assert_eq!(parse_url("http://example.com:0/"), Err(UrlError::BadPort));
        assert_eq!(
            parse_url(&format!("http://{}/", "a".repeat(MAX_PROBE_URL_LEN))),
            Err(UrlError::TooLong)
        );
    }

    #[test]
    fn extract_ip_supports_plain_and_json_bodies() {
        assert_eq!(
            extract_ip(b"203.0.113.7\n"),
            Some("203.0.113.7".parse().unwrap())
        );
        assert_eq!(
            extract_ip(b"{\"ip\":\"203.0.113.7\"}"),
            Some("203.0.113.7".parse().unwrap())
        );
        assert_eq!(
            extract_ip(b"{\"origin\":\"198.51.100.7\"}"),
            Some("198.51.100.7".parse().unwrap())
        );
        assert_eq!(extract_ip(b"<html>nope</html>"), None);
        assert_eq!(extract_ip(b"1.2.3.4, 5.6.7.8"), None);
        assert_eq!(extract_ip(b""), None);
    }

    #[test]
    fn failure_reasons_map_to_contract_enum() {
        assert_eq!(ProbeFailure::Connect.reason(), "connect");
        assert_eq!(ProbeFailure::Dns.reason(), "dns");
        assert_eq!(ProbeFailure::Timeout.reason(), "timeout");
        assert_eq!(ProbeFailure::Tls.reason(), "tls");
        assert_eq!(ProbeFailure::HttpStatus(502).reason(), "http_status");
        assert_eq!(ProbeFailure::InvalidResponse.reason(), "invalid_response");
        assert!(ProbeFailure::Connect.triggers_fail_open());
        assert!(ProbeFailure::Dns.triggers_fail_open());
        assert!(ProbeFailure::Timeout.triggers_fail_open());
        assert!(ProbeFailure::Tls.triggers_fail_open());
        assert!(!ProbeFailure::HttpStatus(500).triggers_fail_open());
        assert!(!ProbeFailure::InvalidResponse.triggers_fail_open());
        assert_eq!(ProbeFailure::HttpStatus(500).upstream_status(), Some(500));
        assert_eq!(ProbeFailure::Connect.upstream_status(), None);
    }

    #[test]
    fn channel_debug_redacts_credentials() {
        let channel = Channel::Proxy {
            name: "us_la_01".to_owned(),
            kind: ProxyType::Socks5,
            host: "198.51.100.7".to_owned(),
            port: 1080,
            username: Some("alice".to_owned()),
            password: Some("s3cret".to_owned()),
        };
        let debug = format!("{channel:?}");
        assert!(!debug.contains("alice"), "{debug}");
        assert!(!debug.contains("s3cret"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
        assert_eq!(channel.via(), "proxy");
        assert_eq!(channel.name(), Some("us_la_01"));
    }

    #[test]
    fn request_line_uses_absolute_form_for_http_proxy() {
        let url = parse_url("http://203.0.113.9:8080/ip").unwrap();
        let direct = Channel::Direct;
        assert!(
            request_text(&url, &direct).starts_with("GET /ip HTTP/1.1"),
            "{}",
            request_text(&url, &direct)
        );
        let proxy = Channel::Proxy {
            name: "p".to_owned(),
            kind: ProxyType::Http,
            host: "203.0.113.10".to_owned(),
            port: 1080,
            username: None,
            password: None,
        };
        assert!(
            request_text(&url, &proxy).starts_with("GET http://203.0.113.9:8080/ip HTTP/1.1"),
            "{}",
            request_text(&url, &proxy)
        );
    }

    #[test]
    fn header_and_status_parsing() {
        let response = "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n203.0.113.7";
        let end = find_header_end(response.as_bytes()).unwrap();
        let header = &response[..end];
        assert_eq!(parse_status(header), Some(200));
        assert_eq!(parse_content_length(header), Some(10));
        assert_eq!(parse_status("garbage"), None);
    }
}
