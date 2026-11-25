# t9 真机验收：带认证 HTTP/SOCKS5 中继（Pixel 9）

- 结论：**FAIL（2 个阻塞缺陷 + 1 个中等问题）**——默认配置下**应用级中继完全不可用**；设备级中继可用。
- 环境：Pixel 9（tokay）4A190DLAQ002QR，Android 17 / SDK 37，KernelSU root；模块装载本次构建（server 4.2 MiB / zygisk arm64-v8a.so）；契约 v1.2（§5.7 / §10.2–§10.4 / #19–#23）。
- 被测代码：HEAD 3ddd031 + 未提交改动（t2/t3/t4/t5/t15 合并态），`cargo run -p xtask -- build -r` 产物 `output/devmask-(release).zip`。
- 证据目录：`docs/design/relay/evidence-t9/`。

## 0. 结论总表

| # | 验收点 | 结论 | 证据 |
| --- | --- | --- | --- |
| 1 | 应用级 http 带凭据 | **FAIL** | F1：应用级 listener 不监听；F2：firewall=false 下连接被 uid_unresolved 关闭 |
| 2 | 应用级 socks5 带凭据 + 远程 DNS | **FAIL（同一缺陷阻塞）** | 端口不监听 / 被 F2 关闭，探针收不到 05 00 |
| 3 | 设备级带凭据 | **PASS** | settings 127.0.0.1:17800；fixture 落 auth=ok digest=7213dc66f53c |
| 4 | uid 防火墙（拒绝 + 允许） | **FAIL** | 拒绝无 05 00 但 reason=uid_unresolved（非 uid_not_allowed）；允许用例被 F2 关闭 |
| 5 | __DIRECT__ / 无凭据代理 | **FAIL（同上阻塞）** | 模块注入属性可读，但应用级数据面不可用，无法证明「仍直连」 |
| 6 | 卸载/禁用 | **PASS** | 端口关闭、三链删除、settings=:0 |
| 7 | 凭据零泄漏 | **PASS** | 5/5 路扫描命中 0 |
| 8 | 绑定面五项 | **PASS** | ss -ltn 仅 127.0.0.1；ss -ltn6 空；::1 / 127.0.0.2 / LAN 全 refused |
| 9 | 中继数据面（设备级） | **PASS** | root 经 17800 取回 198.51.100.8；上游收到正确 Basic 摘要 |
| 10 | fail-closed 注入（模块侧） | **PASS** | 属性 port=9 blocked=true；日志 relay fail-closed reason=port_missing |
| 11 | 竞态 / 幂等 / 强制 apply | **未验证（被 F1 阻塞）** | 防火墙 applied 恒 false，ALLOW 链无法进入一致态 |
| 12 | 设备恢复 | **PASS** | 重装模块 + reboot；settings=:0、无链、API 200 |

## 1. 阻塞缺陷 F1：应用级端口防火墙 apply 失败（重复 uid 0 规则）

默认配置（`[relay].enabled=true`、`firewall=true`）下：

- `GET /egress.relay.listeners`：dev_http(17800) listening=true；**app_http(17835) listening=false**，
  `lastError={code:"egress_relay_firewall_failed", reason:"command_failed"}`；
  `relay.firewall={applied:false, rules:10, allowRules:5, denyRules:2, lastError:{reason:"state_mismatch"}}`。
- `iptables -S DM_EGRESS_RLY_ALLOW`（evidence-t9/firewall-apply-failure.log）里 **17835 的 uid 0 规则出现两次**：

      -N DM_EGRESS_RLY_ALLOW
      -A … --dport 17800 -m owner --uid-owner 0 -j ACCEPT
      -A … --dport 17835 -m owner --uid-owner 0 -j ACCEPT
      -A … --dport 17800 -j ACCEPT
      -A … --dport 17835 -m owner --uid-owner 0 -j ACCEPT     <- 重复
      -A … --dport 17835 -m owner --uid-owner 10325 -j ACCEPT

- 复现：`POST /egress/relay/restart` 两次结果相同；`ss -ltn` 只有 17800；协议级探测 17835 连接被拒。
- 推断：§5.2 B1（每端口一条 uid 0 放行）与 B3（应用端口 allowedUids 含 0）重复生成同一条规则，
  `-S` 校验按期望列表比对 → `state_mismatch` → 按 fail-closed 不 accept。
  修复点应在规则生成去重或期望列表口径统一（server/src/relay/firewall.rs + uid.rs）。
- 影响：**默认配置下应用级中继（带凭据的应用代理）完全不可用**。

## 2. 阻塞缺陷 F2：accept 期 uid 双检无法解析对端 uid（uid_unresolved）

用 `[relay] firewall = false` 绕开 F1 后（排障开关，不是验收放行手段）：

- 两个 listener 均 listening（17800 / 17835），防火墙 applied=false rules=0、链已删。
- 探针属性正确：`http.proxyHost=127.0.0.1`、`http.proxyPort=17835`、`selector.app.fixture=HTTP @ 127.0.0.1/<unresolved>:17835`，
  但 `fetch.app=error=SocketException`、`fetch.ip=error=SocketException`，fixture 侧**没有 listener=18082 请求**。
- server.log：

      WARN relay client rejected proxy=app_http port=17835 uid="unknown" reason="uid_unresolved"

- `GET /egress` 显示 app_http `allowedUids=[0,10325]`（规则期解析正确），失败发生在**accept 后读取对端 uid**；
  同一时刻设备级 17800 正常（uidPolicy=all，不做该检查）。
- 影响：即使关掉防火墙，应用级端口仍拒绝所有连接（含被授权的应用 uid）。

## 3. 中等问题 F3：relay 生效时仍返回 egress_auth_unsupported

PUT 响应 warnings 对带凭据的 app_http/dev_http 仍给 `egress_auth_unsupported`
（"设备端注入不支持认证（libcore 不读取 java.net.socks.*）"）。中继正是为绕开该限制而存在，
relay 生效时该 warning 会误导 UI（WebUI 显示「凭据未生效」）。

## 4. 通过项的证据

### 4.1 设备级中继（item 3）

    settings get global http_proxy            -> 127.0.0.1:17800
    dumpsys connectivity | grep Global proxy  -> Global proxy: [127.0.0.1] 17800 xl=
    # root 经 17800（协议级，设备无 curl）
    printf "GET http://app.fixture/ ..." | toybox nc -q 3 -w 6 127.0.0.1 17800
    -> HTTP/1.1 200 OK / body 198.51.100.8
    # fixture（认证必需）
    listener=18080 GET http://app.fixture/ host=app.fixture auth=ok digest=7213dc66f53c
    listener=18080 CONNECT play.googleapis.com:443 auth=ok digest=7213dc66f53c

摘要与预期一致（sha256("t9user:t9pass-ro3X")[:12] = 7213dc66f53c），fixture 侧从未出现明文。

### 4.2 绑定面（§5.7 第 1 组）

    ss -ltn  | grep 178xx  -> LISTEN 127.0.0.1:17800 / 127.0.0.1:17835
    ss -ltn6 | grep 178xx  -> (empty)
    nc ::1 17835           -> Connection refused
    nc 127.0.0.2 17835     -> Connection refused
    nc 192.168.30.2 17835  -> Connection refused      （LAN_IP 取 ip addr show wlan0）

### 4.3 凭据零泄漏（item 7）

evidence-t9/credential-scan.log：

    PASS device-server.log count=0
    PASS device-audit.jsonl count=0
    PASS api-egress count=0
    PASS api-proxies count=0
    PASS fixture-log count=0
    RESULT: 凭据零泄漏 PASS（5/5 命中 0）

隧道内凭据扫描：fixture 对 CONNECT/SOCKS 隧道记录 `auth_header=0 cred_hit=0`（fixture-selftest.log）。

### 4.4 卸载清理（item 6）

evidence-t9/uninstall-cleanup.log：

    ss -ltn | grep 178xx                     -> (none)
    iptables -S DM_EGRESS_RLY                -> No chain/target/match by that name
    settings get global http_proxy           -> :0
    ps -A | grep devmask_server         -> (none)

### 4.5 fail-closed 注入（#19）

停服务（避免 reconcile 回写）后手工删掉 app_http 的 `local_port`，启动探针：

    EGRESSPROBE: http.proxyPort=9
    EGRESSPROBE: selector.app.fixture=HTTP @ 127.0.0.1/<unresolved>:9
    EGRESSPROBE: fetch.app=error=UnknownHostException（流量不通）
    [WARN] zygisk::spec - [egress] relay fail-closed for proxy 'app_http' reason=port_missing; injecting 127.0.0.1:9
    [INFO] zygisk::hooks - [egress] injecting relay package=io.df.egressprobe type=http port=9 blocked=true
    ss -ltn | grep -E ":9\b"  -> (none)

### 4.6 模块注入（正常路径）

    [INFO] zygisk::hooks - [egress] injecting relay package=io.df.egressprobe type=http port=17835
    EGRESSPROBE: http.proxyHost=127.0.0.1 / http.proxyPort=17835

属性注入与 relay=true 生效；不可用发生在服务端（F1/F2）。

## 5. 未执行的项及原因

- **item 2（应用级 SOCKS5 + 远程 DNS）**：应用级端口不可用（F1/F2）。上游 SOCKS5 fixture（RFC1929 必需、只记摘要、隧道扫描）已就绪并通过本机自测；修复后可直接用 `cfg-app-socks.json` 复跑。
- **#13 重建零空窗 / #21 强制 apply / 幂等**：防火墙始终 applied=false，ALLOW 链没有一致态可比对；firewall=false 下多次 restart 端口稳定（portChanges=[]），但不构成该项证据。
- **item 5（__DIRECT__ / 无凭据代理）**：应用级数据面不可用，无法给出流量级证据；修复后一次跑完。

## 6. 设备最终状态（已恢复）

evidence-t9/restore-state.log：

    module: /data/adb/modules/devmask（本次构建，server 4203288 B / zygisk 1448528 B）
    settings get global http_proxy -> :0
    iptables -S DM_EGRESS_RLY      -> No chain/target/match by that name
    GET /api/v1/egress             -> 200，configured=false，mode=direct
    fixture: 已停止；adb forward 保留 tcp:18686

上轮遗留脏配置（`[egress].default=mac_http`、`[[apps]] flar2.devcheck`）随 uninstall 一并清除，现在是干净安装态。

## 7. 复跑清单（修复后）

1) 修 F1（ALLOW 去重/期望一致）与 F2（accept 期 uid 解析）后，用 `/tmp/t9-run/cfg-app-http.json` PUT；
2) 依次跑 cfg-app-http → cfg-app-socks → cfg-app-plain → cfg-app-dead → cfg-app-direct 五组探针；
3) 补 #13（设备内单条 while 循环 + 20 次 restart；协议级判据 `grep -qw 05` 为空 + 日志 uid_not_allowed）、#21（手工 -F 两链 → applied=false → restart → -S 恢复）、#11（端口撞车重分配）；
4) 收尾同 §4.4/§6。

## 8. 可执行性说明（契约 v1.2 口径）

- 设备无 `curl`、无 `setpriv`；`run-as … nc` 被 SELinux 拦（Timeout）→ 拒绝用例用 uid 2000 + 协议级判据，允许用例只能用应用内探针（io.df.egressprobe，本次 uid=10325）。
- 设备侧探针统一 `printf … | toybox nc -q 3 -w 6`（`-q` 必带）。
- fixture 上游用 LAN：主机 192.168.30.149:18080/18082（HTTP Basic 必需）、18081（SOCKS5 RFC1929 必需）；`adb reverse` 实测不稳定，不使用。
- API 走 `adb forward tcp:18686 tcp:8686` + 主机 curl；token admin123（既有，未新增）。

## 9. t20 复验口径（captain 指定，2026-11-25）

只跑失败项与回归，不重做已 PASS 的六项（设备级中继 / 绑定面 / 凭据扫描 / 卸载清理 / fail-closed 注入 / 模块属性注入）。

| 项 | 通过判据 | 命令要点 |
| --- | --- | --- |
| F1 | `relay.firewall.applied=true`；`app_http` listener `listening=true` 且能 accept；ALLOW 链里同一 (port, uid) 不重复；连续两次 restart 仍成立 | `GET /egress` + `iptables -S DM_EGRESS_RLY_ALLOW` + `ss -ltn` |
| F2 | 应用内探针经应用级 HTTP 成功（`fetch.app=200 body=198.51.100.8`，fixture `listener=18082 auth=ok digest=7213dc66f53c`）；经应用级 SOCKS5 成功且远程 DNS（fixture `CONNECT app.fixture:80` 域名 + `method=userpass(ok) digest=c71343023d5c`），探针 `socksdomain=ok` | 探针 APK + fixture 日志 |
| F3 | relay 生效时 PUT/GET warnings 不再出现 `egress_auth_unsupported` | `curl ... /egress` 过滤 code |
| 回归五组 | cfg-app-http → cfg-app-socks → cfg-app-plain（直连 407，证明不经 relay）→ cfg-app-dead（502 + `egress_relay_upstream_unreachable`）→ cfg-app-direct（`__DIRECT__`，selector=DIRECT、fixture 命中 0） | `/tmp/t9-run/cfg-app-*.json` |
| #13 | 设备内**单条** while 循环持续打 SOCKS 问候 + 20 次 restart：全程无 `05 00`（`grep -qw 05` 为空），日志有 `uid_not_allowed` | `adb shell 'i=0; while [ \$i -lt 120 ]; do …; done'` 后台 + 主机 20 次 restart |
| #21 | 手工 `-F` 两条链 → `relay.firewall.applied=false` → `POST /egress/relay/restart` → `applied=true`、`-S` 三链齐全、未授权 uid 仍被拒 | §5.7 第 4 组 |

若 t17 后仍未修好：直接按 failed 上报，贴 `server.log` 里收关连接的 `reason`（`uid_unresolved` / `uid_not_allowed` / `state_mismatch` 等）与当时 `iptables -S`，不改实现。
