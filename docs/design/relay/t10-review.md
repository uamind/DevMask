# t10 评审：带认证中继（开放代理 / 凭据 / 端口 / fail-open）

- 评审人：reviewer；被审对象：t2 relay、t3 uid 防火墙、t5 SOCKS5、t4 注入改造、t17/t21 加固、t22 真机复验
- 结论：**needs_revision**（1 个 high、6 个 medium、4 个 low）。开放代理主路径（三链 fail-closed、bind 断言、重建零空窗）成立，
  但「accept 期第二道防线」依赖一个不会随 live 状态更新的布尔量，在 uid 归属失败的设备上会把应用级端口变成开放代理。
- 证据：cargo test -p device_faker_server（全 target ok，relay_api 20）、cargo test -p device_faker_module_tests（60 passed）、
  docs/design/relay/t22-acceptance.md 与其 evidence-t22/。

## 1. 七个维度逐条结论

| # | 维度 | 结论 | 关键证据 |
| --- | --- | --- | --- |
| 1 | 开放代理 | **基本成立，有 3 个缺口**（F1 高；F2、F11 中） | 三链结构/顺序与 -S 语义比较（firewall.rs:246-298、113-127）；bind 只 127.0.0.1 + local_addr 复核（service.rs bind_loopback）；t22 #13 150 次探测 0 命中、bind 面五项通过 |
| 2 | 凭据 | 成立（日志/API/错误 details 无明文，内存含 zeroize） | secret.rs:1-80、dto.rs 无 username/password 字段、t22 凭据扫描 5/5 命中 0；F9 为内存卫生小项 |
| 3 | 端口 | 逻辑正确，**端到端证据缺**（F6） | port.rs 保留/哈希/撞车单测（196-274）；relay_api 有 initial / 幂等空 portChanges，无撞车重分配与 DELETE 释放用例 |
| 4 | fail-open | 成立（设备级 bind 失败才清 :0；运行期 listener 死不动 settings；自愈先 relay 后 settings） | egress/mod.rs:123-154；app.rs:371-374；state.rs:214-300；relay warnings 可见（dto.rs:239-265） |
| 5 | 协议 | 成立（CONNECT/absolute、头覆盖、SOCKS5 ATYP/RFC1929、非 2xx 关闭、CONNECT 2xx 无 close） | http.rs:721-762、565-620；socks5.rs:233-442；relay_api 20 例覆盖 |
| 6 | 可检出性文档 | **不成立**（F5：英文文档仍说认证代理不可用；API 文档损坏） | docs/en/CONFIG.md:341、docs/en/SERVER.md:69-71、docs/API.md:1127 |
| 7 | 测试覆盖 | 大体成立，**1 个 high 缺可失败用例**（F1），另有 F6/F10 | 全部测试绿；F1 的「flush 后 unknown uid」无用例；fixture 基础设施见 F10 |

## 2. Findings

### F1 [high] firewall_applied 不会随 live 状态更新 → 手工 -F 后应用级端口变开放代理

- 位置：server/src/relay/service.rs:146（字段）、:660-674（只在 apply/clear 时 store）、:1046-1065（accept 期策略读取）；
  server/src/relay/firewall.rs:184-210（sample 只更新 FirewallState，不更新该 atomic）。
- 问题：accept 期的分层策略是「uid 未知 + firewall_enforced=true → 放行」。这个 true 只来自**最近一次 apply**，
  GET 的 -S 采样（service.rs:251-258）不会同步它。于是：iptables -F DF_EGRESS_RLY_ALLOW / _DENY（手工、或其它 root
  防火墙应用重载规则集）之后，GET 会报 applied=false，但 accept 期仍认为「内核已按 uid 过滤」→ 在 uid 归属失败的设备
  （t22 O1 实测本机就是）上，任何应用连应用级端口都会被放行 → 拿到带凭据的隧道。契约 §4.6 第 5 条明确说双检要管
  「规则被外部清掉」的场景，这条没有兑现。
- 修复要求：
  1. sample 的结果同步写回 firewall_applied（GET 一次即可把陈旧值纠正）；
  2. uid 未知且 flag=true 时做**限频 re-sample**（例如每秒最多一次、加超时与降级），以采样结果决定放行/关闭，
     不能永久信任旧值；
  3. 补可失败用例：apply 成功 → 用假 iptables 清空两链 → GET（applied=false）→ 以「uid 未知」连接应用级端口
     → 必须被 close；当前实现会放行；
  4. 文档写明「该放行只在内核规则经 -S 确认生效期间成立」。

### F2 [medium] 设备级默认指向 socks5 时，暴露一个没人使用、对全部 uid 开放的 SOCKS5 listener

- 位置：server/src/relay/service.rs:382-383、:409（scope 只看是否 default，listener kind 跟 type 走）；
  server/src/egress/mod.rs:135-137（settings 只对 http 类型写 127.0.0.1）；server/src/relay/service.rs:326-339（relayActive 只看 listening）。
- 问题：egress.default 可以指向 socks5 定义（validate_default 不校验类型）。此时 settings 走「上游 host:port」
  （系统代理只能说 HTTP），但中继仍会在 base_port 起一个 SOCKS5 listener，且设备级规则放行**全部 uid**
  （firewall.rs render_accept 设备分支无 uid 条件）。结果是任何应用都能连 127.0.0.1:17800 用这份凭据出网；
  而 API 同时报 relayActive=true，用户/排障看不出 settings 根本没走它。D2 已接受「设备级端口全部 uid」，
  但那条前提是给「系统代理指向它」的场景用的，这里是未声明的额外入口。
- 修复要求：设备级 scope 只对 http 类型成立；socks5 默认不建 device-scope listener（若同时被应用引用，按 app scope +
  uid 白名单建）；relayActive 对「settings 未指向」的定义应为 false；PUT 给明确 warning（或 422），文档同步
  （docs/CONFIG.md:412 的旧表述一并改）。补用例：default=socks5 → 无 device-scope listener、base_port 不监听、settings=上游。

### F3 [medium] accept 期「uid 未知 + applied=true 放行」未进契约/文档；firewall=false 的排障语义被改变

- 位置：server/src/relay/service.rs:1046-1065；契约 docs/plans/relay-contract.md:519 仍写「归属失败一律 close」；
  docs/CONFIG.md:305 只说「解析不到 uid 时该端口不监听」（规则期），docs/SERVER.md:379 只说「未授权 uid 仍拒绝」。
- 问题：实现与契约文字冲突；且 firewall=false + uid 归属失败的 ROM（本机）上，应用级中继会**全部不可用**
  （t22 已实测 SocketException），而文档把这个开关描述成排障可用。
- 修复要求：契约 §4.6/§5.3/§5.5 增一条两阶段策略（规则期不监听；accept 期 unknown + applied=true 放行、否则 close，
  并写出 root-trust 前提与残留风险）；zh/en 文档同步；补「applied=false / firewall=false 时 unknown uid 被拒」的单测。

### F4 [medium] applied 误报窗口（t22 O2 的根因）

- 位置：server/src/relay/service.rs:214-242（rule_spec 用当前 model + 当前 listeners 算期望）、:251-258（view 采样）、
  api/egress.rs:942-1013（prepare → 写文件 → activate）、firewall.rs:166-210（RwLock 只覆盖 apply 与 sample）。
- 问题：prepare 会先把将被关闭的 listener 从 state.listeners 移除、再 bind 新 paused，文件随后写入、最后才 apply。
  在这个序列的任何时刻，GET 都可能读到「新 model + 旧 listeners」并拿它和「旧规则」比较 → applied=false(state_mismatch)，
  尽管 -S 与上一份成功态完全一致。RwLock 挡不住，因为期望态是在锁外算的。这正是 O2 看到的一次性 applied=false。
- 修复要求：把「最近一次成功 apply 的 RuleSpec」存进 RelayService（与 firewall 同一把锁），view 用它比对 applied/计数，
  需要表达「配置已变、尚未 apply」时另给字段；或者把 prepare → 写文件 → activate 与 view 的期望计算放进同一把读写锁。
  补并发用例：PUT/restart 循环期间 GET 循环，断言「-S 未变时不得出现 applied=false」。

### F5 [medium] 文档与实现矛盾 / 文档损坏

- 位置：docs/en/CONFIG.md:341（Authenticated proxies do not work in this release…wait for the P2 in-device authenticated relay）、
  docs/en/SERVER.md:69-71（同一说法）、docs/API.md:1127（reason 列表被写成 @@timeout / @@auth_failed / @@protocol_error）、
  docs/CONFIG.md:412（「设备级…不承载认证…返回 egress_credentials_ignored」——与 K3 相反）。
- 修复要求：英文两份按 zh 版本同步（认证代理经中继可用、P8/P9、fail-closed、uid 分层策略）；修 API 文档 reason 列表；
  确认 tr 文档策略（仓库里 tr 只有 README，若不再维护三语需在 docs/README 声明）。

### F6 [medium] 端口重分配/释放缺端到端证据

- 位置：server/tests/relay_api.rs（20 例：initial portChanges、二次 restart 空 portChanges；**无撞车重分配、无 DELETE 释放**）；
  server/src/relay/port.rs:196-274（只有单元级 hash/撞车/保留）。
- 问题：契约 §10.4 #10/#11/#17 是验收项；t9 被阻塞时未做 #11，t22 也没补。代码路径（service.rs:410-466、589-609）看着正确，
  但没有可失败用例证明「手改撞车 → 重分配 + portChanges(reason=collision) + restartRequiredApps 非空 + 文件写回」，
  也没有「DELETE 后 local_port 释放、listener 关闭、链清理」的用例。
- 修复要求：补 2 个集成用例，并在 t20/t22 真机补 #10/#11。

### F7 [low] 契约「暂停全部 listener」与实现不一致

- 位置：docs/plans/relay-contract.md:153、:677（要求暂停全部、含未变）；service.rs:410-466（未变 listener 不暂停、不关闭）。
- 说明：安全上无缺口（DENY 兜底 + 双检；补充 B/C 已写），但第 2 步文字没改。
- 修复：契约明确「暂停本轮需要重建的 listener；未变 listener 可不暂停（pause 不是安全机制）」。

### F8 [low] prune_deny 早于 accept 恢复执行

- 位置：server/src/relay/firewall.rs:302（apply 内、activate 的 spawn_accept 之前）；契约 §5.4 第 10 步要求 accept 恢复之后。
- 说明：被删端口的 listener 已在 prepare 关闭，删 DENY 不产生裸奔；属文字与实现的顺序差异。
- 修复：契约注明该等价性，或调整顺序。

### F9 [low] 凭据派生副本未清零

- 位置：http.rs connect_head/absolute_head（565、611 拼入 base64）；socks5.rs:261-270（认证缓冲已清零）。
- 说明：Secret 本体 Drop 清零，但拼进请求头的 Vec/String 未 zeroize；仅内存卫生，不构成日志/接口泄漏。
- 修复：写为已知限制，或对 head 缓冲清零。

### F10 [low] 测试基础设施仍有「自证」风险

- 位置：server/src/commands.rs:174-217（FixtureRunner 查表，未注册 → Unavailable）；relay_api 自带假 iptables（t21 已改 device-style）。
- 说明：t21 的 golden 形态覆盖了 -S 解析，但共享 runner 不会校验「真机上 -S 会补 -m tcp/-m owner、重排选项」这类差异。
- 修复：小任务把 device-style -S 的生成/解析抽公共测试支持，避免未来用例用回显式假件自证。

### F11 [medium] 端口抢占风险（本地应用劫持）

- 位置：service.rs:464-522（先关旧、再 bind 新；重试上限 REBIND_BACKOFF）；http.rs/socks5.rs 数据面无对端身份校验。
- 问题：listener 关闭/重启的毫秒窗口里，任何应用都可以 bind 127.0.0.1:178xx；一旦抢到，服务端重绑失败，
  但 settings/注入值仍指向该端口 → 该应用的 HTTP 流量与 CONNECT 目标会落到抢占者手里（HTTP 可篡改，HTTPS 会证书错）。
  端口变化后未重启的旧应用同样会连到被释放的旧端口。
- 修复要求：绑定失败（addr_in_use）时的重试窗口拉长并告警；文档把「本地端口被同机应用抢占」写进已知限制；
  至少在 restartRequiredApps 之外提示旧端口已释放。

## 3. 对 captain 三个问题的答复

1. **O1（uid 归属 / SO_PEERCRED）**：同意你的判断——SO_PEERCRED 只对 AF_UNIX 有效，TCP 会 EOPNOTSUPP；严格路径只有
   NETLINK_INET_DIAG（root + SELinux），列为 P2/已知限制，本轮不修。但前提是 F1 必须修：分层策略的「放行」不能建立在
   一个永不更新的缓存布尔量上。t22 已证明「applied=true 放行、firewall=false 关」两种态，补单测即可。
2. **O2**：根因是 F4（期望态在锁外用「新 model + 新 listeners」计算，规则还是旧的）。不是纯采样窗口，RwLock 也挡不住。
   修复建议见 F4，并补并发用例。
3. **FixtureRunner**：不建议作为 t10 阻塞项；建议单开一条小任务（F10），把 device-style -S 的生成/解析抽公共测试支持，
   并让 relay 相关假件对未注册的 iptables 命令显式失败。

## 4. 已执行的核对（可复跑）

    cargo test -p device_faker_server            # 全 target ok（relay_api 20 例）
    cargo test -p device_faker_module_tests      # 60 passed
    cargo test -p device_faker_server --lib relay --quiet
    grep -rn 'firewall_applied' server/src/relay/service.rs     # 只有 apply/clear 两处 store，无 live 同步
    grep -rn 'proxy.kind != ProxyType::Http' server/src/egress/mod.rs   # 设备级 socks5 不走 settings，但 listener 仍建
    grep -rn 'uid_unresolved_firewall_enforced' docs/plans docs/design  # 契约与 zh 文档均未收录该策略
