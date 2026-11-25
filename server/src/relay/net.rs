// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 上游连接：**单一总预算** + NODELAY（HTTP 与 SOCKS5 两条数据面共用，F4）。
//!
//! 契约要求「上游 TCP 连接（含 DNS）10 s」——是**总量**，不是每个候选地址各 10 s。
//! 因此 DNS 解析与所有 connect 尝试共享同一个 [\`Instant\`] deadline。

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::time::timeout;

/// 剩余预算（已过期时为 0）。
pub fn remaining_budget(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// 在共享 deadline 内连一个地址；成功时顺手打开 TCP_NODELAY。
async fn connect_within(address: SocketAddr, deadline: Instant) -> Result<TcpStream, &'static str> {
    let remaining = remaining_budget(deadline);
    if remaining.is_zero() {
        return Err("timeout");
    }
    match timeout(remaining, TcpStream::connect(address)).await {
        Ok(Ok(stream)) => {
            // 上游 socket 也关 Nagle（与 listener 侧一致）
            let _ = stream.set_nodelay(true);
            Ok(stream)
        }
        Ok(Err(_)) => Err("connect"),
        Err(_) => Err("timeout"),
    }
}

/// 连上游代理：IP 字面量直连；域名解析后逐个尝试，**全部**在 `budget` 内完成。
pub async fn connect(host: &str, port: u16, budget: Duration) -> Result<TcpStream, &'static str> {
    let deadline = Instant::now() + budget;
    if let Ok(ip) = host.parse::<IpAddr>() {
        return connect_within(SocketAddr::new(ip, port), deadline).await;
    }
    let remaining = remaining_budget(deadline);
    if remaining.is_zero() {
        return Err("timeout");
    }
    let addresses: Vec<SocketAddr> =
        match timeout(remaining, tokio::net::lookup_host((host, port))).await {
            Ok(Ok(addresses)) => addresses.collect(),
            Ok(Err(_)) => return Err("dns"),
            Err(_) => return Err("timeout"),
        };
    if addresses.is_empty() {
        return Err("dns");
    }
    for address in addresses {
        match connect_within(address, deadline).await {
            Ok(stream) => return Ok(stream),
            Err("connect") => continue,
            Err(reason) => return Err(reason),
        }
    }
    Err("connect")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_budget_clamps_at_zero() {
        let now = Instant::now();
        assert!(remaining_budget(now + Duration::from_secs(10)) <= Duration::from_secs(10));
        assert!(remaining_budget(now - Duration::from_secs(1)).is_zero());
    }

    #[tokio::test]
    async fn dns_failure_stays_within_the_shared_budget() {
        let started = Instant::now();
        let outcome = connect("no-such-host.invalid", 80, Duration::from_millis(300)).await;
        assert_eq!(outcome.err(), Some("dns"));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "共享预算必须生效，实际 {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn local_listener_connects_with_nodelay() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let accept = tokio::spawn(async move { listener.accept().await });
        let stream = connect("127.0.0.1", port, Duration::from_secs(2))
            .await
            .expect("connect");
        assert!(
            stream.nodelay().expect("nodelay"),
            "上游 socket 必须开 TCP_NODELAY"
        );
        let _ = accept.await;
    }
}
