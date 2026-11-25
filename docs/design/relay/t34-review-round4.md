# t34 复核：H1 直连窗口 / M1 黑洞与 warning / L1 校验 / L2 文档

- 结论：**needs_revision**（1 high / 1 low）。实现与文档均已闭环，但 H1 的「写入序列」用例**不能失败在旧实现上**，
  属于假绿；需要改成真正能复现直连窗口的场景。
- 验证：cargo test -p devmask_server 全 target ok（lib 184、relay_api 33）、module-tests 60 passed。

## 1. H1（PUT 直连窗口）

**实现：通过。** put_egress（server/src/api/egress.rs:846-1122）只有一处 settings apply（:1070），位于
prepare_from_disk + activate（:1045-1047）之后；expected 用重建后的 device_status 计算（:1062-1066）；
exclusionList 在这次唯一的 apply 里按请求语义传递（:1071-1079）。旧的双写已删除。

**用例：不通过（假绿）。** put_never_writes_direct_proxy_while_activating_relay（server/tests/relay_api.rs:1709-1752）
从一个 **settings 未设置** 的 fixture 出发（SettingsSim::default → proxy=None，relay_api.rs:291-300），
第一次 PUT 就把 default 设成带凭据定义。旧实现第一次 apply 的 expected=Direct，而
SystemProxy::apply 在 readback 与期望一致时**不写**（server/src/egress/system.rs:246-249；Direct 与 Unset 匹配，
system.rs:296-300）→ 旧实现根本不会产生 http_proxy :0，断言全部通过。只有先写一个**上游代理**（无凭据 http default
→ settings=上游 host:port）再在第二次 PUT 里加凭据，旧实现的第一次 apply 才会把 http_proxy 从上游改成 :0，
新实现才可能被抓到。

修复要求：把用例改成「两步 PUT」或给 SettingsSim 加 with_proxy 初值：
1) PUT 一个无凭据 http default → 断言 settings=上游 host:port（记录基线）；
2) 再 PUT 同一个定义加 username/password（激活 relay）→ 断言调用序列中没有任何 http_proxy :0，
   http_proxy 的写入恰好 1 次且值为 127.0.0.1:<base_port>，exclusionList 仍在同一次 apply 里。
这样用例才能在旧实现上失败。顺带建议断言 apply 次数（当前仅靠代码审查确认「只有一次 apply」）。

## 2. M1（黑洞与 warning 条件）

**通过。** expected_for_relay（server/src/egress/mod.rs:130-155）：非 http default 且 relay.enabled=true → 黑洞
127.0.0.1:9；enabled=false → Expected::Direct（不写黑洞）。device_socks5_warning（server/src/api/egress.rs:226-250）
只看「有效 default 是否非 http」，不再看 enabled / 凭据。两个用例：
socks5_default_without_credentials_still_warns_and_blackholes（relay_api.rs:1755-1818，无凭据 → warning + 黑洞）、
socks5_default_with_relay_disabled_warns_and_goes_direct（:1820-1867，enabled=false → warning + 直连、readback=null）。
二者在旧实现上都会失败（旧 warning 需要 enabled+凭据），有判别力。文档三句口径（PUT 422 / enabled=true 黑洞 /
enabled=false 直连 + 两种都 warning）在 contract:343/988/998、CONFIG:417、API:1109/1110/1132/1197、SERVER:343/383、
en/CONFIG:343/349、en/SERVER:75 已统一。

## 3. L1（merged default 校验）

**通过。** validate_default_scope 改为 (merged model, 有效 default 名)（server/src/egress/request.rs:427-445），
调用点用 Config::from_toml(新配置) 的 device_default_name（api/egress.rs:963-969）。
changing_existing_default_to_socks5_is_rejected（relay_api.rs:1869+）覆盖「请求不带 default、只把既有 default 定义
type 改成 socks5」→ 422。旧实现会 200，用例有判别力。

## 4. L2（文档计数）

**通过。** 全仓「4 个警告码 / 4 个扁平 / 4 个 relay」0 命中（docs/design 下的历史 memo 除外）；
contract:30/:1211 与 §6.4 均为 5；§6.4 静默条件已把 device_socks5 列为 enabled=false 的例外。

## 5. 回归核对

| 项 | 结果 |
| --- | --- |
| 第二轮 H2（F1 可失败用例） | 在：manual_flush_makes_unknown_uid_connections_fail_closed（flush → 1.1s → 再连必空）+ Revalidator 单测 |
| 第二轮 M1（applied 假窗口） | 在：view 用 applied_spec；get_during_restart_never_reports_a_false_applied_window |
| 第二轮 M2（撞车重分配） | 在：duplicate_local_ports_are_reallocated_with_collision_reason、colliding_port_is_reallocated_and_reports_changes |
| t28 能力 | 在：socks5 app scope、delete_releases、device_bind_failure、http.rs:696-700 头缓冲清零、FixtureRunner with_iptables |
| 测试 | server 全 target ok（lib 184、relay_api 33，含 4 个新用例）；module 60 passed |

## 6. 本轮 findings

### R1 [high] H1 的回归用例不具判别力（旧实现也能通过）

- 位置：server/tests/relay_api.rs:1709-1752；server/tests/relay_api.rs:291-300（SettingsSim 初始 proxy=None）；
  server/src/egress/system.rs:246-249 / 296-300（期望一致时不写）。
- 问题：见 §1。用例从「无代理」出发，旧实现的首次 apply 是 no-op，因此断言「无 :0」在旧实现上同样通过；
  该用例无法在 H1 回归时报警，验收项「写入序列证据」不成立。
- 修复要求：见 §1 的两步 PUT 方案，并断言 http_proxy 写入次数 = 1。

### R2 [low] expected_for_relay 的文档注释仍是旧口径

- 位置：server/src/egress/mod.rs:113-141（表行「命中定义有凭据但本轮不参与中继（SOCKS5 待 t5）」、
  「带凭据的 socks5 一律写黑洞…无凭据的 socks5 保持 v1.10.0 行为」、BLACKHOLE_HOST 注释「socks5 带凭据」）。
- 问题：t5 已完成、t32 已把黑洞条件改为「enabled=true 时任何非 http default」；注释与代码相反，后续维护者
  容易按旧口径改回去。
- 修复：同步注释与表（无凭据 socks5 在 relay 开启时同样黑洞；enabled=false 走 Direct）。

## 7. 已执行核对

    cargo test -p devmask_server            # 全 target ok（lib 184、relay_api 33）
    cargo test -p devmask_module_tests      # 60 passed
    grep -n 'SystemProxy::new' server/src/api/egress.rs        # put_egress 内仅 :1070 一处 apply
    grep -n 'put_never_writes_direct_proxy_while_activating_relay' -A 45 server/tests/relay_api.rs
    grep -rn '4 个警告码\\|4 个扁平\\|4 个 relay' docs/ manager/docs/   # 0 命中
