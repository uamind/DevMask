# t30 第三轮评审：带认证中继（round 3）

- 对象：t29 修复 + t31 文档 + t28 收尾；结论：**needs_revision**（1 high / 1 medium / 2 low）。
- 第二轮 high 均已修复并有可失败用例；本轮唯一 high 是 t29 新引入的 PUT 顺序问题：**重建前先把 settings 写成直连**。
- 验证：cargo test -p device_faker_server 连跑 3 次全 target ok（lib 184、relay_api 29）、cargo test -p device_faker_module_tests 60 passed。

## 1. 第二轮 findings 复核

| 项 | 状态 | 证据 |
| --- | --- | --- |
| H1（文档声明设备级 socks5 全 uid listener） | 已修 | service.rs:394-396 只 http；PUT 422 用例 socks5_device_default_is_rejected_by_put；旧配置黑洞用例 legacy_socks5_default_fails_closed_to_blackhole；contract:343/1139、CONFIG:417、SERVER:383、API:1109/1197、en/CHANGELOG 均已改口径 |
| H2（F1 无可失败用例） | 已修 | manual_flush_makes_unknown_uid_connections_fail_closed（连上 → flush ALLOW → 等 1.1s → 再连必须空）；firewall.rs:971/987 Revalidator 单测（无 spec 回退、限频 ≤1/s、sample 失败 false） |
| M1（applied 假窗口） | 已修 | service.rs:254-266 view 优先 applied_spec；get_during_restart_never_reports_a_false_applied_window（3 restart × 8 GET） |
| M2（撞车重分配用例） | 已修 | duplicate_local_ports_are_reallocated_with_collision_reason、colliding_port_is_reallocated_and_reports_changes |
| t28 能力保留 | 是 | socks5 全部 app scope 用例、DELETE 释放、addr_in_use、http.rs:696-700 头缓冲清零、FixtureRunner with_iptables；put_actually_writes_the_egress_file 另补 |

## 2. 本轮 findings

### H1 [high] PUT 在 relay 重建之前先写 settings，会把设备代理短暂写成直连（fail-open）

- 位置：server/src/api/egress.rs:1036-1052（第一次 apply，注释自述「重建前的 device_status」）、:1069-1084（释放锁后 prepare/activate）、:1086-1096（第二次 apply 修回）。
- 问题：PUT 在 relay 重建前用旧 device_status 算期望。当设备级 listener 还没起来（首次把 default 变成带凭据定义、
  或 listener 刚死）时 expected=Direct → settings put global http_proxy :0，直到第二次 apply 才写回
  127.0.0.1:<base_port>。窗口包含 prepare_from_disk（bind + pkg@uid 解析，单包最长 5s）与 iptables apply，
  不是毫秒级；期间全设备尊重系统代理的应用**静默直连**。若 PUT 前的状态是上游代理（无凭据 → 加凭据），
  这段就是实打实的出口泄漏；而契约 §7.1 / app.rs:371-374 明确要求「先 relay 后 settings」。
- 证据：代码路径明确；第二次 apply 的注释（:1086-1087）承认「首次 PUT 时它还不知道新 listener 已经起来，
  会把 settings 写回直连」，但没有消除第一次写入。
- 修复要求：把 settings 的读回/补写统一移到重建之后（单次 apply，并把 request.exclusionList 一并带上，
  现在第二次 apply 传的是 None）；或在重建前用「预期重建成功」的状态计算 expected（仅 relay→direct 方向允许）。
  补可失败用例：fake runner 记录 settings 写入序列，断言「把 default 变成带凭据定义」的 PUT 过程中
  不得出现 settings put global http_proxy :0（或任何非最终值）。

### M1 [medium] 黑洞条件与 warning 条件不一致 → 部分旧配置静默黑洞化

- 位置：server/src/egress/mod.rs:133-151（任何非 http default 一律 expected=127.0.0.1:9）、
  server/src/api/egress.rs:226-250（warning 只在 relay_enabled && socks5 && has_credentials 时给）。
- 问题：黑洞对「无凭据 socks5 default」和「[relay].enabled=false 的 socks5 default」同样生效（settings=127.0.0.1:9，
  全设备 HTTP 断），但这两种情况都**不产生** egress_relay_device_socks5_unsupported：前者被 has_credentials 挡掉，
  后者被 relay_enabled 挡掉。文档自相矛盾：contract:343 / CONFIG:417 / API:1109 说「旧配置黑洞 + warning」，
  而 contract:988 / API:1132 又写「指向带凭据的 socks5」；contract:999 的静默条件说 enabled=false 不产生任何 relay warning。
  结果：升级后设备级全断，却没有任何 warning，只有 readback=127.0.0.1:9。
- 修复要求：让 warning 条件与黑洞条件一致（任何 socks5 default 都告警；enabled=false 时至少也给这条），
  或把黑洞限制在「本来会走中继」的带凭据场景；同步 contract:988/999、API:1132 的措辞，补两个用例
  （无凭据 socks5 default、enabled=false + socks5 default）。

### L1 [low] PUT 只改定义 type 时绕过 422

- 位置：server/src/api/egress.rs:918（validate_default_scope(&merged, &request)）、server/src/egress/request.rs:427-445（仅当 request.default 为 Some 才检查）。
- 问题：如果 default 已经在配置里，PUT 只把该定义的 type 从 http 改成 socks5（请求里不带 default），校验直接返回空，
  PUT 200 → merged default 是 socks5 → settings 黑洞 + warning（有凭据时）。与文档「PUT 直接把 socks5 写成 default → 422」不一致。
- 修复要求：按 merged 模型的有效 default 校验类型（不依赖请求是否带 default），与既有 default 缺失/__DIRECT__ 校验合并。

### L2 [low] 契约残留「4 个警告码」

- 位置：docs/plans/relay-contract.md:30（只新增 4 个扁平警告码）、:1211（追加 4 个 relay 码）。
- 修复：改为 5 个（t31 已把其它处改成 5，这两处漏了）。

## 3. 七维度结论

1. 开放代理：**成立**（三链 fail-closed、-S 语义比较、bind 断言、重建零空窗、accept 期复核 + ≤1/s 再验证、设备级 socks5 不再建 listener）。
2. 凭据：**成立**（Secret zeroize、请求头写入后清零、日志/API/错误 details 无明文、t22 扫描 5/5）。
3. 端口：**成立**（DELETE 释放、addr_in_use、撞车重分配 + restartRequiredApps 均有端到端用例）。
4. fail-open：**不成立**（H1：PUT 重建前写直连；其余路径符合 §7.1）。
5. 协议：**成立**（http.rs:721-762、socks5.rs；29 例集成全绿）。
6. 可检出性文档：**基本成立**（M1 的两处自相矛盾、L2 的计数残留）。
7. 测试覆盖：**基本成立**（第二轮两个 high 都补了可失败用例；H1 需要新用例）。

## 4. 已执行核对

    cargo test -p device_faker_server            # 连跑 3 次全 target ok（lib 184、relay_api 29）
    cargo test -p device_faker_module_tests      # 60 passed
    grep -n 'settings 的读回/补写与文件是否变化无关' server/src/api/egress.rs   # :1037 首次 apply
    grep -n 'relay 重建完再补一次 settings' server/src/api/egress.rs            # :1086 第二次 apply
    grep -n 'device_socks5_warning' -A 12 server/src/api/egress.rs              # 条件含 relay_enabled && has_credentials
    grep -n '4 个扁平警告码\\|追加 4 个 relay' docs/plans/relay-contract.md     # L2
