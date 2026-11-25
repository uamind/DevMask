# t20 复验（round 2）：带认证中继 —— Pixel 9

- 结论：**FAIL**。F1（防火墙 apply）与 F2（accept 期 uid）**仍未修好**（F1 换了另一条原因）；F3 已修好。
- 被测构建：t17 合并态（server 4200376 B / zygisk 1448528 B，12:15 build），Pixel 9 / Android 17 / KernelSU。
- firewall=false 仅用于**隔离诊断 F2**，不是验收放行手段；默认配置结论以 firewall=true 为准。
- 证据：`docs/design/relay/evidence-t20/`（f1-recheck.log、f2-app-http.log、bind-uid-f2.log、credential-scan.log、uninstall-cleanup.log、verify-commands.log）。

## 1. 结论表（captain 指定范围）

| 项 | 结论 | 证据 |
| --- | --- | --- |
| F1 applied=true + listener accept | **FAIL（原因变了）** | 去重已修（allowRules 无重复 uid0）；但真机 iptables -S 与期望字符串不一致 → state_mismatch → applied=false → 17835 不监听 |
| F2 应用内探针经应用级 HTTP | **FAIL** | firewall=false 下 listening=true、allowedUids=[0,10325]，probe fetch.app=SocketException；server.log `uid="unknown" reason="uid_unresolved"` |
| F2 应用级 SOCKS5 + 远程 DNS | **FAIL（被 F2 阻塞）** | 端口不接受，探针 socksdomain=UnknownHostException |
| F3 不再出现 egress_auth_unsupported | **PASS** | relay 生效的 PUT warnings=[]（只有 F1 时出现 egress_relay_firewall_failed） |
| 回归 cfg-app-plain（无凭据直连） | **PASS** | 属性 192.168.30.149:18080；fixture `listener=18080 GET http://app.fixture/ auth=missing rejected=407`（不经 relay） |
| 回归 cfg-app-direct（__DIRECT__） | **PASS** | http/socks 属性全 null、selector=DIRECT、fetch.app=UnknownHostException |
| 回归 cfg-app-dead（上游不可达） | **FAIL（被 F1/F2 阻塞）** | 应用级端口不接受，收不到 502/egress_relay_upstream_unreachable |
| #13 竞态（设备内 while + 20 restart） | **未验证** | applied 恒 false 且应用端口不接受，协议级判据无意义 |
| #21 强制 apply（-F → applied=false → restart 修复） | **FAIL** | 默认配置 applied 一开始就是 false，restart 后仍 false |

## 2. F1：去重已修，但 -S 文本比对在真机必然不等

    实际 iptables -S:
    -A DM_EGRESS_RLY_ALLOW -d 127.0.0.0/8 -o lo -p tcp -m tcp --dport 17800 -m owner --uid-owner 0 -j ACCEPT
    server 期望串（firewall.rs accept_rule）:
    -o lo -d 127.0.0.0/8 -p tcp --dport 17800 -m owner --uid-owner 0 -j ACCEPT

两处差异：**选项顺序**（`-d` 先于 `-o`）与**隐式 `-m tcp`**；DENY 链同样多出 `-m tcp`。
`sample_locked` 用 `as_set(normalize)` 逐字符串比较（normalize 只压空白）→ 真机恒 `state_mismatch`；
宿主 FixtureRunner 把命令原样回显，所以 t17 新增单测 `applied=true` 也是绿的。
建议：比对改成语义化 argv 比较（去掉隐式 `-m tcp`/`-m owner` 后按选项集合比），或期望串按 `iptables -S` 规范格式生成，并把真机 `-S` 文本作为 golden 回放。

真机结果：`firewall={applied:false, rules:9, allowRules:4, denyRules:2, lastError:{reason:"state_mismatch"}}`，`app_http` `listening=false`，`ss -ltn` 只有 17800。

## 3. F2：uid_unresolved 仍在

`[relay] firewall=false` 诊断态（两 listener 均 listening，`app_http.allowedUids=[0,10325]`）：

    EGRESSPROBE: http.proxyPort=17835
    EGRESSPROBE: selector.app.fixture=HTTP @ 127.0.0.1/<unresolved>:17835
    EGRESSPROBE: fetch.app=error=SocketException
    server.log: WARN relay client rejected proxy=app_http port=17835 uid="unknown" reason="uid_unresolved"

t17 改成查「客户端那一行（local=对端源端口, rem=监听端口，tcp+tcp6）」后真机仍未命中。
建议：把 accept 期 /proc/net/tcp{,6} 的原始行与解析键（hex 端口、状态过滤、netns）打 debug 日志（不含凭据）核对；
或优先用 SO_PEERCRED（Android TCP loopback 可用），/proc 作后备。

## 4. 通过的回归项

- **cfg-app-plain**：无凭据定义不进 relay（listeners 只有 dev_http 17800）；应用属性直接是上游 `192.168.30.149:18080`，
  fixture 落 `auth=missing ... rejected=407` → 直连不经 relay。
- **cfg-app-direct**：`http.proxyHost/socksProxyHost=<null>`、`selector.app.fixture=DIRECT`、`fetch.app=UnknownHostException`。
- **F3**：relay 生效时 PUT/GET warnings 不再含 `egress_auth_unsupported`。
- **凭据零泄漏**：5/5 路扫描命中 0。
- **卸载清理**：178xx 无监听、`DM_EGRESS_RLY` 链不存在、`settings=:0`。
- **bind 面**：ss -ltn 仅 127.0.0.1、ss -ltn6 空、`::1` / `127.0.0.2` / `192.168.30.2` 全部 Connection refused。
- **verify 命令**：module_tests / aarch64 check / fmt --check 全部 exit 0。

## 5. 未执行 / 无法判定

- **#13**：默认配置应用端口不监听；firewall=false 下所有连接又被 F2 关闭，HIT 检测无意义。
- **#21**：默认配置 applied 初始即 false，无法演示「-F 后 false → restart 修复 true」。
- **应用级 SOCKS5 / 远程 DNS**：端口不接受连接，fixture 无 SOCKS 上游握手可验。

## 6. 设备最终状态（已恢复）

重装 t17 构建并 reboot 后：`settings=:0`、无 `DM_EGRESS_RLY` 链、`GET /api/v1/egress` 200（干净安装态）、
bin 4200376 B / zygisk 1448528 B（12:21）、fixture 已停止、adb forward 保留 tcp:18686。

注：reboot 后 service.sh 的 supervisor pid 可能是重启前的旧值（日志「守护循环已在运行(pid=…)，跳过启动」），
本次用 `service.sh stop && start` 拉起；不影响 relay 判定，但建议启动时校验 pid 是否属于本 PID 命名空间。

## 7. t22 复验范围（captain 指定，2026-11-25，追加两条判据）

1) **applied=true 且应用级 listener accept**：不能只看 GET 字段，必须贴 `iptables -S DM_EGRESS_RLY_ALLOW / _DENY` 与
   server 期望串逐条对照（语义比较应吸收选项顺序与隐式 `-m tcp`），并给出 `ss -ltn` 里 127.0.0.1:<app_port> 一行。
2) **uid 归属的两种态都要验**（新策略）：
   - **firewall.applied=true**：拿不到 uid 时按「已 applied」放行 → 应用内探针必须连通（HTTP 200 / SOCKS 05 00）；
   - **firewall=false**：仍 fail-closed → 拿不到 uid 时必须关闭；拿到 uid 且在 allowedUids 内才放行。
   两种态分别用 `[relay] firewall` 显式设置 + `POST /egress/relay/restart`（注意：PUT /egress 会重写 egress.toml 并带回默认 firewall=true，切态要用「改文件 + restart」而不是 PUT）。
3) 其余按 t20 §5 / t9 §9 复跑：`#13` 设备内单条 while 循环 + 20 次 restart（协议级 `grep -qw 05` 为空 + 日志 `uid_not_allowed`）、
   `#21` 手工 -F 两链 → applied=false → restart → applied=true 且 -S 三链齐全、应用级 SOCKS5 + 远程 DNS（fixture 落域名的 CONNECT + RFC1929 摘要）。

复验前的固定动作：`cargo test -p devmask_module_tests` / `cargo check -p devmask --target aarch64-linux-android` / `cargo fmt --check`，
装机走 `cargo run -p xtask -- build -r`（`ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk`）+ `ksud module install` + reboot，boot 后用
`service.sh stop && start` 拉起（规避 supervisor 旧 pid），再开始判据。
