# t25 第二轮评审：带认证中继（round 2）

- 对象：t24 修复 + t26 文档 + t28 收尾；结论：**needs_revision**（2 high / 2 medium）。
- 第一轮 high（F1）实现已落地且逻辑正确，但**没有任何可失败用例**；t28 把「设备级 socks5 不建 listener」修进代码后，
  t26 写的文档/契约/CHANGELOG 反而在 8 处继续声明「base_port 上有一个全 uid 的 SOCKS5 服务端」，文档与实现相反。
- 验证：cargo test -p device_faker_server（全 target ok，relay_api 22 例）、cargo test -p device_faker_module_tests（60 passed）。

## 1. 第一轮 findings 复核

| 项 | 状态 | 证据 |
| --- | --- | --- |
| F1 high（firewall_applied 陈旧） | **实现已修，缺可失败用例（H2）** | service.rs:254-267 采样回写旗标；firewall.rs:583-650 Revalidator（≤1/s、按 applied_spec 复核）；service.rs:1085-1100 accept 期先复核再放行 |
| F2 medium（设备级 socks5 全 uid listener） | **代码已修，文档反向（H1）** | service.rs:394-396 is_device 要求 type=http；但 contract:343、P10:1138、CONFIG:291/417、SERVER:382、API:1109、en/CONFIG:343/349、en/CHANGELOG:25、CHANGELOG:25 仍声明暴露存在 |
| F3 medium（分层策略未进契约/文档） | 已修 | contract §4.6/§4.6.1、CONFIG:418、SERVER:339/380、en/CONFIG:342 |
| F4 medium（applied 误报窗口） | **部分修复（M1）** | applied_spec 已存（service.rs:148/678）但 view 仍用 rule_spec(model)（service.rs:254-258）；PUT 有文件锁（api/egress.rs:847-1013）故窗口收窄到 restart/startup；无并发用例 |
| F5 medium（en 文档/API 损坏） | 已修（除 H1 的 socks5 段） | en/CONFIG:341、en/SERVER、API reason 列表已无 @@ |
| F6 medium（端口端到端证据） | **部分修复（M2）** | 新增 delete_releases_the_local_port_and_reallocation_listens、device_bind_failure_never_leaves_settings_on_that_port；仍无「手改撞车 → 重分配 + portChanges(collision) + restartRequiredApps」用例 |
| F7/F8 low（契约文字） | 已修 | contract:698/706（pause 只限需重建、prune 等价性） |
| F9 low（头缓冲未清零） | 已修 | http.rs:696-700（写入后与失败路径均清零） |
| F10 low（FixtureRunner 自证） | 已修 | commands.rs:180-232 with_iptables + iptables_device_style（真机格式），未注册子命令显式失败 |
| F11 medium（端口劫持） | 已按文档接受 | contract §5.4.1、P11:1139、CONFIG/SERVER/API；实现沿用 1s/5s/15s 退避，文档写明残留风险 |

## 2. 第二轮 findings

### H1 [high] 文档/契约/CHANGELOG 仍声明一个已被代码移除的设备级 SOCKS5 全 uid 暴露；且 socks5 默认无 warning

- 位置（声明方）：docs/plans/relay-contract.md:343、:1138（P10）；docs/CONFIG.md:291、:417；docs/SERVER.md:382；
  docs/API.md:1109；docs/en/CONFIG.md:343、:349；docs/en/CHANGELOG.md:25；docs/CHANGELOG.md:25。
- 位置（实现）：server/src/relay/service.rs:394-396 —— is_device 要求 ProxyType::Http，socks5 默认不会得到 device scope；
  未被 apps 引用时 referenced=false → **根本不建 listener**；被引用时是 app scope + uid 白名单。
  settings 仍按 server/src/egress/mod.rs:135-137 写上游 host:port（系统代理把它当 HTTP 代理 → 不可用）。
- 问题：八处文档（含 CHANGELOG 与 P10 风险表）把「全 uid SOCKS5 服务端」当成现存事实并给出规避建议，与实际相反；
  同时实现侧对「default 指向 socks5」没有任何 warning（PUT 静默成功、设备级代理不可用），第一轮 F2 要求的
  「明确 warning 或 422」未落地。可检出性维度因此仍不成立。
- 修复要求：二选一并统一全部八处：
  a) 保持代码（推荐）：文档改为「设备级中继只支持 http；socks5 默认不建 device listener、不参与中继，
     settings 仍写上游（不可用）」；删除 P10 或改写为「已移除」；PUT 对 socks5 default 给 warning（不加新错误码，
     可复用 egress_credentials_ignored/egress_settings_mismatch 语义或新增 details），并在 CONFIG/SERVER/API 写明；
  b) 恢复代码（不推荐）：重建 device-scope socks5 listener，与现文档一致——但这正是第一轮 F2 要消除的暴露。
  另补一条用例：default=socks5 → 无 device-scope listener、base_port 不监听、有 warning、settings=上游。

### H2 [high] F1 的安全控制没有可失败用例，Revalidator 也未单测

- 位置：server/src/relay/service.rs:1085-1100（accept 期复核）、server/src/relay/firewall.rs:583-650（Revalidator）；
  server/tests/relay_api.rs:1556-1632（只验证 GET false + restart 修复，**flush 后不发起连接**）、
  :1397-1482（只验证 applied=true 时 unknown uid 放行）。
- 问题：第一轮 F1 是本次增量唯一的 high；修复引入了「限频 1/s 复核 + applied_spec 快照 + 原子旗标」三段新逻辑，
  但没有任何测试证明「手工 -F 后，未知 uid 的新连接在 ≤1s 内被 close」。当前只有代码阅读证据；一旦后续重构
  Revalidator 或旗标同步，回归不会被发现（正是第一轮 F5 假绿教训）。
- 修复要求：至少两条可失败用例：
  1) 集成：app 级端口 + applied=true（假 iptables）→ 连接成功（unknown uid 放行）→ flush ALLOW → GET（applied=false）
     → 再连接必须被 close（不返回任何协议字节）；可选：flush 后不 GET，等待 >1s 再连接，验证 Revalidator 自行复核并 close。
  2) 单测：Revalidator 在 min_interval 内返回缓存值、超过间隔会重新 sample、sample 失败时旗标置 false 并返回 false。
  同时把「≤1s 回到 fail-closed」写进测试断言（用可注入时钟或短 min_interval 构造）。

### M1 [medium] applied 误报窗口仍在（已收窄到 restart/startup）

- 位置：server/src/relay/service.rs:254-258（view 用 rule_spec(model) 算期望）、:426-482（prepare 先改 state.listeners）、
  firewall.rs:184-210（锁只覆盖 apply/sample）。
- 问题：PUT 侧有 config/egress 排他锁（api/egress.rs:847-1013），GET 不会看到中段；但 POST /egress/relay/restart
  与启动 reconcile 不持这把锁：当某个 listener 未在监听（要重建）时，prepare 先把它移出 state，旧规则仍在，
  此时 GET 会误报 applied=false(state_mismatch)。t22 O2 的一次性现象与此吻合。
- 修复要求：view 比对「最近一次成功 apply 的 RuleSpec」（applied_spec 已在手）或把 restart/startup 的重建
  与期望计算纳入同一把读写锁；补并发用例（restart 循环 + GET 循环，断言 -S 未变时不得出现 false）。

### M2 [medium] 撞车重分配/restartRequiredApps 仍无端到端用例

- 位置：server/src/relay/service.rs:961（reason=collision）、port.rs:243（仅单元级 candidate 探测）；
  server/tests/relay_api.rs 无「手改两个定义 local_port 相同 → restart → 其一重分配」用例。
- 修复要求：补集成用例断言 portChanges(reason=collision)、端口写回文件、restartRequiredApps 列出引用方、旧端口不再监听。

## 3. 七维度结论（第二轮）

1. 开放代理：**成立**（三链 fail-closed、-S 语义比较、bind 断言、重建零空窗、accept 期复核含 live 复核）；剩余 M1 是观测误报，
   不是放行缺口；H1 是文档错报暴露（实际更安全）。
2. 凭据：**成立**（Secret zeroize、头缓冲写入后清零 http.rs:696-700、日志/API/错误 details 无明文；t22 扫描 5/5 命中 0）。
3. 端口：**基本成立**（DELETE 释放 + addr_in_use 用例已补；撞车重分配见 M2）。
4. fail-open：**成立**（bind 失败才清 :0、运行期 listener 死不动 settings、先 relay 后 settings；startup 顺序 app.rs:371-374）。
5. 协议：**成立**（http.rs:721-762、socks5.rs:233-442；22 例集成全绿）。
6. 可检出性文档：**不成立**（H1）。
7. 测试覆盖：**不成立**（H2；M2）。

## 4. 已执行核对

    cargo test -p device_faker_server            # 全 target ok（relay_api 22）
    cargo test -p device_faker_module_tests      # 60 passed
    grep -rn 'is_device' -A 3 server/src/relay/service.rs          # 设备级要求 http（H1 代码侧）
    grep -rn 'SOCKS5 服务端' docs/plans docs/*.md docs/en/*.md     # 文档仍声明暴露（H1 文档侧）
    grep -rn 'Revalidator\\|revalidate' server/tests               # 无测试（H2）
