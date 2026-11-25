// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 设备内认证中继（relay）的集成测试：listener / 端口表 / 数据面 / API / 失败路径。
//!
//! 覆盖 t2 验收口径：按 `local_port` 起 listener、覆盖式注入 `Proxy-Authorization`、
//! CONNECT 与 absolute-URI 都能转发、>32 KiB 头被拒 400、上游不可达 502、
//! `GET /egress.relay` 与 `/egress/proxies` 字段一致、restart 幂等、bind 失败不写
//! 设备级 settings、凭据零泄漏、`via="relay"` 探测与 fail-open 门控。
//!
//! **真实绑定端口的测试必须串行**：它们共用 17800–17899（[\`RELAY_LOCK\`]）。
//! 其它测试文件的夹具默认不绑定端口（`AppState::with_runner`）。
//!
//! 串行锁是 std `Mutex`，且故意跨 await 持有：它把**整个用例**串起来，而每个
//! `#[tokio::test]` 都有自己的 current-thread runtime，因此不存在互等死锁。
#![allow(clippy::await_holding_lock)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use axum::http::{Method, StatusCode};
use common::{
    ADMIN_TOKEN, OPS_TOKEN, body_json, body_text, fixture_with_relay_and_runner, get_with_token,
    if_match, json_call,
};
use device_faker_server::commands::{CommandOutcome, CommandRunner};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};

/// 真实绑定端口的测试串行锁。
static RELAY_LOCK: Mutex<()> = Mutex::new(());

fn relay_guard() -> std::sync::MutexGuard<'static, ()> {
    RELAY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// 模拟 Android settings（relay 生效时设备级期望值是 127.0.0.1:base_port）
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct SimInner {
    proxy: Option<String>,
    calls: Vec<String>,
    /// pm list packages -U 的固定输出（None = 返回空 → 包未安装）。
    pm_output: Option<String>,
    /// 多用户 uid：userId → uid（优先于 pm_output）。
    pm_uids: std::collections::BTreeMap<String, u32>,
    /// 模拟 iptables 不可用/每条命令都失败（§5.5 失败策略）。
    iptables_failure: bool,
    /// cmd user list 的输出（None = 只有 user 0）。
    user_list: Option<String>,
    /// 假 iptables 的链内容（chain → 规则文本，保持顺序）。
    chains: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default)]
struct SettingsSim {
    inner: Arc<Mutex<SimInner>>,
}

impl SettingsSim {
    fn new() -> Self {
        Self::default()
    }

    /// 每个用户解析出不同 uid（多用户 / pkg@uid 场景）。
    fn with_pm_uids(self, pairs: &[(&str, u32)]) -> Self {
        {
            let mut inner = self.lock();
            for (user, uid) in pairs {
                inner.pm_uids.insert((*user).to_owned(), *uid);
            }
            let mut listing = String::new();
            for (user, _) in pairs {
                listing.push_str(&format!("\tUserInfo{{{user}:User:{user}}} running\n"));
            }
            inner.user_list = Some(listing);
        }
        self
    }

    /// 让所有 iptables 命令都失败（模拟没装 / 没权限，§5.5）。
    fn with_iptables_failure(self) -> Self {
        self.lock().iptables_failure = true;
        self
    }

    /// 读某条链当前的规则（假 iptables）。
    fn chain_rules(&self, chain: &str) -> Vec<String> {
        self.lock().chains.get(chain).cloned().unwrap_or_default()
    }

    /// 让 pm list packages -U 能解析出 uid（应用级中继需要）。
    fn with_pm_uid(self, uid: u32) -> Self {
        self.lock().pm_output = Some(format!("package:com.example.app uid:{uid}\n"));
        self
    }

    fn proxy(&self) -> Option<String> {
        self.lock().proxy.clone()
    }

    fn calls(&self) -> Vec<String> {
        self.lock().calls.clone()
    }

    /// 模拟「有人手工 iptables -F <chain>」：让 -S 与期望不一致。
    fn flush_chain(&self, chain: &str) {
        self.lock().chains.insert(chain.to_owned(), Vec::new());
    }

    fn chain_len(&self, chain: &str) -> usize {
        self.lock().chains.get(chain).map(Vec::len).unwrap_or(0)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SimInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 极简 iptables：只实现 relay 用到的 -N/-F/-X/-S/-C/-A/-I/-D。
    fn iptables(&self, args: &[&str]) -> CommandOutcome {
        let mut inner = self.lock();
        if inner.iptables_failure {
            return CommandOutcome::Failed {
                code: Some(1),
                stderr: String::new(),
            };
        }
        inner.calls.push(format!("iptables {}", args.join(" ")));
        let rest = if args.len() >= 2 && args[0] == "-w" {
            &args[2..]
        } else {
            args
        };
        let failure = || CommandOutcome::Failed {
            code: Some(1),
            stderr: String::new(),
        };
        let (Some(verb), Some(chain)) = (rest.first().copied(), rest.get(1).copied()) else {
            return failure();
        };
        match verb {
            "-N" => {
                inner.chains.entry(chain.to_owned()).or_default();
                CommandOutcome::Ok(String::new())
            }
            "-F" => {
                inner.chains.insert(chain.to_owned(), Vec::new());
                CommandOutcome::Ok(String::new())
            }
            "-X" => {
                inner.chains.remove(chain);
                CommandOutcome::Ok(String::new())
            }
            "-S" => match inner.chains.get(chain) {
                Some(rules) => {
                    let mut out = format!("-N {chain}\n");
                    for rule in rules {
                        // 忠实模拟真机：-d 在 -o 之前，且 --dport/--uid-owner 前带隐式模块标记
                        out.push_str(&format!("-A {chain} {}\n", device_style(rule)));
                    }
                    CommandOutcome::Ok(out)
                }
                None => failure(),
            },
            "-C" => {
                let rule = rest[2..].join(" ");
                let present = inner
                    .chains
                    .get(chain)
                    .is_some_and(|rules| rules.contains(&rule));
                if present {
                    CommandOutcome::Ok(String::new())
                } else {
                    failure()
                }
            }
            "-A" => {
                let rule = rest[2..].join(" ");
                inner.chains.entry(chain.to_owned()).or_default().push(rule);
                CommandOutcome::Ok(String::new())
            }
            "-I" => {
                let tail = rest[2..].join(" ");
                let rule = tail.split_once(' ').map(|(_, rule)| rule).unwrap_or(&tail);
                let rules = inner.chains.entry(chain.to_owned()).or_default();
                rules.insert(0, rule.to_owned());
                CommandOutcome::Ok(String::new())
            }
            "-D" => {
                let rule = rest[2..].join(" ");
                if let Some(rules) = inner.chains.get_mut(chain) {
                    rules.retain(|existing| *existing != rule);
                }
                CommandOutcome::Ok(String::new())
            }
            _ => failure(),
        }
    }
}

fn proxy_host(value: &str) -> Option<String> {
    if value == ":0" || value == "null" {
        return None;
    }
    let (host, _) = value.rsplit_once(':')?;
    (!host.is_empty()).then(|| host.to_owned())
}

fn proxy_port(value: &str) -> Option<u16> {
    if value == ":0" || value == "null" {
        return None;
    }
    value.rsplit_once(':')?.1.parse().ok()
}

/// 把我们的期望规则串渲染成真机 iptables -S 的样子：
/// -d 在 -o 之前，--dport / --uid-owner 前补上隐式模块标记（-m tcp / -m owner）。
fn device_style(rule: &str) -> String {
    let tokens: Vec<&str> = rule.split_whitespace().collect();
    let mut dest: Option<String> = None;
    let mut iface: Option<String> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "-d" => {
                dest = tokens.get(index + 1).map(|value| (*value).to_owned());
                index += 2;
            }
            "-o" => {
                iface = tokens.get(index + 1).map(|value| (*value).to_owned());
                index += 2;
            }
            "--uid-owner" => {
                rest.push("-m".to_owned());
                rest.push("owner".to_owned());
                rest.push(tokens[index].to_owned());
                if let Some(value) = tokens.get(index + 1) {
                    rest.push((*value).to_owned());
                }
                index += 2;
            }
            "--dport" => {
                rest.push("-m".to_owned());
                rest.push("tcp".to_owned());
                rest.push(tokens[index].to_owned());
                if let Some(value) = tokens.get(index + 1) {
                    rest.push((*value).to_owned());
                }
                index += 2;
            }
            other => {
                rest.push(other.to_owned());
                index += 1;
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    if let Some(dest) = dest {
        out.push("-d".to_owned());
        out.push(dest);
    }
    if let Some(iface) = iface {
        out.push("-o".to_owned());
        out.push(iface);
    }
    out.extend(rest);
    out.join(" ")
}

impl CommandRunner for SettingsSim {
    fn run(&self, program: &str, args: &[&str]) -> CommandOutcome {
        if program == "iptables" {
            return self.iptables(args);
        }
        let key = format!("{program} {}", args.join(" "));
        let mut inner = self.lock();
        inner.calls.push(key);
        match (program, args) {
            ("settings", ["put", "global", "http_proxy", value]) => {
                inner.proxy = Some((*value).to_owned());
                CommandOutcome::Ok(String::new())
            }
            ("settings", ["put", "global", "global_http_proxy_exclusion_list", _]) => {
                CommandOutcome::Ok(String::new())
            }
            ("settings", ["get", "global", "http_proxy"]) => {
                CommandOutcome::Ok(inner.proxy.clone().unwrap_or_else(|| "null".to_owned()))
            }
            ("settings", ["get", "global", "global_http_proxy_host"]) => CommandOutcome::Ok(
                inner
                    .proxy
                    .as_deref()
                    .and_then(proxy_host)
                    .unwrap_or_else(|| "null".to_owned()),
            ),
            ("settings", ["get", "global", "global_http_proxy_exclusion_list"]) => {
                CommandOutcome::Ok("null".to_owned())
            }
            ("cmd", ["user", "list"]) => CommandOutcome::Ok(
                inner
                    .user_list
                    .clone()
                    .unwrap_or_else(|| "UserInfo{0:Owner:13} running\n".to_owned()),
            ),
            ("pm", ["list", "packages", "-U", "--user", user, _package]) => {
                match inner.pm_uids.get(*user) {
                    Some(uid) => CommandOutcome::Ok(format!(
                        "package:com.example.app uid:{uid}
"
                    )),
                    None => CommandOutcome::Ok(inner.pm_output.clone().unwrap_or_default()),
                }
            }
            ("dumpsys", ["connectivity"]) => {
                let line = match inner.proxy.as_deref() {
                    Some(value) => match (proxy_host(value), proxy_port(value)) {
                        (Some(host), Some(port)) => {
                            format!("Global proxy: [{host}] port: {port}")
                        }
                        _ => "Global proxy: none".to_owned(),
                    },
                    None => "Global proxy: none".to_owned(),
                };
                CommandOutcome::Ok(format!("{line}\n"))
            }
            _ => CommandOutcome::Ok(String::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// mock 上游代理（记录收到的请求头；支持 absolute-URI 与 CONNECT 隧道）
// ---------------------------------------------------------------------------

struct MockUpstream {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl MockUpstream {
    fn heads(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 关掉 mock（后续连接会被拒 = 上游不可达）。
    fn stop(&self) {
        self.task.abort();
    }
}

async fn spawn_upstream() -> MockUpstream {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("mock upstream");
    let port = listener.local_addr().expect("addr").port();
    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let records = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let records = Arc::clone(&records);
            tokio::spawn(async move {
                let mut buffer: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let read = read_some(&mut stream, &mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                    if buffer.len() > 64 * 1024 {
                        return;
                    }
                }
                let text = String::from_utf8_lossy(&buffer).into_owned();
                records
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(text.clone());
                if text.starts_with("CONNECT") {
                    let _ = write_all(&mut stream, b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await;
                    // 纯字节回显
                    let mut chunk = [0u8; 1024];
                    loop {
                        let read = read_some(&mut stream, &mut chunk).await.unwrap_or(0);
                        if read == 0 {
                            break;
                        }
                        if write_all(&mut stream, &chunk[..read]).await.is_err() {
                            break;
                        }
                    }
                } else {
                    let _ = write_all(
                        &mut stream,
                        b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n203.0.113.7\n",
                    )
                    .await;
                }
            });
        }
    });
    MockUpstream {
        port,
        requests,
        task,
    }
}

// --- 最小异步 IO（不依赖 tokio 的 io-util feature）---

async fn read_some(stream: &mut TcpStream, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut read_buf = tokio::io::ReadBuf::new(buffer);
    std::future::poll_fn(|context: &mut Context<'_>| {
        Pin::new(&mut *stream).poll_read(context, &mut read_buf)
    })
    .await?;
    Ok(read_buf.filled().len())
}

async fn write_all(stream: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    let mut remaining = data;
    std::future::poll_fn(|context: &mut Context<'_>| {
        while !remaining.is_empty() {
            match Pin::new(&mut *stream).poll_write(context, remaining) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(std::io::Error::from(std::io::ErrorKind::WriteZero)));
                }
                Poll::Ready(Ok(written)) => remaining = &remaining[written..],
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
        Poll::Ready(Ok(()))
    })
    .await
}

// ---------------------------------------------------------------------------
// 客户端工具
// ---------------------------------------------------------------------------

/// 连本地中继端口 → 写一段请求 → 读到 EOF。
async fn send_raw(port: u16, request: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连本地中继失败");
    write_all(&mut stream, request.as_bytes())
        .await
        .expect("write request");
    let mut out = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        match read_some(&mut stream, &mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => out.extend_from_slice(&chunk[..read]),
        }
    }
    out
}

/// CONNECT 隧道：握手成功后发一段字节，读回同样的字节。
async fn connect_and_echo(port: u16, authority: &str, payload: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连本地中继失败");
    let request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n");
    write_all(&mut stream, request.as_bytes())
        .await
        .expect("write connect");
    // 读握手响应头
    let mut head: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 512];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = read_some(&mut stream, &mut chunk).await.expect("read head");
        assert!(read > 0, "握手期间对端关闭");
        head.extend_from_slice(&chunk[..read]);
    }
    let head_text = String::from_utf8_lossy(&head).into_owned();
    assert!(head_text.starts_with("HTTP/1.1 200"), "{head_text}");
    write_all(&mut stream, payload)
        .await
        .expect("write payload");
    let mut echoed = vec![0u8; payload.len()];
    let mut filled = 0usize;
    while filled < echoed.len() {
        let read = read_some(&mut stream, &mut echoed[filled..])
            .await
            .expect("read echo");
        assert!(read > 0, "回显期间对端关闭");
        filled += read;
    }
    echoed
}

// ---------------------------------------------------------------------------
// API 工具
// ---------------------------------------------------------------------------

/// 每个用例用自己的 base_port（17800–17899），避免上一个用例的 listener 还没释放时自撞。
fn use_base_port(fixture: &common::Fixture, base_port: u16) {
    let path = fixture.state.egress().store().path().to_path_buf();
    std::fs::write(
        &path,
        format!("version = 1\n\n[relay]\nbase_port = {base_port}\n"),
    )
    .expect("write egress.toml");
}

async fn shutdown(fixture: &common::Fixture) {
    fixture.state.relay().shutdown().await;
}

async fn eg_revision(router: &axum::Router) -> String {
    let body = body_json(get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    body["revision"].as_str().expect("revision").to_owned()
}

async fn put_egress(router: &axum::Router, revision: &str, payload: Value) -> Value {
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(revision))],
        &payload,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "PUT /egress 必须成功");
    body_json(response).await
}

fn warning_codes(body: &Value) -> Vec<String> {
    body["warnings"]
        .as_array()
        .map(|warnings| {
            warnings
                .iter()
                .filter_map(|warning| warning["code"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn credential_proxy(name: &str, port: u16) -> Value {
    json!({
        "name": name,
        "type": "http",
        "host": "127.0.0.1",
        "port": port,
        "username": "alice",
        "password": "s3cret",
    })
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[tokio::test]
async fn put_actually_writes_the_egress_file() {
    // 上次事故的教训：语法平衡 != 语义正确。这条用例只看「文件真的变了」。
    let sim = SettingsSim::new();
    let fixture = common::fixture_with_arc_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    let path = fixture.state.egress().store().path().to_path_buf();
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", 8080)],
        }),
    )
    .await;
    assert_eq!(body["changed"], true, "{body}");
    let written = std::fs::read_to_string(&path).expect("读回 egress.toml");
    assert!(
        written.contains("hk_resi") && written.contains("127.0.0.1"),
        "PUT 必须真的把定义写进文件: {written}"
    );
}

#[tokio::test]
async fn listener_starts_on_local_port_and_injects_basic() {
    const BASE_PORT: u16 = 17800;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    let router = &fixture.router;

    use_base_port(&fixture, BASE_PORT);
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;

    // relay 对象（契约 §6.1）
    assert_eq!(body["relay"]["enabled"], true, "{body}");
    assert_eq!(body["relay"]["basePort"], BASE_PORT, "{body}");
    assert_eq!(body["relay"]["failOpen"], true, "{body}");
    assert_eq!(body["relay"]["limits"]["maxConnections"], 256, "{body}");
    assert_eq!(
        body["relay"]["limits"]["maxConnectionsPerUid"], 64,
        "{body}"
    );
    let listener = &body["relay"]["listeners"][0];
    assert_eq!(listener["proxy"], "hk_resi", "{body}");
    assert_eq!(listener["port"], BASE_PORT, "{body}");
    assert_eq!(listener["scope"], "device", "{body}");
    assert_eq!(listener["listening"], true, "{body}");
    assert_eq!(listener["uidPolicy"], "all", "{body}");
    assert!(listener["lastError"].is_null(), "{body}");
    assert_eq!(
        body["relay"]["portChanges"][0]["reason"], "initial",
        "{body}"
    );
    assert_eq!(
        body["relay"]["restartRequiredApps"],
        json!([]),
        "设备级端口变化不要求重启应用: {body}"
    );

    // 设备级 settings 指向中继（决策 #1 / 契约 §3.1 规则 4）
    assert_eq!(sim.proxy().as_deref(), Some("127.0.0.1:17800"));

    // 端口表落盘 + 0600
    let path = fixture.state.egress().store().path().to_path_buf();
    let text = std::fs::read_to_string(&path).expect("egress.toml");
    assert!(text.contains("local_port = 17800"), "{text}");
    let mode = std::fs::metadata(&path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "端口表与凭据同文件，必须 0600");

    // GET /egress/proxies 与 GET /egress 同构（契约 §6.2）
    let proxies =
        body_json(get_with_token(router, "/api/v1/egress/proxies", ADMIN_TOKEN).await).await;
    assert_eq!(proxies["items"][0]["localPort"], 17800, "{proxies}");
    assert_eq!(proxies["items"][0]["relayActive"], true, "{proxies}");
    let egress = body_json(get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(egress["proxies"][0]["localPort"], 17800, "{egress}");
    assert_eq!(egress["proxies"][0]["relayActive"], true, "{egress}");
    assert_eq!(
        egress["upstream"]["host"], "127.0.0.1",
        "upstream 永远是真实上游: {egress}"
    );

    // 数据面：absolute-URI 转发 + 覆盖式注入
    let response = send_raw(
        BASE_PORT,
        "GET http://example.com/ip HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: Basic Ym9ndXM=\r\nUser-Agent: t2\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response).into_owned();
    assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
    assert!(text.contains("Connection: close"), "{text}");
    assert!(text.contains("203.0.113.7"), "{text}");

    let heads = upstream.heads();
    assert_eq!(heads.len(), 1, "{heads:?}");
    let head = &heads[0];
    assert_eq!(
        head.matches("Proxy-Authorization").count(),
        1,
        "覆盖而非叠加: {head}"
    );
    assert!(
        head.contains("Proxy-Authorization: Basic YWxpY2U6czNjcmV0"),
        "上游必须收到正确的 Basic: {head}"
    );
    assert!(!head.contains("Ym9ndXM="), "客户端同名头必须丢弃: {head}");
    assert!(
        head.starts_with("GET http://example.com/ip HTTP/1.1"),
        "{head}"
    );
    assert!(head.contains("User-Agent: t2"), "端到端头保留: {head}");

    // 释放本用例的 listener：同进程的后续用例要复用 178xx 段。
    shutdown(&fixture).await;
}

#[tokio::test]
async fn connect_form_tunnels_bytes() {
    const BASE_PORT: u16 = 17805;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    let router = &fixture.router;
    use_base_port(&fixture, BASE_PORT);
    let revision = eg_revision(router).await;
    put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;

    let payload = b"ping-through-connect";
    let echoed = connect_and_echo(BASE_PORT, "example.com:443", payload).await;
    assert_eq!(echoed, payload);

    let heads = upstream.heads();
    assert_eq!(heads.len(), 1, "{heads:?}");
    let head = &heads[0];
    assert!(
        head.starts_with("CONNECT example.com:443 HTTP/1.1"),
        "{head}"
    );
    assert!(head.contains("Host: example.com:443"), "{head}");
    assert!(
        head.contains("Proxy-Authorization: Basic YWxpY2U6czNjcmV0"),
        "{head}"
    );
    // 「只发三行头」：不能再多出别的头
    assert_eq!(
        head.lines().count(),
        4,
        "CONNECT 只发请求行 + 2 头 + 空行: {head}"
    );

    // 释放本用例的 listener：同进程的后续用例要复用 178xx 段。
    shutdown(&fixture).await;
}

#[tokio::test]
async fn oversized_header_is_rejected_with_400() {
    const BASE_PORT: u16 = 17810;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    let router = &fixture.router;
    use_base_port(&fixture, BASE_PORT);
    let revision = eg_revision(router).await;
    put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;

    let request = format!(
        "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nX-Big: {}\r\n\r\n",
        "a".repeat(40 * 1024)
    );
    let response = send_raw(BASE_PORT, &request).await;
    let text = String::from_utf8_lossy(&response).into_owned();
    assert!(text.starts_with("HTTP/1.1 400"), "{text}");
    assert!(upstream.heads().is_empty(), "超限请求不得转发到上游");

    // 释放本用例的 listener：同进程的后续用例要复用 178xx 段。
    shutdown(&fixture).await;
}

#[tokio::test]
async fn upstream_unreachable_maps_to_502_and_warns() {
    const BASE_PORT: u16 = 17815;
    let _guard = relay_guard();
    // 先占一个端口再关掉：保证「连上去会被拒」
    let probe = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let dead_port = probe.local_addr().expect("addr").port();
    drop(probe);

    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    let router = &fixture.router;
    use_base_port(&fixture, BASE_PORT);
    let revision = eg_revision(router).await;
    put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", dead_port)],
        }),
    )
    .await;

    let response = send_raw(
        BASE_PORT,
        "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response).into_owned();
    assert!(text.starts_with("HTTP/1.1 502"), "{text}");

    let body = body_json(get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    let codes = warning_codes(&body);
    assert!(
        codes
            .iter()
            .any(|code| code == "egress_relay_upstream_unreachable"),
        "{codes:?}"
    );
    let warning = body["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "egress_relay_upstream_unreachable")
        .expect("warning");
    assert_eq!(warning["details"]["scope"], "device", "{warning}");
    assert_eq!(warning["details"]["port"], BASE_PORT, "{warning}");
    assert_eq!(warning["details"]["reason"], "connect", "{warning}");

    // 释放本用例的 listener：同进程的后续用例要复用 178xx 段。
    shutdown(&fixture).await;
}

#[tokio::test]
async fn bind_failure_skips_settings_and_warns() {
    const BASE_PORT: u16 = 17820;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    // 抢占设备级端口：让 relay 绑定失败（EADDRINUSE）
    let occupied = std::net::TcpListener::bind(("127.0.0.1", BASE_PORT)).expect("occupy");
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    let router = &fixture.router;
    use_base_port(&fixture, BASE_PORT);
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;

    let listener = &body["relay"]["listeners"][0];
    assert_eq!(listener["listening"], false, "{body}");
    assert_eq!(
        listener["lastError"]["code"], "egress_relay_bind_failed",
        "{body}"
    );
    assert_eq!(listener["lastError"]["reason"], "addr_in_use", "{body}");
    let codes = warning_codes(&body);
    assert!(
        codes.iter().any(|code| code == "egress_relay_bind_failed"),
        "{codes:?}"
    );
    let warning = body["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "egress_relay_bind_failed")
        .expect("warning");
    assert_eq!(warning["details"]["scope"], "device", "{warning}");
    assert_eq!(warning["details"]["port"], BASE_PORT, "{warning}");

    // 决策 #1：绑定失败 → 设备级**不启用代理**（写 :0 或保持直连），
    // 重点是绝不把 settings 指向一个没人监听的端口。
    let settings_value = sim.proxy();
    assert!(
        settings_value.is_none() || settings_value.as_deref() == Some(":0"),
        "绑定失败时 settings 必须是直连，实际 {settings_value:?}"
    );
    assert_ne!(
        settings_value.as_deref(),
        Some(format!("127.0.0.1:{BASE_PORT}").as_str())
    );
    drop(occupied);

    // 释放本用例的 listener：同进程的后续用例要复用 178xx 段。
    shutdown(&fixture).await;
}

#[tokio::test]
async fn restart_is_idempotent_and_keeps_listening() {
    const BASE_PORT: u16 = 17825;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    let router = &fixture.router;
    use_base_port(&fixture, BASE_PORT);
    let revision = eg_revision(router).await;
    put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;

    let first = body_json(
        json_call(
            router,
            Method::POST,
            "/api/v1/egress/relay/restart",
            OPS_TOKEN,
            &[("content-type", "application/json")],
            &json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(first["relay"]["listeners"][0]["listening"], true, "{first}");
    assert_eq!(
        first["relay"]["portChanges"],
        json!([]),
        "无变化时不能报端口变化: {first}"
    );
    assert_eq!(first["relay"]["restartRequiredApps"], json!([]), "{first}");
    assert!(first["restartedAt"].is_string(), "{first}");

    // 第二次同样稳定：端口不变、仍然在监听
    let second = body_json(
        json_call(
            router,
            Method::POST,
            "/api/v1/egress/relay/restart",
            OPS_TOKEN,
            &[("content-type", "application/json")],
            &json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(
        second["relay"]["listeners"][0]["port"], BASE_PORT,
        "{second}"
    );
    assert_eq!(second["relay"]["portChanges"], json!([]), "{second}");

    // 重建后数据面仍然可用（listener 真的在 accept）
    let response = send_raw(
        BASE_PORT,
        "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
    )
    .await;
    assert!(
        String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200"),
        "restart 后必须仍然可转发"
    );

    // 请求体不接受任何字段
    let response = json_call(
        router,
        Method::POST,
        "/api/v1/egress/relay/restart",
        OPS_TOKEN,
        &[("content-type", "application/json")],
        &json!({ "force": true }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // 释放本用例的 listener：同进程的后续用例要复用 178xx 段。
    shutdown(&fixture).await;
}

#[tokio::test]
async fn probe_via_relay_uses_the_local_port_and_never_fails_open() {
    const BASE_PORT: u16 = 17830;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    let router = &fixture.router;
    use_base_port(&fixture, BASE_PORT);
    let revision = eg_revision(router).await;
    put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
            "probeUrl": "http://example.com/ip",
        }),
    )
    .await;

    // 成功：via=relay 恒回 via="relay" + relayPort
    let body = body_json(
        json_call(
            router,
            Method::POST,
            "/api/v1/egress/probe",
            OPS_TOKEN,
            &[("content-type", "application/json")],
            &json!({ "via": "relay" }),
        )
        .await,
    )
    .await;
    assert_eq!(body["via"], "relay", "{body}");
    assert_eq!(body["relayPort"], BASE_PORT, "{body}");
    assert_eq!(body["proxy"], "hk_resi", "{body}");

    // 上游挂掉：via=relay 失败 → 502，但**绝不**动 settings（fail-closed）
    let before = sim.proxy();
    assert_eq!(
        before,
        Some(format!("127.0.0.1:{BASE_PORT}")),
        "设备级 settings 必须指向本用例的中继端口"
    );
    upstream.stop();
    let response = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[("content-type", "application/json")],
        &json!({ "via": "relay" }),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "中继把上游失败映射成 502"
    );
    assert_eq!(
        sim.proxy(),
        before,
        "via=relay 的失败一律不动 settings（否则 fail-closed 变 IP 泄漏）"
    );
    assert_eq!(
        sim.calls()
            .iter()
            .filter(|call| call.starts_with("settings put global http_proxy :0"))
            .count(),
        0,
        "不得出现任何清直连"
    );

    // 释放本用例的 listener：同进程的后续用例要复用 178xx 段。
    shutdown(&fixture).await;
}

#[tokio::test]
async fn relay_disabled_keeps_legacy_reference_behaviour() {
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    // 预置 [relay] enabled=false + 一条带 local_port 的定义（PUT 不写 [relay]，只能手写文件）。
    // 关闭中继时端口表必须**保留**（契约 §1.6：不释放、不重算）。
    let path = fixture.state.egress().store().path().to_path_buf();
    std::fs::write(
        &path,
        format!(
            "version = 1\n\n[relay]\nenabled = false\n\n[proxies.hk_resi]\ntype = \"http\"\nhost = \"127.0.0.1\"\nport = {}\nusername = \"alice\"\npassword = \"s3cret\"\nlocal_port = 17860\n",
            upstream.port
        ),
    )
    .expect("write");

    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;

    assert_eq!(body["relay"]["enabled"], false, "{body}");
    assert_eq!(body["relay"]["listeners"], json!([]), "{body}");
    assert_eq!(body["proxies"][0]["localPort"], Value::Null, "{body}");
    assert_eq!(body["proxies"][0]["relayActive"], false, "{body}");
    let codes = warning_codes(&body);
    assert!(
        codes
            .iter()
            .any(|code| code == "egress_credentials_ignored"),
        "relay 关闭时回到 v1.10.0 语义（K3）: {codes:?}"
    );
    assert!(
        codes.iter().any(|code| code == "egress_auth_unsupported"),
        "relay 未生效且带凭据才报 K1: {codes:?}"
    );
    // 设备级写上游真实地址（不指向中继）
    assert_eq!(
        sim.proxy().as_deref(),
        Some(format!("127.0.0.1:{}", upstream.port).as_str())
    );
    // 端口表在文件里**保留**（不释放、不重算），只是不生效
    let text = std::fs::read_to_string(&path).expect("egress.toml");
    assert!(
        text.contains("local_port = 17860"),
        "关闭中继不能释放端口表: {text}"
    );

    // 释放本用例的 listener：同进程的后续用例要复用 178xx 段。
    shutdown(&fixture).await;
}

#[tokio::test]
async fn credentials_never_reach_logs_or_responses() {
    const BASE_PORT: u16 = 17800;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    let router = &fixture.router;

    let capture = CaptureLog::default();
    let buffer = Arc::clone(&capture.0);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(capture)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    use_base_port(&fixture, BASE_PORT);
    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;
    let put_text = body_text(response).await;
    let _ = send_raw(
        BASE_PORT,
        "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
    )
    .await;
    let get_text = body_text(get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    let restart_text = body_text(
        json_call(
            router,
            Method::POST,
            "/api/v1/egress/relay/restart",
            OPS_TOKEN,
            &[("content-type", "application/json")],
            &json!({}),
        )
        .await,
    )
    .await;
    drop(guard);

    let logs = String::from_utf8(
        buffer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
    )
    .expect("utf-8 log");
    let basic = "YWxpY2U6czNjcmV0";
    for (what, text) in [
        ("PUT 响应", &put_text),
        ("GET 响应", &get_text),
        ("restart 响应", &restart_text),
        ("服务日志", &logs),
    ] {
        assert!(!text.contains("s3cret"), "{what} 出现密码: {text}");
        assert!(!text.contains("alice"), "{what} 出现用户名: {text}");
        assert!(
            !text.contains(basic),
            "{what} 出现 Proxy-Authorization 值: {text}"
        );
    }
    assert!(
        !logs.is_empty(),
        "测试期间必须真的产生过日志（否则断言无意义）"
    );

    // 释放本用例的 listener：同进程的后续用例要复用 178xx 段。
    shutdown(&fixture).await;
}

#[tokio::test]
async fn socks5_device_default_is_rejected_by_put() {
    const BASE_PORT: u16 = 17890;
    let _guard = relay_guard();
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let response = common::send_body(
        router,
        Method::PUT,
        "/api/v1/egress",
        Some(ADMIN_TOKEN),
        &[
            ("if-match", &revision),
            ("content-type", "application/json"),
        ],
        Some(
            serde_json::to_vec(&json!({
                "default": "socks_only",
                "proxies": [socks5_proxy("socks_only", 1080)],
            }))
            .expect("json"),
        ),
    )
    .await;
    assert_eq!(
        response.status(),
        422,
        "设备级 default 指向 socks5 必须 422（只支持 http）"
    );
    let body = body_json(response).await;
    let text = body.to_string();
    assert!(
        text.contains("invalid_egress_default"),
        "422 必须带 invalid_egress_default 校验码: {text}"
    );
    assert!(
        text.contains("default"),
        "校验项要指向 default 字段: {text}"
    );
    shutdown(&fixture).await;
}

#[tokio::test]
async fn legacy_socks5_default_fails_closed_to_blackhole() {
    const BASE_PORT: u16 = 17894;
    let _guard = relay_guard();
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    use_base_port(&fixture, BASE_PORT);
    // 已存在的旧配置：config.toml 的 default 指向 socks5，egress.toml 里有该定义
    let config_path = fixture.config_path.clone();
    std::fs::write(
        &config_path,
        [
            "[egress]",
            r#"default = "socks_only""#,
            "",
            "[templates.pixel_9]",
            r#"packages = ["com.example.app"]"#,
        ]
        .join(&char::from(10u8).to_string()),
    )
    .expect("写 config.toml");
    let egress_path = fixture.state.egress().store().path().to_path_buf();
    std::fs::write(
        &egress_path,
        [
            "version = 1",
            "",
            "[proxies.socks_only]",
            r#"type = "socks5""#,
            r#"host = "203.0.113.7""#,
            "port = 1080",
            r#"username = "u""#,
            r#"password = "p""#,
        ]
        .join(&char::from(10u8).to_string()),
    )
    .expect("写 egress.toml");

    // 触发一次 relay 重建（settings 只在写路径上落）
    let restarted = common::send_body(
        &fixture.router,
        Method::POST,
        "/api/v1/egress/relay/restart",
        Some(ADMIN_TOKEN),
        &[("content-type", "application/json")],
        Some(b"{}".to_vec()),
    )
    .await;
    assert_eq!(restarted.status(), 200, "restart 必须成功");
    let body =
        body_json(get_with_token(&fixture.router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert!(
        body["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning["code"] == "egress_relay_device_socks5_unsupported"),
        "旧配置也必须显式告警: {body}"
    );
    let settings = body["readback"]["settingsValue"].as_str().unwrap_or("");
    assert_eq!(
        settings, "127.0.0.1:9",
        "必须写黑洞（fail-closed），不得直连也不得泄漏 socks5 上游地址: {settings}"
    );
    shutdown(&fixture).await;
}

#[tokio::test]
async fn manual_flush_makes_unknown_uid_connections_fail_closed() {
    const BASE_PORT: u16 = 17891;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new().with_pm_uid(10123);
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "proxies": [credential_proxy("jp_app", upstream.port)],
            "apps": [{ "package": "com.example.app", "egress": "jp_app" }],
        }),
    )
    .await;
    assert_eq!(body["relay"]["firewall"]["applied"], true, "{body}");
    let port = u16::try_from(
        body["relay"]["listeners"][0]["port"]
            .as_u64()
            .expect("port"),
    )
    .expect("u16");

    // 规则生效 + uid 无法归属（宿主没有 /proc/net/tcp）→ 分层策略放行
    let first = send_raw(
        port,
        "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
    )
    .await;
    assert!(
        !first.is_empty(),
        "防火墙已生效时 uid 未知应放行，实际 0 字节"
    );

    // 手工 -F 掉 ALLOW 链（模拟第三方脚本/排障误操作），并等过复核窗口
    sim.flush_chain("DF_EGRESS_RLY_ALLOW");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    // 不依赖 GET：直接再连，必须被 fail-closed 关闭
    let second = send_raw(
        port,
        "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
    )
    .await;
    assert!(
        second.is_empty(),
        "复核发现规则被清后 uid 未知必须关闭，实际 {} 字节",
        second.len()
    );
    shutdown(&fixture).await;
}

#[tokio::test]
async fn get_during_restart_never_reports_a_false_applied_window() {
    const BASE_PORT: u16 = 17893;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;

    // 并发：一边重启（重建管线 + -S），一边狂读 GET。
    // view 的期望态取 applied_spec（最近一次成功 apply），规则没变时不能出现 applied=false。
    let restarts = async {
        for _ in 0..3 {
            let response = common::send_body(
                router,
                Method::POST,
                "/api/v1/egress/relay/restart",
                Some(ADMIN_TOKEN),
                &[],
                None,
            )
            .await;
            assert_eq!(response.status(), 200, "restart 必须成功");
        }
    };
    let reads = async {
        for _ in 0..8 {
            let body = body_json(get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
            assert_eq!(
                body["relay"]["firewall"]["applied"], true,
                "重启期间不得出现 applied=false 的假窗口: {body}"
            );
            tokio::task::yield_now().await;
        }
    };
    tokio::join!(restarts, reads);
    shutdown(&fixture).await;
}

#[tokio::test]
async fn duplicate_local_ports_are_reallocated_with_collision_reason() {
    const BASE_PORT: u16 = 17895;
    let _guard = relay_guard();
    let sim = SettingsSim::new().with_pm_uid(10123);
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    use_base_port(&fixture, BASE_PORT);
    // 两个应用各自绑定一个定义（否则不受引用、不会分配端口）
    let config_path = fixture.config_path.clone();
    std::fs::write(
        &config_path,
        [
            "[[apps]]",
            r#"package = "com.example.one""#,
            r#"egress = "alpha""#,
            "",
            "[[apps]]",
            r#"package = "com.example.two""#,
            r#"egress = "beta""#,
        ]
        .join(&char::from(10u8).to_string()),
    )
    .expect("写 config.toml");

    // 手改文件：两个应用级定义写了同一个 local_port
    let egress_path = fixture.state.egress().store().path().to_path_buf();
    let duplicate = [
        "version = 1",
        "",
        "[proxies.alpha]",
        r#"type = "http""#,
        r#"host = "127.0.0.1""#,
        "port = 8080",
        r#"username = "u""#,
        r#"password = "p""#,
        "local_port = 17850",
        "",
        "[proxies.beta]",
        r#"type = "http""#,
        r#"host = "127.0.0.1""#,
        "port = 8080",
        r#"username = "u""#,
        r#"password = "p""#,
        "local_port = 17850",
    ]
    .join(&char::from(10u8).to_string());
    std::fs::write(&egress_path, &duplicate).expect("写 egress.toml");

    let router = &fixture.router;
    let response = common::send_body(
        router,
        Method::POST,
        "/api/v1/egress/relay/restart",
        Some(ADMIN_TOKEN),
        &[("content-type", "application/json")],
        Some(b"{}".to_vec()),
    )
    .await;
    assert_eq!(response.status(), 200, "restart 必须成功");
    let restart_body = body_json(response).await;
    // portChanges 只出现在触发重建的那次响应里（GET 不复述历史）
    let changes = restart_body["relay"]["portChanges"]
        .as_array()
        .expect("portChanges");
    assert!(
        !changes.is_empty(),
        "撞车重分配必须上报 portChanges: {restart_body}"
    );
    assert!(
        !restart_body["relay"]["restartRequiredApps"]
            .as_array()
            .expect("restartRequiredApps")
            .is_empty(),
        "重分配必须提示重启应用: {restart_body}"
    );
    let body = body_json(get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    let listeners = body["relay"]["listeners"].as_array().expect("listeners");
    let ports: Vec<u64> = listeners
        .iter()
        .filter_map(|listener| listener["port"].as_u64())
        .collect();
    assert_eq!(ports.len(), 2, "{body}");
    assert_ne!(ports[0], ports[1], "撞车必须重分配到不同端口: {body}");
    assert!(
        changes
            .iter()
            .any(|change| change["to"].as_u64() == Some(ports[1].max(ports[0]))),
        "portChanges 必须记录新端口: {restart_body}"
    );
    let written = std::fs::read_to_string(&egress_path).expect("读回 egress.toml");
    assert!(
        written.contains("local_port = 17850"),
        "其中一个定义的端口应保留: {written}"
    );
    assert!(
        written.matches("local_port").count() >= 2,
        "端口表必须写回文件: {written}"
    );
    shutdown(&fixture).await;
}

#[tokio::test]
async fn colliding_port_is_reallocated_and_reports_changes() {
    const BASE_PORT: u16 = 17892;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    // 占住该名字的首选端口，强迫分配器线性探测到下一个候选
    let preferred = device_faker_server::relay::port::candidate_ports("jp_app", BASE_PORT)[0];
    let blocker = tokio::net::TcpListener::bind(("127.0.0.1", preferred))
        .await
        .expect("占住首选端口");
    let sim = SettingsSim::new().with_pm_uid(10123);
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "proxies": [credential_proxy("jp_app", upstream.port)],
            "apps": [{ "package": "com.example.app", "egress": "jp_app" }],
        }),
    )
    .await;
    let port = u16::try_from(
        body["relay"]["listeners"][0]["port"]
            .as_u64()
            .expect("port"),
    )
    .expect("u16");
    assert_ne!(port, preferred, "首选端口被占时必须换端口: {body}");
    assert_eq!(body["relay"]["listeners"][0]["listening"], true, "{body}");
    assert!(
        !body["relay"]["portChanges"]
            .as_array()
            .expect("portChanges")
            .is_empty(),
        "重分配必须上报 portChanges: {body}"
    );
    assert!(
        body["relay"]["restartRequiredApps"]
            .as_array()
            .expect("restartRequiredApps")
            .iter()
            .any(|app| app == "com.example.app"),
        "重分配必须提示重启应用: {body}"
    );
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("新端口必须可用");
    drop(blocker);
    shutdown(&fixture).await;
}

#[tokio::test]
async fn delete_releases_the_local_port_and_reallocation_listens() {
    const BASE_PORT: u16 = 17888;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;
    let port = u16::try_from(
        body["relay"]["listeners"][0]["port"]
            .as_u64()
            .expect("port"),
    )
    .expect("u16");
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("首次分配后端口必须可用");

    // DELETE：把 proxies 置空 → 端口表释放
    let revision = eg_revision(router).await;
    let body = put_egress(router, &revision, json!({ "proxies": [] })).await;
    assert!(
        body["relay"]["listeners"]
            .as_array()
            .expect("listeners")
            .is_empty(),
        "删除后不该有 listener: {body}"
    );
    let after = tokio::time::timeout(
        std::time::Duration::from_millis(600),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await;
    assert!(
        after.map(|result| result.is_err()).unwrap_or(true),
        "旧端口必须不再监听: {port}"
    );

    // 重新加回来：重新分配到端口并可用
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;
    let port2 = u16::try_from(
        body["relay"]["listeners"][0]["port"]
            .as_u64()
            .expect("port"),
    )
    .expect("u16");
    tokio::net::TcpStream::connect(("127.0.0.1", port2))
        .await
        .expect("重新分配后端口必须可用");
    shutdown(&fixture).await;
}

#[tokio::test]
async fn device_bind_failure_never_leaves_settings_on_that_port() {
    const BASE_PORT: u16 = 17889;
    let _guard = relay_guard();
    // 先占住 base_port，制造 addr_in_use
    let blocker = tokio::net::TcpListener::bind(("127.0.0.1", BASE_PORT))
        .await
        .expect("占位监听");
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;
    let listener = &body["relay"]["listeners"][0];
    assert_eq!(
        listener["listening"], false,
        "端口被占时必须 listening=false: {body}"
    );
    let settings = body["readback"]["settingsValue"].as_str().unwrap_or("");
    assert!(
        !settings.contains(&BASE_PORT.to_string()),
        "绑定失败后 settings 不得继续指向该端口: {settings}"
    );
    assert!(
        body["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning["code"] == "egress_relay_bind_failed"),
        "必须给 egress_relay_bind_failed: {body}"
    );
    drop(blocker);
    shutdown(&fixture).await;
}

#[tokio::test]
async fn put_never_writes_direct_proxy_while_activating_relay() {
    // t30 H1 的判别性用例：**两步 PUT**。
    //
    // 为什么不能一步：fixture 初始没有代理，旧实现「重建前先 apply」在这种状态下
    // expected=Direct，而 settings 本来就是 :0 → no-op，旧实现也能过（假绿）。
    // 所以先把无凭据上游配成 default（settings=上游 host:port），再加凭据激活 relay；
    // 旧实现会在激活那一步先写 :0（直连窗口），本用例会立刻红。
    const BASE_PORT: u16 = 17896;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;

    // ① 无凭据的 http default：中继不参与，settings 指上游 host:port
    let revision = eg_revision(router).await;
    put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [{
                "name": "hk_resi",
                "type": "http",
                "host": "127.0.0.1",
                "port": upstream.port,
            }],
        }),
    )
    .await;
    let step1 = sim.calls();
    let last_proxy = step1
        .iter()
        .rev()
        .find(|call| call.contains("http_proxy "))
        .expect("第一步必须写 settings");
    assert_eq!(
        *last_proxy,
        format!("settings put global http_proxy 127.0.0.1:{}", upstream.port),
        "无凭据阶段 settings 必须指向上游: {step1:?}"
    );

    // ② 同一 default 加凭据 → 激活 relay
    let before = sim.calls().len();
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
            "exclusionList": ["10.0.0.0/8"],
        }),
    )
    .await;
    let step2: Vec<String> = sim.calls().into_iter().skip(before).collect();
    let writes: Vec<&String> = step2
        .iter()
        .filter(|call| call.contains("http_proxy "))
        .collect();
    assert!(
        !step2.iter().any(|call| call.contains("http_proxy :0")),
        "激活中继期间不得写直连: {step2:?}"
    );
    assert_eq!(
        writes.len(),
        1,
        "激活 relay 的 PUT 只能写一次 http_proxy（旧实现会先写 :0 再修回）: {step2:?}"
    );
    assert_eq!(
        *writes[0],
        format!("settings put global http_proxy 127.0.0.1:{BASE_PORT}"),
        "唯一那次写入必须是中继端口: {step2:?}"
    );
    assert!(
        step2
            .iter()
            .any(|call| call.contains("global_http_proxy_exclusion_list 10.0.0.0/8")),
        "exclusionList 必须与那次写入在同一次 apply 里: {step2:?}"
    );
    assert_eq!(
        body["readback"]["settingsValue"],
        format!("127.0.0.1:{BASE_PORT}"),
        "{body}"
    );
    shutdown(&fixture).await;
}

#[tokio::test]
async fn socks5_default_without_credentials_still_warns_and_blackholes() {
    // M1：黑洞与 warning 条件必须一致——无凭据 socks5 也被黑洞化，所以也得告警。
    const BASE_PORT: u16 = 17897;
    let _guard = relay_guard();
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    use_base_port(&fixture, BASE_PORT);
    let config_path = fixture.config_path.clone();
    std::fs::write(
        &config_path,
        ["[egress]", r#"default = "socks_only""#].join(&char::from(10u8).to_string()),
    )
    .expect("写 config.toml");
    let egress_path = fixture.state.egress().store().path().to_path_buf();
    std::fs::write(
        &egress_path,
        [
            "version = 1",
            "",
            "[proxies.socks_only]",
            r#"type = "socks5""#,
            r#"host = "203.0.113.7""#,
            "port = 1080",
        ]
        .join(&char::from(10u8).to_string()),
    )
    .expect("写 egress.toml");

    let restarted = common::send_body(
        &fixture.router,
        Method::POST,
        "/api/v1/egress/relay/restart",
        Some(ADMIN_TOKEN),
        &[("content-type", "application/json")],
        Some(b"{}".to_vec()),
    )
    .await;
    assert_eq!(restarted.status(), 200);
    let body =
        body_json(get_with_token(&fixture.router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert!(
        body["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning["code"] == "egress_relay_device_socks5_unsupported"),
        "无凭据 socks5 default 也必须告警: {body}"
    );
    assert_eq!(
        body["readback"]["settingsValue"], "127.0.0.1:9",
        "被黑洞化就必须写黑洞: {body}"
    );
    shutdown(&fixture).await;
}

#[tokio::test]
async fn socks5_default_with_relay_disabled_warns_and_goes_direct() {
    // M1：relay 未启用 → 按「未配置」处理（直连），但同样要告警。
    const BASE_PORT: u16 = 17898;
    let _guard = relay_guard();
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    use_base_port(&fixture, BASE_PORT);
    std::fs::write(
        fixture.config_path.clone(),
        ["[egress]", r#"default = "socks_only""#].join(&char::from(10u8).to_string()),
    )
    .expect("写 config.toml");
    std::fs::write(
        fixture.state.egress().store().path(),
        [
            "version = 1",
            "",
            "[relay]",
            "enabled = false",
            "",
            "[proxies.socks_only]",
            r#"type = "socks5""#,
            r#"host = "203.0.113.7""#,
            "port = 1080",
            r#"username = "u""#,
            r#"password = "p""#,
        ]
        .join(&char::from(10u8).to_string()),
    )
    .expect("写 egress.toml");
    let restarted = common::send_body(
        &fixture.router,
        Method::POST,
        "/api/v1/egress/relay/restart",
        Some(ADMIN_TOKEN),
        &[("content-type", "application/json")],
        Some(b"{}".to_vec()),
    )
    .await;
    assert_eq!(restarted.status(), 200);
    let body =
        body_json(get_with_token(&fixture.router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert!(
        body["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning["code"] == "egress_relay_device_socks5_unsupported"),
        "relay 未启用时同样要告警: {body}"
    );
    assert_eq!(
        body["readback"]["settingsValue"], "null",
        "未启用中继按未配置处理（直连）: {body}"
    );
    shutdown(&fixture).await;
}

#[tokio::test]
async fn changing_existing_default_to_socks5_is_rejected() {
    // L1：请求体里没有 default，但把已存在的 default 定义改成 socks5 也必须 422。
    let sim = SettingsSim::new();
    let fixture = common::fixture_with_arc_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", 8080)],
        }),
    )
    .await;
    let revision = eg_revision(router).await;
    let response = common::send_body(
        router,
        Method::PUT,
        "/api/v1/egress",
        Some(ADMIN_TOKEN),
        &[
            ("if-match", &revision),
            ("content-type", "application/json"),
        ],
        Some(
            serde_json::to_vec(&json!({
                "proxies": [{
                    "name": "hk_resi",
                    "type": "socks5",
                    "host": "127.0.0.1",
                    "port": 1080,
                    "username": "u",
                    "password": "p",
                }],
            }))
            .expect("json"),
        ),
    )
    .await;
    assert_eq!(
        response.status(),
        422,
        "把已有 default 改成 socks5 必须 422"
    );
    let body = body_json(response).await;
    assert!(
        body.to_string().contains("invalid_egress_default"),
        "{body}"
    );
}

#[tokio::test]
async fn put_preserves_relay_section_and_local_ports() {
    // 契约 §1.8 / D3：[relay] 段与 local_port 是 server 拥有字段，
    // 普通 PUT（只改 default/probe）必须原样保留，不能把 firewall 带回默认 true。
    let sim = SettingsSim::new();
    let fixture = common::fixture_with_arc_runner(Some(common::CONFIG_TEXT), Arc::new(sim));
    let path = fixture.state.egress().store().path().to_path_buf();
    let seed = [
        "version = 1",
        "",
        "[relay]",
        "firewall = false",
        "base_port = 17800",
        "",
        "[proxies.hk_resi]",
        r#"type = "http""#,
        r#"host = "127.0.0.1""#,
        "port = 8080",
        r#"username = "alice""#,
        r#"password = "s3cret""#,
        "local_port = 17831",
    ]
    .join("\n");
    std::fs::write(&path, &seed).expect("write egress.toml");
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "probeUrl": "http://example.com/ip",
        }),
    )
    .await;
    let after = std::fs::read_to_string(&path).expect("read after");
    assert!(
        after.contains("firewall = false"),
        "PUT 不得把 [relay].firewall 改回默认 true: {after}"
    );
    assert!(
        after.contains("local_port = 17831"),
        "PUT 不得清掉端口表: {after}"
    );
}

#[tokio::test]
async fn put_rejects_read_only_local_port() {
    let fixture = common::fixture();
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let payloads = [
        json!({ "proxies": [{ "name": "p", "type": "http", "host": "h", "port": 1, "localPort": 17801 }] }),
        json!({ "proxies": [{ "name": "p", "type": "http", "host": "h", "port": 1, "local_port": 17801 }] }),
        json!({ "relay": { "enabled": false } }),
        json!({ "relayEnabled": true }),
    ];
    for payload in payloads {
        let response = json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &payload,
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{payload}"
        );
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "validation_failed", "{body}");
        let errors = body["error"]["details"]["errors"]
            .as_array()
            .expect("details.errors");
        assert!(
            errors.iter().any(|error| error["code"] == "unknown_field"),
            "{body}"
        );
    }
    // 只读字段被拒时整体不落盘
    assert!(!fixture.state.egress().store().path().exists());
}

#[tokio::test]
async fn app_scoped_listener_resolves_uids_and_requires_restart() {
    const BASE_PORT: u16 = 17840;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new().with_pm_uid(10123);
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "proxies": [credential_proxy("jp_app", upstream.port)],
            "apps": [{ "package": "com.example.app", "egress": "jp_app" }],
        }),
    )
    .await;

    let listener = &body["relay"]["listeners"][0];
    assert_eq!(listener["scope"], "app", "{body}");
    assert_eq!(listener["uidPolicy"], "restricted", "{body}");
    assert_eq!(listener["allowedUids"], json!([0, 10123]), "{body}");
    assert_eq!(listener["listening"], true, "{body}");
    let port = u16::try_from(listener["port"].as_u64().expect("port")).expect("u16");
    assert_ne!(port, BASE_PORT, "应用级端口不得占用 base_port: {body}");

    // 规则证据（契约 §5.2）：应用级端口只有 root + 绑定 uid 两条 ACCEPT，
    // 其它 uid 落不到 ALLOW → 命中 DENY 的 REJECT；入口链顺序冻结。
    let allow = sim.chain_rules("DF_EGRESS_RLY_ALLOW");
    assert!(
        allow
            .iter()
            .any(|rule| rule.contains(&format!("--dport {port} -m owner --uid-owner 10123"))),
        "{allow:?}"
    );
    assert!(
        allow.iter().any(|rule| rule.contains("--uid-owner 0")),
        "{allow:?}"
    );
    assert!(
        !allow.iter().any(|rule| rule.contains(&format!("--dport {port} "))
            && !rule.contains("--uid-owner")),
        "应用级端口不得出现放行全部 uid 的规则: {allow:?}"
    );
    let deny = sim.chain_rules("DF_EGRESS_RLY_DENY");
    assert!(
        deny.iter()
            .any(|rule| rule.contains(&format!("--dport {port} ")) && rule.contains("REJECT")),
        "{deny:?}"
    );
    assert_eq!(
        sim.chain_rules("DF_EGRESS_RLY"),
        vec![
            "-j DF_EGRESS_RLY_ALLOW".to_owned(),
            "-j DF_EGRESS_RLY_DENY".to_owned(),
            "-j RETURN".to_owned()
        ],
        "入口链必须是 ALLOW→DENY→RETURN"
    );
    assert_eq!(
        body["relay"]["restartRequiredApps"],
        json!(["com.example.app"]),
        "首次分配端口 → 引用它的应用必须重启: {body}"
    );

    // restricted 端口的 accept 期 uid 双检（契约 §4.6 第 4–5 条）：
    // host（非 Linux）拿不到 /proc/net/tcp 的 uid → fail-closed，直接 close、不回任何字节。
    // 真机上同一用例由授权 uid 连接才应成功（t9 验收）。
    let response = send_raw(
        port,
        "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
    )
    .await;
    // t21 分层策略：host 上拿不到 uid，但假 iptables 让 firewall.applied=true，
    // 内核 owner match 已按 uid 过滤 → 放行（不再一律 close）。真机上这里由
    // /proc 归属成功或防火墙兜底，两条路都通。
    assert!(
        String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200"),
        "防火墙已生效时 uid 未知应放行并正常转发，实际 {} 字节",
        response.len()
    );
    shutdown(&fixture).await;
}

#[tokio::test]
async fn app_scoped_uid_failure_is_fail_closed() {
    const BASE_PORT: u16 = 17845;
    const APP_PORT: u16 = 17846;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    // 没有 pm 输出 → uid 解析失败（fail-closed）
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    let path = fixture.state.egress().store().path().to_path_buf();
    // 预置一条带 local_port 的定义：解析失败时端口表必须**保留**、不重分配
    std::fs::write(
        &path,
        format!(
            "version = 1\n\n[relay]\nbase_port = {BASE_PORT}\n\n[proxies.jp_app]\ntype = \"http\"\nhost = \"127.0.0.1\"\nport = {}\nusername = \"alice\"\npassword = \"s3cret\"\nlocal_port = {APP_PORT}\n",
            upstream.port
        ),
    )
    .expect("write egress.toml");
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "proxies": [credential_proxy("jp_app", upstream.port)],
            "apps": [{ "package": "com.example.app", "egress": "jp_app" }],
        }),
    )
    .await;

    assert_eq!(
        body["proxies"][0]["localPort"], APP_PORT,
        "端口表必须保留、不重分配: {body}"
    );
    let listener = &body["relay"]["listeners"][0];
    assert_eq!(listener["listening"], false, "{body}");
    assert_eq!(
        listener["lastError"]["code"], "egress_relay_uid_unknown",
        "{body}"
    );
    let codes = warning_codes(&body);
    assert!(
        codes.iter().any(|code| code == "egress_relay_uid_unknown"),
        "{codes:?}"
    );
    let warning = body["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "egress_relay_uid_unknown")
        .expect("warning");
    assert_eq!(warning["details"]["scope"], "app", "{warning}");
    assert_eq!(
        warning["details"]["package"], "com.example.app",
        "{warning}"
    );
    assert_eq!(
        warning["details"]["reason"], "package_not_installed",
        "{warning}"
    );

    // 端口上没有任何 listener：连接被拒（fail-closed 的可观测结果）
    let refused = TcpStream::connect(("127.0.0.1", APP_PORT)).await;
    assert!(
        refused.is_err() || {
            // 万一端口被同机其它进程占用，至少要确认不是我们的 listener
            false
        }
    );
    shutdown(&fixture).await;
}

#[tokio::test]
async fn firewall_applied_comes_from_live_snapshot_and_restart_repairs() {
    const BASE_PORT: u16 = 17850;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [credential_proxy("hk_resi", upstream.port)],
        }),
    )
    .await;

    // applied / allowRules / denyRules 来自同一次 -S：
    // 入口链 3 条 + 本端口 root 1 条 + 设备级 1 条 ACCEPT + 1 条 DENY
    assert_eq!(body["relay"]["firewall"]["applied"], true, "{body}");
    assert_eq!(body["relay"]["firewall"]["allowRules"], 2, "{body}");
    assert_eq!(body["relay"]["firewall"]["denyRules"], 1, "{body}");
    assert_eq!(
        body["relay"]["firewall"]["rules"], 6,
        "3 + ALLOW + DENY: {body}"
    );
    assert!(body["relay"]["firewall"]["lastError"].is_null(), "{body}");
    assert!(sim.chain_len("DF_EGRESS_RLY_ALLOW") >= 2);

    // 手工 -F ALLOW（等价于被外部清掉）→ 现算必须 false，reason=state_mismatch
    sim.flush_chain("DF_EGRESS_RLY_ALLOW");
    let after = body_json(get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(after["relay"]["firewall"]["applied"], false, "{after}");
    assert_eq!(
        after["relay"]["firewall"]["lastError"]["reason"], "state_mismatch",
        "命令成功但规则不符，不得用 command_failed 冒充: {after}"
    );
    let warning = after["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "egress_relay_firewall_failed")
        .expect("防火墙不一致必须有 warning");
    assert_eq!(warning["details"]["reason"], "state_mismatch", "{warning}");
    assert_eq!(warning["details"]["chain"], "DF_EGRESS_RLY", "{warning}");

    // restart 强制 apply → 修回 applied=true；随后 GET 仍是 true（无 TTL 缓存也稳定）
    let repaired = body_json(
        json_call(
            router,
            Method::POST,
            "/api/v1/egress/relay/restart",
            OPS_TOKEN,
            &[("content-type", "application/json")],
            &json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(repaired["relay"]["firewall"]["applied"], true, "{repaired}");
    let again = body_json(get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(again["relay"]["firewall"]["applied"], true, "{again}");

    // 服务退出 / 中继关闭：三链被删（与 settings 清理同一条收口路径）
    shutdown(&fixture).await;
    for chain in ["DF_EGRESS_RLY", "DF_EGRESS_RLY_ALLOW", "DF_EGRESS_RLY_DENY"] {
        assert!(
            sim.chain_rules(chain).is_empty(),
            "退出后必须删链 {chain}: {:?}",
            sim.chain_rules(chain)
        );
    }
}

#[tokio::test]
async fn multi_user_package_resolves_each_user_uid() {
    const BASE_PORT: u16 = 17855;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    // 两个用户解析出不同 uid
    let sim = SettingsSim::new().with_pm_uids(&[("0", 10123), ("999", 10199)]);
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    // 包名带 @999：只解析该用户
    let body = put_egress(
        router,
        &revision,
        json!({
            "proxies": [credential_proxy("jp_app", upstream.port)],
            "apps": [{ "package": "com.example.app@999", "egress": "jp_app" }],
        }),
    )
    .await;
    let listener = &body["relay"]["listeners"][0];
    assert_eq!(
        listener["allowedUids"],
        json!([0, 10199]),
        "pkg@999 只解析 999: {body}"
    );

    // 去掉 @999：对每个已存在用户解析一次 → 并集（升序去重）
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({ "apps": [{ "package": "com.example.app", "egress": "jp_app" }] }),
    )
    .await;
    let listener = &body["relay"]["listeners"][0];
    assert_eq!(listener["allowedUids"], json!([0, 10123, 10199]), "{body}");
    let allow = sim.chain_rules("DF_EGRESS_RLY_ALLOW");
    for uid in ["0", "10123", "10199"] {
        assert!(
            allow
                .iter()
                .any(|rule| rule.contains(&format!("--uid-owner {uid}"))),
            "多用户 uid {uid} 必须有独立 ACCEPT: {allow:?}"
        );
    }
    shutdown(&fixture).await;
}

#[tokio::test]
async fn firewall_failure_blocks_app_ports_but_keeps_device() {
    const BASE_PORT: u16 = 17860;
    let _guard = relay_guard();
    let upstream = spawn_upstream().await;
    // iptables 每条命令都失败（没装 / 没权限）
    let sim = SettingsSim::new()
        .with_pm_uid(10123)
        .with_iptables_failure();
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "default": "hk_resi",
            "proxies": [
                credential_proxy("hk_resi", upstream.port),
                credential_proxy("jp_app", upstream.port),
            ],
            "apps": [{ "package": "com.example.app", "egress": "jp_app" }],
        }),
    )
    .await;

    assert_eq!(body["relay"]["firewall"]["applied"], false, "{body}");
    let listeners = body["relay"]["listeners"].as_array().expect("listeners");
    // 设备级照常（不依赖防火墙，§0 D2）
    let device = listeners
        .iter()
        .find(|listener| listener["scope"] == "device")
        .expect("device listener");
    assert_eq!(device["listening"], true, "{body}");
    // 应用级不监听（宁可不服务，也不裸奔，§5.5）
    let app = listeners
        .iter()
        .find(|listener| listener["scope"] == "app")
        .expect("app listener");
    assert_eq!(app["listening"], false, "{body}");
    assert_eq!(
        app["lastError"]["code"], "egress_relay_firewall_failed",
        "{body}"
    );
    let warning = body["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "egress_relay_firewall_failed")
        .expect("防火墙失败必须有 warning");
    assert_eq!(warning["details"]["op"], "apply", "{warning}");
    assert_eq!(warning["details"]["reason"], "command_failed", "{warning}");
    assert_eq!(warning["details"]["chain"], "DF_EGRESS_RLY", "{warning}");

    // 应用级端口确实没有 listener（连接被内核拒绝）
    let app_port = u16::try_from(app["port"].as_u64().expect("port")).expect("u16");
    assert!(
        TcpStream::connect(("127.0.0.1", app_port)).await.is_err(),
        "防火墙失败时应用级端口绝不能监听"
    );
    shutdown(&fixture).await;
}

/// 把 tracing 输出收进内存，供同步断言。
#[derive(Clone, Default)]
struct CaptureLog(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureLog {
    type Writer = CaptureLog;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

// ---------------------------------------------------------------------------
// SOCKS5（t5）：本地服务端 + 上游 RFC1929 + 远程 DNS
// ---------------------------------------------------------------------------

/// 读满 N 字节（测试侧，不依赖 io-util）。
async fn read_exact_n(stream: &mut TcpStream, length: usize) -> Vec<u8> {
    let mut buffer = vec![0u8; length];
    let mut filled = 0usize;
    while filled < length {
        let read = read_some(stream, &mut buffer[filled..])
            .await
            .expect("read");
        assert!(read > 0, "对端提前关闭");
        filled += read;
    }
    buffer
}

/// mock 上游 SOCKS5 代理：记录握手/认证/请求，成功后回 0 并回显。
struct MockSocks5 {
    port: u16,
    records: Arc<Mutex<Vec<String>>>,
}

impl MockSocks5 {
    fn records(&self) -> Vec<String> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

async fn spawn_socks5_upstream(auth_ok: bool) -> MockSocks5 {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("mock socks5");
    let port = listener.local_addr().expect("addr").port();
    let records: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let sink = Arc::clone(&sink);
            tokio::spawn(async move {
                let record = |line: String| {
                    sink.lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(line);
                };
                // 1) 问候：VER NMETHODS METHODS...
                let head = read_exact_n(&mut stream, 2).await;
                let count = usize::from(head[1]);
                let methods = read_exact_n(&mut stream, count).await;
                record(format!(
                    "greeting:{:02x},{:02x},{:02x}",
                    head[0], head[1], methods[0]
                ));
                if !methods.contains(&0x02) {
                    return;
                }
                write_all(&mut stream, &[0x05, 0x02]).await.ok();
                // 2) RFC1929
                let header = read_exact_n(&mut stream, 2).await;
                assert_eq!(header[0], 0x01);
                let user = read_exact_n(&mut stream, usize::from(header[1])).await;
                let pass_len = read_exact_n(&mut stream, 1).await;
                let pass = read_exact_n(&mut stream, usize::from(pass_len[0])).await;
                record(format!(
                    "auth:{}:{}",
                    String::from_utf8_lossy(&user),
                    String::from_utf8_lossy(&pass)
                ));
                if !auth_ok {
                    write_all(&mut stream, &[0x01, 0x01]).await.ok();
                    return;
                }
                write_all(&mut stream, &[0x01, 0x00]).await.ok();
                // 3) CONNECT（域名必须原样到达，本地不解析）
                let request = read_exact_n(&mut stream, 4).await;
                assert_eq!(request[0], 0x05);
                let host = match request[3] {
                    0x01 => {
                        let bytes = read_exact_n(&mut stream, 4).await;
                        std::net::Ipv4Addr::from([bytes[0], bytes[1], bytes[2], bytes[3]])
                            .to_string()
                    }
                    0x03 => {
                        let length = read_exact_n(&mut stream, 1).await;
                        let name = read_exact_n(&mut stream, usize::from(length[0])).await;
                        String::from_utf8_lossy(&name).into_owned()
                    }
                    other => panic!("unexpected atyp {other}"),
                };
                let port_bytes = read_exact_n(&mut stream, 2).await;
                let port = u16::from_be_bytes([port_bytes[0], port_bytes[1]]);
                record(format!(
                    "request:atyp={} host={} port={}",
                    request[3], host, port
                ));
                write_all(
                    &mut stream,
                    &[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0x1F, 0x90],
                )
                .await
                .ok();
                // 4) 回显隧道
                loop {
                    let mut chunk = [0u8; 1024];
                    let read = match read_some(&mut stream, &mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    };
                    if write_all(&mut stream, &chunk[..read]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    // 任务句柄故意不保留：mock 随测试 runtime 一起结束
    drop(task);
    MockSocks5 { port, records }
}

/// 走完本地 SOCKS5 握手并发出请求，返回 (stream, REP, 完整回复)。
async fn socks5_request(relay_port: u16, request: &[u8]) -> (TcpStream, u8, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", relay_port))
        .await
        .expect("连本地 SOCKS5 端口");
    write_all(&mut stream, &[0x05, 0x01, 0x00])
        .await
        .expect("greeting");
    let choice = read_exact_n(&mut stream, 2).await;
    assert_eq!(choice, [0x05, 0x00], "本地必须选 0x00（无认证）");
    write_all(&mut stream, request).await.expect("request");
    let head = read_exact_n(&mut stream, 4).await;
    let mut reply = head.clone();
    let tail = match head[3] {
        0x01 => 6,
        0x04 => 18,
        0x03 => {
            let length = read_exact_n(&mut stream, 1).await;
            reply.extend_from_slice(&length);
            usize::from(length[0]) + 2
        }
        _ => 6,
    };
    let tail = read_exact_n(&mut stream, tail).await;
    reply.extend_from_slice(&tail);
    (stream, head[1], reply)
}

fn domain_request(host: &str, port: u16) -> Vec<u8> {
    let mut request = vec![0x05, 0x01, 0x00, 0x03, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    request
}

fn socks5_proxy(name: &str, port: u16) -> Value {
    json!({
        "name": name,
        "type": "socks5",
        "host": "127.0.0.1",
        "port": port,
        "username": "alice",
        "password": "s3cret",
    })
}

#[tokio::test]
async fn socks5_listener_forwards_with_rfc1929_and_remote_dns() {
    const BASE_PORT: u16 = 17865;
    let _guard = relay_guard();
    let upstream = spawn_socks5_upstream(true).await;
    let sim = SettingsSim::new().with_pm_uid(10123);
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;

    let capture = CaptureLog::default();
    let buffer = Arc::clone(&capture.0);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(capture)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "proxies": [socks5_proxy("jp_socks", upstream.port)],
            "apps": [{ "package": "com.example.app", "egress": "jp_socks" }],
        }),
    )
    .await;
    let port = u16::try_from(
        body["relay"]["listeners"][0]["port"]
            .as_u64()
            .expect("port"),
    )
    .expect("u16");
    assert_eq!(body["relay"]["listeners"][0]["scope"], "app", "{body}");
    assert_eq!(body["relay"]["listeners"][0]["listening"], true, "{body}");
    assert_ne!(
        body["relay"]["listeners"][0]["port"],
        u64::from(BASE_PORT),
        "socks5 不得占用 base_port（设备级只对 http 成立）: {body}"
    );

    let (mut stream, code, reply) = socks5_request(port, &domain_request("example.com", 80)).await;
    assert_eq!(code, 0x00, "上游认证成功必须回 REP=0: {reply:?}");
    write_all(&mut stream, b"ping").await.expect("tunnel write");
    let echoed = read_exact_n(&mut stream, 4).await;
    assert_eq!(&echoed, b"ping", "隧道必须双向可用");
    drop(stream);
    drop(guard);

    let records = upstream.records();
    assert!(
        records.iter().any(|line| line == "greeting:05,01,02"),
        "有凭据时只报 0x02: {records:?}"
    );
    assert!(
        records.iter().any(|line| line == "auth:alice:s3cret"),
        "必须按 RFC1929 出示凭据: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|line| line == "request:atyp=3 host=example.com port=80"),
        "域名必须原样透传给上游（远程 DNS，本地不解析）: {records:?}"
    );
    let logs = String::from_utf8(
        buffer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
    )
    .expect("utf-8 log");
    for secret in ["s3cret", "alice", "YWxpY2U6czNjcmV0"] {
        assert!(
            !logs.contains(secret),
            "服务日志不得出现凭据 {secret}: {logs}"
        );
    }
    shutdown(&fixture).await;
}

#[tokio::test]
async fn socks5_upstream_auth_failure_maps_to_rep_01_and_warns() {
    const BASE_PORT: u16 = 17870;
    let _guard = relay_guard();
    let upstream = spawn_socks5_upstream(false).await;
    let sim = SettingsSim::new().with_pm_uid(10123);
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "proxies": [socks5_proxy("jp_socks", upstream.port)],
            "apps": [{ "package": "com.example.app", "egress": "jp_socks" }],
        }),
    )
    .await;
    let port = u16::try_from(
        body["relay"]["listeners"][0]["port"]
            .as_u64()
            .expect("port"),
    )
    .expect("u16");
    assert_eq!(body["relay"]["listeners"][0]["scope"], "app", "{body}");

    let (_stream, code, reply) = socks5_request(port, &domain_request("example.com", 80)).await;
    assert_eq!(code, 0x01, "上游认证失败 → REP=0x01: {reply:?}");

    let body = body_json(get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    let warning = body["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "egress_relay_upstream_unreachable")
        .expect("认证失败必须给 upstream_unreachable");
    assert_eq!(warning["details"]["reason"], "auth_failed", "{warning}");
    let text = body.to_string();
    assert!(
        !text.contains("s3cret") && !text.contains("alice"),
        "{text}"
    );
    shutdown(&fixture).await;
}

#[tokio::test]
async fn socks5_rejects_bind_udp_and_bad_methods() {
    const BASE_PORT: u16 = 17875;
    let _guard = relay_guard();
    let upstream = spawn_socks5_upstream(true).await;
    let sim = SettingsSim::new().with_pm_uid(10123);
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "proxies": [socks5_proxy("jp_socks", upstream.port)],
            "apps": [{ "package": "com.example.app", "egress": "jp_socks" }],
        }),
    )
    .await;
    let port = u16::try_from(
        body["relay"]["listeners"][0]["port"]
            .as_u64()
            .expect("port"),
    )
    .expect("u16");
    assert_eq!(body["relay"]["listeners"][0]["scope"], "app", "{body}");

    // BIND / UDP ASSOCIATE → 0x07
    for command in [0x02u8, 0x03] {
        let mut request = domain_request("example.com", 80);
        request[1] = command;
        let (_stream, code, reply) = socks5_request(port, &request).await;
        assert_eq!(code, 0x07, "CMD={command} 必须回 0x07: {reply:?}");
    }
    // ATYP 不支持 → 0x08
    let mut request = domain_request("example.com", 80);
    request[3] = 0x09;
    let (_stream, code, _reply) = socks5_request(port, &request).await;
    assert_eq!(code, 0x08, "不支持的 ATYP 必须回 0x08");
    // 只报 0x02（本地不做认证）→ 05 FF 并关闭
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    write_all(&mut stream, &[0x05, 0x01, 0x02])
        .await
        .expect("greeting");
    let answer = read_exact_n(&mut stream, 2).await;
    assert_eq!(answer, [0x05, 0xFF], "本地只接受 0x00");
    shutdown(&fixture).await;
}

#[tokio::test]
async fn socks5_upstream_unreachable_maps_to_rep_01() {
    const BASE_PORT: u16 = 17880;
    let _guard = relay_guard();
    let dead = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let dead_port = dead.local_addr().expect("addr").port();
    drop(dead);
    let sim = SettingsSim::new().with_pm_uid(10123);
    let fixture = fixture_with_relay_and_runner(Some(common::CONFIG_TEXT), Arc::new(sim.clone()));
    use_base_port(&fixture, BASE_PORT);
    let router = &fixture.router;
    let revision = eg_revision(router).await;
    let body = put_egress(
        router,
        &revision,
        json!({
            "proxies": [socks5_proxy("jp_socks", dead_port)],
            "apps": [{ "package": "com.example.app", "egress": "jp_socks" }],
        }),
    )
    .await;
    let port = u16::try_from(
        body["relay"]["listeners"][0]["port"]
            .as_u64()
            .expect("port"),
    )
    .expect("u16");
    assert_eq!(body["relay"]["listeners"][0]["scope"], "app", "{body}");

    let (_stream, code, _reply) = socks5_request(port, &domain_request("example.com", 80)).await;
    assert_eq!(code, 0x01, "上游不可达 → REP=0x01");
    let body = body_json(get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    let warning = body["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "egress_relay_upstream_unreachable")
        .expect("必须给 upstream_unreachable");
    assert_eq!(warning["details"]["reason"], "connect", "{warning}");
    shutdown(&fixture).await;
}
