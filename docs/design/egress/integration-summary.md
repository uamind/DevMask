# egress 集成收口：全量校验与交付说明（t13）

- 时间：2026-11-24 23:26–23:35 CST
- 主机：Darwin 27.3.0 arm64 / rustc 1.99.0 / cargo 1.99.0 / node v26.13.1
- 代码基线：HEAD 822bbc7（**未 commit、未 push**；工作区 79 改 + 17 新 = 96 项）
- 本任务范围：只读校验 + 本目录文档；**未修改任何实现代码**

## 1. 第 1 组命令（逐条实跑，全部 exit 0）

| # | 命令 | 退出码 | 备注 |
| --- | --- | --- | --- |
| 1-1 | cargo fmt --check | **0** | 全仓 79 个改动文件均 fmt 干净 |
| 1-2 | cargo clippy --workspace --exclude devmask --all-targets -- -D warnings | **0** | core/server/manager/module-tests/xtask 全绿，无 warning |
| 1-3 | cargo test -p devmask_core -p devmask_server -p devmask_manager | **0** | **483 passed / 0 failed** |
| 1-4 | cargo check -p devmask --target aarch64-linux-android | **0** | ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk |

## 2. 第 2 组命令（全部 exit 0）

| # | 命令 | 退出码 | 备注 |
| --- | --- | --- | --- |
| 2-1 | python webui/build_webui.py | **0** | npm ci + vue-tsc + eslint + vite build |
| 2-2 | npm --prefix manager/webui run build | **0** | dist 产物更新 |
| 2-3 | (cd manager/webui && npx vue-tsc --noEmit) | **0** | 额外补跑（契约未列，交付依赖它） |

原始输出：docs/design/egress/evidence-t13/commands.log。

## 3. 测试数字

| crate / 套件 | 用例数 | 说明 |
| --- | --- | --- |
| devmask_core | **65** | lib 0 + config_semantics 10 + edit_document 19 + **egress_config 14** + real_config 2 + store 11 + validate_schema 9 |
| devmask_server | **258** | lib 123 + apps_api 9 + config_api 21 + contract 3 + **egress_api 45** + overview_api 3 + packages_device_api 9 + skeleton 15 + static_site 8 + templates_api 13 + write_semantics 9 |
| devmask_manager | **160** | lib 71 + apply_flow 14 + devices_flow 14 + e2e_flow 4 + **egress_flow 17** + manager_api 17 + ops_endpoints 8 + review_reverify 15 |
| 小计（1-3 命令） | **483** | 0 failed |
| devmask_module_tests | **32** | 独立可跑（cargo test -p devmask_module_tests = 0）：注入计划 / 重放调度 / 降级分支 / 凭据脱敏 |
| **egress 专项合计** | **108** | core 14 + server 45 + manager 17 + module 32 |
| 真机（非本机命令） | — | t9 / t15 / t18 三份报告 + evidence 目录：设备侧 egress 全链路（5 端点、settings 读写、探测、__DIRECT__、零注入、hook fired / 重放窗口） |

## 4. 版本与契约一致性

| 项 | 值 | 一致性 |
| --- | --- | --- |
| 根 crate devmask | 1.10.0 | 模块版本 v1.10.0 / versionCode 20261201（module/module.prop） |
| devmask_server | 1.10.0 | GET /api/v1/status 报 server.version=1.10.0 |
| devmask_core | 1.7.0 | 内部库版本，不对外暴露 |
| devmask_manager | 0.2.0 | GET /healthz 报 version=0.2.0（另一条版本线，schemaVersion=2） |
| CHANGELOG | [v1.9.0 → v1.10.0] - 2026-12-01 首条即 egress 全量说明 | 与实现一致（新功能 / 改进 / 修复三节） |
| CONTRACT_OPERATIONS | [(&str,&str); 29]（server/src/api/mod.rs:42） | **29** |
| docs/openapi.yaml 操作数 | 29（21 个 path，方法级 29） | **与 CONTRACT_OPERATIONS 相等**；其中 egress 5 个（/egress GET/PUT/DELETE、/egress/probe POST、/egress/proxies GET） |
| CAPABILITIES | [&str; 8]，末位为 "egress" | status 响应与实现一致（t18 真机实测 capabilities 含 egress） |

## 5. 改动清单（按目录分组）

跟踪文件：**79 改 / +10495 −247**；未跟踪新增 **17**。

| 目录 | 改 | 新 | 主要内容 |
| --- | --- | --- | --- |
| core/ | 5 | 1 | config.rs（EgressSection / AppConfig.egress / 名字校验 /Schema）、validate.rs、edit.rs（set_egress）、schema.rs、lib.rs；新增 tests/egress_config.rs（14 用例） |
| server/ | 16 | 3 | api/mod.rs（29 操作 + capabilities）、api/egress.rs（5 端点）、egress/（file / system / probe / state / dto / request：0600 + 原子写 + 探测 + 自愈）、commands.rs（settings/dumpsys 封装 + 5s 超时）、util.rs（错误首行化）、dto.rs / error.rs / store.rs / app.rs / lib.rs 等；新增 tests/egress_api.rs（45 用例） |
| src/（Zygisk 模块） | 4 | 1 | 新增 egress.rs（代理解析 + 注入计划 + 重放调度 + 降级分支）、hooks.rs（属性 / ProxySelector 注入 + setpriority PLT hook + 重放 + never-fired 诊断）、lib.rs（显式才注入 + 保留映射判定）、spec.rs（[egress_proxy]）、companion.rs |
| manager/ | 28 | 4 | 新增 api/egress.rs + api/egress_dto.rs（9 个操作）、tests/egress_flow.rs（17 用例）、webui/src/pages/ProxiesPage.vue；db.rs（schema v2 三表 + jobs.kind 重建）、crypto.rs（AES-256-GCM 凭据）、jobs.rs（run_egress）、device_client.rs、error.rs、ids.rs、api/{devices,dto,mod,overview}.rs、docs/{API,ARCH,RUN,api-types}、mock_device.py（egress 五端点）等 |
| webui/（设备端） | 9 | 2 | backend/egress.ts + stores/egress.ts 新增；SettingsPage.vue「网络出口」卡片、backend/{http,ksu,index,types,errors}.ts、types/index.ts、utils/{config,i18n}.ts（三语 101 键） |
| module/ | 2 | — | module.prop（v1.10.0 / 20261201）、uninstall.sh（停服后清 :0 + 删运行时目录） |
| docs/ | 11 | 4 | API.md / CONFIG.md / SERVER.md / CHANGELOG.md / README.md / llms.txt / openapi.yaml（+ en/tr 同步）；新增 docs/plans/{egress-contract,proxy-support-plan}.md、docs/design/egress/、docs/design/manager-egress/ |
| tests/ | 2 | — | tests/check.sh、tests/fixtures/module/module.prop |
| module-tests/ | — | 1 | 新增 workspace 成员（宿主可跑模块单测，32 用例） |
| .github/ | — | 1 | workflows/test.yml（宿主机侧 fmt / clippy / test 门禁，t22） |
| 根 | 2 | — | Cargo.toml（members + version）、Cargo.lock |

## 6. 残留扫描（真实代理地址 / 疑似密码）

扫描范围：排除 target/、node_modules/、.git/、dist/、.agent-teams/、二进制与图片，共 531 个文本文件。

- **IPv4**：非文档网段 / 非私网命中 4 个，逐个核对为**示例或测试值**：1.2.3.4、5.6.7.8（probe / manager 单测示例 URL）、5.92.42.36（第三方 icon 数据包内字符串）。
- **真实出口 IP 一处，已脱敏**：docs/design/egress/t10-acceptance.md 里 t10 UI 探测 toast 记录过本机公网出口 IP，已替换为 <public-egress-ip>（保留「探测成功」语义）。
- **密码字面量**：6 处，全部在 docs/design/egress/evidence-{t11,t24,t30}/ 的复现脚本与输出里，值为明显的合成占位（s3cret / n3wpass / "..."），非真实凭据；按「示例占位符不算」保留。
- **Token 形态命中**：manager/docs/API.md 的 dfm_7Kx9…（文档示例响应）、tests/api/*.sh 的 df_test_token_0123456789（测试 Token）与 df_assert_* 函数名（正则误报）——均非真实凭据。
- 结论：**仓库内没有真实代理地址或真实密码残留**；唯一真实公网地址已脱敏。

## 7. 遗留限制与已知代价（交付时需向用户说明）

### 7.1 必列四条（captain 指定口径）

**a) 带认证的代理本期不支持，可用 IP 白名单**
SOCKS5 的 java.net.socks.username/password 在进程内可读，但 libcore 的 SOCKS 客户端**不发送**这两个属性（Android 17 实测代理侧 methods=0002 user_ok=0 pass_ok=0 → auth-failed）；HTTP Basic **未实现**（只注入 host/port，记 unsupported-auth / egress_auth_unsupported）。因此带凭据的代理请用**代理侧 IP 白名单**授权；「设备内本地认证中继」属 **P2**，本期未实现。Manager 侧探测也只支持无认证/不对认证做处理的 HTTP 代理（socks5 直接 422，见 7.2）。

**b) 应用级注入的三条已知代价**
① **armed 进程常驻模块代码页**：显式配 [[apps]].egress 的进程会保留模块 .so 的代码页（重放线程与 PLT hook 都在里面，zygisk 不能 dlclose），但 Zygisk 用无名/匿名映射，**按名字扫 `/proc/<pid>/maps` 看不到**（实测 armed 进程 3170 行命中 0），检测只能比匿名可执行段——这是「重放」换来的代价。
② **运行中改设备级代理需重启应用**：重放窗口固定 0–2000ms，框架只在 handleBindApplication 写一次代理属性；窗口之后再改设备级代理不会被已启动进程纠正。
③ **setpriority hook 未命中时退化为一次性注入**：libandroid_runtime 镜像找不到或 PLT hook 提交失败时只记一行 replay hook unavailable，本次进程没有重放，注入值可能随后被框架覆盖（表现为应用级 HTTP 出口不生效 / __DIRECT__ 拉不回直连）。诊断四行：armed（武装成功）/ fired（命中）/ unavailable（没挂上）/ not fired（窗口到点或退出兜底仍未命中，文案 `replay hook not fired within {window}ms (no replay started)`）。

**c) 「总是 PUT」的代价：每次 apply 一次设备写入，需要 write 作用域，凭据随请求体下发**
Manager 不再做本地比较，每次 apply 都对每台设备发一次 PUT /api/v1/egress（写入 egress.toml + config.toml + 应用 settings），因此：① 幂等判定权在设备——设备回 changed:false 时 Manager 记 ok + egress_unchanged；② 设备 Token 必须有 **write** 作用域，只读 Token 会得到 403 forbidden_scope；③ 解密后的 username/password 随请求体下发（走与设备 Token 相同的通道：默认 loopback 明文 HTTP + Bearer，LAN 无 TLS）。明文只在内存（Zeroizing）、不落日志/审计/响应，设备侧写 0600；缓解是 loopback + Token、代理侧 IP 白名单、P2 本地认证中继。

**d) 真机验证覆盖范围**
真机证据仅覆盖 **Pixel 9（4A190DLAQ002QR）/ Android 17（SDK 37）/ KernelSU** 一台一套环境（t9 / t15 / t18 报告 + evidence-t9/t15/t18）。其它机型、ROM、Android 版本未验证：SpecializeCommon 的 setpriority 调用顺序、libcore 的 ProxySelector/socks 行为、settings 的 :0 语义都可能不同，特别是「重放触发点」和「ProxySelector.of 是否存在」。

### 7.2 其它已知限制

1. **ProxySelector.of(...) 在 Android 17 / SDK 37 不存在**：selector 注入恒降级（每进程一行 proxy_selector degraded reason=of_method_missing sdk=Some(37)），属性路径才是权威载体；F1/F2 不依赖 selector。
2. **Manager 侧探测只支持 HTTP**：socks5 → 422 unsupported_proxy_type（egress_checks 落 ok=0）。
3. **settings delete 在 Android 17 清不掉代理**：清除只走 settings put global http_proxy :0（脏状态由启动 reconcile 自愈）。
4. **en 文档缺口（不影响代码，已开 t36 跟进）**：replay not fired 的文案/时机已在 zh 对齐（docs/CONFIG.md:344/350、docs/SERVER.md:305，全仓 grep「never fired / 进程退出时判定」zh 0 命中）；en 侧 docs/en/CONFIG.md:313 的 Observability 块仍缺 never-fired 行，docs/en/SERVER.md 无该段落。captain 已开 **t36（architect）** 专门补，本任务不返工。
5. **设备现场状态（captain 同步；t13 未连真机）**：t26 修过真机 server.toml 的重复 token 块（两个 id="t17-qa" 导致服务启动即退出），现已清理为单个 default token。后续跑真机用例前先 adb shell ps -A | grep devmask_server 确认服务在跑，避免把「服务没起」误判成实现失败。

## 8. 交付状态

- **未执行 git commit / git push**：HEAD 仍是 **822bbc7**（git log -1 未变），git status 仍是 **96 项未提交改动**（79 改 + 17 新），等 captain 交用户确认后再处理提交。
- **测试产物全部保留、未删任何一项**（已逐个核对存在）：
  - 证据目录 8 个：docs/design/egress/evidence-t10、-t11、-t12、-t13、-t15、-t18、-t24、-t30
  - fixtures 3 项：docs/design/egress/fixtures/local_proxy.py、mock_device_egress.py、probe-app/
  - 探针 APK 源码与产物：docs/design/egress/fixtures/probe-app/src/io/df/egressprobe/MainActivity.java、AndroidManifest.xml、build.sh、egress-probe.apk（12691 B）、debug.keystore、build 中间产物
  - UI 截图：docs/design/manager-egress/ 共 13 张
  - 报告 9 份：docs/design/egress/{integration-summary, t9-acceptance, t10-acceptance, t11-review, t12-review, t15-acceptance, t18-acceptance, t24-review, t30-review}.md
  - 主线 mock：manager/docs/mock_device.py（已并入 egress 五端点）
