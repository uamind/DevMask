// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 本地 SOCKS5 服务端 + 上游 RFC1929 客户端（契约 §4.2 / §4.4 / §4.7）。
//!
//! 本地侧**不做认证**（应用不需要懂认证）：只接受 method 0x00；收到 0x02 之类的
//! 方法列表一律回 05 FF 并关闭。上游侧用 RFC 1929 出示凭据。
//!
//! 关键约束：
//!
//! - **远程 DNS**：ATYP=0x03 的域名**原样**转给上游解析，本地不做任何解析（不泄漏、不说谎）；
//! - CMD != CONNECT（BIND / UDP ASSOCIATE）→ REP=0x07；ATYP 不支持 → 0x08；
//!   RSV != 0 / 空域名 / PORT=0 → 0x01；
//! - 上游认证失败 → REP=0x01 + egress_relay_upstream_unreachable(reason=auth_failed)；
//! - 隧道与 HTTP 侧共用 crate::relay::pipe::pump：**两个方向都受 idle 约束**。

use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::relay::http::ProxyContext;
use crate::relay::net;
use crate::relay::pipe::{now_millis, pump, read_some, shutdown_write, write_all};

/// 协议版本。
pub const VER: u8 = 0x05;
/// 无认证方法。
pub const METHOD_NO_AUTH: u8 = 0x00;
/// 用户名/密码方法（RFC 1929）。
pub const METHOD_USERPASS: u8 = 0x02;
/// 没有可接受的方法。
pub const METHOD_NONE: u8 = 0xFF;
/// CONNECT 命令。
pub const CMD_CONNECT: u8 = 0x01;
/// IPv4 地址。
pub const ATYP_V4: u8 = 0x01;
/// 域名地址（远程解析）。
pub const ATYP_DOMAIN: u8 = 0x03;
/// IPv6 地址。
pub const ATYP_V6: u8 = 0x04;
/// 成功。
pub const REP_SUCCEEDED: u8 = 0x00;
/// 一般失败（含上游不可达 / 认证失败 / 超限）。
pub const REP_FAILURE: u8 = 0x01;
/// 命令不支持（BIND / UDP ASSOCIATE）。
pub const REP_COMMAND_UNSUPPORTED: u8 = 0x07;
/// 地址类型不支持。
pub const REP_ATYP_UNSUPPORTED: u8 = 0x08;

/// 解析后的 CONNECT 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// 地址类型（1/3/4）。
    pub atyp: u8,
    /// 目标主机（域名原样保留，本地不解析）。
    pub host: String,
    /// 目标端口。
    pub port: u16,
    /// ATYP + ADDR + PORT 的原始字节（转发给上游时逐字节照抄）。
    pub raw: Vec<u8>,
}

/// 请求被拒的原因（映射到 REP）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    /// 非 CONNECT（BIND / UDP ASSOCIATE）。
    NotConnect,
    /// RSV != 0 / 空域名 / PORT = 0 / 版本不对 / 读失败。
    Malformed,
    /// ATYP 不支持。
    AddressType,
}

impl RequestError {
    /// 回给客户端的 REP。
    pub fn reply(self) -> u8 {
        match self {
            RequestError::NotConnect => REP_COMMAND_UNSUPPORTED,
            RequestError::Malformed => REP_FAILURE,
            RequestError::AddressType => REP_ATYP_UNSUPPORTED,
        }
    }
}

/// 带预算的精确读取（超时与 EOF 要能区分，用来选 warning 的 reason）。
async fn read_exact_within<R>(
    reader: &mut R,
    buffer: &mut [u8],
    budget: Duration,
) -> Result<(), &'static str>
where
    R: AsyncRead + Unpin,
{
    let mut filled = 0usize;
    while filled < buffer.len() {
        match timeout(budget, read_some(reader, &mut buffer[filled..])).await {
            Ok(Ok(0)) | Ok(Err(_)) => return Err("protocol_error"),
            Ok(Ok(read)) => filled += read,
            Err(_) => return Err("timeout"),
        }
    }
    Ok(())
}

/// 方法协商：只接受 0x00；接受后回 05 00，否则回 05 FF。
pub async fn negotiate_method<R, W>(reader: &mut R, writer: &mut W, budget: Duration) -> bool
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut head = [0u8; 2];
    if read_exact_within(reader, &mut head, budget).await.is_err() {
        return false;
    }
    if head[0] != VER {
        return false;
    }
    let count = usize::from(head[1]);
    if count == 0 {
        let _ = write_all(writer, &[VER, METHOD_NONE]).await;
        return false;
    }
    let mut methods = vec![0u8; count];
    if read_exact_within(reader, &mut methods, budget)
        .await
        .is_err()
    {
        return false;
    }
    if !methods.contains(&METHOD_NO_AUTH) {
        // 只提供 0x02 之类的（本地不做认证）→ 05 FF + 关闭
        let _ = write_all(writer, &[VER, METHOD_NONE]).await;
        return false;
    }
    write_all(writer, &[VER, METHOD_NO_AUTH]).await.is_ok()
}

/// 读取 CONNECT 请求（ATYP 1/3/4）。
pub async fn read_request<R>(reader: &mut R, budget: Duration) -> Result<Request, RequestError>
where
    R: AsyncRead + Unpin,
{
    let mut head = [0u8; 4];
    read_exact_within(reader, &mut head, budget)
        .await
        .map_err(|_| RequestError::Malformed)?;
    if head[0] != VER {
        return Err(RequestError::Malformed);
    }
    if head[1] != CMD_CONNECT {
        return Err(RequestError::NotConnect);
    }
    if head[2] != 0x00 {
        return Err(RequestError::Malformed);
    }
    let atyp = head[3];
    let (host, address) = match atyp {
        ATYP_V4 => {
            let mut bytes = [0u8; 4];
            read_exact_within(reader, &mut bytes, budget)
                .await
                .map_err(|_| RequestError::Malformed)?;
            (std::net::Ipv4Addr::from(bytes).to_string(), bytes.to_vec())
        }
        ATYP_V6 => {
            let mut bytes = [0u8; 16];
            read_exact_within(reader, &mut bytes, budget)
                .await
                .map_err(|_| RequestError::Malformed)?;
            (std::net::Ipv6Addr::from(bytes).to_string(), bytes.to_vec())
        }
        ATYP_DOMAIN => {
            let mut length = [0u8; 1];
            read_exact_within(reader, &mut length, budget)
                .await
                .map_err(|_| RequestError::Malformed)?;
            if length[0] == 0 {
                return Err(RequestError::Malformed);
            }
            let mut name = vec![0u8; usize::from(length[0])];
            read_exact_within(reader, &mut name, budget)
                .await
                .map_err(|_| RequestError::Malformed)?;
            let host = String::from_utf8_lossy(&name).into_owned();
            let mut raw = vec![length[0]];
            raw.extend_from_slice(&name);
            (host, raw)
        }
        _ => return Err(RequestError::AddressType),
    };
    let mut port_bytes = [0u8; 2];
    read_exact_within(reader, &mut port_bytes, budget)
        .await
        .map_err(|_| RequestError::Malformed)?;
    let port = u16::from_be_bytes(port_bytes);
    if port == 0 {
        return Err(RequestError::Malformed);
    }
    let mut raw = vec![atyp];
    raw.extend_from_slice(&address);
    raw.extend_from_slice(&port_bytes);
    Ok(Request {
        atyp,
        host,
        port,
        raw,
    })
}

/// 回一个 REP（BND 用 0.0.0.0:0，契约允许）。
pub async fn reply<W>(writer: &mut W, code: u8) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    write_all(writer, &[VER, code, 0x00, ATYP_V4, 0, 0, 0, 0, 0, 0]).await
}

/// 在协议解析阶段拒绝一条连接（超限流等）：回 REP 后关闭。
pub async fn reject<W>(stream: &mut W, code: u8)
where
    W: AsyncWrite + Unpin,
{
    let _ = reply(stream, code).await;
    let _ = shutdown_write(stream).await;
}

/// 上游握手（RFC 1929）：有凭据只报 0x02，无凭据报 0x00。
pub async fn negotiate_upstream<R, W>(
    reader: &mut R,
    writer: &mut W,
    credentials: Option<&crate::relay::Credentials>,
    budget: Duration,
) -> Result<(), &'static str>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let greeting: [u8; 3] = match credentials {
        Some(_) => [VER, 1, METHOD_USERPASS],
        None => [VER, 1, METHOD_NO_AUTH],
    };
    write_all(writer, &greeting).await.map_err(|_| "connect")?;
    let mut answer = [0u8; 2];
    read_exact_within(reader, &mut answer, budget).await?;
    if answer[0] != VER {
        return Err("protocol_error");
    }
    match (credentials, answer[1]) {
        (Some(credentials), METHOD_USERPASS) => {
            let user = credentials.username().expose().to_vec();
            let pass = credentials.password().expose().to_vec();
            if user.len() > 255 || pass.len() > 255 {
                return Err("protocol_error");
            }
            let mut auth = Vec::with_capacity(3 + user.len() + pass.len());
            auth.push(0x01);
            auth.push(user.len() as u8);
            auth.extend_from_slice(&user);
            auth.push(pass.len() as u8);
            auth.extend_from_slice(&pass);
            let written = write_all(writer, &auth).await.map_err(|_| "connect");
            // 先清零再判定（凭据不在内存里多留一份）
            auth.iter_mut().for_each(|byte| *byte = 0);
            written?;
            let mut status = [0u8; 2];
            read_exact_within(reader, &mut status, budget).await?;
            if status[1] != 0x00 {
                return Err("auth_failed");
            }
            Ok(())
        }
        (None, METHOD_NO_AUTH) => Ok(()),
        // 上游选了没提供的方法 → 协议错
        _ => Err("protocol_error"),
    }
}

/// 读上游 REPLY（VER REP RSV ATYP BND.ADDR BND.PORT），返回原始字节。
async fn read_reply<R>(reader: &mut R, budget: Duration) -> Result<Vec<u8>, &'static str>
where
    R: AsyncRead + Unpin,
{
    let mut head = [0u8; 4];
    read_exact_within(reader, &mut head, budget).await?;
    if head[0] != VER {
        return Err("protocol_error");
    }
    let mut reply = head.to_vec();
    let address_len = match head[3] {
        ATYP_V4 => 4usize,
        ATYP_V6 => 16usize,
        ATYP_DOMAIN => {
            let mut length = [0u8; 1];
            read_exact_within(reader, &mut length, budget).await?;
            reply.push(length[0]);
            usize::from(length[0])
        }
        _ => return Err("protocol_error"),
    };
    let mut rest = vec![0u8; address_len + 2];
    read_exact_within(reader, &mut rest, budget).await?;
    reply.extend_from_slice(&rest);
    Ok(reply)
}

/// 服务一条 SOCKS5 客户端连接（限流与 uid 双检已由 accept 循环做完）。
pub async fn serve(client: TcpStream, context: Arc<ProxyContext>) {
    let budget = context.timings.client_head;
    // 提前拆成读写两半：协商阶段要同时读方法列表、写选择结果。
    let (mut client_read, mut client_write) = client.into_split();
    // 1) 方法协商（只接受 0x00）
    if !negotiate_method(&mut client_read, &mut client_write, budget).await {
        return;
    }
    // 2) 请求
    let request = match read_request(&mut client_read, context.timings.client_head).await {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(
                proxy = %context.upstream.label,
                port = context.port,
                reason = ?error,
                "relay socks5 request rejected"
            );
            let _ = reply(&mut client_write, error.reply()).await;
            return;
        }
    };
    // 3) 连上游
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
            let _ = reply(&mut client_write, REP_FAILURE).await;
            return;
        }
    };
    let (mut upstream_read, mut upstream_write) = upstream.into_split();

    // 4) 上游握手 + RFC 1929
    if let Err(reason) = negotiate_upstream(
        &mut upstream_read,
        &mut upstream_write,
        context.upstream.credentials.as_deref(),
        context.timings.upstream_handshake,
    )
    .await
    {
        context.record_upstream_failure(reason);
        let _ = reply(&mut client_write, REP_FAILURE).await;
        return;
    }

    // 5) CONNECT：ATYP/ADDR/PORT 原样转给上游（域名由上游解析）
    let mut head = vec![VER, CMD_CONNECT, 0x00];
    head.extend_from_slice(&request.raw);
    if write_all(&mut upstream_write, &head).await.is_err() {
        context.record_upstream_failure("connect");
        let _ = reply(&mut client_write, REP_FAILURE).await;
        return;
    }

    // 6) 上游 REPLY 原样回传（REP != 0 是目标侧问题，不产生 warning）
    let upstream_reply =
        match read_reply(&mut upstream_read, context.timings.upstream_handshake).await {
            Ok(reply) => reply,
            Err(reason) => {
                context.record_upstream_failure(reason);
                let _ = reply(&mut client_write, REP_FAILURE).await;
                return;
            }
        };
    let status = upstream_reply[1];
    if write_all(&mut client_write, &upstream_reply).await.is_err() {
        return;
    }
    if status != REP_SUCCEEDED {
        return;
    }
    context.clear_upstream_failure();

    // 7) 双向管道（两方向都受 idle 约束）
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let address = listener.local_addr().expect("addr");
        let client = TcpStream::connect(address).await.expect("connect");
        let (server, _) = listener.accept().await.expect("accept");
        (client, server)
    }

    fn request_bytes(atyp: u8, address: &[u8], port: u16) -> Vec<u8> {
        let mut bytes = vec![VER, CMD_CONNECT, 0x00, atyp];
        bytes.extend_from_slice(address);
        bytes.extend_from_slice(&port.to_be_bytes());
        bytes
    }

    async fn parse(bytes: Vec<u8>) -> Result<Request, RequestError> {
        let (mut client, mut server) = tcp_pair().await;
        let budget = Duration::from_secs(1);
        let writer = tokio::spawn(async move {
            let _ = write_all(&mut client, &bytes).await;
        });
        let parsed = read_request(&mut server, budget).await;
        let _ = writer.await;
        parsed
    }

    #[tokio::test]
    async fn accepts_ipv4_domain_and_ipv6_targets() {
        let request = parse(request_bytes(ATYP_V4, &[203, 0, 113, 9], 443))
            .await
            .expect("ipv4");
        assert_eq!(request.atyp, ATYP_V4);
        assert_eq!(request.host, "203.0.113.9");
        assert_eq!(request.port, 443);
        assert_eq!(request.raw, vec![ATYP_V4, 203, 0, 113, 9, 0x01, 0xBB]);

        let domain = b"example.com";
        let mut with_len = vec![domain.len() as u8];
        with_len.extend_from_slice(domain);
        let request = parse(request_bytes(ATYP_DOMAIN, &with_len, 80))
            .await
            .expect("domain");
        assert_eq!(request.host, "example.com");
        assert_eq!(request.raw[0], ATYP_DOMAIN, "域名必须原样保留（远程解析）");

        let mut v6 = [0u8; 16];
        v6[15] = 1;
        let request = parse(request_bytes(ATYP_V6, &v6, 8080))
            .await
            .expect("ipv6");
        assert_eq!(request.host, "::1");
        assert_eq!(request.port, 8080);
    }

    #[tokio::test]
    async fn rejects_bind_udp_and_malformed_requests() {
        for command in [0x02u8, 0x03] {
            let mut bytes = request_bytes(ATYP_V4, &[127, 0, 0, 1], 80);
            bytes[1] = command;
            let error = parse(bytes).await.expect_err("必须拒绝");
            assert_eq!(error, RequestError::NotConnect);
            assert_eq!(error.reply(), REP_COMMAND_UNSUPPORTED);
        }
        let mut bytes = request_bytes(ATYP_V4, &[127, 0, 0, 1], 80);
        bytes[2] = 0x01;
        assert_eq!(parse(bytes).await, Err(RequestError::Malformed));
        assert_eq!(
            parse(request_bytes(0x09, &[1, 2, 3, 4], 80)).await,
            Err(RequestError::AddressType)
        );
        assert_eq!(
            parse(request_bytes(ATYP_DOMAIN, &[0x00], 80)).await,
            Err(RequestError::Malformed)
        );
        assert_eq!(
            parse(request_bytes(ATYP_V4, &[127, 0, 0, 1], 0)).await,
            Err(RequestError::Malformed)
        );
        let mut bytes = request_bytes(ATYP_V4, &[127, 0, 0, 1], 80);
        bytes[0] = 0x04;
        assert_eq!(parse(bytes).await, Err(RequestError::Malformed));
    }

    #[tokio::test]
    async fn method_negotiation_accepts_only_no_auth() {
        let budget = Duration::from_secs(1);
        let (mut client, server) = tcp_pair().await;
        let (mut server_read, mut server_write) = server.into_split();
        let task = tokio::spawn(async move {
            let _ = negotiate_method(&mut server_read, &mut server_write, budget).await;
        });
        write_all(&mut client, &[VER, 1, METHOD_NO_AUTH])
            .await
            .expect("write");
        let mut answer = [0u8; 2];
        read_exact_within(&mut client, &mut answer, budget)
            .await
            .expect("read");
        assert_eq!(answer, [VER, METHOD_NO_AUTH]);
        let _ = task.await;

        let (mut client, server) = tcp_pair().await;
        let (mut server_read, mut server_write) = server.into_split();
        let task = tokio::spawn(async move {
            let _ = negotiate_method(&mut server_read, &mut server_write, budget).await;
        });
        write_all(&mut client, &[VER, 1, METHOD_USERPASS])
            .await
            .expect("write");
        let mut answer = [0u8; 2];
        read_exact_within(&mut client, &mut answer, budget)
            .await
            .expect("read");
        assert_eq!(answer, [VER, METHOD_NONE]);
        let _ = task.await;
    }

    #[tokio::test]
    async fn upstream_negotiation_uses_rfc1929_and_maps_auth_failure() {
        let budget = Duration::from_secs(1);
        let (mut client, server) = tcp_pair().await;
        let (mut server_read, mut server_write) = server.into_split();
        let credentials = crate::relay::Credentials::new("alice", "s3cret");
        let task = tokio::spawn(async move {
            negotiate_upstream(
                &mut server_read,
                &mut server_write,
                Some(&credentials),
                budget,
            )
            .await
        });
        let mut greeting = [0u8; 3];
        read_exact_within(&mut client, &mut greeting, budget)
            .await
            .expect("greeting");
        assert_eq!(greeting, [VER, 1, METHOD_USERPASS], "只报 0x02");
        write_all(&mut client, &[VER, METHOD_USERPASS])
            .await
            .expect("method");
        let mut head = [0u8; 2];
        read_exact_within(&mut client, &mut head, budget)
            .await
            .expect("head");
        assert_eq!(head, [0x01, 5]);
        let mut user = [0u8; 5];
        read_exact_within(&mut client, &mut user, budget)
            .await
            .expect("user");
        assert_eq!(&user, b"alice");
        let mut pass_len = [0u8; 1];
        read_exact_within(&mut client, &mut pass_len, budget)
            .await
            .expect("plen");
        let mut pass = vec![0u8; usize::from(pass_len[0])];
        read_exact_within(&mut client, &mut pass, budget)
            .await
            .expect("pass");
        assert_eq!(&pass, b"s3cret");
        write_all(&mut client, &[0x01, 0x01]).await.expect("status");
        assert_eq!(task.await.expect("join"), Err("auth_failed"));
    }
}
