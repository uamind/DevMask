// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 应用级出口代理（方案 B）：见 docs/plans/egress-contract.md §2.4 与 §5。
//!
//! 数据流分三段，各管一段，互不依赖对方的实现细节：
//!
//! 1. **companion（root 域）**：[load_egress] 读 0600 的 `egress.toml`，
//!    [spec_proxies] 先按中继规则改写（带凭据 → 回环），[resolve_for_package] 再按
//!    `[[apps]].egress` → `[egress].default` → 直连 的三级继承链算出某个包**实际**要用的
//!    代理，[EgressProxy::to_toml] 把它写进
//!    `GetSpec` 响应 TOML 的 `[egress_proxy]` 小节（全内存，不落盘）。
//! 2. **模块侧（zygote 域）**：[from_spec_doc] 从同一份 TOML 里取回该小节。模块
//!    **不读** `egress.toml`——它碰不到 0600 文件，暴露面更小（契约 §5.1 末）。
//! 3. **进程内注入**：[injection_plan] 把代理翻译成「设置/清除哪些系统属性 + 装哪个
//!    `ProxySelector`」的纯数据计划，由 `hooks::apply_egress_proxy` 交给 JNI 执行。
//!    把计划做成纯函数，是为了能在没有 JNI、没有真机的环境里断言「http 代理到底设置
//!    了哪几个键、值是什么」。
//!
//! 凭据边界（契约 §7.1）：密码只存在于这两个进程的内存里；[EgressProxy] 的 `Debug`
//! 实现把密码打码，本模块的任何日志路径都不输出它。
//!
//! 中继（v1.11 增量，契约 docs/plans/relay-contract.md §3，v1.2 语义）：
//!
//! - **带凭据**的定义在 `[relay].enabled = true` 且分到 `local_port` 时改写成
//!   `127.0.0.1:<local_port>` + `relay = true`（[spec_proxies]），既不往应用进程里塞
//!   凭据、也不暴露上游地址；
//! - **fail-closed**（R12/R13）：中继开着却拿不到可用端口 → `127.0.0.1:9` 哨兵；
//!   模块侧收到任何非法 relay 视图也一律改写成该哨兵并经 [injection_plan] 注入
//!   （从而 arm replay），**绝不**返回「未配置」或回退上游——那会让应用裸奔；
//! - **凭据载体自 v1.2 起全部移除**：socks 的两个凭据属性与
//!   `Authenticator` 在任何路径都不写（§9.1 K1 / §9.2 B13）；
//! - **无凭据**、`__DIRECT__` 与「未配置」的 spec 与 v1.10.0 逐字节一致
//!   （[spec::render_doc] 只在真的有代理时写 `[egress_proxy]`）。

use std::{collections::HashMap, fmt, fs};

use log::warn;

use crate::config::{Config, DIRECT_MARKER};

/// `GetSpec` 下发的 TOML 里承载「本应用实际用哪个代理」的小节名。
///
/// 与 `egress.toml` 的 `[proxies.<name>]` 字段同名（`type` / `host` / `port` /
/// `username` / `password`），只多一个 `type = "direct"` 表示「强制直连」。
pub const SPEC_SECTION: &str = "egress_proxy";

/// 代理定义与探测策略文件：0600，只有 root 域（companion / server）读得到。
pub const EGRESS_PATH: &str = "/data/adb/devmask/config/egress.toml";

/// `egress.toml` 目前唯一支持的 `version`（契约 §2.1）。
const SUPPORTED_VERSION: i64 = 1;

/// 本地中继的监听地址：relay 生效时应用侧看到的地址**必然**是回环（契约 §3.2 不变量）。
pub const RELAY_HOST: &str = "127.0.0.1";

/// `local_port` 端口池的下界（契约 §2.3：17800..=17899）。
pub const RELAY_PORT_MIN: u16 = 17800;

/// `local_port` 端口池的上界。
pub const RELAY_PORT_MAX: u16 = 17899;

/// 某个端口是否落在中继端口池内。
pub fn is_relay_port(port: u16) -> bool {
    (RELAY_PORT_MIN..=RELAY_PORT_MAX).contains(&port)
}

/// fail-closed 哨兵端口（契约 §3.3 规则 1/2/6：RELAY_FAIL_CLOSED_PORT = 9）。
///
/// 指向它 = **主动断网**：内核立刻回 ECONNREFUSED（不是 DROP，也不是换出口），应用得到
/// 「连接被拒」。用它替代 v1.1 的「非法 relay 视图 → 当作未配置 → 继承设备级或直连」，
/// 后者会在应当走代理时让应用裸奔（R12）。
pub const RELAY_FAIL_CLOSED_PORT: u16 = 9;

// 编译期断言：哨兵端口**绝不能**落进中继端口池（契约 §3.3 规则 1 的硬要求）。
const _: () = assert!(RELAY_FAIL_CLOSED_PORT < RELAY_PORT_MIN);

/// 需要「清除」的系统属性全集：`__DIRECT__` 与协议互斥都会用到。
const PROXY_PROPERTY_KEYS: [&str; 6] = [
    "http.proxyHost",
    "http.proxyPort",
    "https.proxyHost",
    "https.proxyPort",
    "socksProxyHost",
    "socksProxyPort",
];

/// 代理协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressType {
    Http,
    Socks5,
    /// 只在 spec 的 `[egress_proxy]` 里出现（`[[apps]].egress = "__DIRECT__"`）；
    /// `egress.toml` 的 `[proxies.*].type` 不接受它（契约 §2.5 保留值）。
    Direct,
}

impl EgressType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Socks5 => "socks5",
            Self::Direct => "direct",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "http" => Some(Self::Http),
            "socks5" => Some(Self::Socks5),
            "direct" => Some(Self::Direct),
            _ => None,
        }
    }
}

/// 一条已解析的出口代理（`egress.toml` 的 `[proxies.<name>]`，或 spec 的
/// `[egress_proxy]`）。
///
/// `local_port` 与 `relay` 是中继增量的两个字段，**各自只在一个方向上存在**：
///
/// - `local_port`：只出现在 `egress.toml`（server 分配、0600 落盘），**永不**写进 spec；
/// - `relay`：只出现在 spec 的 `[egress_proxy]`（`relay = true` 表示地址是本地中继），
///   `egress.toml` 里不认这个键。
///
/// 之所以放同一个结构体里：两条链路共享「type/host/port/username/password」五个键的
/// 解析与渲染，拆成两个类型会让 [EgressProxy::from_table] 复制一遍校验。代价是渲染两侧
/// 各自要显式清掉对方专有的字段——[spec_proxies] 与 [EgressProxy::to_toml] 各做一次。
#[derive(Clone, PartialEq, Eq)]
pub struct EgressProxy {
    pub kind: EgressType,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    /// `egress.toml` `[proxies.<name>].local_port`：只读、由 server 分配（契约 §2.3）。
    pub local_port: Option<u16>,
    /// spec `[egress_proxy].relay`：地址是本地中继（契约 §3.3）。
    pub relay: bool,
}

/// relay 视图归一化的结果（纯数据；日志由 [from_spec_doc] 与 hooks 打）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayNormalized {
    /// 可直接注入的代理：type=direct 时是主动直连；其余目标已归一（非法 → 127.0.0.1:9）。
    pub proxy: EgressProxy,
    /// 目标非法时的原因；None = 目标合法（含哨兵 9）。
    pub failure: Option<RelayTargetFailure>,
    /// spec 里是否带过凭据：只用于日志的 creds=0|1，值本身已丢弃。
    pub had_credentials: bool,
}

/// 非法 relay 视图的两类原因（契约 §3.3 规则 2 的 reason= 取值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayTargetFailure {
    /// host 不是回环，或端口既不是哨兵 9 也不在 17800..=17899
    InvalidTarget,
    /// 段里根本没有 port
    PortMissing,
}

impl RelayTargetFailure {
    /// 日志里的 reason= 取值（契约 §3.3 规则 2，冻结字符串）。
    pub fn reason(self) -> &'static str {
        match self {
            Self::InvalidTarget => "invalid_target",
            Self::PortMissing => "port_missing",
        }
    }
}

/// 非法 relay 视图的降级日志行（契约 §3.3 规则 2 的冻结格式）。
///
/// 抽成纯函数有两个原因：真机日志检索要稳定（`reason=` 是固定字符串），以及宿主机
/// 单测能直接断言「这行不含凭据」——函数签名里根本没有凭据参数，凭据也就无从拼进来。
pub fn relay_fail_closed_line(
    package: &str,
    failure: RelayTargetFailure,
    had_credentials: bool,
) -> String {
    format!(
        "[egress] relay fail-closed package={package} reason={} creds={}",
        failure.reason(),
        u8::from(had_credentials)
    )
}

/// 「仅多带凭据」的修复日志行（契约 §3.3 规则 3：剥凭据照常注入，不是降级）。
pub fn relay_creds_stripped_line(package: &str) -> String {
    format!("[egress] relay creds stripped package={package}")
}

/// 「凭据被忽略」的日志行（契约 §9.1 K1：v1.2 起任何路径都不注入凭据载体）。
///
/// http 与 socks5 **共用同一行**（只换 type）：socks 无认证时也不能静默——排障时
/// 「为什么 socks 代理连上了却被上游拒」必须能在日志里看到 credentials ignored。
pub fn unsupported_auth_line(package: &str, kind: EgressType) -> String {
    format!(
        "[egress] unsupported-auth package={package} type={}          (host/port injected only, credentials ignored)",
        kind.as_str()
    )
}

impl EgressProxy {
    /// `__DIRECT__`：强制直连（不设置任何代理地址属性）。
    pub fn direct() -> Self {
        Self {
            kind: EgressType::Direct,
            host: None,
            port: None,
            username: None,
            password: None,
            local_port: None,
            relay: false,
        }
    }

    /// `http` / `socks5` 定义（测试构造用；线上定义一律来自 `egress.toml` 的解析）。
    #[cfg(test)]
    pub fn endpoint_proxy(kind: EgressType, host: impl Into<String>, port: u16) -> Self {
        Self {
            kind,
            host: Some(host.into()),
            port: Some(port),
            username: None,
            password: None,
            local_port: None,
            relay: false,
        }
    }

    /// 这条代理是否指向本地中继，且地址满足契约 §3.3 规则 1 的三项校验：
    /// host 是回环、端口 ∈ {9} ∪ [17800,17899]、没有任何凭据。
    pub fn relay_target_is_valid(&self) -> bool {
        self.kind != EgressType::Direct
            && self.host.as_deref() == Some(RELAY_HOST)
            && self
                .port
                .is_some_and(|port| port == RELAY_FAIL_CLOSED_PORT || is_relay_port(port))
            && !self.has_credentials()
    }

    /// 这是不是 fail-closed 哨兵（127.0.0.1:9 + relay=true，契约 §3.3 规则 6）。
    pub fn is_fail_closed(&self) -> bool {
        self.relay
            && self.kind != EgressType::Direct
            && self.host.as_deref() == Some(RELAY_HOST)
            && self.port == Some(RELAY_FAIL_CLOSED_PORT)
    }

    /// fail-closed 哨兵代理：127.0.0.1:9 + relay=true + 无凭据（契约 §3.3 规则 2）。
    pub fn fail_closed(kind: EgressType) -> Self {
        Self {
            // direct 不是可注入的地址族；哨兵至少封住一个协议族，缺省用 http。
            kind: if kind == EgressType::Direct {
                EgressType::Http
            } else {
                kind
            },
            host: Some(RELAY_HOST.to_string()),
            port: Some(RELAY_FAIL_CLOSED_PORT),
            username: None,
            password: None,
            local_port: None,
            relay: true,
        }
    }

    /// 归一化 spec 里的 relay 视图（契约 §3.3 规则 1–4）。
    ///
    /// 三件事，顺序固定：
    /// 1. type=direct + relay=true（自相矛盾）→ 主动直连（清 6 个属性 + NO_PROXY），不是「未配置」；
    /// 2. 凭据一律剥掉（v1.2 起凭据不进应用进程，§9.1 K1 / §9.2 B13）；
    /// 3. 目标非法（host 非回环 / 端口越界 / 缺 port）→ 换成哨兵 127.0.0.1:9，
    ///    由调用方记 reason，**绝不**返回「无计划」或回退上游（R12）。
    pub fn relay_normalized(&self) -> RelayNormalized {
        let had_credentials = self.has_credentials();

        if self.kind == EgressType::Direct {
            return RelayNormalized {
                proxy: Self::direct(),
                failure: None,
                had_credentials,
            };
        }

        let mut proxy = self.clone();
        proxy.username = None;
        proxy.password = None;
        proxy.local_port = None;
        proxy.relay = true;

        let failure = if proxy.port.is_none() {
            Some(RelayTargetFailure::PortMissing)
        } else if proxy.relay_target_is_valid() {
            // 凭据已在上面剥掉，所以这里只剩「host 回环 + 端口 ∈ {9} ∪ 端口池」两条。
            None
        } else {
            Some(RelayTargetFailure::InvalidTarget)
        };
        if failure.is_some() {
            proxy = Self::fail_closed(self.kind);
        }

        RelayNormalized {
            proxy,
            failure,
            had_credentials,
        }
    }

    /// 把一条**带凭据的上游定义**改写成「本地中继」视图（契约 §3.3 的字段表）。
    ///
    /// 凭据与上游地址都不进 spec：应用进程只看到 127.0.0.1:<local_port>，认证发生在
    /// 设备内的中继里。
    fn relay_view(&self, local_port: u16) -> Self {
        Self {
            kind: self.kind,
            host: Some(RELAY_HOST.to_string()),
            port: Some(local_port),
            username: None,
            password: None,
            local_port: None,
            relay: true,
        }
    }

    /// 带凭据（username 或 password 任一非空）。
    pub fn has_credentials(&self) -> bool {
        self.username.as_deref().is_some_and(|v| !v.is_empty())
            || self.password.as_deref().is_some_and(|v| !v.is_empty())
    }

    /// `http` / `socks5` 必需的 `(host, port)`；`direct`、缺字段或端口非法时返回 `None`。
    pub fn endpoint(&self) -> Option<(&str, u16)> {
        if self.kind == EgressType::Direct {
            return None;
        }
        let host = self.host.as_deref().filter(|host| !host.is_empty())?;
        let port = self.port.filter(|port| *port > 0)?;
        Some((host, port))
    }

    /// 渲染成 spec 的 `[egress_proxy]` 表（键名与契约 §2.1 的 `egress.toml` 对齐）。
    ///
    /// 渲染规则（v1.2 冻结）：
    /// - `local_port` **永不**进 spec（契约 §2.3：只读、只属于 `egress.toml`）；
    /// - `username`/`password` **任何路径都不写**（契约 §3.3 字段表 + §9.2 B13）：
    ///   凭据只在设备内的中继里出示，写进 spec 等于把凭据交给同 uid 的应用进程；
    /// - 只有 `relay = true` 这个内部标记会进 spec。
    pub fn to_toml(&self) -> toml::Value {
        let mut table = toml::map::Map::new();
        table.insert(
            "type".to_string(),
            toml::Value::String(self.kind.as_str().to_string()),
        );
        if let Some(host) = &self.host {
            table.insert("host".to_string(), toml::Value::String(host.clone()));
        }
        if let Some(port) = self.port {
            table.insert("port".to_string(), toml::Value::Integer(i64::from(port)));
        }
        if self.relay {
            table.insert("relay".to_string(), toml::Value::Boolean(true));
        }
        toml::Value::Table(table)
    }

    /// 从 `[proxies.<name>]` / `[egress_proxy]` 表里解析一条定义；结构不合法返回 `None`。
    ///
    /// 未知键按契约 §2.1「忽略但保留」处理——这里只读自己认识的键（五个基础键 +
    /// `local_port` + `relay`），不回写文件，也就不会丢掉用户手写的内容。
    fn from_table(table: &toml::Value) -> Option<Self> {
        let kind = table
            .get("type")
            .and_then(toml::Value::as_str)
            .and_then(EgressType::parse)?;

        let port = match table.get("port") {
            Some(toml::Value::Integer(port)) if (1..=65535).contains(port) => Some(*port as u16),
            Some(_) => return None,
            None => None,
        };

        Some(Self {
            kind,
            host: non_empty_str(table, "host"),
            port,
            username: non_empty_str(table, "username"),
            password: non_empty_str(table, "password"),
            local_port: parse_local_port(table),
            // 只有 spec 的 `relay = true` 有意义；`egress.toml` 不认这个键，缺省即 `false`。
            relay: table.get("relay").and_then(toml::Value::as_bool) == Some(true),
        })
    }
}

/// 读 `local_port`：只有落在中继端口池内的整数才算数（契约 §2.3）。
///
/// 越界/类型不对一律按「没有分配」处理：真正的降级决策与那行 warn 在 [spec_proxies]
/// 里统一做，这里保持纯解析、不打日志（同一份文件会被解析多次）。
fn parse_local_port(table: &toml::Value) -> Option<u16> {
    match table.get("local_port") {
        Some(toml::Value::Integer(port)) if (0..=i64::from(u16::MAX)).contains(port) => {
            let port = *port as u16;
            is_relay_port(port).then_some(port)
        }
        _ => None,
    }
}

/// 把代理写进 spec 文档的根表（`spec::render_doc` 的唯一入口）。
///
/// 单独抽成函数是为了让「写进 spec → 再从 spec 读回来」这条链路可以脱离 companion
/// 的渲染逻辑单独断言（见下方测试）。「未配置」不进这个函数——没有小节才是未配置。
pub fn insert_spec_section(root: &mut toml::map::Map<String, toml::Value>, proxy: &EgressProxy) {
    root.insert(SPEC_SECTION.to_string(), proxy.to_toml());
}

/// 读字符串键：非字符串/空串都按「未设置」。**不做 trim**：密码里的空白是内容的一部分。
fn non_empty_str(table: &toml::Value, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 凭据字段的 Debug 表示：**只区分有没有设置，永不出原文**（契约 §7.1）。
///
/// 用户名和密码一视同仁——只遮密码是不够的：SOCKS 的 username 往往就是账号/工单号，
/// 泄露它一样能定位到人。`Some("<redacted>")` 仍然能看出「这条代理带没带凭据」，
/// 排障够用。
fn mask_credential(value: &Option<String>) -> Option<&'static str> {
    value.as_ref().map(|_| "<redacted>")
}

impl fmt::Debug for EgressProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EgressProxy")
            .field("type", &self.kind.as_str())
            .field("host", &self.host)
            .field("port", &self.port)
            // 契约 §7.1：凭据（username 与 password）都不得出现在日志/审计里。
            // Debug 打码是为了让一次误用的 `{proxy:?}` 也不会泄露。
            .field("username", &mask_credential(&self.username))
            .field("password", &mask_credential(&self.password))
            .finish()
    }
}

// ── ① companion 侧：读 egress.toml + 解析继承链 ──────────────────────────────

/// `egress.toml` 的 `[relay]` 段（契约 §2.2）。缺省即「中继开启」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelaySettings {
    /// `[relay].enabled`：缺省 `true`；类型非法时按契约 §2.5 降级为 `false`。
    pub enabled: bool,
}

impl Default for RelaySettings {
    /// 契约 §2.2：`[relay]` 整段缺失 == 全部默认值，即**中继默认开**。
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// 一份解析后的 `egress.toml`：代理定义 + 中继开关 + 「是否已迁移」的原始事实。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressFile {
    pub relay: RelaySettings,
    /// 文件里是否出现过 `[relay]` 键（判断「已迁移」的一半，见 [Self::is_migrated]）。
    pub relay_present: bool,
    pub proxies: HashMap<String, EgressProxy>,
}

impl EgressFile {
    /// 「文件已迁移」= 有 `[relay]` 键，**或** 任何定义带 `local_port`（契约 §3.2 规则 7）。
    ///
    /// 未迁移 = v1.10.0 的老文件（既没 `[relay]` 也没人写过端口）：这种文件里中继视为
    /// **明确未启用**，带凭据的定义回退上游（R13 保留的可用性窗口）；一旦迁移过
    /// （server 写过 `[relay]` 或端口表），缺 `local_port` 就是故障 → fail-closed。
    pub fn is_migrated(&self) -> bool {
        self.relay_present
            || self
                .proxies
                .values()
                .any(|proxy| proxy.local_port.is_some())
    }
}

/// 解析 `egress.toml` 文本（纯函数；文件 IO 在 [load_egress]）。
///
/// 出口代理是可选功能：语法错误、版本不认识都只记一行日志并返回**默认值**（中继开、
/// 没有代理）——配置坏了不能让机型伪装整体失效。语法错误的详细定位由 server 侧的
/// `GET /api/v1/egress` 报（契约 §2.1 末），这里不重复展开（错误详情可能带行内原文，
/// 而行可能是 password）。
pub fn parse_egress(content: &str) -> EgressFile {
    let mut file = EgressFile::default();

    let document: toml::Value = match toml::from_str(content) {
        Ok(document) => document,
        Err(err) => {
            warn!(
                "egress.toml unparsable ({}); app-level egress disabled",
                err.message()
            );
            return file;
        }
    };

    if let Some(version) = document.get("version").and_then(toml::Value::as_integer)
        && version != SUPPORTED_VERSION
    {
        warn!("egress.toml version {version} is not supported; app-level egress disabled");
        return file;
    }

    file.relay_present = document.get("relay").is_some();
    file.relay = parse_relay_settings(document.get("relay"));

    let Some(table) = document.get("proxies").and_then(toml::Value::as_table) else {
        return file;
    };

    for (name, value) in table {
        match EgressProxy::from_table(value) {
            // `direct` 不是合法代理名（只能由 `[[apps]].egress` 表达），缺 host/port
            // 的定义也注入不了：都当坏条目跳过，避免把「半张配置」当成可用代理。
            Some(proxy) if proxy.endpoint().is_some() => {
                file.proxies.insert(name.clone(), proxy);
            }
            _ => warn!("egress.toml: proxy '{name}' is unusable; ignored"),
        }
    }

    file
}

/// 读 `[relay]` 段；单个键非法只降级该键（契约 §2.5：**不**报 500、不写文件）。
fn parse_relay_settings(section: Option<&toml::Value>) -> RelaySettings {
    let Some(section) = section else {
        return RelaySettings::default();
    };

    let Some(table) = section.as_table() else {
        // `[relay]` 写成标量/数组：整段视为非法，按 §2.5「enabled 非法 → false」保守处理。
        warn!("[egress] relay config degraded key=enabled (section is not a table)");
        return RelaySettings { enabled: false };
    };

    let enabled = match table.get("enabled") {
        None => true,
        Some(toml::Value::Boolean(enabled)) => *enabled,
        Some(_) => {
            warn!("[egress] relay config degraded key=enabled");
            false
        }
    };

    RelaySettings { enabled }
}

/// 读 0600 的 `egress.toml`（文件不存在/不可读按「没有代理」处理）。
///
/// companion（root 域）每渲染一次 spec 调一次；模块进程不碰这个文件（契约 §5.1 末）。
pub fn load_egress() -> EgressFile {
    match fs::read_to_string(EGRESS_PATH) {
        Ok(content) => parse_egress(&content),
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    "egress.toml unreadable ({}); app-level egress disabled",
                    err.kind()
                );
            }
            EgressFile::default()
        }
    }
}

/// 「要写进 spec 的代理表」+ 渲染期间产生的降级日志（由 `spec::render_map` 统一输出）。
#[derive(Debug, Default)]
pub struct SpecProxies {
    pub proxies: HashMap<String, EgressProxy>,
    /// 一行一条，**不含任何凭据**。三种来源：中继明确关闭的回退、文件未迁移的回退、
    /// 以及「中继开着却缺 `local_port`」的 fail-closed。
    pub warnings: Vec<String>,
}

/// 把 `egress.toml` 的代理表翻译成 spec 视图（契约 §3.2 的应用级判定，v1.2 收窄）。
///
/// 规则顺序即优先级：
/// 1. **无凭据** → 上游 `host:port`（不经中继）；
/// 2. **带凭据 + `[relay].enabled=false`** → 回退上游 `host:port`，**不写凭据载体**（§3.2 规则 5）；
/// 3. **带凭据 + 文件未迁移**（无 `[relay]` 且无任何 `local_port`）→ 同上回退（R13 的可用性窗口）；
/// 4. **带凭据 + relay 开 + 已迁移 + 有 `local_port`** → `127.0.0.1:<local_port>` + `relay = true`；
/// 5. **带凭据 + relay 开 + 已迁移 + 缺 `local_port`** → **fail-closed** `127.0.0.1:9`
///    （R13：中继开着就不是回退理由；池耗尽/写回失败/手改坏都走这条）。
///
/// 任何分支都不把 username/password 写进 spec（v1.2 起一律不写，§9.2 B13）。
pub fn spec_proxies(file: &EgressFile) -> SpecProxies {
    let mut view = SpecProxies {
        proxies: HashMap::with_capacity(file.proxies.len()),
        warnings: Vec::new(),
    };
    let migrated = file.is_migrated();

    for (name, proxy) in &file.proxies {
        // 上游视图：清掉 `egress.toml` 专有的 `local_port` 与 spec 专有的 `relay`，
        // 并且**永不**带凭据（`to_toml` 也不再写，这里同步清掉便于断言）。
        let mut upstream = proxy.clone();
        upstream.local_port = None;
        upstream.relay = false;
        upstream.username = None;
        upstream.password = None;

        // 只有「可用的上游定义」才谈得上走中继：缺 host/port 的定义注入不了，改写成本地
        // 端口反而可能指到别的定义的中继上。
        let relay_candidate = proxy.kind != EgressType::Direct
            && proxy.endpoint().is_some()
            && proxy.has_credentials();
        if !relay_candidate {
            view.proxies.insert(name.clone(), upstream);
            continue;
        }

        let endpoint = endpoint_text(proxy);

        if !file.relay.enabled {
            // 契约 §3.2 规则 5：中继**明确关闭** → 回退上游，且不写凭据载体。
            view.warnings.push(format!(
                "[egress] relay disabled: proxy '{name}' injected as upstream {endpoint} without credentials"
            ));
            view.proxies.insert(name.clone(), upstream);
            continue;
        }

        if !migrated {
            // R13 保留的唯一「不算故障」的回退：老文件还没被 server 迁移过。
            view.warnings.push(format!(
                "[egress] relay not migrated: proxy '{name}' injected as upstream {endpoint} without credentials"
            ));
            view.proxies.insert(name.clone(), upstream);
            continue;
        }

        match proxy.local_port {
            Some(local_port) => {
                view.proxies
                    .insert(name.clone(), proxy.relay_view(local_port));
            }
            None => {
                // 契约 §3.2 规则 7（R13）：中继开着、文件已迁移，却缺端口 → fail-closed。
                // 绝不回退上游、不静默直连。
                view.warnings.push(format!(
                    "[egress] relay fail-closed for proxy '{name}' reason=port_missing; injecting {RELAY_HOST}:{RELAY_FAIL_CLOSED_PORT}"
                ));
                view.proxies
                    .insert(name.clone(), EgressProxy::fail_closed(proxy.kind));
            }
        }
    }

    view
}

/// 把定义的可用上游写成 `host:port`（只用于日志；定义不可用时给 `-`）。
fn endpoint_text(proxy: &EgressProxy) -> String {
    proxy
        .endpoint()
        .map(|(host, port)| format!("{host}:{port}"))
        .unwrap_or_else(|| "-".to_string())
}

/// 解析某个包**需要应用级注入**的出口代理。
///
/// 只有 `[[apps]].egress` 显式写了值才返回 `Some`：
///
/// - 键不存在 / 空串 → `None`：该应用**继承设备级行为**，而设备级代理本来就由框架在
///   每个进程里写好（方案 A），再注入一遍纯属重复——代价却是每个进程多一个 PLT hook、
///   一个重放线程、并且模块 .so 不能被 dlclose（见 hooks::arm_egress_replay）。
///   t9 真机验收时就踩过这个：设备级 default 让**每个**应用都起了重放线程。
/// - `__DIRECT__` → 强制直连（必须注入并重放，否则框架会把设备级代理写回来）；
/// - 合法代理名 → 该代理；名字在 `egress.toml` 里不存在时「视为未配置」：**不**回落到设备级
///   默认，也不注入——用户显式指定的代理不存在，静默换成另一个出口比直连更危险
///   （出口 IP 与用户预期不符）。只在 companion 日志留一行。
pub fn resolve_for_package(
    config: &Config,
    proxies: &HashMap<String, EgressProxy>,
    package: &str,
) -> Option<EgressProxy> {
    let choice = config
        .get_app_config(package)
        .and_then(|app| app.egress.as_deref())
        .filter(|value| !value.is_empty())?;

    resolve_name(proxies, choice, package)
}

fn resolve_name(
    proxies: &HashMap<String, EgressProxy>,
    name: &str,
    package: &str,
) -> Option<EgressProxy> {
    if name == DIRECT_MARKER {
        return Some(EgressProxy::direct());
    }
    match proxies.get(name) {
        Some(proxy) => Some(proxy.clone()),
        None => {
            warn!("egress: '{package}' references undefined proxy '{name}'; no injection");
            None
        }
    }
}

// ── ② 模块侧：从 GetSpec 的 TOML 取回 [egress_proxy] ─────────────────────────

/// 从 `GetSpec` 的 TOML 里取回 `[egress_proxy]`。小节不存在 = 未配置（返回 `None`）。
///
/// 两条**完全不同**的错误口径（v1.2 冻结，很容易搞混）：
///
/// - **非 relay 段**解析失败 → 仍按「未配置」处理（`None`）：spec 是 companion 自己渲染的，
///   真坏了也不该让目标应用启动失败（契约 §5.5）；日志只报小节坏了，不打印内容；
/// - **`relay = true` 的段** → **永远返回一个可注入的代理**（R12）：目标非法就换成哨兵
///   `127.0.0.1:9`，绝不返回「未配置」（那会让应用继承设备级代理或直连 = 裸奔），也绝不
///   回退上游。唯一例外是同段还带凭据：剥掉后照常注入（§3.3 规则 3）。
///
/// `package` 只用于日志定位。
pub fn from_spec_doc(document: &str, package: &str) -> Option<EgressProxy> {
    let value: toml::Value = toml::from_str(document).ok()?;
    let section = value.get(SPEC_SECTION)?;

    // 先看**原始表**里的 relay：即使 type/port 解析不出来，也必须 fail-closed，
    // 而不是「未配置」（§3.3 规则 1/2）。
    if section.get("relay").and_then(toml::Value::as_bool) == Some(true) {
        return Some(relay_spec_proxy(section, package));
    }

    let Some(proxy) = EgressProxy::from_table(section) else {
        warn!("[egress] spec [{SPEC_SECTION}] is malformed; app-level egress disabled");
        return None;
    };
    if proxy.endpoint().is_none() && proxy.kind != EgressType::Direct {
        warn!("[egress] spec [{SPEC_SECTION}] has no usable host/port; egress disabled");
        return None;
    }
    Some(proxy)
}

/// `relay = true` 的段：归一化 + 打点，**恒返回**可注入的代理（契约 §3.3 规则 2–4）。
fn relay_spec_proxy(section: &toml::Value, package: &str) -> EgressProxy {
    let Some(proxy) = EgressProxy::from_table(section) else {
        // type / port 类型非法：连表都解析不出来，直接哨兵（协议族按 type 尽量猜）。
        let line = relay_fail_closed_line(
            package,
            RelayTargetFailure::InvalidTarget,
            section_has_credentials(section),
        );
        warn!("{line}");
        return EgressProxy::fail_closed(relay_sentinel_kind(section));
    };

    let normalized = proxy.relay_normalized();
    match normalized.failure {
        Some(failure) => {
            let line = relay_fail_closed_line(package, failure, normalized.had_credentials);
            warn!("{line}");
        }
        // 规则 3：仅多带凭据 → 剥掉照常注入中继目标（这是修复，不是降级）。
        None if normalized.had_credentials => {
            let line = relay_creds_stripped_line(package);
            warn!("{line}");
        }
        None => {}
    }
    normalized.proxy
}

/// 段里是否带过凭据（只看有没有非空值，**不打印值**）。
fn section_has_credentials(section: &toml::Value) -> bool {
    non_empty_str(section, "username").is_some() || non_empty_str(section, "password").is_some()
}

/// 表解析失败时用来选哨兵协议族：type 认识就用它，否则按 http 兜底。
fn relay_sentinel_kind(section: &toml::Value) -> EgressType {
    match section
        .get("type")
        .and_then(toml::Value::as_str)
        .and_then(EgressType::parse)
    {
        Some(EgressType::Socks5) => EgressType::Socks5,
        _ => EgressType::Http,
    }
}

// ── ③ 进程内注入计划（纯逻辑，JNI 见 hooks::apply_egress_proxy）──────────────

/// 一条系统属性动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyOp {
    Set {
        key: &'static str,
        value: String,
    },
    /// 清除父进程继承来的同名属性（`__DIRECT__` 与协议互斥都靠它）。
    Clear {
        key: &'static str,
    },
}

/// 要装到 `java.net.ProxySelector.setDefault` 的形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorSpec {
    /// `ProxySelector.of(Proxy.NO_PROXY)`——`__DIRECT__` 的「主动直连」。
    Direct,
    /// `ProxySelector.of(new Proxy(Type.HTTP|SOCKS, InetSocketAddress.createUnresolved(host, port)))`
    Proxy {
        kind: EgressType,
        host: String,
        port: u16,
    },
}

/// 一次进程内注入的完整计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectionPlan {
    /// 按顺序执行；单条失败只跳过这一条（契约 §5.5）。
    pub property_ops: Vec<PropertyOp>,
    pub selector: Option<SelectorSpec>,
    /// spec 里带了凭据（只可能来自旧版 companion）：v1.2 起**任何路径都不注入凭据载体**
    /// （契约 §9.1 K1 / §9.2 B13），该标志只驱动一行 unsupported-auth 日志。
    pub unsupported_auth: bool,
}

/// 把解析结果翻译成注入计划；`None` 只可能出现在**非 relay** 的残缺定义上
/// （`http`/`socks5` 缺 host 或 port），调用方不注入任何东西。
///
/// **relay 视图永远有计划**（契约 §3.3 规则 2/7）：非法目标先归一成哨兵
/// `127.0.0.1:9`、`type=direct` 归一成主动直连——两条都必须走本函数，因为只有它返回
/// 计划才会 arm replay（否则框架 ~95ms 后把属性重写回设备级值，fail-closed 失效）。
pub fn injection_plan(proxy: &EgressProxy) -> Option<InjectionPlan> {
    if proxy.relay {
        let normalized = proxy.relay_normalized();
        return plan_for(&normalized.proxy);
    }
    plan_for(proxy)
}

/// 按 `kind`/`endpoint` 生成计划（不看 `relay` 标记；归一化由 [injection_plan] 完成）。
fn plan_for(proxy: &EgressProxy) -> Option<InjectionPlan> {
    match proxy.kind {
        EgressType::Direct => Some(InjectionPlan {
            // 契约 §5.2：`__DIRECT__` 是**主动**直连——不设置任何代理地址属性，只清掉
            // 父进程可能继承的同名属性（zygote 里一般没有，纯防御），再尝试装
            // direct ProxySelector（系统代理开着时，靠它把应用拉回直连）。
            property_ops: PROXY_PROPERTY_KEYS
                .iter()
                .map(|key| PropertyOp::Clear { key })
                .collect(),
            selector: Some(SelectorSpec::Direct),
            unsupported_auth: false,
        }),
        EgressType::Http => {
            let (host, port) = proxy.endpoint()?;
            Some(InjectionPlan {
                property_ops: vec![
                    PropertyOp::Set {
                        key: "http.proxyHost",
                        value: host.to_string(),
                    },
                    PropertyOp::Set {
                        key: "http.proxyPort",
                        value: port.to_string(),
                    },
                    PropertyOp::Set {
                        key: "https.proxyHost",
                        value: host.to_string(),
                    },
                    PropertyOp::Set {
                        key: "https.proxyPort",
                        value: port.to_string(),
                    },
                    // 契约 §5.3 互斥：SOCKS 属性指向 HTTP 端口会让 SOCKS 握手打到 HTTP
                    // 口上，表现为「全都连不上」。
                    PropertyOp::Clear {
                        key: "socksProxyHost",
                    },
                    PropertyOp::Clear {
                        key: "socksProxyPort",
                    },
                ],
                selector: Some(SelectorSpec::Proxy {
                    kind: EgressType::Http,
                    host: host.to_string(),
                    port,
                }),
                unsupported_auth: proxy.has_credentials(),
            })
        }
        EgressType::Socks5 => {
            let (host, port) = proxy.endpoint()?;
            Some(InjectionPlan {
                property_ops: vec![
                    PropertyOp::Set {
                        key: "socksProxyHost",
                        value: host.to_string(),
                    },
                    PropertyOp::Set {
                        key: "socksProxyPort",
                        value: port.to_string(),
                    },
                    // 契约 §5.3 反方向的互斥：HTTP 属性指向 SOCKS 端口会让客户端把
                    // `CONNECT` 发到 SOCKS 口上。
                    PropertyOp::Clear {
                        key: "http.proxyHost",
                    },
                    PropertyOp::Clear {
                        key: "http.proxyPort",
                    },
                    PropertyOp::Clear {
                        key: "https.proxyHost",
                    },
                    PropertyOp::Clear {
                        key: "https.proxyPort",
                    },
                ],
                selector: Some(SelectorSpec::Proxy {
                    kind: EgressType::Socks5,
                    host: host.to_string(),
                    port,
                }),
                unsupported_auth: proxy.has_credentials(),
            })
        }
    }
}

// ── ④ 后置重放：F1/F2 的修复模型 ─────────────────────────────────────────────
//
// 真机证据（docs/design/egress/t9-acceptance.md §5.2/§5.3）：`pre_app_specialize` 里写进
// `http.proxyHost` 的值会在 ~95ms 后被框架改写——`ActivityThread` 绑定应用时按**设备级**
// `ProxyInfo` 调 `android.net.Proxy.setHttpProxySystemProperty(...)`，把 http/https 代理属性
// 整体重写一遍（设备级直连则直接清空）。zygisk 的 pre/post_app_specialize 都早于它，
// 所以「只写一次」必然被吃掉：应用级 http 代理看不到、`__DIRECT__` 也拉不回直连。
//
// 修法见 `hooks::arm_egress_replay`：注入之后起一个短命线程，按本节的**时间表**读回属性，
// 一发现与期望不符（= 框架刚写过）就再写一遍。本节只放纯逻辑——期望、偏差、时间表、
// 循环——读/写/睡眠全部由调用方注入，因此宿主机上可以用假时钟精确复现
// 「注入 → 95ms 被覆盖 → 下一次 tick 修回」这条时间序（见测试），不需要真机。

/// 一条「注入后必须一直成立」的系统属性期望。
///
/// v1.2 起注入清单只有 6 个代理属性（凭据载体已移除，§9.2 B13），因此这里没有
/// 「需要打码的期望值」——所有键值都是 host/port，可以安全进日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayExpectation {
    /// 属性名（全部是 Java 系统属性，不是 Android 属性）。
    pub key: &'static str,
    /// `None` = 必须不存在/空串；`Some(_)` = 必须等于该值。
    pub want: Option<String>,
}

/// 把注入计划翻译成属性期望：`Set` → 必须等于该值，`Clear` → 必须为空。
pub fn replay_expectations(plan: &InjectionPlan) -> Vec<ReplayExpectation> {
    plan.property_ops
        .iter()
        .map(|op| match op {
            PropertyOp::Set { key, value } => ReplayExpectation {
                key,
                want: Some(value.clone()),
            },
            PropertyOp::Clear { key } => ReplayExpectation { key, want: None },
        })
        .collect()
}

/// 一条读回与期望的偏差（键值都是 host/port，无凭据，可安全进日志）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub key: &'static str,
    pub want: Option<String>,
    /// 实际读回（空串归一化为 `None`）。
    pub got: Option<String>,
}

impl Divergence {
    /// 日志文本：`实际->期望`。
    pub fn describe(&self) -> String {
        format!(
            "{}={}->{}",
            self.key,
            self.got.as_deref().unwrap_or("-"),
            self.want.as_deref().unwrap_or("-")
        )
    }
}

/// 逐条比较读回与期望；`read` 返回 `None` 表示该属性不存在（或为 null）。
pub fn divergences_of<C, R>(
    expectations: &[ReplayExpectation],
    ctx: &mut C,
    mut read: R,
) -> Vec<Divergence>
where
    R: FnMut(&mut C, &str) -> Option<String>,
{
    let mut divergences = Vec::new();
    for expectation in expectations {
        let got = read(ctx, expectation.key).filter(|value| !value.is_empty());
        if got == expectation.want {
            continue;
        }
        divergences.push(Divergence {
            key: expectation.key,
            want: expectation.want.clone(),
            got,
        });
    }
    divergences
}

/// 重放窗口总时长（毫秒，相对注入时刻）。
pub const REPLAY_WINDOW_MS: u64 = 2000;

/// 重放时间表（毫秒，**相对注入时刻的绝对偏移**，严格升序）。
///
/// 三段：
/// - **热点区** `0..=160` 每 2ms：框架在 `ActivityThread.handleBindApplication` 里、
///   `Application` 构造之后、`installContentProviders`/`Application.onCreate` 之前调
///   `ActivityThread.updateHttpProxy(app)` → `Proxy.setHttpProxyConfiguration(...)`，
///   把 http/https 代理属性（含 nonProxyHosts 与 ProxySelector）整体重写一遍。t9 实测这次
///   覆盖发生在注入后 ~95ms，应用第一次读属性则在 Activity 起来之后——所以 2ms 的读回节奏
///   足够抢在它之前，同时把 CPU 代价压在几十次属性读以内；
/// - **收窄区** `165..=300` 每 5ms：覆盖更慢的机型/更慢的冷启动；
/// - **冷却区** `300..=2000` 几何退避：只为慢冷启动兜底。
///
/// 已知边界：设备级代理在应用**运行中**被改（AMS 广播 `updateHttpProxy`）会落在窗口之外，
/// 这种情况本次不覆盖（见 [arm_egress_replay] 的注释）。
pub fn replay_offsets_ms() -> Vec<u64> {
    let mut offsets: Vec<u64> = (0..=160).step_by(2).collect();
    offsets.extend((165..=300).step_by(5));

    let mut offset = 340u64;
    while offset < REPLAY_WINDOW_MS {
        offsets.push(offset);
        offset = offset * 5 / 4 + 10;
    }
    offsets.push(REPLAY_WINDOW_MS);

    offsets
}

/// 一次 tick 的结果（日志与测试共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayTick {
    Clean {
        at_ms: u64,
    },
    Diverged {
        at_ms: u64,
        found: Vec<Divergence>,
        applied: usize,
    },
}

/// 重放循环的汇总。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplaySummary {
    pub probes: usize,
    pub replays: usize,
    pub divergences: usize,
    pub clean_streak: usize,
    /// 最后一次动作之后，属性是否处于期望态。
    pub state_matches: bool,
}

/// 纯逻辑重放循环：`sleep_until` 睡到相对注入时刻的第 N 毫秒，`read`/`write` 对同一个
/// 上下文 `ctx` 操作（线上是 JNI 的 `Env`，测试里是一个 HashMap）。
///
/// 返回逐个 tick 的结果（用于打时间序日志）与汇总。任何写失败都只体现在 `applied`
/// 计数上，不 panic、不外抛——注入链路「失败只记日志」的语义一路保留到这里。
///
/// **不做早退**：我们无法知道框架到底哪一刻写（t9 实测 ~95ms，本次真机实测 ~28ms），
/// 提前收工就可能漏掉后一次覆盖。整张表跑完只有 ~120 次属性读（线程绝大部分时间在睡），
/// 换来的确定性远比省这点开销值。
pub fn run_replay_loop<C, R, W, S>(
    ctx: &mut C,
    expectations: &[ReplayExpectation],
    mut read: R,
    mut write: W,
    mut sleep_until: S,
) -> (Vec<ReplayTick>, ReplaySummary)
where
    R: FnMut(&mut C, &str) -> Option<String>,
    W: FnMut(&mut C, &ReplayExpectation) -> bool,
    S: FnMut(u64),
{
    let mut ticks = Vec::new();
    let mut summary = ReplaySummary::default();

    for offset in replay_offsets_ms() {
        sleep_until(offset);

        let found = divergences_of(expectations, ctx, &mut read);
        summary.probes += 1;

        if found.is_empty() {
            summary.clean_streak += 1;
            summary.state_matches = true;
            ticks.push(ReplayTick::Clean { at_ms: offset });
        } else {
            // 期望是「整套状态」，偏差出现时把整套再写一遍（幂等，且能一并修掉框架
            // 顺手写进来的其它键，例如 __DIRECT__ 场景下的 http.proxyPort）。
            let mut applied = 0usize;
            for expectation in expectations {
                if write(ctx, expectation) {
                    applied += 1;
                }
            }
            summary.replays += 1;
            summary.divergences += 1;
            summary.clean_streak = 0;
            summary.state_matches = applied == expectations.len();
            ticks.push(ReplayTick::Diverged {
                at_ms: offset,
                found,
                applied,
            });
        }
    }

    (ticks, summary)
}

// ── ⑤ arming 的降级判定（F-2：两条真实降级分支要有可驱动的单测）──────────────
//
// `arm_egress_replay` 在真机上只有两种会失败的方式：目标镜像 stat 不到、`plt_hook_commit()`
// 被 zygisk 拒绝；加上排障开关一共三条退化路径。把「决策 + 日志文案 + 是否保留模块映射」
// 抽到这里做成纯逻辑，hooks 只做 JNI/PLT 适配，宿主机就能直接驱动这些分支并断言。

/// arming 失败的两条**真实**降级分支。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayArmFailure {
    /// `libandroid_runtime.so` 两条候选路径都 stat 不到
    LibAndroidRuntimeMissing,
    /// `plt_hook_commit()` 被 zygisk 拒绝
    PltHookCommitFailed(String),
}

impl ReplayArmFailure {
    /// `reason=` 的取值：与历史日志用同一套字符串，方便老日志检索。
    pub fn reason(&self) -> &'static str {
        match self {
            Self::LibAndroidRuntimeMissing => "libandroid_runtime_not_found",
            Self::PltHookCommitFailed(_) => "commit_failed",
        }
    }

    /// 统一的降级日志行（注入链路唯一来源；hooks 只负责输出）。
    pub fn log_line(&self) -> String {
        match self {
            Self::LibAndroidRuntimeMissing => {
                format!("[egress] replay hook unavailable reason={}", self.reason())
            }
            Self::PltHookCommitFailed(error) => format!(
                "[egress] replay hook unavailable reason={} error={error}",
                self.reason()
            ),
        }
    }
}

/// 第一阶段决策：debug 护栏 + 排障开关 + 镜像定位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArmPlan {
    /// 排障开关命中（且 debug = true）：整条链路退化为一次性注入
    DisabledByDebugProp,
    /// 目标镜像缺失：直接降级
    Unavailable(ReplayArmFailure),
    /// 可以注册并提交 hook
    Proceed { device: u64, inode: u64 },
}

/// arming 的最终结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArmOutcome {
    /// hook 已提交，重放线程会被拉起来
    Armed,
    /// 排障开关命中
    DisabledByDebugProp,
    Unavailable(ReplayArmFailure),
}

impl ArmOutcome {
    /// 只有 Armed 才需要保留模块映射（重放线程与 hook 的代码都在 .so 里）。
    pub fn keeps_module_mapped(&self) -> bool {
        matches!(self, Self::Armed)
    }

    /// 需要落日志的行（Armed 不产生降级日志）。
    pub fn log_line(&self) -> Option<String> {
        match self {
            Self::Armed => None,
            Self::DisabledByDebugProp => Some(
                "[egress] replay hook unavailable reason=debug_disable_replay \
                 (debug.devmask.egress.disable_replay is set; one-shot injection only)"
                    .to_string(),
            ),
            Self::Unavailable(failure) => Some(failure.log_line()),
        }
    }
}

/// 第一阶段：`debug` 护栏 + 镜像定位（纯逻辑；宿主机可直接驱动「开关命中」与「镜像缺失」）。
pub fn plan_replay_arm<P, I>(debug: bool, prop_disabled: P, image: I) -> ArmPlan
where
    P: FnOnce() -> bool,
    I: FnOnce() -> Option<(u64, u64)>,
{
    // 护栏：config.toml 的 debug 关闭时，排障属性一律不生效。
    if debug && prop_disabled() {
        return ArmPlan::DisabledByDebugProp;
    }
    match image() {
        Some((device, inode)) => ArmPlan::Proceed { device, inode },
        None => ArmPlan::Unavailable(ReplayArmFailure::LibAndroidRuntimeMissing),
    }
}

/// 第二阶段：把 `plt_hook_commit()` 的结果映射成结论（纯逻辑）。
pub fn finish_replay_arm(commit: Result<(), String>) -> ArmOutcome {
    match commit {
        Ok(()) => ArmOutcome::Armed,
        Err(error) => ArmOutcome::Unavailable(ReplayArmFailure::PltHookCommitFailed(error)),
    }
}

// ── ⑥ armed 但 hook 一直没命中（M-5：可观测性）────────────────────────────────

/// 「武装成功、但重放的触发 hook 始终没命中」时的一行诊断。
///
/// 三个要素缺一不可（宿主机单测会逐条断言）：
/// 1. **窗口**：等了多久（`within {window_ms}ms`）；
/// 2. **等待时长**：实际等到这一刻用了多久（`waited={waited_ms}ms`）；
/// 3. **后果**：模块代码页被保留（armed 就跳过 `dlclose`，且**不能**在这里卸载——
///    重放线程/hook 的代码就在这个 .so 里，卸载即崩）。
///
/// 文案放在这里而不是 hooks 里，是为了让宿主机能直接驱动断言（hooks 只在 Android 编）。
pub fn replay_not_fired_line(window_ms: u64, waited_ms: u64, package: &str) -> String {
    let package = if package.is_empty() { "-" } else { package };
    format!(
        "[egress] replay hook not fired within {window_ms}ms (no replay started); \
         module retained (code pages stay mapped for this process) \
         package={package} waited={waited_ms}ms"
    )
}

/// 该不该打上面那行：只有「武装过 + 从未 spawn + 还没报过」才打。
///
/// 正常路径（hook 命中、重放线程已起）必须**完全静默**——这是 t26 的硬要求。
pub fn should_report_never_fired(
    replay_active: bool,
    spawned: bool,
    already_reported: bool,
) -> bool {
    replay_active && !spawned && !already_reported
}
#[cfg(test)]
mod tests {
    use super::*;

    fn proxies(entries: &[(&str, EgressProxy)]) -> HashMap<String, EgressProxy> {
        entries
            .iter()
            .map(|(name, proxy)| ((*name).to_string(), proxy.clone()))
            .collect()
    }

    fn config_from(toml_text: &str) -> Config {
        Config::from_toml(toml_text).expect("test config parses")
    }

    fn http_proxy() -> EgressProxy {
        EgressProxy::endpoint_proxy(EgressType::Http, "203.0.113.10", 8080)
    }

    fn socks_proxy() -> EgressProxy {
        EgressProxy::endpoint_proxy(EgressType::Socks5, "198.51.100.7", 1080)
    }

    fn value_of(plan: &InjectionPlan, key: &str) -> Option<String> {
        plan.property_ops.iter().find_map(|op| match op {
            PropertyOp::Set { key: op_key, value } if *op_key == key => Some(value.clone()),
            _ => None,
        })
    }

    fn cleared(plan: &InjectionPlan, key: &str) -> bool {
        plan.property_ops
            .iter()
            .any(|op| matches!(op, PropertyOp::Clear { key: op_key } if *op_key == key))
    }

    #[test]
    fn http_plan_sets_both_http_and_https_properties() {
        let plan = injection_plan(&http_proxy()).expect("http plan");
        assert_eq!(
            value_of(&plan, "http.proxyHost").as_deref(),
            Some("203.0.113.10")
        );
        assert_eq!(value_of(&plan, "http.proxyPort").as_deref(), Some("8080"));
        assert_eq!(
            value_of(&plan, "https.proxyHost").as_deref(),
            Some("203.0.113.10")
        );
        assert_eq!(value_of(&plan, "https.proxyPort").as_deref(), Some("8080"));
        assert_eq!(value_of(&plan, "socksProxyHost"), None);
        assert_eq!(value_of(&plan, "socksProxyPort"), None);
        assert!(cleared(&plan, "socksProxyHost"));
        assert_eq!(
            plan.selector,
            Some(SelectorSpec::Proxy {
                kind: EgressType::Http,
                host: "203.0.113.10".to_string(),
                port: 8080,
            })
        );
        assert!(!plan.unsupported_auth);
    }

    #[test]
    fn socks_plan_sets_only_socks_properties() {
        let plan = injection_plan(&socks_proxy()).expect("socks plan");
        assert_eq!(
            value_of(&plan, "socksProxyHost").as_deref(),
            Some("198.51.100.7")
        );
        assert_eq!(value_of(&plan, "socksProxyPort").as_deref(), Some("1080"));
        for key in [
            "http.proxyHost",
            "http.proxyPort",
            "https.proxyHost",
            "https.proxyPort",
        ] {
            assert_eq!(
                value_of(&plan, key),
                None,
                "{key} must not be set for socks5"
            );
            assert!(cleared(&plan, key), "{key} must be cleared for socks5");
        }
        assert_eq!(
            plan.selector,
            Some(SelectorSpec::Proxy {
                kind: EgressType::Socks5,
                host: "198.51.100.7".to_string(),
                port: 1080,
            })
        );
    }

    #[test]
    fn direct_plan_sets_no_proxy_property_at_all() {
        let plan = injection_plan(&EgressProxy::direct()).expect("direct plan");
        assert!(
            plan.property_ops
                .iter()
                .all(|op| matches!(op, PropertyOp::Clear { .. })),
            "__DIRECT__ must never set a proxy property: {:?}",
            plan.property_ops
        );
        for key in PROXY_PROPERTY_KEYS {
            assert!(cleared(&plan, key));
        }
        assert_eq!(plan.selector, Some(SelectorSpec::Direct));
        assert!(!plan.unsupported_auth);
    }

    #[test]
    fn http_credentials_are_unsupported_and_never_planned_into_properties() {
        let mut proxy = http_proxy();
        proxy.username = Some("user".to_string());
        proxy.password = Some("pas sw0rd".to_string());
        let plan = injection_plan(&proxy).expect("http plan");
        assert!(plan.unsupported_auth);
        assert!(
            !plan.property_ops.iter().any(
                |op| matches!(op, PropertyOp::Set { value, .. } if value.contains("pas sw0rd"))
            ),
            "http credentials must not be planned into system properties"
        );
    }

    #[test]
    fn socks_proxy_never_gets_credential_carriers() {
        // v1.2（§9.1 K1 / §9.2 B13）：凭据只在设备内的中继里出示，应用进程拿不到；
        // 于是注入清单里只有 6 个代理属性，连 socks 的凭据属性都不出现。
        let mut proxy = socks_proxy();
        proxy.username = Some("user".to_string());
        proxy.password = Some("pas sw0rd".to_string());
        let plan = injection_plan(&proxy).expect("socks plan");

        assert!(
            plan.unsupported_auth,
            "带凭据的定义必须留一行 unsupported-auth"
        );
        assert!(
            !plan.property_ops.iter().any(
                |op| matches!(op, PropertyOp::Set { key, .. } if key.starts_with("java.net."))
            ),
            "{:?}",
            plan.property_ops
        );
        assert!(
            replay_expectations(&plan)
                .iter()
                .all(|expectation| !expectation.key.starts_with("java.net.")),
            "凭据载体不得出现在重放清单里"
        );
    }

    #[test]
    fn incomplete_plan_is_rejected() {
        let mut proxy = http_proxy();
        proxy.port = None;
        assert!(injection_plan(&proxy).is_none());
        proxy.port = Some(0);
        assert!(injection_plan(&proxy).is_none());
        proxy.port = Some(8080);
        proxy.host = Some(String::new());
        assert!(injection_plan(&proxy).is_none());
    }

    #[test]
    fn only_explicit_app_level_egress_is_injected() {
        let config = config_from(
            r#"
            [egress]
            default = "us_la_01"

            [[apps]]
            package = "com.example.a"
            egress = "jp_tokyo_02"

            [[apps]]
            package = "com.example.b"

            [[apps]]
            package = "com.example.c"
            egress = "__DIRECT__"

            [[apps]]
            package = "com.example.d"
            egress = ""
            "#,
        );
        let proxies = proxies(&[("us_la_01", http_proxy()), ("jp_tokyo_02", socks_proxy())]);

        // 显式应用级代理 → 注入该代理
        assert_eq!(
            resolve_for_package(&config, &proxies, "com.example.a"),
            Some(socks_proxy())
        );
        // 键不存在 → **不注入**：设备级行为由框架写进每个进程，重复注入只会给每个进程
        // 多一个 hook + 重放线程（t9 真机踩过）。
        assert_eq!(
            resolve_for_package(&config, &proxies, "com.example.b"),
            None
        );
        // __DIRECT__ → 必须注入并重放，否则框架会把设备级代理写回来（F2）
        assert_eq!(
            resolve_for_package(&config, &proxies, "com.example.c"),
            Some(EgressProxy::direct())
        );
        // 空串等价于「未配置」→ 同样不注入
        assert_eq!(
            resolve_for_package(&config, &proxies, "com.example.d"),
            None
        );
        // 只由模板绑定、没有 [[apps]] 记录的包 → 不注入
        assert_eq!(
            resolve_for_package(&config, &proxies, "com.example.e"),
            None
        );
    }

    #[test]
    fn no_egress_configuration_means_no_injection() {
        let config = config_from(
            r#"
            [[apps]]
            package = "com.example.a"
            model = "Pixel 9"
            "#,
        );
        assert_eq!(
            resolve_for_package(&config, &HashMap::new(), "com.example.a"),
            None
        );
    }

    #[test]
    fn undefined_proxy_name_is_not_injected() {
        let config = config_from(
            r#"
            [egress]
            default = "us_la_01"

            [[apps]]
            package = "com.example.a"
            egress = "missing_proxy"
            "#,
        );
        let proxies = proxies(&[("us_la_01", http_proxy())]);
        // 用户显式指定的代理不存在：不注入（也不静默换成设备级默认）。
        assert_eq!(
            resolve_for_package(&config, &proxies, "com.example.a"),
            None
        );
        // 设备级默认本身不存在时同样不注入。
        let empty = HashMap::new();
        assert_eq!(resolve_for_package(&config, &empty, "com.example.b"), None);
    }

    #[test]
    fn spec_section_round_trips_without_credentials() {
        // v1.2（§9.2 B13）：to_toml 不写 username/password，所以「写进去 → 读回来」
        // 只剩 type/host/port；凭据字段必须为空，且文档里连字段名都不该出现。
        let mut proxy = socks_proxy();
        proxy.username = Some("user".to_string());
        proxy.password = Some("p@ss=\"word\"".to_string());

        let mut root = toml::map::Map::new();
        root.insert(SPEC_SECTION.to_string(), proxy.to_toml());
        root.insert("debug".to_string(), toml::Value::Boolean(true));
        let document = toml::to_string(&toml::Value::Table(root)).expect("render spec");

        assert!(!document.contains("username"), "{document}");
        assert!(!document.contains("password"), "{document}");
        assert!(!document.contains("p@ss"), "{document}");

        let parsed = from_spec_doc(&document, "com.example.a").expect("spec parses");
        assert_eq!(parsed, socks_proxy());
        assert!(!parsed.has_credentials());
    }

    #[test]
    fn spec_document_with_apps_and_egress_round_trips() {
        let mut root = toml::map::Map::new();
        root.insert("debug".to_string(), toml::Value::Boolean(false));
        let mut app = toml::map::Map::new();
        app.insert(
            "package".to_string(),
            toml::Value::String("com.example.a".to_string()),
        );
        app.insert(
            "model".to_string(),
            toml::Value::String("Pixel 9".to_string()),
        );
        root.insert(
            "apps".to_string(),
            toml::Value::Array(vec![toml::Value::Table(app)]),
        );
        insert_spec_section(&mut root, &http_proxy());

        let document = toml::to_string(&toml::Value::Table(root)).expect("render spec");

        // 渲染出来的文档必须同时满足两件事：模块能取回代理，Config 仍能整体解析
        //（[egress_proxy] 是 Config 不认识的顶层键，靠 serde 忽略未知字段拿到兼容）。
        assert_eq!(
            from_spec_doc(&document, "com.example.a"),
            Some(http_proxy())
        );
        let config = Config::from_toml(&document).expect("spec still parses as Config");
        assert_eq!(config.apps.len(), 1);
        assert_eq!(config.apps[0].package, "com.example.a");
        assert_eq!(config.apps[0].model.as_deref(), Some("Pixel 9"));
    }

    #[test]
    fn spec_without_section_means_unconfigured() {
        let document = r#"
            debug = false

            [[apps]]
            package = "com.example.a"
            model = "Pixel 9"
        "#;
        assert_eq!(from_spec_doc(document, "com.example.a"), None);
        assert_eq!(from_spec_doc("not = toml = at all", "com.example.a"), None);
    }

    #[test]
    fn direct_section_round_trips() {
        let mut root = toml::map::Map::new();
        root.insert(SPEC_SECTION.to_string(), EgressProxy::direct().to_toml());
        let document = toml::to_string(&toml::Value::Table(root)).expect("render spec");
        assert_eq!(
            from_spec_doc(&document, "com.example.a"),
            Some(EgressProxy::direct())
        );
    }

    #[test]
    fn proxy_table_parsing_validates_port_and_type() {
        let table: toml::Value = toml::from_str(
            r#"
            type = "socks5"
            host = "198.51.100.7"
            port = 1080
            username = ""
            password = "secret"
        "#,
        )
        .expect("parse table");
        let proxy = EgressProxy::from_table(&table).expect("valid proxy");
        assert_eq!(proxy.kind, EgressType::Socks5);
        assert_eq!(proxy.username, None, "empty username means unset");
        assert_eq!(proxy.password.as_deref(), Some("secret"));
        assert!(proxy.has_credentials());

        let bad_port: toml::Value = toml::from_str(
            r#"
            type = "http"
            host = "203.0.113.10"
            port = 70000
        "#,
        )
        .expect("parse table");
        assert!(EgressProxy::from_table(&bad_port).is_none());

        let bad_type: toml::Value = toml::from_str(
            r#"
            type = "socks4"
            host = "203.0.113.10"
            port = 1080
        "#,
        )
        .expect("parse table");
        assert!(EgressProxy::from_table(&bad_type).is_none());
    }

    #[test]
    fn debug_output_redacts_username_and_password() {
        let mut proxy = socks_proxy();
        // 用带辨识度的假凭据：断言「原文不出现」时不会被字段名（username/password）误伤。
        proxy.username = Some("socks-user-42".to_string());
        proxy.password = Some("top-secret".to_string());
        let rendered = format!("{proxy:?}");

        // L-2：username 与 password 都不许出原文。
        assert!(!rendered.contains("socks-user-42"), "{rendered}");
        assert!(!rendered.contains("top-secret"), "{rendered}");
        // 但仍然能看出「这两个字段有没有设置」——排障需要。
        assert_eq!(rendered.matches("<redacted>").count(), 2, "{rendered}");

        // 没有凭据时不应出现打码占位。
        let plain = http_proxy();
        let rendered_plain = format!("{plain:?}");
        assert!(!rendered_plain.contains("<redacted>"), "{rendered_plain}");
    }

    // ── ④ 后置重放（F1/F2）──────────────────────────────────────────────

    type SimState = HashMap<&'static str, String>;

    fn expect_value(expectations: &[ReplayExpectation], key: &str) -> Option<String> {
        expectations
            .iter()
            .find(|expectation| expectation.key == key)
            .and_then(|expectation| expectation.want.clone())
    }

    fn state_from(expectations: &[ReplayExpectation]) -> SimState {
        expectations
            .iter()
            .filter_map(|expectation| {
                expectation
                    .want
                    .clone()
                    .map(|value| (expectation.key, value))
            })
            .collect()
    }

    fn simulate_loop(
        plan: &InjectionPlan,
        overwrite_at_ms: u64,
        overwrite: &dyn Fn(&mut SimState),
    ) -> (Vec<ReplayTick>, ReplaySummary, SimState) {
        let expectations = replay_expectations(plan);
        let mut state = state_from(&expectations);

        let clock = std::cell::Cell::new(0u64);
        let framework_written = std::cell::Cell::new(false);

        let sleep_until = |offset: u64| clock.set(offset);
        let read = |state: &mut SimState, key: &str| {
            if !framework_written.get() && clock.get() >= overwrite_at_ms {
                framework_written.set(true);
                overwrite(state);
            }
            state.get(key).cloned()
        };
        let write = |state: &mut SimState, expectation: &ReplayExpectation| {
            match &expectation.want {
                Some(value) => {
                    state.insert(expectation.key, value.clone());
                }
                None => {
                    state.remove(expectation.key);
                }
            }
            true
        };

        let (ticks, summary) = run_replay_loop(&mut state, &expectations, read, write, sleep_until);
        (ticks, summary, state)
    }

    /// 设备级代理写进进程的样子（t9 证据：框架把设备级 host/port 写进 http/https 四项）。
    fn write_device_proxy(state: &mut SimState) {
        for (key, value) in [
            ("http.proxyHost", "10.0.0.9"),
            ("http.proxyPort", "18080"),
            ("https.proxyHost", "10.0.0.9"),
            ("https.proxyPort", "18080"),
        ] {
            state.insert(key, value.to_string());
        }
    }

    #[test]
    fn http_plan_replay_expectations_are_the_four_proxy_props_plus_clears() {
        let plan = injection_plan(&http_proxy()).expect("plan");
        let expectations = replay_expectations(&plan);

        assert_eq!(
            expect_value(&expectations, "http.proxyHost").as_deref(),
            Some("203.0.113.10")
        );
        assert_eq!(
            expect_value(&expectations, "http.proxyPort").as_deref(),
            Some("8080")
        );
        assert_eq!(
            expect_value(&expectations, "https.proxyHost").as_deref(),
            Some("203.0.113.10")
        );
        assert_eq!(expect_value(&expectations, "socksProxyHost"), None);
        // v1.2：注入清单只有 6 个代理属性，没有任何凭据载体（§9.1 K1）。
        assert!(
            expectations
                .iter()
                .all(|expectation| expectation.key.starts_with("http.")
                    || expectation.key.starts_with("https.")
                    || expectation.key.starts_with("socksProxy")),
            "{expectations:?}"
        );
    }

    #[test]
    fn socks_plan_expectations_never_carry_credentials() {
        let mut proxy = socks_proxy();
        proxy.username = Some("user".to_string());
        proxy.password = Some("s3cret".to_string());
        let expectations = replay_expectations(&injection_plan(&proxy).expect("plan"));

        assert_eq!(
            expect_value(&expectations, "socksProxyHost").as_deref(),
            Some("198.51.100.7")
        );
        assert_eq!(
            expect_value(&expectations, "socksProxyPort").as_deref(),
            Some("1080")
        );
        // v1.2：socks 的两个凭据属性一律不注入、不重放（§9.1 K1 / §9.2 B13）。
        assert!(
            expectations
                .iter()
                .all(|expectation| !expectation.key.starts_with("java.net.")),
            "{expectations:?}"
        );
        assert!(
            !expectations
                .iter()
                .filter_map(|expectation| expectation.want.as_deref())
                .any(|value| value.contains("s3cret")),
            "凭据值不得进入重放清单"
        );
    }

    #[test]
    fn divergences_detect_framework_overwrite() {
        let expectations = replay_expectations(&injection_plan(&http_proxy()).expect("plan"));
        let mut state = state_from(&expectations);
        write_device_proxy(&mut state);

        let found = divergences_of(&expectations, &mut state, |state: &mut SimState, key| {
            state.get(key).cloned()
        });

        let keys: Vec<&str> = found.iter().map(|divergence| divergence.key).collect();
        assert_eq!(
            keys,
            vec![
                "http.proxyHost",
                "http.proxyPort",
                "https.proxyHost",
                "https.proxyPort"
            ]
        );
        assert!(
            found
                .iter()
                .any(|divergence| divergence.describe() == "http.proxyPort=18080->8080"),
            "{found:?}"
        );
    }

    #[test]
    fn empty_string_counts_as_absent() {
        let plan = injection_plan(&EgressProxy::direct()).expect("plan");
        let expectations = replay_expectations(&plan);
        let mut state: SimState = HashMap::new();
        state.insert("http.proxyHost", String::new());

        let found = divergences_of(&expectations, &mut state, |state: &mut SimState, key| {
            state.get(key).cloned()
        });
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn divergence_describe_shows_got_and_want() {
        // v1.2 起重放清单只有 host/port（无凭据），所以 describe 直接给「实际->期望」。
        let divergence = Divergence {
            key: "socksProxyHost",
            want: Some("127.0.0.1".to_string()),
            got: Some("10.0.0.9".to_string()),
        };
        assert_eq!(divergence.describe(), "socksProxyHost=10.0.0.9->127.0.0.1");
    }

    #[test]
    fn replay_schedule_is_sorted_dense_early_and_covers_window() {
        let offsets = replay_offsets_ms();
        assert_eq!(offsets.first().copied(), Some(0));
        assert_eq!(offsets.last().copied(), Some(REPLAY_WINDOW_MS));
        assert!(
            offsets.windows(2).all(|pair| pair[0] < pair[1]),
            "时间表必须严格升序：{offsets:?}"
        );
        // 热点区（≤60ms）必须每 ≤2ms 一次——框架覆盖发生在 ~95ms，留给我们的只有这几毫秒。
        let hot: Vec<u64> = offsets
            .iter()
            .copied()
            .filter(|offset| *offset <= 60)
            .collect();
        assert!(hot.len() >= 30, "热点区 tick 太少：{hot:?}");
        assert!(hot.windows(2).all(|pair| pair[1] - pair[0] <= 2));
        // 90~150ms（t9 实测的覆盖位）也要 ≤5ms 一次。
        let around: Vec<u64> = offsets
            .iter()
            .copied()
            .filter(|offset| *offset >= 90 && *offset <= 150)
            .collect();
        assert!(around.windows(2).all(|pair| pair[1] - pair[0] <= 5));
    }

    #[test]
    fn replay_loop_repairs_http_overwrite_at_95ms() {
        let plan = injection_plan(&http_proxy()).expect("plan");
        let (ticks, summary, state) = simulate_loop(&plan, 95, &write_device_proxy);

        let diverged = ticks
            .iter()
            .find_map(|tick| match tick {
                ReplayTick::Diverged {
                    at_ms,
                    found,
                    applied,
                } => Some((*at_ms, found.clone(), *applied)),
                ReplayTick::Clean { .. } => None,
            })
            .expect("必须在框架覆盖后的下一个 tick 发现偏差");

        // 覆盖发生在 95ms；时间表是 2ms 一档，所以必须在下一次 tick（≤2ms 内）发现
        assert!(
            (95..=97).contains(&diverged.0),
            "必须在覆盖后的一个 tick 内发现，实际 {}ms",
            diverged.0
        );
        assert_eq!(diverged.1.len(), 4);
        assert_eq!(diverged.2, replay_expectations(&plan).len());

        // 修回之后应用读到的是**应用级**值，不是设备级 10.0.0.9/18080
        assert_eq!(
            state.get("http.proxyHost").map(String::as_str),
            Some("203.0.113.10")
        );
        assert_eq!(
            state.get("http.proxyPort").map(String::as_str),
            Some("8080")
        );
        assert_eq!(
            state.get("https.proxyPort").map(String::as_str),
            Some("8080")
        );
        assert!(!state.contains_key("socksProxyHost"));

        assert_eq!(summary.divergences, 1);
        assert_eq!(summary.replays, 1);
        assert!(summary.state_matches);
        // 不早退：窗口内一直盯着（框架可能晚于第一次覆盖再写一次），所以跑满整张表
        let last_tick = ticks
            .last()
            .map(|tick| match tick {
                ReplayTick::Clean { at_ms } | ReplayTick::Diverged { at_ms, .. } => *at_ms,
            })
            .expect("ticks");
        assert_eq!(last_tick, REPLAY_WINDOW_MS);
    }

    #[test]
    fn replay_loop_restores_direct_after_framework_writes_device_proxy() {
        let plan = injection_plan(&EgressProxy::direct()).expect("plan");
        let (ticks, summary, state) = simulate_loop(&plan, 95, &write_device_proxy);

        assert_eq!(summary.divergences, 1, "{ticks:?}");
        for key in [
            "http.proxyHost",
            "http.proxyPort",
            "https.proxyHost",
            "https.proxyPort",
        ] {
            assert_eq!(state.get(key), None, "{key} 必须被清空（__DIRECT__ 直连）");
        }
        assert!(summary.state_matches);
    }

    #[test]
    fn replay_loop_without_overwrite_stays_clean() {
        let plan = injection_plan(&http_proxy()).expect("plan");
        // 覆盖时刻设到窗口之外：读回一直与期望一致，一次都不该重放。
        let (ticks, summary, state) =
            simulate_loop(&plan, REPLAY_WINDOW_MS + 1, &write_device_proxy);

        assert_eq!(summary.replays, 0);
        assert_eq!(summary.divergences, 0);
        assert!(summary.state_matches);
        assert_eq!(
            state.get("http.proxyPort").map(String::as_str),
            Some("8080")
        );
        assert_eq!(ticks.len(), replay_offsets_ms().len());
    }

    #[test]
    fn replay_loop_keeps_watching_after_the_first_repair() {
        // 第一次覆盖修好之后仍继续盯到窗口结束：若框架在更晚的时刻又写一次，也能被抓住。
        let plan = injection_plan(&http_proxy()).expect("plan");
        let (ticks, summary, state) = simulate_loop(&plan, 95, &write_device_proxy);
        assert_eq!(summary.divergences, 1);
        assert_eq!(
            ticks.last().map(|tick| match tick {
                ReplayTick::Clean { at_ms } | ReplayTick::Diverged { at_ms, .. } => *at_ms,
            }),
            Some(REPLAY_WINDOW_MS)
        );
        assert_eq!(
            state.get("http.proxyPort").map(String::as_str),
            Some("8080")
        );
    }

    // ── ⑤ arming 降级（F-2：两条真实降级分支 + 排障开关）──────────────────

    #[test]
    fn arm_plan_honours_the_debug_guardrail() {
        // debug = false：即使属性开着也照常走 Proceed（普通设备不会因残留属性变行为）
        assert_eq!(
            plan_replay_arm(false, || true, || Some((7, 9))),
            ArmPlan::Proceed {
                device: 7,
                inode: 9
            }
        );
        // debug = true 且属性开启：退化为一次性注入，连镜像都不查
        assert_eq!(
            plan_replay_arm(true, || true, || panic!("镜像不该被查")),
            ArmPlan::DisabledByDebugProp
        );
        // debug = true 且属性关闭：正常查镜像
        assert_eq!(
            plan_replay_arm(true, || false, || Some((1, 2))),
            ArmPlan::Proceed {
                device: 1,
                inode: 2
            }
        );
    }

    #[test]
    fn missing_libandroid_runtime_degrades_with_the_expected_log() {
        let plan = plan_replay_arm(true, || false, || None);
        assert_eq!(
            plan,
            ArmPlan::Unavailable(ReplayArmFailure::LibAndroidRuntimeMissing)
        );
        let outcome = ArmOutcome::Unavailable(ReplayArmFailure::LibAndroidRuntimeMissing);
        assert!(outcome.log_line().is_some(), "降级必须有日志");
        let line = outcome.log_line().expect("log line");
        assert!(
            line.contains("reason=libandroid_runtime_not_found"),
            "{line}"
        );
        assert!(!line.contains("error="), "{line}");
        assert!(
            !outcome.keeps_module_mapped(),
            "没挂上 hook 不该保留模块映射"
        );
    }

    #[test]
    fn commit_failure_degrades_with_the_expected_log() {
        let outcome = finish_replay_arm(Err("boom".to_string()));
        assert_eq!(
            outcome,
            ArmOutcome::Unavailable(ReplayArmFailure::PltHookCommitFailed("boom".to_string()))
        );
        assert!(!outcome.keeps_module_mapped());
        let line = outcome.log_line().expect("log line");
        assert!(line.contains("reason=commit_failed"), "{line}");
        assert!(line.contains("error=boom"), "{line}");
    }

    #[test]
    fn armed_outcome_keeps_module_mapped_and_logs_nothing() {
        let outcome = finish_replay_arm(Ok(()));
        assert_eq!(outcome, ArmOutcome::Armed);
        assert!(outcome.keeps_module_mapped());
        assert_eq!(outcome.log_line(), None);
    }

    #[test]
    fn debug_switch_outcome_is_logged_and_does_not_keep_the_module() {
        let outcome = ArmOutcome::DisabledByDebugProp;
        assert!(!outcome.keeps_module_mapped());
        let line = outcome.log_line().expect("开关退化必须留痕");
        assert!(line.contains("reason=debug_disable_replay"), "{line}");
        assert!(line.contains("one-shot injection only"), "{line}");
    }

    // ── ⑥ armed 但 hook 没命中（M-5）────────────────────────────────────

    #[test]
    fn replay_not_fired_line_has_window_wait_and_retention() {
        let line = replay_not_fired_line(REPLAY_WINDOW_MS, 2043, "com.example.a");
        // ① 窗口；② 实际等待时长；③ 代码页保留的后果说明。
        assert!(line.contains("not fired within 2000ms"), "{line}");
        assert!(line.contains("waited=2043ms"), "{line}");
        assert!(line.contains("module retained"), "{line}");
        assert!(line.contains("code pages stay mapped"), "{line}");
        assert!(line.contains("package=com.example.a"), "{line}");
        // 不能暗示「会卸载模块」。
        assert!(!line.contains("dlclose"), "{line}");
        // 包名缺失时用 - 占位，不留空洞。
        assert!(replay_not_fired_line(REPLAY_WINDOW_MS, 2000, "").contains("package=-"));
    }

    #[test]
    fn never_fired_report_is_silent_on_the_normal_path() {
        // 正常路径：hook 命中过（spawned）→ 不打。
        assert!(!should_report_never_fired(true, true, false));
        // 已经报过 → 不重复打。
        assert!(!should_report_never_fired(true, false, true));
        // 没武装（未配置 egress / arming 失败）→ 不打。
        assert!(!should_report_never_fired(false, false, false));
        // 只有这一种组合要打：武装了、从没 spawn、还没报过。
        assert!(should_report_never_fired(true, false, false));
    }

    // ── ⑦ 中继：`[relay]` + `local_port` → spec 视图（契约 §2.2 / §3.2 / §3.3）────

    /// 带凭据的上游定义；`local_port` 由「server 是否已分配」决定。
    fn credential_proxy(
        kind: EgressType,
        host: &str,
        port: u16,
        local_port: Option<u16>,
    ) -> EgressProxy {
        let mut proxy = EgressProxy::endpoint_proxy(kind, host, port);
        proxy.username = Some("relay-user".to_string());
        proxy.password = Some("relay-secret".to_string());
        proxy.local_port = local_port;
        proxy
    }

    fn relay_file(enabled: bool, entries: &[(&str, EgressProxy)]) -> EgressFile {
        EgressFile {
            relay: RelaySettings { enabled },
            relay_present: true,
            proxies: proxies(entries),
        }
    }

    /// 把一个代理渲染成完整的 spec 文档（只有 [egress_proxy] 一小节）。
    fn spec_section_text(proxy: &EgressProxy) -> String {
        let mut root = toml::map::Map::new();
        insert_spec_section(&mut root, proxy);
        toml::to_string(&toml::Value::Table(root)).expect("render spec")
    }

    /// 老文件（未迁移）：既没有 `[relay]` 键，也没有任何 `local_port`。
    fn unmigrated_file(entries: &[(&str, EgressProxy)]) -> EgressFile {
        EgressFile {
            relay: RelaySettings::default(),
            relay_present: false,
            proxies: proxies(entries),
        }
    }

    #[test]
    fn egress_toml_parses_relay_switch_and_local_port() {
        let file = parse_egress(
            r#"
            version = 1

            [relay]
            enabled = false

            [proxies.hk_resi]
            type = "http"
            host = "203.0.113.10"
            port = 8080
            username = "relay-user"
            password = "relay-secret"
            local_port = 17800

            [proxies.us_la]
            type = "socks5"
            host = "198.51.100.7"
            port = 1080
            local_port = 17850
            "#,
        );

        assert!(!file.relay.enabled);
        assert!(file.relay_present, "有 [relay] 键 = 已迁移");
        assert_eq!(file.proxies["hk_resi"].local_port, Some(17800));
        assert!(file.proxies["hk_resi"].has_credentials());
        assert_eq!(file.proxies["us_la"].local_port, Some(17850));
        assert!(!file.proxies["us_la"].relay, "egress.toml 不认 relay 键");
    }

    #[test]
    fn relay_switch_defaults_to_enabled_and_degrades_when_mistyped() {
        // 整段缺失 == 默认值（契约 §2.2：中继默认开）
        assert!(parse_egress("version = 1").relay.enabled);
        // 段在、键缺 == 默认值
        assert!(
            parse_egress("version = 1\n[relay]\nbase_port = 17800")
                .relay
                .enabled
        );
        // 显式关闭
        assert!(
            !parse_egress("version = 1\n[relay]\nenabled = false")
                .relay
                .enabled
        );
        // 类型非法 → §2.5「视为 false」（宁可不中继，也不用未知状态）
        assert!(
            !parse_egress("version = 1\n[relay]\nenabled = \"true\"")
                .relay
                .enabled
        );
        // 整段写错类型 → 同样降级为 false
        assert!(!parse_egress("version = 1\nrelay = 1").relay.enabled);
    }

    #[test]
    fn local_port_outside_the_relay_pool_is_ignored() {
        let file = parse_egress(
            r#"
            version = 1

            [proxies.a]
            type = "http"
            host = "203.0.113.10"
            port = 8080
            username = "u"
            password = "p"
            local_port = 17799

            [proxies.b]
            type = "http"
            host = "203.0.113.10"
            port = 8080
            username = "u"
            password = "p"
            local_port = 17800

            [proxies.c]
            type = "http"
            host = "203.0.113.10"
            port = 8080
            username = "u"
            password = "p"
            local_port = 17899

            [proxies.d]
            type = "http"
            host = "203.0.113.10"
            port = 8080
            username = "u"
            password = "p"
            local_port = 17900

            [proxies.e]
            type = "http"
            host = "203.0.113.10"
            port = 8080
            username = "u"
            password = "p"
            local_port = "17800"
            "#,
        );

        assert_eq!(file.proxies["a"].local_port, None);
        assert_eq!(file.proxies["b"].local_port, Some(17800));
        assert_eq!(file.proxies["c"].local_port, Some(17899));
        assert_eq!(file.proxies["d"].local_port, None);
        assert_eq!(file.proxies["e"].local_port, None);
    }

    #[test]
    fn spec_proxies_rewrites_only_credentialed_proxies_to_the_loopback() {
        let file = relay_file(
            true,
            &[
                (
                    "hk_resi",
                    credential_proxy(EgressType::Http, "203.0.113.10", 8080, Some(17800)),
                ),
                (
                    "jp_socks",
                    credential_proxy(EgressType::Socks5, "198.51.100.20", 1080, Some(17831)),
                ),
                (
                    "us_la",
                    EgressProxy::endpoint_proxy(EgressType::Socks5, "198.51.100.7", 1080),
                ),
            ],
        );

        let view = spec_proxies(&file);
        assert!(view.warnings.is_empty(), "{:?}", view.warnings);

        // 带凭据 + 有 local_port → 回环 + relay=true，凭据与上游地址都不在视图里
        let http = &view.proxies["hk_resi"];
        assert_eq!(http.host.as_deref(), Some(RELAY_HOST));
        assert_eq!(http.port, Some(17800));
        assert!(http.relay);
        assert_eq!(http.username, None);
        assert_eq!(http.password, None);
        assert_eq!(http.local_port, None, "local_port 不跟着进 spec");

        let socks = &view.proxies["jp_socks"];
        assert_eq!(socks.host.as_deref(), Some(RELAY_HOST));
        assert_eq!(socks.port, Some(17831));
        assert!(socks.relay);

        // 无凭据 → 上游地址，且不带 relay 标记
        let plain = &view.proxies["us_la"];
        assert_eq!(plain.host.as_deref(), Some("198.51.100.7"));
        assert_eq!(plain.port, Some(1080));
        assert!(!plain.relay);
    }

    #[test]
    fn spec_proxies_fails_closed_when_relay_enabled_and_local_port_missing() {
        // R13：中继开着、文件已迁移（本 fixture 有 [relay]），却缺 local_port（池耗尽 /
        // 写回失败 / 手改坏）→ 哨兵 127.0.0.1:9，**不回退上游**。
        let file = relay_file(
            true,
            &[(
                "hk_legacy",
                credential_proxy(EgressType::Http, "203.0.113.10", 8080, None),
            )],
        );

        let view = spec_proxies(&file);
        let proxy = &view.proxies["hk_legacy"];
        assert_eq!(proxy.host.as_deref(), Some(RELAY_HOST));
        assert_eq!(proxy.port, Some(RELAY_FAIL_CLOSED_PORT));
        assert!(proxy.relay);
        assert!(proxy.is_fail_closed());
        assert!(!proxy.has_credentials());
        assert_eq!(proxy.local_port, None);
        assert_ne!(proxy.port, Some(0), "绝不能注入 0 端口");

        assert_eq!(view.warnings.len(), 1, "{:?}", view.warnings);
        let line = &view.warnings[0];
        assert!(line.contains("fail-closed"), "{line}");
        assert!(line.contains("port_missing"), "{line}");
        assert!(line.contains("hk_legacy"), "{line}");
        // 不回退上游：上游地址不出现在这一行里
        assert!(!line.contains("203.0.113.10"), "{line}");
        // 日志边界：凭据原文一个字符都不能出现
        assert!(!line.contains("relay-user"), "{line}");
        assert!(!line.contains("relay-secret"), "{line}");
    }

    #[test]
    fn spec_proxies_falls_back_upstream_when_relay_is_disabled() {
        let file = relay_file(
            false,
            &[(
                "hk_resi",
                credential_proxy(EgressType::Http, "203.0.113.10", 8080, Some(17800)),
            )],
        );

        let view = spec_proxies(&file);
        let proxy = &view.proxies["hk_resi"];
        assert_eq!(proxy.host.as_deref(), Some("203.0.113.10"));
        assert_eq!(proxy.port, Some(8080));
        assert!(!proxy.relay);
        assert!(!proxy.is_fail_closed(), "明确关闭不是故障，不该断网");
        // v1.2（§9.2 B13）：回退窗口也不带凭据载体——spec 里连凭据都不出现。
        assert!(!proxy.has_credentials());
        assert_eq!(proxy.local_port, None);

        assert_eq!(view.warnings.len(), 1, "{:?}", view.warnings);
        let line = &view.warnings[0];
        assert!(line.contains("relay disabled"), "{line}");
        assert!(line.contains("203.0.113.10:8080"), "{line}");
        assert!(!line.contains("relay-user"), "{line}");
        assert!(!line.contains("relay-secret"), "{line}");

        let document = spec_section_text(proxy);
        assert!(!document.contains("username"), "{document}");
        assert!(!document.contains("password"), "{document}");
    }

    #[test]
    fn relay_spec_toml_carries_only_type_host_port_and_flag() {
        let file = relay_file(
            true,
            &[(
                "jp_socks",
                credential_proxy(EgressType::Socks5, "198.51.100.20", 1080, Some(17831)),
            )],
        );
        let view = spec_proxies(&file);
        let table = view.proxies["jp_socks"].to_toml();
        let keys: Vec<&str> = table
            .as_table()
            .expect("table")
            .keys()
            .map(String::as_str)
            .collect();

        // 契约 §3.3 字段表：relay 生效时只有 type/host/port + relay=true
        assert_eq!(keys, vec!["type", "host", "port", "relay"]);
        assert_eq!(
            table.get("relay").and_then(toml::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            table.get("host").and_then(toml::Value::as_str),
            Some(RELAY_HOST)
        );
        assert_eq!(
            table.get("port").and_then(toml::Value::as_integer),
            Some(17831)
        );
    }

    #[test]
    fn relay_spec_round_trips_and_plans_without_credential_carriers() {
        let file = relay_file(
            true,
            &[
                (
                    "hk_resi",
                    credential_proxy(EgressType::Http, "203.0.113.10", 8080, Some(17800)),
                ),
                (
                    "jp_socks",
                    credential_proxy(EgressType::Socks5, "198.51.100.20", 1080, Some(17831)),
                ),
            ],
        );
        let view = spec_proxies(&file);

        for name in ["hk_resi", "jp_socks"] {
            let proxy = view.proxies[name].clone();
            let document = spec_section_text(&proxy);

            // 模块侧读回来必须与原视图一致，且通过 §3.3 的三项校验
            let parsed = from_spec_doc(&document, "com.example.a").expect("relay spec parses");
            assert_eq!(parsed, proxy);
            assert!(parsed.relay_target_is_valid());

            // relay 视图不得规划任何凭据载体（否则凭据会暴露进应用进程）
            let plan = injection_plan(&parsed).expect("relay plan");
            assert!(!plan.unsupported_auth, "{name}");
            assert_eq!(
                replay_expectations(&plan).len(),
                PROXY_PROPERTY_KEYS.len(),
                "{name}"
            );
            assert!(
                replay_expectations(&plan)
                    .iter()
                    .all(|item| !item.key.starts_with("java.net.")),
                "{name}: relay 计划里不该有凭据载体"
            );
        }
    }

    /// 拼一段 relay=true 的 spec 文档（host/port 可选，用来构造各种非法视图）。
    fn relay_spec_document(kind: &str, host: Option<&str>, port: Option<u16>) -> String {
        let mut section = format!("type = \"{kind}\"\nrelay = true\n");
        if let Some(host) = host {
            section.push_str(&format!("host = \"{host}\"\n"));
        }
        if let Some(port) = port {
            section.push_str(&format!("port = {port}\n"));
        }
        format!("debug = false\n\n[egress_proxy]\n{section}")
    }

    #[test]
    fn fail_closed_port_is_outside_the_pool_and_is_a_valid_target() {
        // 契约 §3.3 规则 1：哨兵 9 不在 17800..=17899，但它是**合法**的 relay 目标。
        assert_eq!(RELAY_FAIL_CLOSED_PORT, 9);
        assert!(!is_relay_port(RELAY_FAIL_CLOSED_PORT));

        for kind in [EgressType::Http, EgressType::Socks5] {
            let sentinel = EgressProxy::fail_closed(kind);
            assert_eq!(sentinel.host.as_deref(), Some(RELAY_HOST));
            assert_eq!(sentinel.port, Some(RELAY_FAIL_CLOSED_PORT));
            assert!(sentinel.relay);
            assert!(!sentinel.has_credentials());
            assert!(sentinel.is_fail_closed());
            assert!(sentinel.relay_target_is_valid());
        }
        // direct 不是可注入地址族：哨兵兜底成 http，避免把「断网」写成「直连」。
        assert_eq!(
            EgressProxy::fail_closed(EgressType::Direct).kind,
            EgressType::Http
        );
    }

    #[test]
    fn relay_normalization_classifies_reasons_and_drops_credentials() {
        // 缺 port → port_missing
        let mut proxy = EgressProxy::endpoint_proxy(EgressType::Http, RELAY_HOST, 17800);
        proxy.relay = true;
        proxy.port = None;
        let normalized = proxy.relay_normalized();
        assert_eq!(normalized.failure, Some(RelayTargetFailure::PortMissing));
        assert_eq!(
            normalized.failure.map(RelayTargetFailure::reason),
            Some("port_missing")
        );
        assert!(normalized.proxy.is_fail_closed());

        // host 非回环 / 端口越界 / 端口是上游端口 → invalid_target
        for (host, port) in [
            ("10.0.0.5", 17800u16),
            (RELAY_HOST, 18000),
            (RELAY_HOST, 8080),
        ] {
            let mut proxy = EgressProxy::endpoint_proxy(EgressType::Http, host, port);
            proxy.relay = true;
            let normalized = proxy.relay_normalized();
            assert_eq!(
                normalized.failure,
                Some(RelayTargetFailure::InvalidTarget),
                "{host}:{port}"
            );
            assert_eq!(
                normalized.failure.map(RelayTargetFailure::reason),
                Some("invalid_target")
            );
            assert!(normalized.proxy.is_fail_closed());
        }

        // 仅多带凭据 → 合法目标 + had_credentials（值被丢弃）
        let mut proxy = credential_proxy(EgressType::Http, RELAY_HOST, 17800, None);
        proxy.relay = true;
        let normalized = proxy.relay_normalized();
        assert_eq!(normalized.failure, None);
        assert!(normalized.had_credentials);
        assert!(!normalized.proxy.has_credentials());
        assert_eq!(normalized.proxy.port, Some(17800));
    }

    #[test]
    fn invalid_relay_specs_fail_closed_and_are_armed_for_replay() {
        // R12：非法 relay 视图 → 哨兵 127.0.0.1:9，并**经过 injection_plan**
        // （只有有计划才会 arm replay；返回 None 会让框架 95ms 后把属性重写回设备级）。
        for (kind, host, port) in [
            ("http", Some("10.0.0.5"), Some(17800u16)),
            ("http", Some(RELAY_HOST), Some(18000)),
            ("http", Some(RELAY_HOST), Some(8080)),
            ("http", Some(RELAY_HOST), None),
            ("socks4", Some(RELAY_HOST), Some(17800)),
        ] {
            let document = relay_spec_document(kind, host, port);
            let proxy = from_spec_doc(&document, "com.example.a").unwrap_or_else(|| {
                panic!("非法 relay 视图必须 fail-closed，不能返回未配置: {kind} {host:?} {port:?}")
            });

            assert_eq!(proxy.host.as_deref(), Some(RELAY_HOST));
            assert_eq!(proxy.port, Some(RELAY_FAIL_CLOSED_PORT));
            assert!(proxy.relay);
            assert!(proxy.is_fail_closed());
            assert!(!proxy.has_credentials());
            assert_eq!(proxy.kind, EgressType::Http);

            let plan =
                injection_plan(&proxy).expect("fail-closed 必须有注入计划（否则不会 arm replay）");
            assert_eq!(
                value_of(&plan, "http.proxyHost").as_deref(),
                Some(RELAY_HOST)
            );
            assert_eq!(value_of(&plan, "http.proxyPort").as_deref(), Some("9"));
            assert_eq!(value_of(&plan, "https.proxyPort").as_deref(), Some("9"));
            assert!(cleared(&plan, "socksProxyHost"));
            assert!(cleared(&plan, "socksProxyPort"));
            assert_eq!(
                plan.selector,
                Some(SelectorSpec::Proxy {
                    kind: EgressType::Http,
                    host: RELAY_HOST.to_string(),
                    port: RELAY_FAIL_CLOSED_PORT,
                })
            );

            // 重放期望覆盖 6 个属性：框架改写后 replay 必须能把 9 写回去。
            let expectations = replay_expectations(&plan);
            assert_eq!(expectations.len(), PROXY_PROPERTY_KEYS.len());
            assert_eq!(
                expectations
                    .iter()
                    .find(|expectation| expectation.key == "http.proxyPort")
                    .and_then(|expectation| expectation.want.as_deref()),
                Some("9")
            );
        }
    }

    #[test]
    fn sentinel_relay_spec_stays_fail_closed_for_socks5() {
        let document = "debug = false\n\n[egress_proxy]\ntype = \"socks5\"\nhost = \"127.0.0.1\"\nport = 9\nrelay = true\n";
        let proxy = from_spec_doc(document, "com.example.a").expect("哨兵是合法输入");
        assert!(proxy.is_fail_closed());

        let plan = injection_plan(&proxy).expect("plan");
        assert_eq!(
            value_of(&plan, "socksProxyHost").as_deref(),
            Some(RELAY_HOST)
        );
        assert_eq!(value_of(&plan, "socksProxyPort").as_deref(), Some("9"));
        assert!(cleared(&plan, "http.proxyHost"));
        assert_eq!(
            plan.selector,
            Some(SelectorSpec::Proxy {
                kind: EgressType::Socks5,
                host: RELAY_HOST.to_string(),
                port: RELAY_FAIL_CLOSED_PORT,
            })
        );
    }

    #[test]
    fn relay_spec_with_only_extra_credentials_is_stripped_and_injected() {
        let document = "debug = false\n\n[egress_proxy]\ntype = \"socks5\"\nhost = \"127.0.0.1\"\nport = 17831\nrelay = true\nusername = \"relay-user\"\npassword = \"relay-secret\"\n";
        let proxy = from_spec_doc(document, "com.example.a").expect("合法目标");
        assert!(proxy.relay);
        assert_eq!(proxy.host.as_deref(), Some(RELAY_HOST));
        assert_eq!(proxy.port, Some(17831));
        assert!(!proxy.has_credentials());
        assert!(!proxy.is_fail_closed(), "多带凭据是修复，不是降级");

        let plan = injection_plan(&proxy).expect("plan");
        assert_eq!(
            value_of(&plan, "socksProxyHost").as_deref(),
            Some(RELAY_HOST)
        );
        assert_eq!(value_of(&plan, "socksProxyPort").as_deref(), Some("17831"));
        assert!(cleared(&plan, "http.proxyHost"));
        assert!(!plan.unsupported_auth);
        assert!(
            !plan
                .property_ops
                .iter()
                .any(|op| matches!(op, PropertyOp::Set { value, .. } if value.contains("relay-"))),
            "凭据值不得进入注入属性"
        );
        assert!(
            !replay_expectations(&plan)
                .iter()
                .filter_map(|expectation| expectation.want.as_deref())
                .any(|value| value.contains("relay-")),
            "凭据值不得进入重放清单"
        );
    }

    #[test]
    fn direct_relay_spec_is_active_direct_not_unconfigured() {
        // 契约 §3.3 规则 4：type=direct + relay=true 自相矛盾 → 主动直连。
        let document = "debug = false\n\n[egress_proxy]\ntype = \"direct\"\nrelay = true\n";
        let proxy = from_spec_doc(document, "com.example.a")
            .expect("direct+relay 必须按主动直连处理（返回 None 会继承设备级代理）");
        assert_eq!(proxy, EgressProxy::direct());

        let plan = injection_plan(&proxy).expect("direct plan");
        assert!(
            plan.property_ops
                .iter()
                .all(|op| matches!(op, PropertyOp::Clear { .. })),
            "{:?}",
            plan.property_ops
        );
        for key in PROXY_PROPERTY_KEYS {
            assert!(cleared(&plan, key));
        }
        assert_eq!(
            plan.selector,
            Some(SelectorSpec::Direct),
            "必须是 NO_PROXY selector"
        );
        assert_eq!(
            replay_expectations(&plan).len(),
            PROXY_PROPERTY_KEYS.len(),
            "清属性同样要重放（否则框架会写回设备级代理）"
        );
    }

    #[test]
    fn spec_proxies_falls_back_upstream_only_before_migration() {
        // 未迁移（无 [relay] 且无任何 local_port）→ 回退上游，且不写凭据载体。
        let file = unmigrated_file(&[(
            "hk_resi",
            credential_proxy(EgressType::Http, "203.0.113.10", 8080, None),
        )]);
        assert!(!file.is_migrated());
        let view = spec_proxies(&file);
        let proxy = &view.proxies["hk_resi"];
        assert_eq!(proxy.host.as_deref(), Some("203.0.113.10"));
        assert_eq!(proxy.port, Some(8080));
        assert!(!proxy.relay);
        assert!(!proxy.is_fail_closed(), "未迁移不是故障，不该断网");
        assert!(!proxy.has_credentials());
        assert_eq!(view.warnings.len(), 1, "{:?}", view.warnings);
        assert!(
            view.warnings[0].contains("not migrated"),
            "{:?}",
            view.warnings
        );

        // 只写了 local_port、没有 [relay] → 也算已迁移：走中继。
        let migrated = EgressFile {
            relay: RelaySettings::default(),
            relay_present: false,
            proxies: proxies(&[(
                "hk_resi",
                credential_proxy(EgressType::Http, "203.0.113.10", 8080, Some(17800)),
            )]),
        };
        assert!(migrated.is_migrated());
        let view = spec_proxies(&migrated);
        assert_eq!(view.proxies["hk_resi"].port, Some(17800));
        assert!(view.proxies["hk_resi"].relay);
    }

    #[test]
    fn migration_state_is_relay_key_or_any_local_port() {
        assert!(!parse_egress("version = 1").is_migrated());
        assert!(parse_egress("version = 1\n[relay]\n").is_migrated());
        assert!(parse_egress("version = 1\n[relay]\nenabled = false").is_migrated());
        assert!(
            parse_egress(
                "version = 1\n[proxies.a]\ntype = \"http\"\nhost = \"203.0.113.10\"\nport = 8080\nusername = \"u\"\nlocal_port = 17800\n"
            )
            .is_migrated()
        );
        assert!(
            !parse_egress(
                "version = 1\n[proxies.a]\ntype = \"http\"\nhost = \"203.0.113.10\"\nport = 8080\nusername = \"u\"\n"
            )
            .is_migrated()
        );
    }

    #[test]
    fn spec_toml_never_carries_credentials_on_any_path() {
        // 中继生效
        let relay = spec_proxies(&relay_file(
            true,
            &[(
                "hk",
                credential_proxy(EgressType::Http, "203.0.113.10", 8080, Some(17800)),
            )],
        ));
        assert!(!spec_section_text(&relay.proxies["hk"]).contains("username"));

        // 明确关闭（回退窗口）
        let off = spec_proxies(&relay_file(
            false,
            &[(
                "hk",
                credential_proxy(EgressType::Http, "203.0.113.10", 8080, Some(17800)),
            )],
        ));
        assert!(!spec_section_text(&off.proxies["hk"]).contains("username"));

        // 未迁移（回退窗口）
        let legacy = spec_proxies(&unmigrated_file(&[(
            "hk",
            credential_proxy(EgressType::Http, "203.0.113.10", 8080, None),
        )]));
        let document = spec_section_text(&legacy.proxies["hk"]);
        assert!(!document.contains("username"), "{document}");
        assert!(!document.contains("password"), "{document}");

        // fail-closed 哨兵同样不带
        let failed = spec_proxies(&relay_file(
            true,
            &[(
                "hk",
                credential_proxy(EgressType::Http, "203.0.113.10", 8080, None),
            )],
        ));
        let document = spec_section_text(&failed.proxies["hk"]);
        assert!(!document.contains("username"), "{document}");
        assert!(document.contains("relay = true"), "{document}");
    }

    #[test]
    fn credentialed_fallbacks_are_marked_unsupported_for_both_protocols() {
        // 契约 §9.1 K1：回退窗口里 http 与 socks5 都「带凭据但用不上」，两者都必须
        // unsupported_auth=true（socks 不得静默无认证），且共用同一行日志（只换 type）。
        for kind in [EgressType::Http, EgressType::Socks5] {
            let mut proxy = EgressProxy::endpoint_proxy(kind, "203.0.113.10", 8080);
            proxy.username = Some("relay-user".to_string());
            proxy.password = Some("relay-secret".to_string());

            let plan = injection_plan(&proxy).expect("fallback plan");
            assert!(plan.unsupported_auth, "{kind:?} 必须标记 unsupported-auth");

            let line = unsupported_auth_line("com.example.a", kind);
            assert!(
                line.starts_with(&format!(
                    "[egress] unsupported-auth package=com.example.a type={}",
                    kind.as_str()
                )),
                "{line}"
            );
            assert!(line.contains("credentials ignored"), "{line}");
            assert!(!line.contains("relay-user"), "{line}");
            assert!(!line.contains("relay-secret"), "{line}");
        }
    }

    #[test]
    fn fail_closed_log_line_is_frozen_and_never_carries_credentials() {
        // 契约 §3.3 规则 2 的冻结格式：真机 logcat 检索依赖 reason= 的稳定取值。
        let line = relay_fail_closed_line("com.example.a", RelayTargetFailure::InvalidTarget, true);
        assert_eq!(
            line,
            "[egress] relay fail-closed package=com.example.a reason=invalid_target creds=1"
        );
        let line = relay_fail_closed_line("com.example.b", RelayTargetFailure::PortMissing, false);
        assert_eq!(
            line,
            "[egress] relay fail-closed package=com.example.b reason=port_missing creds=0"
        );

        // 日志行不含凭据：函数签名里根本没有凭据参数，凭据值也无从出现。
        assert!(!line.contains("relay-user"));
        assert!(!line.contains("relay-secret"));

        // 规则 3 的修复日志同样冻结。
        assert_eq!(
            relay_creds_stripped_line("com.example.a"),
            "[egress] relay creds stripped package=com.example.a"
        );
    }
}
