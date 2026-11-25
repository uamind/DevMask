# t36 复核：H1 判别性用例 / M1 / L1 / L2（最终轮）

- 结论：**pass**。t34 的唯一 high（H1 用例假绿）已改为两步 PUT 判别性用例，t32 的实现与 t33 的文档口径保持一致；
  前两轮 findings 无回退。
- 验证：cargo test -p devmask_server 全 target ok（lib 184、relay_api 33）、module-tests 60 passed、fmt --check 通过。

## 1. H1（PUT 直连窗口）

实现（api/egress.rs）：put_egress 只有一处 settings apply（:1070），位于 prepare_from_disk + activate（:1045-1047）之后；
expected 用重建后的 device_status；exclusionList 在同一次 apply 里传递（:1071-1079）。

用例（relay_api.rs:1709-1796，已改两步）：
1. 先 PUT 无凭据 http default，断言 settings=上游 127.0.0.1:<upstream_port>（建立基线）；
2. 再 PUT 同一 default 加凭据激活 relay，断言：
   - step2 中没有 http_proxy :0；
   - step2 中 http_proxy 写入恰好 1 次，且值为 127.0.0.1:<base_port>；
   - exclusionList 与那次写入在同一次 apply 里；
   - readback 为 127.0.0.1:<base_port>。

判别性分析：旧实现第二步会在重建前先 apply，期望 Direct；此时 readback=上游 host:port → 不匹配 → 写 :0，
重建后再写 relay → 既命中「出现 :0」也命中「writes.len()==2」。因此该用例在旧实现上必红。通过。

## 2. M1（黑洞与 warning 条件）

expected_for_relay（egress/mod.rs:130-155）：非 http default + relay.enabled=true → 127.0.0.1:9；enabled=false → Direct。
device_socks5_warning（api/egress.rs:226-250）只看有效 default 是否非 http，不再看 enabled/凭据。
用例：socks5_default_without_credentials_still_warns_and_blackholes（relay_api.rs:1798+，无凭据 → warning+黑洞）、
socks5_default_with_relay_disabled_warns_and_goes_direct（enabled=false → warning+直连、readback=null）。
文档口径（contract:343/988/998、CONFIG:417、API:1109/1110/1132/1197、SERVER:343/383、en/CONFIG:343/349、en/SERVER:75）
与实现一致。通过。

## 3. L1（merged default 校验）

validate_default_scope（request.rs:427-445）按合并后模型 + 有效 default 名校验；调用点用新配置的
device_default_name（api/egress.rs:963-969）。changing_existing_default_to_socks5_is_rejected 覆盖
「请求不带 default、只把既有 default 定义 type 改成 socks5 → 422」；旧实现会 200。通过。

## 4. L2（文档计数）

全仓「4 个警告码 / 4 个扁平 / 4 个 relay」在 docs/ 与 manager/docs/ 下 0 命中（仅历史 review memo 提及）；
契约 :30/:1211 与 §6.4 均为 5；§6.4 静默条件已把 device_socks5 列为 enabled=false 的例外。通过。

## 5. 回归核对

| 项 | 证据 |
| --- | --- |
| 第二轮 H2（F1 可失败用例） | manual_flush_makes_unknown_uid_connections_fail_closed + Revalidator 单测在 |
| 第二轮 M1（applied 假窗口） | view 用 applied_spec；get_during_restart_never_reports_a_false_applied_window 在 |
| 第二轮 M2（撞车重分配） | duplicate_local_ports_are_reallocated_with_collision_reason、colliding_port_is_reallocated_and_reports_changes 在 |
| t28 能力 | socks5 app scope、delete_releases、device_bind_failure、http.rs:696-700 头缓冲清零、FixtureRunner with_iptables 在 |
| 测试 | server 全 target ok（lib 184、relay_api 33）；module 60 passed；fmt --check 通过 |

## 6. 非阻塞观察（不记 finding）

- server/src/egress/mod.rs:124 的 BLACKHOLE_HOST 注释仍写「（socks5 带凭据）」，而 t32 起任何非 http default 在
  relay 开启时都黑洞；表格与函数内注释已对齐，仅这一句可顺手改掉。
- device_socks5_warning 保留 relay_enabled 形参但不再使用（let _），无害，可在后续清理时删掉。

## 7. 已执行核对

    cargo test -p devmask_server            # 全 target ok（lib 184、relay_api 33）
    cargo test -p devmask_server --test relay_api   # 33 passed
    cargo test -p devmask_module_tests      # 60 passed
    cargo fmt --check -p devmask_server     # 通过
    grep -rn '4 个警告码\\|4 个扁平\\|4 个 relay' docs/ manager/docs/   # 0 命中
