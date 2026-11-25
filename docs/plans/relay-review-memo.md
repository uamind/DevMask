# Relay 增量 · 评审预审记录（reviewer）

> 本文件是评审工作记录，**不是**交付文档（交付文档由 t11 收口）。
> 对象：docs/plans/relay-contract.md（t1 已冻结）+ 现有基线代码。
> 状态：t2/t5/t9 未完成，设备端 relay 代码尚未落地；以下是**契约层**必改项与 t9 验收口径修正，
> 代码落地后按第 4 节逐条核对，t10 出正式 verdict。
> 已接受前提（不重复报）：K1 应用级 spec 不带凭据、K2 新增 DF_EGRESS_RLY、K3 设备级经中继承载认证。

## 1. 结论摘要

| # | 级别 | 问题 | 位置 |
| --- | --- | --- | --- |
| R1 | blocker | 「暂停 accept」不构成零空窗，且会把窗口期连接**缓存**到恢复后照常服务；启动时序未定义 | 契约 §5.4 L601–616、§5.2 L542 |
| R2 | blocker | 契约冻结「探测不经中继」，与 captain 的 via=relay 口径直接冲突；未定义 relay 探测与 fail-open 的边界 | 契约 §3.5 L387–392、§9.2 B4 L989 |
| R3 | high | §5.7 第 3/4 条验收命令在 root 下执行且完全相同，期望值写错；照抄会得到假绿 | 契约 §5.7 L657–661、§10.3 L1083–1087 |
| R4 | high | IPv4-only 的取舍依赖「只 bind 127.0.0.1」，但绑定面没有硬断言与验收项；IPv6 / 127.0.0.2 / LAN IP 未覆盖 | 契约 §5.1 L530–531、§5.2 L573 |
| R5 | high | uid 归属失败口径是 fail-open（仍接受连接），而它正是重建窗口的唯一兜底；uid_not_allowed 日志语义未定义 | 契约 §4.6 L470–475、§4.8 L515 |
| R6 | high | 端口稳定性：重建时未写死「先 close 旧 socket 再 bind 同端口」，会自撞 EADDRINUSE → C3 误重分配 | 契约 §1.5 L147–154、§5.4、§6.3 |
| R7 | medium | uid 解析 / iptables 失败的**时序**未写死（解析 → 装规则 → 才 accept） | 契约 §5.3 L593–599、§5.5 L620–629 |
| R8 | medium | 代价清单缺一条：中继把应用出网变成 root 出网 → 绕过第三方 uid 防火墙；[relay] 无 API 关闭通道 | 契约 §8.1 L937–945、§2.2 L261 |
| R9 | medium | 认证代理 + 既有 fail_open=true：上游探测失败会清 settings → 静默直连（IP 泄漏），未写进代价 | 契约 §7.4 L900–914 |
| R10 | low | 分配时 bind 探测后立即 close，存在 TOCTOU | 契约 §1.3 L107 |
| R11 | low | uid 归属失败时不计 per-uid 限额，单 uid 上限可绕过（全局 256 仍生效） | 契约 §4.6 L475 |

## 2. 逐条说明

### R1 [blocker] 零空窗不成立

契约 §5.4 的冻结流程是「暂停 accept（不关监听 socket）→ 算规则 → -F → 逐条 -A → 恢复 accept」，
并在 L613 断言「窗口内不会新建连接」。这个断言事实错误：

1. **TCP 握手由内核完成**。暂停 accept() 只停用户态取连接，SYN 仍被内核接受、连接进入 accept 队列（backlog 128）。即窗口内**新连接照常建立**。
2. **恢复 accept 后这些连接照常被服务**。契约没有要求 drain 或拒绝 backlog 里的连接，于是窗口期连进来的未授权 uid 会拿到一条合法隧道；且「已建立的连接不受规则影响」（§5.4 L613 自述），可以长跑（HTTP keep-alive / SOCKS5 隧道）。
3. 因此「暂停 accept」把窗口从「毫秒级 flush→A」放大成「整段重建时间 + backlog 排队」，方向是**恶化**而不是补偿。攻击侧成本极低：127.0.0.1:17831 上死循环 connect，命中即持久化。
4. **启动 / 首次 reconcile 的时序完全未定义**：新 listener 是「先 bind 并 accept，再装规则」还是反过来？契约只写「重建」。

**要求修法（三选一，建议 a）**：

a. **两条链**：静态 DF_EGRESS_RLY_DENY（每端口一条 REJECT，只在端口集合变化时改）+ 可重建 DF_EGRESS_RLY_ALLOW（ACCEPT）。挂接顺序 OUTPUT → ALLOW → DENY。重建只 -F ALLOW，空链时流量落到 DENY → 全拒（fail-closed，只有短暂不可用，没有裸奔窗口）。新增端口顺序：先加 DENY 端口规则 → 再加 ACCEPT → 最后才让 listener accept。
b. 代际链 + 单条原子换跳转：新链 DF_EGRESS_RLY_G<N> 建满，再 iptables -R OUTPUT <n> -j <新链>（单条规则替换在 xtables 锁内原子），最后清旧链。
c. 只用 iptables-restore 单事务提交（注意 --noflush 不能原子替换单链，需要整表快照，实施成本最高）。

另外两条必须写进契约的硬约束：

- **listener 生命周期固定为**：create(paused) → 解析 uid → 装规则 → 校验 iptables -S 与期望一致 → 才 accept。任何一步失败 → 保持 paused 且 listening=false（§5.5 已有结果口径，缺时序）。
- **重建时恢复 accept 之前**，把窗口期内排队的连接按 R5 的 accept 期 uid 校验再判一次——这是唯一能覆盖已排队连接的机制。

**验收（t9 必加）**：以 uid 2000 死循环 connect(127.0.0.1:17831)，同时以 root 执行 20 次 POST /egress/relay/restart，断言 20 个窗口内没有任何一次 connect 成功（当前契约实现必然失败）。

### R2 [blocker] probe 口径与 via=relay

契约 §3.5 与 §9.2 B4 明确冻结「探测不经中继」，理由是不自环、把「上游可用」与「中继健康」分开观测。
这与 captain 的验收口径冲突：**「探测绿但应用不通」会在 relay 场景原样复发**——探测直接连上游成功，而本地 listener 未监听、防火墙拒绝了目标 uid、或中继转发有 bug 时，应用侧仍然全失败，API 却全绿。

**要求修法**：

- POST /egress/probe 增加请求字段 via: "upstream" | "relay"（缺省 upstream，与 v1.10.0 逐字节兼容）。via=relay 的语义：**按 package（或设备级默认）选中该定义的中继端口，连 127.0.0.1:<local_port>，以 Channel::Proxy{host:"127.0.0.1", port:local_port, kind, 无凭据} 走现有 probe 代码**，一次覆盖「本地监听可连 + 中继转发 + 上游认证 + 目标站点往返」。
- 响应 via 回显 "relay"，另加 relayPort；upstream.host/port 仍回真实上游。
- **fail-open 门控（安全关键）**：只有 via=upstream 且 reason ∈ {connect,dns,timeout,tls} 才允许 fail_open 清 settings。via=relay 失败**一律不动 settings**（否则「中继死 → 探测失败 → 清系统代理 → 全设备静默直连」，把 fail-closed 变成 IP 泄漏），只回 relay 侧 warning。
- 失败语义：via=relay 且该端口未监听（local_port 有值但 listening=false）→ egress.probe_failed，details.reason="relay_not_listening"；连接被拒 / 超时 → "relay_connect" / "timeout"。不新增对公错误码。
- 文档：§6.6 端点 28 的「不变」改成「请求 +1 字段、响应 +1 字段」；docs/API.md §5.28 三语同步；manager 探测按钮（t6）要能选 via。
- **边界必须写清**：via=relay 由 root（uid 0）发起，而链对 uid 0 永远放行，所以它证明的是「中继数据面可用」，**不证明**「目标应用 uid 能连」；后者只能靠 t9 在应用 uid 下实测（见 R3）。

**验收（t9 必加）**：① 正常态：via=upstream 与 via=relay 均 ok，且 via=relay 的出口 IP 等于上游出口；② 让该端口不监听（把 pm 换坏或 [relay].enabled=false）→ via=relay 红（relay_not_listening）、via=upstream 绿，且 settings **不变**；③ 改错凭据 → 两者都红，settings 不变。

### R3 [high] §5.7 / §10.3 的验收命令是错的

§5.7 第 3 条（L658）声称「未授权 uid（shell 是 uid 2000）」却写成 adb shell su -c 'toybox nc ...'——su 之后是 **uid 0**，而 §5.2 规则 1 对 uid 0 在所有 178xx 端口 ACCEPT（L546–547）。所以该命令期望「rc 非 0」**必然失败**；第 4 条（L661）与第 3 条命令完全相同、只换注释，也无法证明「授权 uid 能连」。§10.3 L1083–1087 照抄了这个口径。

**要求修法**：

- 拒绝用例：adb shell 'toybox nc -w 2 127.0.0.1 17831 </dev/null; echo rc=$?'（不加 su，uid 2000）。
- 允许用例：必须真正以目标 uid 执行，二选一并在契约写明可用那条——adb shell su -c 'setpriv --reuid 10123 --regid 10123 --clear-groups toybox nc ...'（setpriv 不一定有）或 run-as（仅 debuggable），或在应用进程内跑探针。**契约不应给出在验收机上不可执行 / 语义错误的命令**。
- 同批补：per-uid 64 上限、全局 256 上限、ss -ltn6、127.0.0.2、LAN IP（见 R4）。

### R4 [high] 绑定面 / IPv6 / 127.0.0.2

契约 §5.1「只做 IPv4、不装 ip6tables」的合理性**完全依赖**「listener 只 bind 127.0.0.1」，但这条现在只是散文（L573），没有硬断言、没有实现约束、没有验收项。三种绑定错误的后果：

| 绑定 | 后果 |
| --- | --- |
| 0.0.0.0 | device 级规则 -d 127.0.0.0/8 覆盖 127.0.0.2，且 **INPUT 无任何规则** → wlan0 上任何设备可白用上游凭据（LAN 开放代理，最严重） |
| ::（双栈） | ::1 完全绕过 DF_EGRESS_RLY（无 ip6tables），应用级 uid 白名单失效；::ffff:127.0.0.1 走 v4 映射通常仍受 iptables 约束，需实测确认 |
| localhost（解析） | 结果取决于解析顺序，可能落到 ::1 |

**要求修法**：

- 契约把「bind 目标 = SocketAddr::from(([127,0,0,1], port))，IPv4 字面量」写成硬约束；bind 成功后用 getsockname() 复核 family == AF_INET && addr == 127.0.0.1，不符 → 关掉、按失败处理。
- 禁止 0.0.0.0 / :: / 主机名 / [::1]；SO_REUSEADDR 只用于重建，不用 SO_REUSEPORT。
- 验收四条：ss -ltn | grep 178xx 全为 127.0.0.1；ss -ltn6 | grep 178xx 为空；nc ::1 17831 失败；nc 127.0.0.2 17831 失败；nc <LAN_IP> 17831 失败（最后一条同时证明不是开放代理）。

### R5 [high] uid 归属失败是 fail-open，uid_not_allowed 语义缺失

§4.6 第 4 条（L475）冻结「3 次重试仍找不到 → **仍然接受**，只不计单 uid 计数」。这在单纯做限流时没问题，但 §4.8 日志样例（L515）又出现 relay client rejected proxy=jp_socks port=17831 uid=2000 reason=uid_not_allowed——说明契约心里有一个「accept 期 uid 允许表」，却从未定义它。两者不能同时留着。

**要求修法（建议采纳 accept 期校验，它同时是 R1 的第二道防线）**：

- **device 级端口**（uidPolicy=all）：归属失败 → 接受（不变），只计数跳过。
- **app 级 restricted 端口**：解析出 uid 且不在 allowedUids → 立即 close，日志 relay client rejected ... uid_not_allowed（uid 不是凭据，允许记录）；**归属失败 → 也 close**（fail-closed）并记 uid unresolved。
- 这样即使 R1 的窗口存在、即使有人手工 iptables -F，未授权 uid 也拿不到隧道。
- 代价：/proc/net/tcp 读失败（极端）会误拒合法连接；日志可见、可重试，比裸奔好。

### R6 [high] 端口稳定性：重启自撞

§1.5 第 3 步「先原子写回 egress.toml，再按新端口建 listener」、第 5 步「旧端口的 listener 关闭」，而 §5.4 的重建只「暂停 accept，不关 socket」。若 reconcile 按字面实现（先 bind 新 listener、后关旧），**同一个端口不可能被 bind 两次**（SO_REUSEADDR 不允许两个 active listen）→ EADDRINUSE → C3 判定为「被外部占用」→ 误重分配 + 写回文件 + restartRequiredApps 抖动。验收 #10（连续两次 restart 端口不变）会直接失败，且每次重启都逼用户重启应用。

**要求修法**：顺序写死为「pause accept → **close 旧 socket** → 分配/校验端口（本进程自己刚释放的端口排除在 C3 之外）→ bind 新 listener（paused）→ 装规则 → accept」。分配算法的 bind 探测必须先排除本进程自己的 listener。

### R7 [medium] 失败路径的时序

§5.3/§5.5 只冻结结果（「该端口不监听」），没冻结时序。必须写：**先解析 pkg@uid（5 s 上限）→ 解析成功才进入 bind+规则 → 规则校验通过才 accept**；解析失败或 iptables 失败 → 该端口**从不 accept**（且不得残留上一轮 listener）。POST /egress/relay/restart 必须对**全部** listener 先 pause（含即将被销毁的），不存在「一边被重建、一边还在 accept」的实例。

验收：把 pm 换成失败脚本 → listeners[].listening=false + ss -ltn 无该端口 + 链内无该端口 ACCEPT；把 iptables 换坏 → 全部 app 端口不监听、device 端口仍监听、firewall.applied=false。

### R8 [medium] 代价清单缺 P8 + 关闭通道

中继让「应用出网」变成「root 进程出网」：任何按 uid 生效的第三方防火墙（AFWall+、NetGuard、厂商联网管控）看到的都是 root 的连接，**用户「禁止该应用联网」的策略被中继绕过**。settings 指向 127.0.0.1 会放大这一点。这不是 K3 的重复，是 K3 的下游后果，现有 §8.1 P1–P7 一条都没写。

**要求修法**：§8.1 增 P8（谁看得见：第三方防火墙、root；强度：中）并写进三语已知限制；[relay].enabled / firewall 不在 API 可写范围（§2.2 L261），关闭只能编辑 0600 文件 + restart——必须写进 docs/CONFIG.md 与 docs/SERVER.md，否则用户找不到开关。

### R9 [medium] fail-open 与认证代理的隐私语义

[probe].fail_open 默认 true（既有）。relay 生效后，上游探测失败（connect/dns/timeout/tls）会清 settings 回 :0，全设备**静默直连**。对「带认证代理 = 指定出口/隐私」的用户，这是出口被改而不自知。契约 §7.4 只说「三时点判定不变」，没有一句代价说明。

**要求修法**：§8.1 增一条「fail-open 会改出口」；docs/CONFIG.md 写明「要保证不直连，把 [probe].fail_open 设为 false」；GET /egress 在 relay 生效且 fail_open=true 时给提示（可复用现有 warning 机制，不新增码也可）。不做默认值变更（避免影响既有用户）。

### R10 [low] 端口探测 TOCTOU

§1.3 第 4 步 bind 探测后立即 close，再在别处 bind 正式 listener → 中间可能被外部抢占。建议分配时**直接持有 listener socket**（paused），把探测和真实绑定合并；close 只发生在重建。

### R11 [low] per-uid 限额可绕过

R5 采纳后，restricted 端口归属失败会 close，问题自然消失；device 级端口归属失败仍不计 per-uid 限额（受全局 256 约束）。在契约里显式写「device 级端口不承诺 per-uid 限额」，避免被当成 bug。

## 3. t9 必须补的验收（在当前 §5.7 / §10.2–10.4 之外）

    # A. 重启竞态：窗口内未授权 uid 不得拿到隧道（R1）
    adb shell 'while true; do toybox nc -w 1 127.0.0.1 17831 </dev/null >/dev/null 2>&1 && echo HIT; done' &
    for i in $(seq 20); do adb shell su -c 'curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" -d "{}" http://127.0.0.1:8686/api/v1/egress/relay/restart' >/dev/null; done
    # 期望：全程无 HIT

    # B. 绑定面（R4）
    adb shell su -c 'ss -ltn | grep -E ":178[0-9]{2}"'      # 全 127.0.0.1
    adb shell su -c 'ss -ltn6 | grep -E ":178[0-9]{2}"'     # 空
    adb shell 'toybox nc -w 2 ::1 17831 </dev/null; echo rc=$?'          # 非 0
    adb shell 'toybox nc -w 2 127.0.0.2 17831 </dev/null; echo rc=$?'    # 非 0
    adb shell 'toybox nc -w 2 <LAN_IP> 17831 </dev/null; echo rc=$?'     # 非 0

    # C. 授权 / 未授权 uid（R3）
    adb shell 'toybox nc -w 2 127.0.0.1 17831 </dev/null; echo rc=$?'    # 2000 → 非 0
    adb shell su -c 'setpriv --reuid 10123 --regid 10123 --clear-groups toybox nc -w 2 127.0.0.1 17831 </dev/null; echo rc=$?'  # 授权 uid → 0

    # D. 探测对照（R2）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -d '{"via":"relay"}' "$BASE/egress/probe" | jq '{ok,via,relayPort,reason}'
    # 使端口不监听后：via=relay 红、via=upstream 绿、settings 不变
    adb shell su -c 'settings get global http_proxy'

    # E. 端口稳定性（R6）
    adb shell su -c 'grep local_port /data/adb/device_faker/config/egress.toml'
    # 两次 restart 前后逐行 diff，必须完全一致；响应 portChanges / restartRequiredApps 均为空

## 4. 代码落地后的核对清单（t10 用）

1. bind 目标字面量与 getsockname 复核（R4）。
2. listener 生命周期：paused → 规则 → accept；失败不 accept（R1/R7）。
3. -F 与 -A 之间是否仍有 ACCEPT 能力（R1）；重建是否先 close 旧 socket（R6）。
4. accept 期 uid 校验与 allowedUids 比对；归属失败 close（R5）。
5. iptables 是否走 CommandRunner（program+args，不经 shell 拼接）、-w 兼容、owner 不支持识别（§5.5）。
6. pm list packages -U --user 输出解析：多用户、@userId、空输出、非零退出、5 s 超时、共享 uid 去重（R7）。
7. settings 写入顺序：bind+规则就绪后再写、读回校验；失败清 :0（§7.1）。
8. 凭据边界：server.log / api-audit / 全部 API 响应 / relay 日志 / spec 里 password、Proxy-Authorization、base64 命中 0。
9. via=relay 实现与 fail-open 门控（R2）。
10. 卸载 / 禁用清理顺序：停 accept → 摘跳转 → 清链 → 删链 → settings（§5.6）。
11. 端口表原子写 0600、local_port 不进 diff/导出、PUT 传 localPort/local_port → 422（§1.8/§6.5）。
12. 回归：无凭据 / __DIRECT__ / 未配置三类的 spec 与 v1.10.0 逐字节一致。
    （本条后半句已过时：t4 已接线，relay 校验与降级判定见第 5 节。）

## 5. t4 追加评审（模块注入的两处「降级即直连」）

前提：t4 已合入（src/egress.rs / src/spec.rs / src/hooks.rs）；本机复跑
cargo test -p device_faker_module_tests 为 49 passed / 0 failed。以下按 captain 提问判。

### R12 [high] 非法 relay 视图按「未配置」处理 = fail-open

现状：from_spec_doc 对 relay=true 做三项校验，任一不成立 → warn + **返回 None**；
hooks::apply_egress_proxy 对 None **直接 return**（不写属性、不 arm replay）。后果是应用继承设备级
代理或直连——若设备级是 :0，就是静默真实 IP 出口，且 API / WebUI 完全看不到（spec 是 companion 内部产物）。

判定：**不接受**。理由：
1. 该路径正常不可达（v1.11 server 只产出合法回环视图；旧 server 根本不写 relay 键），出现即 bug 或伪造，
   失败必须响亮；
2. 返回 None 不会 arm replay，框架 ~95ms 后按设备级 ProxyInfo 重写 http/https 属性，
   可能把应用**重新指到设备级中继或直连**——这比「不注入」更彻底；
3. fail-closed 成本可控：127.0.0.1:9 落在中继端口池外，绝不会误连别的定义的 listener，
   连接立即 ECONNREFUSED（我们不 DROP），表现为断网而不是换出口。

要求：
- 新常量 RELAY_FAIL_CLOSED_PORT = 9，并断言不在 17800..=17899。
- 非法目标 → 仍返回一个可注入的代理（推荐 relay=true + host=127.0.0.1 + port=9，模块识别为
  「显式阻塞」并在日志标注 blocked=true），**必须经过 injection_plan**，从而 arm replay。
- 唯一例外：host/port 合法、只是多带了凭据 → 剥掉凭据照常注入中继目标（修复，不是降级）。
- type=direct + relay=true（自相矛盾）→ 按「主动直连」返回 Direct（清属性 + NO_PROXY selector），
  不能返回 None（None 会继承设备级代理，违背 __DIRECT__ 语义）。
- 日志：[egress] relay fail-closed package=... reason=invalid_target|port_missing creds=0|1，不含凭据。
- 现状 injection_plan 返回 None 时 hooks 打的是 error=missing endpoint，语义误导，一并改掉。
- 残留风险写进文档：注入只覆盖 Java 层，native / WebView 本来就不受控（契约 §3.4），fail-closed 不承诺它们。

### R13 [medium] 旧配置回退的两条边界

现状 spec_proxies：relay 开 && 带凭据 && 无 local_port → 回退「上游 host:port + 凭据载体」+ warn。
这条回退只在「文件尚未被 server reconcile（v1.10 形态）」时讲得通；一旦文件已迁移（有 [relay] 或已有
local_port），缺端口只可能来自池耗尽 / 写回失败 / 手工改坏——此时把 SOCKS 凭据写回应用进程，直接破坏
「relay 生效时凭据不出现在应用进程」的不变量，而且 SOCKS 路径可能「看起来能用」，把故障藏起来。

判定：
- 回退**必须保留**（拒绝注入 = 不写属性 = 继承设备级 / 直连，同样是静默换出口，更差）；但把条件收窄为
  「中继明确未启用」：[relay].enabled=false，或「文件未迁移」（无 [relay] 键且没有任何 local_port）。
- 「enabled=true 且已迁移但该定义缺 local_port」→ **不回退**，走 R12 的 fail-closed（127.0.0.1:9）。
- 回退里的凭据载体建议一并去掉（HTTP 的 Authenticator 本就不装；java.net.socks.username/password 已被
  K1 记录为实测不成立）——写进去只增加暴露面、不增加可用性。属 K1 相邻的契约收窄，需 architect 落版；
  若坚持 v1.10 字节兼容，则至少限定在显式 enabled=false 并写文档。
- 文档必须显式声明（CONFIG.md「网络出口 → 已知限制」、SERVER.md §10.8、三语 + CHANGELOG）：
  ① relay 关闭或文件未迁移时，SOCKS 凭据会被写进应用进程的系统属性（同 uid 代码可读），HTTP Basic 仍不可用；
  ② relay 开启但端口未就绪 / 视图非法时 fail-closed：应用断网，不静默直连，凭据不进应用进程；
  ③ 恢复路径（等 server reconcile / POST /egress/relay/restart / 重启应用）。

### t4 追加测试要求（改完复验）

1. 非法 host（非 127.0.0.1）/ 池外端口 / direct+relay → 计划为 127.0.0.1:9，另一协议属性被清，无 secret 期望。
2. relay 合法但带凭据 → 注入合法端口，plan.socks_credentials=None、unsupported_auth=false。
3. 缺 local_port 两态：文件未迁移 → 上游回退（或按收窄后的口径）；已迁移 → 127.0.0.1:9。
4. 日志断言：fail-closed 标记存在、凭据原文与上游地址不出现。
5. t9 真机：手工把某个定义的 local_port 改成池外 / 撞车值 → 重启应用 → 断网 + logcat 有 fail_closed +
   settings 不变 + 日志无凭据；修好并重启应用后恢复。

## 6. 契约 v1.1 复核（captain 四点）

### R14 [high] §5.4 第 1 步「幂等短路」与 §6.3「全量重建」矛盾 → 手工 -F 后 restart 不修复

证据：§5.4 第 1 步（L665）写「与当前已生效态完全一致 → 跳到 9（不动规则）」；§6.3（L880）写
「全量重建 listener 与防火墙链」「重建 ALLOW（-F + 按序 -A）并按 §5.4 校验」。
若「当前已生效态」取内存快照，则用户手工 iptables -F ALLOW / -F DENY 之后调 restart 会短路，
规则仍为空、firewall.applied 仍是 true。安全上还有 accept 期 uid 双检兜底，但：
(a) 文档承诺的修复通道失效；(b) 双检从「备份」变成唯一防线，与「两层互为备份」的设计不符；
(c) 观测说谎（applied=true 实际为 false）。

要求：① 显式 POST /egress/relay/restart 必须**强制 apply**（跳过短路）；② 短路判定必须包含一次
iptables -S 三链校验（或 firewall.applied 直接由 -S 结果推出）；③ 测试：手工 -F 两链 → restart →
-S 恢复、firewall.applied=true、未授权 uid 仍被拒。

### R15 [high] §5.7 手工 -F 用例的判据不成立

现状命令：iptables -F ALLOW / -F DENY 后用 toybox nc -w 2 127.0.0.1 17831 </dev/null; echo rc=$?
判「rc 非 0」。两链清空后入口链仍存在但无规则 → RETURN → OUTPUT 默认 ACCEPT → **内核完成握手**，
nc 的 connect 成功、rc 通常为 0，与服务端是否拒绝无关。该用例会假失败，或被实现方用错误方式
「修好」（例如把拒绝放到读协议之后）。

要求改为协议级证据（三选一）：① printf '\x05\x01\x00' | nc -w 2 127.0.0.1 17831 | od -An -tx1
期望输出为空（正常应回 05 00）；② HTTP：发 GET http://example/ HTTP/1.1，期望无响应直接 EOF；
③ logcat 断言 relay client rejected ... uid=2000 reason=uid_not_allowed 出现。
同时要求实现：restricted 端口 accept 后、**读首字节前**完成 uid 双检（§4.6 第 5 条已写，
t9 必须验证「先检后读」）。

同一判据问题也出现在 §5.7 第 3 组与 §10.4 #13（「未授权 uid 持续 connect + 20 次 restart →
全程没有一次成功连接」）：accept 暂停期间内核仍完成握手，**connect 成功 ≠ 拿到中继服务**；
该用例会对正确实现报假失败，并可能诱导实现方在重建时关闭监听 socket（反而破坏「进行中连接不受影响」）。
应改成协议级证据：printf '\x05\x01\x00' | nc ... 不得回 05 00；或对每次「能连上」的样本断言
logcat 里有对应的 uid_not_allowed close（即服务前拒绝）。§10.4 #13 措辞建议改为
「没有一次拿到中继服务（协议级）」。

### R16 [low] firewall=false 与 accept 期双检的文档冲突

§5.5 说 firewall=false 时应用级端口「对所有 uid 可达」；但 §4.6 第 5 条的双检对 restricted 端口
无条件生效 → 实际仍会拒绝非白名单 uid（除非归属失败）。二者取一：推荐保留双检，并把 §5.5 措辞改成
「规则层不设防，accept 期仍按 allowedUids 拒绝，归属失败才放行」；不建议 firewall=false 连双检一起关。

### 实现证据状态（本轮实测）

（已过时，见 §7：t2 核心已落盘，t3 防火墙仍未落。）

## 7. v1.2 复核（captain 三处结论）

### 同意 1：凭据载体彻底移除（R13 扩大）

条件（t10 会验）：
- **删除写入面**而不是留死代码：InjectionPlan.socks_credentials 删除或恒 None；replay_expectations 不再
  产出 java.net.socks.username / password；src/ 内 grep 这两个键命中 0（测试与注释另计）。
- unsupported_auth 对 http 与 socks 的「带凭据回退」都为 true；hooks 日志不能只写 type=http，socks 也要
  留一行 credentials ignored（否则 socks 静默无认证）。
- 文档 / CHANGELOG 明确：≤v1.1 会写凭据载体，v1.2 起不会；关闭 / 未迁移窗口里带认证代理 = 不可用
  （407 / 认证失败），只能靠上游 IP 白名单。
- 测试：旧 server spec（带 username/password、无 relay）→ plan 无 socks_credentials、unsupported_auth=true、
  replay 期望无 secret。

### 同意 2：firewall.applied 由 -S 现算（R14）

条件（t10 会验）：
- GET 的 -S 采样**不得与重建并发**（拿同一把 reconcile 读锁，或显式 rebuilding 字段），否则每次 restart
  期间的 GET 都会 applied=false，Manager 误报。
- 不一致 / 失败要**同时**进 warnings：建议新增 reason="state_mismatch"（手工 -F 用），不要拿
  command_failed 冒充（命令其实成功了）。否则 applied=false 只藏在 relay 对象里，等于把「观测说谎」挪位。
- rules / allowRules / denyRules 必须来自**同一次** -S 快照（不能 applied 用 -S、计数用内存）。
- 超时：GET 每次 1–3 次 spawn，单次上限建议 ≤1s（不能吃 CommandRunner 的 5s，否则一次 GET 卡 5 秒）；
  失败不 500。
- 不做 TTL 缓存（或 TTL=0 且写路径失效），否则 t9 在 -F 后立即 GET 会读到 stale true。

### 同意 3：R15 协议级判据 + 先检后读

文内已改好（§4.6 第 5 条、§5.7 第 2/3 组、§10.4 #13/#22）。两个 nit：
① 第 3 组 HIT 检测 grep -q 05 是子串匹配，建议 grep -qw 05 或整行等于 "05 00"；
② 第 3 组探测循环跑在 host（每轮一个新 adb shell），采样密度约 10/s，毫秒级窗口可能漏；建议改成设备内
单条 while 循环（adb shell 'timeout 30 sh -c ...'）持续打，才配得上「20 次 restart 零命中」的结论。

### 预检（t2 已落代码的正面证据；t3 未落）

- bind：SocketAddr::new(V4(127.0.0.1), port) + local_addr 复核（server/src/relay/service.rs bind_loopback）。
- accept 期双检：spawn_connection 在 http::serve 之前做 resolve_peer_uid + allowed_uids 判定，uid 未知或
  不在集合 → shutdown —— 顺序满足「先检后读」。
- probe：Channel::Relay{name, port} 不带凭据；fail-open 门控 if failure.triggers_fail_open() && !via_relay。
- **偏差（低）**：via=relay 且 [relay].enabled=false 但 local_port 仍在端口表时，实现走 relay_active=false
  → 502 relay_not_listening；契约 §3.5 规则 1 明确把 enabled=false 归入 409 egress.not_configured。
  建议先判 enabled（及 has_credentials）再判 listening，或契约放宽该条。
- 防火墙（t3）仍无代码：五条「无裸奔」论证无法验收，t10 保持 needs_revision。
  （t3 已开始落 firewall.rs：ALLOW_CHAIN / DENY_CHAIN / apply / clear 已存在，验证仍待 t3 完成。）

## 8. t2 手写异步 IO 复核（captain 指定 high 风险项）

结论：**手写已够，不需要加 tokio io-util / copy_bidirectional**。四个问题里三个（a/b/c 的内存面）实现正确，
唯一的实质缺口（写阻塞时 idle 计时失效）**换成 copy_bidirectional 也解决不了**，加一个 watchdog 即可。

逐条证据（server/src/relay/http.rs）：

a) **短写**：write_all（L205–224）用 remaining 切片循环 poll_write，Ok(0) → WriteZero，
   Pending → 返回 Pending 并保留剩余切片；不是只调一次。通过。
b) **半关闭**：pump（L844–874）在读到 EOF/错误/静默后对 writer 调 poll_shutdown（L227–233）；
   serve（L773–790）把 client→upstream 放进独立任务、主任务跑 upstream→client，两侧各自 shutdown(WR)，
   另一方向继续；TLS close_notify 的 FIN 顺序正确。通过。
c) **背压/缓冲**：PUMP_BUFFER = 16 KiB（L36）每方向固定；头区上限 32 KiB + 单次 2 KiB 读块（L236–263），
   无无界 Vec 累积；写按 read 的块同步 await，天然背压。通过（连接面问题见 F1）。
d) **取消安全**：relay 内**没有 select!**（全模块 grep 无命中）；唯一被丢弃的 future 是
   timeout(idle, read_some) 与 timeout(idle, &mut body)：前者 Pending 时不用掉字节、Ready 时 timeout 不会
   触发，安全；后者之后显式 abort，任务 drop 即关 fd。无数据丢失、无 fd 泄漏。通过。

F1 [high] **写阻塞时 idle 计时失效（契约 §4.6 违背）**
- pump 的 idle 只包住 read_some（L856）；write_all（L860）**没有任何超时**。对端不收（TCP 零窗口）时，
  该 pump 永久阻塞；另一个方向若无读事件，也不会触发静默判定 → 连接永挂。
- 后果：连接对象与 limiter guard（service.rs spawn_connection）都不释放 → 单 uid 64 / 全局 256 被占满，
  新连接一律 503；fd 也一直占着。任一应用「连上不读」即可，device 级端口对全部 uid 开放。
- 要求：加**连接级 watchdog**（1 s tick，now - last ≥ idle 时 abort 两个方向），忠实实现「双向均无字节即关闭」；
  或退一步把 write_all 包进 timeout(idle, …)（略严于契约：另一方向活跃时也会被切）。仅换 copy_bidirectional
  无效——它同样没有写超时。

F2 [medium] **非 2xx 响应后仍进入双向泵**
- serve L767–790：写回响应头后**无条件**进入 pump。CONNECT 收到 407/4xx 后，客户端后续字节会被转发到
  上游代理连接（契约 §4.3 要求「非 2xx 原样回传后关闭」；§4.1 也要求一个请求-响应周期后关闭）。
- 要求：status 非 2xx → 写完响应直接 return（drop 两侧 socket），不 pump。

F3 [low-medium] **CONNECT 2xx 被注入 Connection: close**
- rewrite_response_head（L633–663）对所有响应追加 Connection: close，CONNECT 200 也带；契约 §4.3 要求
  「状态行与头原样回传（剥离 hop-by-hop）」。部分客户端可能据此立即关掉刚建立的隧道。
- 要求：CONNECT 2xx 不追加 Connection: close（或契约明确要求并验证主流客户端不受影响）。

F4 [low-medium] **上游连接预算可超 10 s**
- connect_upstream（L806–834）先 timeout(budget, lookup_host)，再对**每个**解析地址各 timeout(budget, connect)
  → 最坏 N×10 s；契约 §4.6 是「上游 TCP 连接（含 DNS）10 s」总预算。
- 要求：用 deadline（Instant + 剩余时间）贯穿 DNS 与多次 connect。

F5 [medium] **IO 边界无用例**
- http.rs 的 tests 只覆盖解析/头改写/头区上限；write_all 的短写、pump 的半关闭、写阻塞收尾、
  非 2xx 关闭都没有单测；relay_api 的 connect_form_tunnels_bytes 只覆盖正常往返。
- 要求补：① 每次只写 1 字节的 mock Writer → 全部字节写完；② Ok(0) → WriteZero；③ reader 先 Pending 再给数据
  → 不丢字节；④ 半关闭：一侧 EOF → 对侧收到 shutdown 且另一方向继续；⑤ 写阻塞 + 短 idle → 连接在 idle 内被收尾。

Nit：上游 socket 没设 TCP_NODELAY（service.rs 只对 client 设），TLS 握手往返会多付 Nagle 延迟。

OpenAPI：PENDING_OPENAPI 只豁免 1 个操作（POST /api/v1/egress/relay/restart），CONTRACT_OPERATIONS=30，
范围正确，按「临时豁免、t16 失效」处理；t10 不记 finding（t16 后必须移除并补 docs/API.md §5.30）。

## 9. t3 / t5 复核（t2 的 F1 已由 t5 提前修）

前提说明：复核期间工作树在动（http.rs 11:57 后又改了一轮，一度编不过），下列结论以我最后一次
可编译状态（relay 单测 40 passed、relay_api 19 passed）为准；t10 时须按最终树复跑。

### 9.1 一套 pump / 两方向 idle：是，但回归测试空转（medium）

- 只有 server/src/relay/pipe.rs 定义 read_some / write_all / shutdown_write / pump；http.rs 与 socks5.rs
  都是 use crate::relay::pipe::...，没有第二份实现。通过。
- pump：读 timeout(idle)、写 timeout(idle)、shutdown timeout(idle)，两方向都用同一个 pump。通过。
- **问题**：pump_stops_when_the_writer_never_drains 把对端 drop 了（let (_, mut writer_half) = tcp_pair()），
  写立即报错而不是阻塞；实测该用例 0.01s 结束（idle = 150ms），根本没走到写超时 —— 用例通过的理由错了。
  要求改成 mock：AsyncRead 恒有数据 + AsyncWrite 恒 Pending，断言 pump 在 idle 量级内返回；
  或真 TCP 保住对端不读并灌满缓冲。
- 语义偏差（低）：写超时按「单方向 idle」算，比契约「任一方向有字节即重置」更严（另一方向活跃时也会被切）。
  可接受，但契约应补一句，免得实现与文字对不上。

### 9.2 t3 两处取舍

a) 未变 listener 不暂停 accept：**接受**。但理由要改：暂停 accept 不会打断已建立连接（只影响新连接排队），
   所以「怕打断在连连接」不成立；真正成立的是「DENY 兜底 + ALLOW 空链全拒 + accept 期双检」。
   实现上 kept 判定含 port/scope/upstream/allowed_uids/listening 五项，kept 的端口仍在 P_deny 与 ALLOW 里，
   窗口内新连接落到 DENY → 全拒；restricted 端口另有 accept 期双检。安全上无缺口。
   两个补充：① 未变 listener 在 ALLOW 重建期间会短暂全拒（可用性抖动，写文档）；
   ② apply 失败时未变 app listener 仍 listening（规则可能已空），唯一防线是双检——与 §5.5「应用级失败不监听」
   的字面不一致，建议契约明确「未变 listener 不因本轮失败关闭」，否则 t10 按字面记偏差。
b) 设备级端口放行全部 uid：**接受**（K3 / §0 D2 前提）。代价（任意应用可借凭据出网）已在 P6/P8 写明，
   有 per-uid 64 限流与告警，无异议。

### 9.3 t5 SOCKS5 认证失败映射

REP=0x01 + 共用 egress_relay_upstream_unreachable(reason=auth_failed)，与 HTTP 407 原样回传 + 同一警告
**口径一致**：HTTP 侧应用能看到 407；SOCKS 本地腿不做认证、RFC1928 没有「代理认证失败」这个 REP，
0x01 是唯一合理映射；状态面共用同一警告码与 reason，API/WebUI 不分叉。integration 用例
socks5_upstream_auth_failure_maps_to_rep_01_and_warns 已覆盖。接受。

### 9.4 F2–F5 / nodelay 现状（截至本次复跑）

- F2 已修（http.rs：CONNECT 非 2xx 写回后 shutdown_write + return）；**无用例**。
- F3 已修（rewrite_response_head(head, append_close) + connect_ok）；单测只覆盖 append_close=true，
  **缺 append_close=false / CONNECT 2xx 无 close 的用例**。
- F4 **只修了 http.rs**（Instant deadline + connect_within/remaining_budget），socks5.rs 的 connect_upstream
  仍是「每地址各 10s」→ 两份实现已漂移，必须一起修（建议抽到 pipe 或公共模块）。
- F5 部分：pipe 两个用例（其一空转）、无 1 字节短写、无半关闭、无 F2/F3 用例。
- nodelay：仍未加（service.rs 只对 client 设）。
- t5 不改 Cargo.toml、继续用手写 pump：**接受**（与 t2 结论一致）；但 connect_upstream 必须合并，
  否则每次修上游预算都要改两处（这次已经漏了一处）。
