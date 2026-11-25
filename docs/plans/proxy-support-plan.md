# 出口代理（Egress Proxy）支持需求梳理

> 状态：需求草案，待确认后开工
> 日期：2026-11-24
> 关联目录：core/src/、server/src/、manager/src/、webui/src/、manager/webui/src/
> 前置能力：设备端 /api/v1 v1.9.0（24 端点）、Manager /relay/api/v1 v0.1.0（18 端点）

## 1. 背景

DevMask 目前只改「机型指纹」，不改「网络出口」。而在真实风控里，机型与 IP 的一致性是最常见的关联维度：

- 同一局域网出口下挂着 N 台不同机型的设备，设备互相可被关联；
- 机型伪装成海外版，出口 IP 却是国内 IDC，属于一眼假；
- 同一批账号共用一条出口 IP，平台侧按 IP 聚合即可成团。

因此需要一套可控、可验证、可回滚、可批量下发的出口代理能力：既能给整台设备指定出口，也能给单个应用单独指定出口（同机多应用需要不同 IP 时）。

## 2. 名词消歧（先钉死边界）

「代理」在本项目里至少三种含义，本次只做 A：

| 编号 | 含义 | 现状 | 本次 |
| --- | --- | --- | --- |
| A | **设备出口代理**：设备上应用流量经代理出网 | 无任何网络干预 | ✅ 主目标 |
| B | **管理端出站代理**：Manager 访问设备时走代理（设备在代理后面） | reqwest 直连 | ⏳ P2 可选 |
| C | **访问入口反代**：WebUI 前置 nginx/TLS | 文档已给手工配置 | ❌ 不做代码 |

另一个必然撞名的点：Manager 已有 ANY /relay/api/v1/devices/{id}/proxy/api/v1/{*path}，那是「**API 透传**」，不是网络代理。本需求统一使用 **egress** 命名，新端点不复用 proxy 词，避免 AI 与用户理解错位。

## 3. 用户故事

- **U1 单应用**：给 com.example.app 配一条美国 SOCKS5，重启后它出口 IP 变成美国，其余应用不变。
- **U2 整机**：给一台设备所有应用配同一个代理（适合整机出海场景）。
- **U3 多设备**：Manager 里维护代理池（若干条），一台设备绑一条，批量下发并回读结果。
- **U4 校验**：下发后自动探测出口 IP，记录 IP / 地区 / 延迟，展示在设备卡片上；地区与机型不匹配时给出告警。
- **U5 回滚**：代理不可用时一键直连，应用网络立刻恢复。
- **U6 脱敏**：代理密码在任何 API 响应、WebUI、日志、审计里都不回显。

## 4. 现状盘点（代码事实）

- 模块侧只做 JVM 字段覆写（src/hooks.rs 改 android.os.Build）与属性区 COW 伪造（src/cow_props/），**没有任何网络层干预**。
- server/src/commands.rs 的命令白名单是 getprop / pm / am / dumpsys / monkey，**没有 settings / iptables**。
- config.toml 权限 **0644**（目录 0700 root），且会被 GET /api/v1/config、备份、WebUI 导出带走，**明文密码不能放这里**。
- Manager 的 proxy 路由是 API 透传语义（见 §2）。

## 5. 真机可行性（已在 Pixel 9 实测）

设备：4A190DLAQ002QR，Pixel 9，Android 17 / SDK 37，KernelSU。

| 验证项 | 结果 |
| --- | --- |
| root | su -c id → uid=0，域 u:r:ksu:s0（与 devmask_server 同域同 uid） |
| settings put global http_proxy host:port | ✅ 立即生效，dumpsys connectivity 显示 Global proxy: [host] port |
| 清除 | ⚠️ settings delete global http_proxy **不清除**：global_http_proxy_host/port 仍在，代理继续生效。必须 settings put global http_proxy :0（已实测恢复 Global proxy: none） |
| 相关键 | http_proxy、global_http_proxy_host、global_http_proxy_port、global_http_proxy_exclusion_list、global_proxy_pac_url |
| iptables -m owner --uid-owner N -j REDIRECT | ✅ 加/列/删均正常 |
| nft | ❌ 不存在 |
| curl / wget | ❌ 都没有；只有 nc、ping |

结论：系统级 HTTP 代理这条链路在本机可走通，服务端（同域、uid 0）具备执行 settings 的权限。**探测出口 IP 必须用 Rust 自己发请求**（设备上没有 curl/wget）。

## 6. 方案空间与取舍

### 方案 A：系统级 HTTP 代理（写 settings global）

- 覆盖：尊重系统代理的应用（OkHttp / HttpURLConnection / WebView / 多数 SDK 的默认链路）
- 协议：**仅 HTTP/HTTPS CONNECT，不支持 SOCKS5**
- 粒度：整机
- 生效：新连接；已建立的连接要应用重连
- 代价：零新增依赖，改动最小
- 风险：全机生效，代理挂了全机断网（需 fail-open 开关）；http.proxyHost 对应用可见（属可检测项，但伪装场景本就要它生效）

### 方案 B：应用级进程内注入（模块内，Zygisk）

在 app_specialize 阶段为目标进程设置 http.proxyHost/Port、https.proxyHost/Port、socksProxyHost/Port，必要时替换 ProxySelector.setDefault(...)。

- 覆盖：Java 层 HTTP（OkHttp / HttpURLConnection）；**native 层（cronet、自研 C++ 栈、QUIC）不覆盖**
- 协议：HTTP + **SOCKS5**（Proxy.Type.SOCKS）
- 粒度：**每应用独立**，且不改系统设置（更隐蔽）
- 生效：进程重启（与现有配置语义一致）
- 待 PoC：Android 17 上 ProxySelector.getDefault() 是否读 http.proxyHost；OkHttp 默认 selector 的取值时机

### 方案 C：iptables owner match + 本地透明代理（tun2socks/redsocks）

- 覆盖：全流量（含 native、UDP；QUIC 需 DROP 降级）
- 代价：需内嵌守护进程 + TUN/TPROXY，与 VPN 冲突，工程量大
- 结论：P2 再评估，不进首个增量

### 方案 D：VpnService

- 需独立 APK + 用户授权 VPN，与本模块「纯 Zygisk native」形态冲突 → 不做

### 推荐：A 打底 + B 做精

- **A**：整机 HTTP 出口，零依赖，立即可用；
- **B**：按应用精细指定，支持 SOCKS5，不动系统设置；
- 两者共用同一份代理定义；**应用级优先于设备级**，显式 "__DIRECT__" 表示该应用强制直连。

## 7. 功能需求

### FR-1 设备级代理（A）

- 写入/清除 settings global http_proxy（含 global_http_proxy_exclusion_list 可选）；
- 清除必须走 :0（delete 无效，已验证）；
- 幂等：值相同不重复写；
- 返回 mode/applied/upstream 与 dumpsys 读回的真实值（防「写了没生效」）。

### FR-2 应用级代理（B）

- apps[].egress = "<代理名>" / "__DIRECT__"；
- 进程内设置系统属性 + ProxySelector，只在目标应用生效；
- 与 A 冲突时 B 优先。

### FR-3 出口 IP 探测

- POST /api/v1/egress/probe：用当前生效链路请求可配置的 probe URL（默认明文 HTTP 服务，避免引入 TLS 栈），返回 exitIp / latencyMs / checkedAt；
- 超时 5s，失败返回结构化错误码（egress.probe_failed）；
- Manager 侧另有独立探测（验证代理本身可用，与设备链路无关）。

### FR-4 回滚

- DELETE /api/v1/egress 恢复直连；
- fail-open 开关：代理不可用时是否放行直连（默认 **fail-open**，避免一次配错让设备变砖）；
- 卸载（uninstall.sh）与模块禁用时清空系统代理设置。

### FR-5 代理池与绑定（Manager）

- 代理池 CRUD（名称、类型、host、port、账号、密码、地区、标签、启用位）；
- 绑定：device（整机）或 device + app（应用级）；
- 批量下发复用现有 apply pipeline（revision / If-Match / 412 重试 / job 结果），新增 job kind egress；
- 绑定关系与探测结果入库，供 Dashboard 展示。

### FR-6 凭据保护

- 设备端：代理定义写独立 egress.toml，权限 **0600**；config.toml 只存代理名引用（保持 0644 可分享/可导出）；
- API：只回 {name, type, host, port, hasCredentials}，**永不回密码**；
- Manager：密码用现有 DeviceKey（AES-256-GCM，AAD 绑 proxy id）加密落库；
- 日志/审计：任何路径都不出现密码明文。

### FR-7 UI

- 设备端 SettingsPage 新增「网络出口」卡片：开关、类型、host/port/账号/密码、应用级覆盖、探测按钮 + 出口 IP 展示；
- Manager 新增「代理池」页：列表、健康状态、绑定数、批量绑定；
- DevicesPage 设备卡片显示出口 IP + 代理名，支持筛选「未配代理」。

### FR-8 审计

- 记录 actor / 时间 / 设备 / 应用 / 代理名（不含密码）/ 结果码。

### FR-9 重启恢复

- A 依赖 settings（持久化，重启仍在）；服务启动时按配置自愈一次（值不对就重写）；
- B 由配置驱动，应用重启即恢复。

## 8. 数据模型

### 设备端

config.toml（0644，可导出）：

```toml
[egress]
default = "us_la_01"        # 设备级默认（对应方案 A）

[[apps]]
package = "com.example.a"
egress  = "jp_tokyo_02"     # 应用级覆盖（方案 B）

[[apps]]
package = "com.example.b"
egress  = "__DIRECT__"      # 强制直连，不受设备级影响
```

egress.toml（**0600**，不进 API 明文、不进导出）：

```toml
[proxies.us_la_01]
type     = "http"           # http | socks5
host     = "203.0.113.10"
port     = 1080
username = "user"
password = "pass"

[probe]
url             = "http://203.0.113.9/ip"
timeout_seconds = 5
fail_open       = true
```

### Manager（SQLite，schema v2）

| 表 | 关键列 |
| --- | --- |
| egress_proxies | id, name(uniq), kind, host, port, username_cipher, password_cipher, region, tags, enabled, created_at, updated_at |
| device_egress | device_id, app('' = 整机), proxy_id, updated_at, PK(device_id, app) |
| egress_checks | id, device_id, app, proxy_id, exit_ip, latency_ms, ok, error_code, checked_at |

## 9. API 草案

### 设备端 /api/v1

| 方法 | 路径 | scope | 说明 |
| --- | --- | --- | --- |
| GET | /egress | read | 当前模式、生效值、上游（脱敏）、各应用覆盖、最近探测 |
| PUT | /egress | write | 写 egress.toml + 应用 A；支持 If-Match/412 |
| DELETE | /egress | write | 恢复直连并清空系统设置 |
| POST | /egress/probe | ops | 经当前链路探测出口 IP |
| GET | /egress/proxies | read | 代理定义列表（脱敏） |

### Manager /relay/api/v1

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET/POST | /egress-proxies | 列表 / 新建 |
| GET/PATCH/DELETE | /egress-proxies/{id} | 详情 / 更新 / 删除（被绑定则 409） |
| POST | /egress-proxies/{id}/probe | Manager 侧直连探测（验证代理本身） |
| GET/PUT | /devices/{id}/egress | 查询 / 绑定设备与应用 |
| POST | /devices/egress/apply | 批量下发，返回 job |

## 10. 兼容性与已知限制

- 方案 A 只覆盖 HTTP/HTTPS；**SOCKS5 需方案 B**。
- UDP/QUIC 不受 HTTP 代理约束（QUIC 会绕过代理直连），需要时可 iptables DROP udp/443（P2）。
- DNS：HTTP CONNECT 由代理解析域名（不泄漏）；但应用「先本地解析再连 IP」的场景会绕过。
- 已建立的连接不会切链路，需应用重连。
- VPN 类应用会覆盖 settings 代理。
- WebView 走系统代理（A 有效，B 无效）；native 栈反之。
- 方案 A 的 http.proxyHost 对应用可见，属可检测项。

## 11. 分期与验收

### P0（建议先做）

设备级 HTTP 代理（方案 A）+ 状态读回 + 探测 + 清除 + 设备端「网络出口」卡片 + 卸载清理。

验收：

1. PUT /egress 后 dumpsys connectivity 显示 Global proxy，GET /egress 的 applied=true；
2. POST /egress/probe 返回的出口 IP 与代理出口一致；
3. DELETE /egress 后 Global proxy: none，出口 IP 恢复；
4. settings delete 的坑不再复现（走 :0）；
5. 幂等：重复 PUT 不重复写；If-Match 不符返回 412；
6. 响应/日志/审计全局断言无密码明文。

### P1

应用级注入（方案 B，含 SOCKS5）+ Manager 代理池 / 绑定 / 批量下发 / Dashboard 展示 + 探测结果入库。

### P2（按需）

iptables + tun2socks 全流量（含 UDP/QUIC）；Manager 出站代理（B 类）。

## 12. 待确认（附建议默认值）

| # | 决策点 | 建议 |
| --- | --- | --- |
| 1 | 粒度 | P0 设备级，P1 应用级（都做） |
| 2 | 协议 | HTTP(S) + SOCKS5；SOCKS5 由应用级承载 |
| 3 | 认证 | 支持用户名/密码（Basic） |
| 4 | 凭据存放 | 独立 egress.toml 0600，config.toml 只存引用 |
| 5 | 代理挂掉 | 默认 **fail-open**（放行直连），可配 fail-closed |
| 6 | 出口 IP 探测 | 做，并把结果展示到 Manager |
| 7 | Manager 出站代理 | 本期不做 |

## 13. 许可

本增量沿用 GPL-3.0-or-later。新增 Rust / TypeScript / 文档文件保留：

```text
SPDX-License-Identifier: GPL-3.0-or-later
Copyright (c) 2025-2026 Seyud
DevMask: https://github.com/uamind/DevMask
```
