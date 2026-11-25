# 带认证代理（Authenticated Egress）支持计划书

> 状态：草案，待确认后开工
> 日期：2026-11-25
> 关联：docs/plans/proxy-support-plan.md（本轮已交付的 egress 能力）、docs/plans/egress-contract.md（现行契约）
> 前置事实：v1.10.0 已支持设备级 HTTP 出口与应用级 HTTP/SOCKS5 注入，但**带用户名密码的代理不可用**

## 1. 问题

当前实现里，代理凭据三条路都走不通（真机实测，见 docs/design/egress/t15-acceptance.md 的 O1）：

| 通道 | 现状 | 原因 |
| --- | --- | --- |
| 设备级（settings global http_proxy） | 只能写 host:port | Android 的全局代理设置没有凭据字段 |
| 应用级 HTTP（http.proxyHost） | 只注入 host:port，日志记 unsupported-auth | 需要 java.net.Authenticator 或 OkHttp 的 proxyAuthenticator，native 侧装不了 |
| 应用级 SOCKS5（socksProxyHost + java.net.socks.username/password） | 属性注入成功但**认证失败** | libcore 的 SOCKS 客户端不读这两个属性（fixture 实测 methods=0002 user_ok=0 pass_ok=0） |

结果是：**住宅/机房代理用账号密码授权时，这台设备上用不了**，只能靠代理侧 IP 白名单。这对"多设备多出口"是硬伤——IP 白名单意味着出口 IP 必须固定，无法按设备轮换。

## 2. 目标与非目标

### 2.1 目标

1. 设备级与应用级都支持 username/password 代理，覆盖 HTTP(S) CONNECT 与 SOCKS5。
2. 对应用**零改造**：应用只看到 127.0.0.1:<port> 的普通代理，不需要它懂认证。
3. 凭据不出设备：只在设备内中继进程内存里出现，不落新文件、不进日志、不进审计、不进任何 API 响应。
4. 不把设备变成开放代理：非授权 uid 连不上中继端口。
5. 既有语义不回退：__DIRECT__、fail-open、reconcile、0600、卸载清理。

### 2.2 非目标

- 不做 TLS 到上游（https://proxy 这种"代理自身 TLS"形态，留 P3）。
- 不做透明代理 / tun2socks 全流量。
- 不做 UDP/QUIC 经代理。
- 不改 Manager 凭据存储模型（继续 AES-GCM）。
- 不试图让 libcore 自己会认证（不可行，见 §3）。

## 3. 方案比较

### 方案 A（推荐）：设备内本地认证中继

在 device_faker_server（已是 root、常驻、有 tokio）里跑一个**只监听 127.0.0.1 的转发代理**：

    app ──(普通代理，无认证)──> 127.0.0.1:P_local ──(注入凭据)──> 上游代理 ──> 互联网

- 应用侧：http.proxyHost=127.0.0.1、http.proxyPort=P_local（或 socksProxyHost/Port）
- 中继侧：
  - 上游 HTTP：对 CONNECT 与 absolute-URI 请求补 Proxy-Authorization: Basic base64(user:pass)
  - 上游 SOCKS5：本地按 SOCKS5 服务端会话说，上游用 RFC 1929 用户名密码认证
- 设备级：settings global http_proxy 127.0.0.1:P_device，由中继带上默认代理的凭据

优点：一处实现覆盖三种通道；应用零改造；支持 SOCKS5；凭据只在 root 进程内存。
缺点：多一跳（延迟与少量 CPU）；设备级依赖中继存活；多一个 loopback 监听端口（可检出性需评估）。

### 方案 B：Java 层装 Authenticator

需要 java.net.Authenticator 的具体子类，native 造不出来，得随模块发 dex 用 InMemoryDexClassLoader 加载。即便做到，OkHttp 的代理认证走 proxyAuthenticator、**不读** java.net.Authenticator，而主流 App 大量用 OkHttp → 覆盖率不可接受。**否决。**

### 方案 C：tun2socks / TPROXY

能全流量覆盖，但要内嵌 TUN 栈、与 VPN 冲突、UDP/QUIC 另算，工程量大一个数量级。**留给 P3。**

结论：做 A。

## 4. 架构

    ┌──────────────── device_faker_server (root) ────────────────┐
    │  egress 模块（已有：egress.toml / settings / probe）        │
    │  relay 模块（新增）                                        │
    │   ├─ 端口表：proxy name → 127.0.0.1:P_local（持久化）       │
    │   ├─ listener：HTTP 代理（CONNECT + absolute-URI）          │
    │   ├─ listener：SOCKS5 服务端（本地无认证，P1）              │
    │   ├─ 上游客户端：HTTP Basic / SOCKS5 RFC1929                │
    │   └─ uid 防火墙：iptables owner match（默认开）             │
    └────────────────────────────────────────────────────────────┘
              ▲                                    │
              │ GetSpec（含 local_port）            │ CONNECT / 绝对 URI
    ┌─────────┴────────┐                   ┌───────▼──────┐
    │ Zygisk companion  │                   │   目标 App   │
    └───────────────────┘                   └──────────────┘

### 4.1 端口分配（关键决策）

端口必须在**注入时已知**（Zygisk 在 app_specialize 里设置属性，没有 IPC 往返余地），且跨重启稳定（否则应用重启后指向旧端口）。

做法：**由 server 分配并持久化**。

- egress.toml 新增 [relay] 段：enabled = true、base_port = 17800（预留 17800–17899，最多 100 个定义）。
- 每个 [proxies.<name>] 增加只读字段 local_port（server 写，客户端 PUT 不接受）。
- companion 渲染 spec 时读同一文件的 local_port，把 host=127.0.0.1、port=local_port 写进 [egress_proxy]。
- 设备级默认代理用保留端口 base_port（如 17800）；应用级从 17801 起按名字稳定哈希 + 线性探测分配。
- 定义删除时释放端口；端口表随 egress.toml 一起原子落盘；server 启动时按表重建 listener。

### 4.2 何时启用中继

只有当代理**带凭据**时才把应用指向中继；无凭据代理保持直连上游（少一跳、少暴露）。

    生效规则（按优先级）：
    1. [[apps]].egress = "__DIRECT__" → 直连（不变）
    2. 命中定义且 (username 或 password 非空) → host=127.0.0.1, port=local_port
    3. 命中定义但无凭据 → 原样注入上游 host/port（与 v1.10.0 相同）

设备级同理：[egress].default 指向带凭据的定义 → settings 写 127.0.0.1:base_port；否则写上游 host:port。

### 4.3 上游协议处理

| 上游 type | 本地监听 | 转发方式 | 认证注入 |
| --- | --- | --- | --- |
| http | HTTP 代理（CONNECT + absolute-URI） | 建 TCP 到上游，转发请求行/头 | 覆盖式写入 Proxy-Authorization: Basic …（客户端自带的同名头一律丢弃） |
| socks5 | SOCKS5 服务端（P1） | 本地 SOCKS5 握手（接受 method 0x00）→ 上游 SOCKS5 握手（method 0x02 + RFC1929）→ 双向管道 | 上游握手时发凭据 |

细节要求：

- 只处理 HTTP/1.1；请求头上限 32 KiB，超过回 400。
- CONNECT 目标端口不限；不改写 Host。
- 不做 TLS 终止，CONNECT 之后是纯字节管道。
- 空闲超时 120s，握手超时 10s，并发上限默认 256（可配）。
- 上游不可达：回 502 + 一行日志（relay upstream connect failed，不含凭据）。

## 5. 安全设计

### 5.1 防止设备内被当成开放代理

loopback 端口默认对所有 uid 可达，任何 App 都能拿它当免费代理用我们的凭据出网。三层缓解：

1. **uid 防火墙（默认开）**：iptables owner match 只放行授权 uid
   - 应用级端口：只放行该代理绑定到的包 uid（多用户按 userId 逐个解析）+ root
   - 设备级端口：放行全部 uid（语义上它就是"整机代理"），但仍拒绝非回环来源
   - 规则：-A OUTPUT -o lo -p tcp --dport P -m owner --uid-owner U -j ACCEPT，之后 -j REJECT；链名 DF_EGRESS_RLY
   - 解析失败（包未安装、uid 取不到）→ 该端口**不监听**，并给 warning
2. **拒绝非 loopback 源**：listener 只 bind 127.0.0.1；出站连接源地址也固定回环。
3. **连接计数与速率**：单 uid 并发上限（默认 64），超限直接断开并计数。

### 5.2 凭据保护

- 只在 relay 进程内存；重新加载时对旧值 zeroize。
- 不进 egress.toml 以外的任何文件；egress.toml 仍 0600。
- 不写日志：所有日志只记代理名与错误类别，不记用户名/密码/Proxy-Authorization 值。
- 不返响应：GET /egress 只回 hasCredentials（不变）。
- 内存转储风险：root 才能读 /proc/<pid>/mem，与现状同级别。

### 5.3 可检出性

- 中继端口出现在 127.0.0.1 的监听列表。Android 9+ 起 /proc/net/tcp 对普通 App 只暴露自身 socket，跨 App 枚举受限；但同 uid 的检测代码仍可能看到。**结论：可检出性上升，写入 CONFIG 已知限制**（与 armed 进程常驻代码页同级）。
- 应用侧看到的代理地址从"真实上游"变成 127.0.0.1，这本身是更强的信号。缓解：仅对**带凭据**的代理启用中继。

## 6. 数据模型与 API 变更

### 6.1 egress.toml（0600）

    [relay]
    enabled = true
    base_port = 17800

    [proxies.hk_resi]
    type = "http"
    host = "203.0.113.10"
    port = 8080
    username = "user"
    password = "pass"
    local_port = 17801        # server 分配，客户端只读

    [proxies.jp_socks]
    type = "socks5"
    host = "198.51.100.20"
    port = 1080
    username = "u2"
    password = "p2"
    local_port = 17802

### 6.2 设备端 API

| 变更 | 说明 |
| --- | --- |
| GET /egress 响应新增 relay 对象 | {enabled, basePort, listeners:[{proxy, port, listening, activeConnections, lastError}], firewall:{applied, rules, lastError}} |
| GET /egress/proxies 每项新增 | localPort、relayActive |
| 新增 POST /egress/relay/restart（ops） | 重建 listener 与防火墙规则，返回新状态；幂等 |
| 新增 warning 码 | egress_relay_bind_failed、egress_relay_firewall_failed、egress_relay_uid_unknown、egress_relay_upstream_unreachable |
| 错误码不变 | 上游失败仍是 egress.probe_failed（探测）/ 502（数据面），不新增对公错误码 |

PUT /egress 语义扩展：proxies[].local_port 只读，客户端传了 → 422 unknown_field（与"凭据键不可出现在 config.toml"同款严格度）。

### 6.3 Manager

- 不改表结构（凭据已在 egress_proxies.password_cipher）。
- DeviceSummary.egress 增 relay:{enabled, active} 供 UI 显示"经设备中继"。
- Manager 侧探测：P2 起给 reqwest 开 socks feature，使带认证 SOCKS5 的 POST /egress-proxies/{id}/probe 也能真连（当前只支持 HTTP）。

## 7. 测试与验收

### 7.1 单测（relay 纯逻辑）

- HTTP 请求头解析：absolute-URI / CONNECT / 畸形请求（超大头、缺 Host、非 HTTP/1.1）
- Proxy-Authorization 注入：客户端自带同名头被覆盖而非叠加；base64 正确
- SOCKS5 服务端握手：method 选择、CONNECT 请求解析（IPv4/域名/IPv6 ATYP）
- SOCKS5 上游握手：RFC1929 用户名密码、认证失败（0x01）映射
- 端口分配：稳定性、冲突探测、删除后释放
- uid 防火墙规则生成：多用户、包未安装、规则已存在时幂等

### 7.2 集成（server/tests）

- 起一个**要求认证**的 mock 上游代理（扩展 docs/design/egress/fixtures/local_proxy.py），验证：
  - 应用级 http 带凭据：relay 注入后上游 200；不带 Proxy-Authorization 直连上游 → 407
  - 设备级带凭据：settings 指向 relay 端口，经 relay 出网成功
  - 上游 407（凭据错）→ 应用收到 407，日志不含凭据
  - 上游不可达 → 502 + egress_relay_upstream_unreachable
  - 非授权 uid 连接应用级端口 → 拒绝（iptables 生效）
- fail-open：relay 绑定失败时设备级 settings 是否回退（按 §8 决策）

### 7.3 真机验收（Pixel 9，沿用 t18 装置）

- 带认证 HTTP：应用进程内 http.proxyHost=127.0.0.1，真实流量经 relay 到上游（fixture 记录 Proxy-Authorization 摘要，不记明文）
- 带认证 SOCKS5：同上，走远程 DNS
- __DIRECT__ 在"设备级带认证代理"下仍能直连
- 无凭据代理仍直连上游（不经过 relay）
- 卸载/禁用：relay 端口关闭、iptables 链清空
- 安全扫描：server.log / audit / 全部 API 响应 / relay 日志中凭据命中 0

## 8. 需要拍板的决策

| # | 决策点 | 选项 | 建议 |
| --- | --- | --- | --- |
| 1 | 设备级带认证代理是否也走中继 | a) 走（能认证，但中继挂了整机断网） b) 不走（设备级只能无认证） | **a**，并把 relay 绑定失败视为"不启用设备级代理"而不是半死不活 |
| 2 | uid 防火墙默认 | a) 默认开 b) 默认关可配 | **a**，没有它等于把凭据借给全设备 |
| 3 | SOCKS5 本地服务端（P1） | a) 做 b) 不做（socks5 带凭据仍报 unsupported） | **a** |
| 4 | 上游 TLS（https://proxy） | a) 本轮不做 b) 一并做 | **a**，P3 |
| 5 | 并发/内存预算 | 默认 256 连接、64/uid | 按此，可配 |
| 6 | 「可检出性上升」写进 CONFIG 已知限制 | 是/否 | **是** |

## 9. 分期与工作量

| 阶段 | 内容 | 预估 |
| --- | --- | --- |
| P0 | relay 核心（HTTP 上游 Basic）+ 端口分配与持久化 + 应用级 http 带凭据 + 设备级带凭据 + uid 防火墙 + API/warnings + 单测/集成 | server 约 1200–1600 行 + 测试 |
| P1 | 本地 SOCKS5 服务端 + 上游 RFC1929 + 远程 DNS + 真机验收 | 约 500–700 行 |
| P2 | Manager 侧：relay 状态展示、reqwest socks feature（socks5 探测真连）、文档与 CHANGELOG | 约 200 行 + 文档 |
| P3（可选） | 上游 TLS、tun2socks 全流量 | 另行评估 |

依赖顺序：P0 → P1 → P2；P0 内「契约冻结 → relay 核心 → 端口/防火墙 → 设备端 API → 真机验收」。

## 10. 风险

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| relay 挂了导致设备级断网 | 高 | service.sh 已有守护；relay 绑定失败时不写 settings；自愈时优先校验 relay 存活 |
| 端口冲突 | 中 | 预留 17800–17899，启动时探测，冲突则换端口并写回 egress.toml（提示应用重启） |
| uid 解析错误（多用户/重装） | 中 | 按 pkg@uid 逐个解析；失败则端口不监听 + warning；apply 后重建规则 |
| 性能（多一跳） | 中 | 纯字节管道 copy_bidirectional；只对带凭据代理启用 |
| 可检出性上升 | 中 | 仅带凭据才启用；文档声明；P3 再评估 |
| 凭据误入日志 | 高 | 单测断言日志不含凭据；code review 重点项 |

## 11. 许可

本增量沿用 GPL-3.0-or-later。新增 Rust / TypeScript / 文档文件保留：

    SPDX-License-Identifier: GPL-3.0-or-later
    Copyright (c) 2025-2026 Seyud
    Device Faker: https://github.com/Seyud/device_faker
