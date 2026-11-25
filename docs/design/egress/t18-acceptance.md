# t18 复验（第三轮）：设备端出口代理（Pixel 9）

- 设备：Pixel 9（tokay），4A190DLAQ002QR，Android 17 / SDK 37，KernelSU root
- 模块：v1.10.0，/data/adb/modules/devmask；设备端服务 devmask_server 1.10.0
- 代码基线：HEAD 822bbc7 + 未提交改动（含 t14 重放修复与 t17 的排障开关/降级路径收敛）
- 本轮验证 t17 交付：**未修改任何实现代码**，改动只在 docs/design/egress/
- 装机：libzygisk.so（t17 构建，22:33）推送后 adb reboot 重新加载；服务启动日志 tokens=1

## 结论总表（8 条验收）

| # | 验收项 | 结论 | 证据 |
| --- | --- | --- | --- |
| 1 | 复用 fixture/探针，覆盖应用级 http、socks5、__DIRECT__、未配置与继承零注入 | **PASS** | 第 3 节 A/B/C/D 四组 |
| 2 | F1/F2：进程内属性正确 + fixture 按 listener 端口归因证明流量走向 | **PASS** | 18082 归因 + 设备级仍 18080；__DIRECT__ 侧 app.fixture 命中 0 |
| 3 | 第 6 项豁免口径：降级分支有可失败的宿主单测、装机二进制含两条降级文案、正常路径 hook fired 有真机日志 | **PASS** | 第 4 节（单测 30 passed 在真机执行；两条文案 strings 命中；fired 原文） |
| 4 | 实现里没有任何系统属性/环境变量旁路开关（grep 断言） | **PASS** | 第 5 节（源码 + 装机二进制双重断言；唯一开关受 debug 护栏且只降级） |
| 5 | maps 可检出性按新文档口径核对，并记录 armed 进程保留映射的判定方式 | **PASS** | 第 6 节（按名 0 命中属预期；companion 可见；保留映射用行为证据判定） |
| 6 | 回归：设备级 PUT/probe/DELETE 与脏状态自愈仍 PASS（:0 无残留） | **PASS** | 第 7 节 |
| 7 | 收尾：settings :0、token 移除、探针卸载、fixture 停止、adb reverse 撤销 + 最终输出 | **PASS** | 第 8 节 |
| 8 | 报告 docs/design/egress/t18-acceptance.md | **PASS** | 本文件 |

**本轮无阻塞发现**；三条非阻塞观察见第 9 节。

## 1. verify 命令（契约要求的三条）

真机无关的三条命令（原始输出 evidence-t18/verify-commands.log）：

~~~
=== cargo check -p devmask --target aarch64-linux-android ===
CHECK_EXIT=0

=== cargo fmt -p devmask --check ===
FMT_EXIT=0

=== cargo clippy -p devmask --target aarch64-linux-android --all-targets -- -D warnings ===
CLIPPY_EXIT=0
~~~

补充（单测为什么不在 macOS 上跑）：本 crate 的 prop-rs-android 依赖是 Android-only，macOS 上
cargo test -p devmask --lib 直接编译失败（真实错误：cannot find value SYS_futex / FUTEX_WAKE in crate libc）。
因此改为 **把 aarch64 单测二进制推到真机执行**（见第 4 节），比宿主机跑更接近真实目标。

## 2. 装置

| 组件 | 说明 |
| --- | --- |
| fixtures/local_proxy.py | 三监听：HTTP 18080（设备级）/ HTTP 18082（应用级）/ SOCKS5 18081；每条日志带 listener=&lt;端口&gt; 做归因 |
| 虚拟主机 | probe.fixture→198.51.100.7（设备级探测用）、app.fixture→198.51.100.8（只有探针应用用）、198.51.100.9→raw 标记 |
| fixtures/probe-app/ | 探针 APK io.df.egressprobe：打印进程内代理属性 + ProxySelector 选择结果 + 两次 HttpURLConnection（app.fixture / 198.51.100.7）；凭据只输出 &lt;set:true/false&gt; |
| adb 隧道 | adb reverse tcp:18080/18081/18082，adb forward tcp:18686 tcp:8686 |
| 临时 Token | 追加 id="t18-qa"，结束后恢复 server.toml 备份（tokens=1） |

## 3. A/B/C/D 四组真机实测

### A 应用级 HTTP（设备级 mac_http:18080 + 应用级 mac_http2:18082）

~~~
PUT HTTP 200 {"changed": true, "mode": "proxy", "default": "mac_http",
              "apps": [{"package":"io.df.egressprobe","egress":"mac_http2","effective":"proxy","proxy":"mac_http2"}]}

11-24 22:39:13.944 10435 I EGRESSPROBE: http.proxyHost=127.0.0.1
11-24 22:39:13.944 10435 I EGRESSPROBE: http.proxyPort=18082
11-24 22:39:13.944 10435 I EGRESSPROBE: selector.app.fixture=HTTP @ 127.0.0.1/<unresolved>:18082
11-24 22:39:13.944 10435 I EGRESSPROBE: fetch.app=status=200 body=198.51.100.8
11-24 22:39:13.944 10435 I EGRESSPROBE: fetch.ip=status=200 body=198.51.100.7

--- fixture（listener 归因）---
2026-11-24T22:39:09+0800 proto=http listener=18082 GET http://app.fixture/ host=app.fixture auth=0
2026-11-24T22:39:09+0800 proto=http listener=18082 GET http://198.51.100.7/ host=198.51.100.7 auth=0
2026-11-24T22:39:12+0800 proto=http listener=18080 CONNECT phonedeviceverification-pa.googleapis.com:443 auth=0
2026-11-24T22:39:17+0800 proto=http listener=18080 CONNECT digitalassetlinks.googleapis.com:443 auth=0

--- DevMask logcat ---
22:39:07.958 [egress] replay hook armed package=io.df.egressprobe
22:39:07.979 [egress] replay hook fired symbol=setpriority
22:39:07.979 [egress] replay start package=io.df.egressprobe trigger=post_zygote_setpriority
22:39:09.980 [egress] replay package=io.df.egressprobe t=32ms reason=framework_overwrite applied=6/6 changed=[http.proxyPort=18080->18082, https.proxyPort=18080->18082]
22:39:09.981 [egress] replay done package=io.df.egressprobe probes=118 replays=1 divergences=1 state=match elapsed=2000ms
~~~

### B 应用级 SOCKS5（设备级代理仍开启）

~~~
11-24 22:39:25.032 10733 I EGRESSPROBE: http.proxyHost=<null>
11-24 22:39:25.032 10733 I EGRESSPROBE: http.proxyPort=<null>
11-24 22:39:25.032 10733 I EGRESSPROBE: socksProxyHost=127.0.0.1
11-24 22:39:25.032 10733 I EGRESSPROBE: socksProxyPort=18081
11-24 22:39:25.032 10733 I EGRESSPROBE: java.net.socks.username=dfsocks
11-24 22:39:25.032 10733 I EGRESSPROBE: java.net.socks.password=<set:true>
11-24 22:39:25.032 10733 I EGRESSPROBE: selector.app.fixture=SOCKS @ 127.0.0.1/<unresolved>:18081
11-24 22:39:25.032 10733 I EGRESSPROBE: fetch.app=status=200 body=198.51.100.8

--- fixture ---
2026-11-24T22:39:20+0800 proto=socks5 listener=18081 method=noauth CONNECT app.fixture:80
2026-11-24T22:39:20+0800 proto=socks5 listener=18081 tunnel-request app.fixture 164 bytes
2026-11-24T22:39:23+0800 proto=http listener=18080 CONNECT securitydomain-pa.googleapis.com:443 auth=0
~~~

§5.3 互斥成立（http/https 全 null），且请求真经 SOCKS 隧道（tunnel-request 164 bytes）。

### C __DIRECT__（设备级 http_proxy=127.0.0.1:18080 仍然生效）

~~~
settings=127.0.0.1:18080
11-24 22:39:36.025 10978 I EGRESSPROBE: http.proxyHost=<null>
11-24 22:39:36.025 10978 I EGRESSPROBE: http.proxyPort=<null>
11-24 22:39:36.025 10978 I EGRESSPROBE: socksProxyHost=<null>
11-24 22:39:36.025 10978 I EGRESSPROBE: selector.app.fixture=DIRECT
11-24 22:39:36.025 10978 I EGRESSPROBE: fetch.app=error=UnknownHostException
app.fixture 命中数=0

--- 同刻设备级 probe ---
{"ok": true, "exitIp": "198.51.100.7", "latencyMs": 9, "via": "proxy", "proxy": "mac_http", ...}
2026-11-24T22:39:40+0800 proto=http listener=18080 GET http://probe.fixture/ host=probe.fixture auth=0
~~~

### D 未配置 / 继承零注入（com.reqable.android，config.toml 命中 0）

~~~
D-1 设备级 default=mac_http 开启（继承场景）
  settings=127.0.0.1:18080
  module log 行数 before=4282 after=4282 delta=0
  该包 [egress] 行=0
  该进程 DevMask logcat 行=1（该行是 adbd 记录我们自己的 grep 命令行，不是模块日志）
  pid=11132 maps 命中 devmask|arm64-v8a|zygisk=0

D-2 设备级 :0 直连（未配置场景）
  module log 行数 before=4283 after=4283 delta=0
  该包 [egress] 行=0
  pid=11358 maps 命中=0
~~~

## 4. 第 6 项（hook fired / unavailable）按豁免口径核对

### 4.1 降级分支的宿主单测可在仓库内直接跑通（t21 交付形态）

t21 新增的 workspace 成员 **devmask_module_tests**（module-tests/）把 src/egress.rs 的纯逻辑
（含 plan_replay_arm / finish_replay_arm / 三条降级路径）搬成宿主可编译的独立 crate，依赖面压到
log + toml + devmask_core，**严禁引入 Android-only crate**。仓库内直接跑：

~~~
$ cargo test -p devmask_module_tests
running 30 tests
test egress::tests::arm_plan_honours_the_debug_guardrail ... ok
test egress::tests::missing_libandroid_runtime_degrades_with_the_expected_log ... ok
test egress::tests::commit_failure_degrades_with_the_expected_log ... ok
test egress::tests::armed_outcome_keeps_module_mapped_and_logs_nothing ... ok
test egress::tests::debug_switch_outcome_is_logged_and_does_not_keep_the_module ... ok
... （其余 25 个：plan/互斥/重放窗口/凭据脱敏/spec 解析等）
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
MODULE_TESTS_EXIT=0
~~~

（evidence-t18/module-tests-in-repo.log）——降级分支不是「只在 /tmp harness 里」，CI 可直接执行。

补充（更贴近目标平台，非必需）：devmask 本体在 macOS 上编不出来（prop-rs-android 是 Android-only，
真实错误：cannot find value SYS_futex / FUTEX_WAKE in crate libc），所以另把 **aarch64 单测二进制推到真机**
跑了一遍同一套用例：30 passed / 0 failed，单用例复跑（missing_libandroid_runtime / commit_failure /
debug_switch / arm_plan）各 1 passed。见 evidence-t18/unit-tests-on-device.log。

### 4.2 装机二进制含两条降级文案

~~~
$ strings /data/adb/modules/devmask/zygisk/arm64-v8a.so | grep -E 'replay hook unavailable|debug_disable_replay'
[egress] replay hook unavailable reason=commit_failed error=
[egress] replay hook unavailable reason=libandroid_runtime_not_found
[egress] replay hook unavailable reason=debug_disable_replay (debug.devmask.egress.disable_replay is set; one-shot injection only)
~~~

### 4.3 正常路径 hook fired 有真机日志

见第 3 节 A 组：armed → fired symbol=setpriority → replay start → t=32ms applied=6/6 → done probes=118 elapsed=2000ms。
另有 E3（开关属性改成 0）同样 armed/fired + t=30ms state=match。

## 5. 旁路开关 grep 断言（源码 + 装机二进制双重）

### 5.1 源码

~~~
$ grep -rn 'env::var|getenv|std::env' src/            -> (none)
$ grep -rn 'cfg(feature|cfg(debug_assertions' src/    -> (none)
$ grep -rn '"debug\.|"devmask\.' src/
src/hooks.rs:749:  const NAME: &std::ffi::CStr = c"debug.devmask.egress.disable_replay";
src/egress.rs:775: "[egress] replay hook unavailable reason=debug_disable_replay (debug.devmask.egress.disable_replay is set; one-shot injection only)"
~~~

即：**整个 src/ 里只有一个系统属性读取点**，且它是 t17 按复核要求新增的排障开关。

### 5.2 装机二进制

~~~
$ strings arm64-v8a.so | grep -E '^(debug|persist|ro)\.'
debug.devmask.egress.disable_replay        <- egress 唯一开关
ro.build./ro.product./ro.vendor.../persist.sys.timezone  <- COW 机型伪装用的属性名（既有功能，与 egress 无关）
$ strings arm64-v8a.so | grep -E 'DEVMASK_|DF_[A-Z_]+' -> (none)
~~~

### 5.3 该开关不构成旁路（它只降级、且有两条护栏）

实现（src/hooks.rs 的 plan_replay_arm 调用点 + src/egress.rs 的 plan_replay_arm）：
- 护栏 a：必须 config.toml 里 debug = true 才生效（debug 关闭时不改变任何行为）；
- 护栏 b：只认 1/true/yes/on（trim + 大小写不敏感），空值/0/其它值一律视为关闭；
- 语义：**只跳过 arming**（登记任务 + PLT hook + 保留代码页），注入本身照旧执行 → 退化为一次性注入，
  并打一行与真实降级同族的 unavailable 日志；没有任何「跳过注入 / 跳过探测 / 跳过校验」的路径，
  也不影响服务端（settings/配置）任何行为。

真机四条路径（evidence-t18/t18-e-switch.log）：

| 场景 | 进程内 http.proxyPort | 日志 / 归因 |
| --- | --- | --- |
| E1 prop=1 + debug=true | **18080**（一次性，被框架覆盖后不重放） | 模块日志 [egress] replay hook unavailable reason=debug_disable_replay；22:40:15.490 该进程无 armed / 无 kept mapped；fixture 落 listener=18080 |
| E2 prop=1 + debug=false（护栏 a） | **18082**（开关被忽略，重放照常） | fixture 落 listener=18082 |
| E3 prop=0 + debug=true（护栏 b） | **18082**（视为关闭） | armed → fired → t=30ms applied=6/6 → done elapsed=2001ms；fixture 落 listener=18082 |
| 默认（prop 空） | **18082** | 见第 3 节 A 组 |

E1 关键原文：

~~~
[2026-11-24 22:40:15.490] [INFO] zygisk::hooks - [egress] injected package=io.df.egressprobe type=http host=127.0.0.1 port=18082 creds=0 ok=6 failed=0 selector=degraded
[2026-11-24 22:40:15.490] [WARN] zygisk::hooks - [egress] replay hook unavailable reason=debug_disable_replay (debug.devmask.egress.disable_replay is set; one-shot injection only)
11-24 22:40:21.490 11559 I EGRESSPROBE: http.proxyPort=18080
2026-11-24T22:40:17+0800 proto=http listener=18080 GET http://app.fixture/ host=app.fixture auth=0
~~~

结论：**没有旁路开关**。唯一的系统属性是 debug 门控的排障开关，作用方向是「关掉重放」（让 QA 能制造 unavailable 样本），
不会让任何验证误判为通过；环境变量读取为零（源码与二进制双重确认）。

补充：**开关 → 清空属性 → 恢复 armed** 的同轮闭环（同一构建、同一配置，仅改属性）：

~~~
(1) prop=1（debug=true）
  本轮新增模块日志：
    [egress] injected package=io.df.egressprobe type=http host=127.0.0.1 port=18082 ... ok=6 failed=0
    [egress] replay hook unavailable reason=debug_disable_replay (… one-shot injection only)
  armed 出现次数=0；kept mapped 次数=0
  logcat: [egress] replay hook unavailable package=io.df.egressprobe reason=debug_disable_replay
  进程内 http.proxyPort=18080；fixture listener=18080 GET http://app.fixture/

(2) setprop ... ""（清空）
  本轮新增模块日志：
    [egress] injected ... ok=6 failed=0
    [egress] replay hook armed image=libandroid_runtime ...
    [egress] module kept mapped for the replay window
  logcat: replay start → replay t=28ms reason=framework_overwrite applied=6/6 → done probes=118 elapsed=2001ms state=match
  进程内 http.proxyPort=18082；fixture listener=18082 GET http://app.fixture/
~~~

即：开关只影响「是否挂重放」，清空后立刻恢复正常的 armed/重放路径。

## 6. maps 可检出性（按新文档口径）

文档（docs/CHANGELOG.md 第 20 行，t17 期间的文档任务已改）：

> ① 显式配置 egress 的进程会保留**常驻模块代码页**（重放线程与 PLT hook 都在 .so 里）——但**按名字扫 /proc/&lt;pid&gt;/maps 看不到**
> （Zygisk 用无名/匿名映射，实测 armed 进程 3170 行命中 0），真判断要比匿名可执行段

真机核对（evidence-t18/t18-f-maps.log）：

~~~
armed pid=11689
按名命中 devmask|arm64-v8a|libzygisk = 0
maps 总行数 = 3198；含 [anon 的行数 = 478

companion 进程（pid 2004）maps 里能看到模块路径：
7c2ae8e000-7c2aed1000 r--p 00000 fe:40 170938 /data/adb/modules/devmask/zygisk/arm64-v8a.so
7c2aed4000-7c2aafe7000 r-xp 00042000 ...（同上，r-xp 段）
~~~

**armed 进程「保留映射」的判定方式**（maps 按名查不到，因此不用名字判定）：
1. hook fired 发生在 specialize 之后（22:40:35.622），说明模块代码在 zygote 过渡完成后仍被执行；
2. 重放线程在**应用进程内**跑满 2001ms（logcat replay done elapsed=2001ms，tid 11694），线程全程执行本 .so 的代码；
3. 重放结束后进程继续存活（kill -0 = yes）、继续创建线程（Activity 重建/旋转），期间 setpriority 仍被 PLT hook 接管——
   若 .so 已被 dlclose，这些晚到的调用执行到的就是野指针，进程必 SIGSEGV；实测无崩溃。
→ 结论与文档一致：代码页常驻、但**按名字不可检出**；要判定只能比匿名可执行段（或像本轮这样用行为证据）。

## 7. 回归：设备级 PUT / probe / DELETE + 脏状态自愈

~~~
G-1 状态：settings=127.0.0.1:18080；dumpsys Global proxy: [127.0.0.1] 18080 xl=

G-2 probe：HTTP 200 {"ok": true, "exitIp": "198.51.100.7", "latencyMs": 10, "via": "proxy", "proxy": "mac_http", ...}
    fixture: listener=18080 GET http://probe.fixture/ host=probe.fixture auth=0

G-3 脏状态（settings delete global http_proxy）：
    Deleted 1 rows；settings get -> null；但 global_http_proxy_host=127.0.0.1 / port=18080 残留，dumpsys 仍显示代理生效
    重启服务 -> [egress reconcile 完成 ... wrote=true effective_mode="from_config"]
    重启后 settings=127.0.0.1:18080，dumpsys 一致（自愈）

G-4 DELETE /egress?confirm=true：
    {"changed": true, "mode": "direct", "default": null, "applied": true,
     "readback": {"settingsValue": ":0", "dumpsysGlobalProxy": "Global proxy: none", "matches": true}}
    settings get = :0；global_http_proxy_host=（空）；global_http_proxy_port=0；dumpsys Global proxy: none
~~~

## 8. 收尾状态

~~~
$ adb devices
List of devices attached
4A190DLAQ002QR	device

$ adb shell 'settings list global | grep -i proxy'
global_http_proxy_exclusion_list=
global_http_proxy_host=
global_http_proxy_port=0
global_proxy_pac_url=
http_proxy=:0

$ adb shell 'dumpsys connectivity | grep -i proxy'
mMultiProxyEnabled: false
ProxyTracker:
  Global proxy: none
  Default proxy: none

$ adb shell grep -c egressprobe config.toml   -> 0
$ adb shell grep -c "[egress]" config.toml    -> 0
$ adb shell ls .../egress.toml                -> No such file or directory
$ adb shell grep -a tokens= server.log | tail -1 -> tokens=1（临时 t18-qa Token 已移除）
$ adb reverse --list                          -> （空）
~~~

已清理：DELETE /egress；移除测试期 [[apps]] io.df.egressprobe 记录；恢复 server.toml 备份；
卸载探针应用；删除 /data/local/tmp 下的单测二进制与临时文件；删除测试期 egress.toml；
清除 debug.devmask.egress.disable_replay（空值）；停 fixture；adb reverse --remove-all / forward --remove；
config.toml 的 debug 行恢复为 t14 时的 true。

## 9. 非阻塞观察

1. **O1（沿用 t15）带认证的 SOCKS5 走不通**：属性注入正确（java.net.socks.username=dfsocks、password=&lt;set:true&gt;），
   但 libcore 的 SOCKS 客户端不用这两个属性发凭据（t15 实测 fixture 侧 methods=0002 user_ok=0 pass_ok=0）。
   本轮 socks 用例用 noauth 代理验证链路，契约 §5.1 第 7 项在该机型的限制仍建议文档标注。
2. **O2 重放窗口时长固定 2000ms**：本轮实测 elapsed=2000–2001ms、probes=118；框架在 handleBindApplication 只写一次，
   因此窗口外的设备级改动不会纠正（需重启应用），与文档一致。属设计取舍，不是缺陷。
3. **O3（已由 t21 解决）** devmask 本体在 macOS 上编不出来（prop-rs-android Android-only），
   但 t21 的 module-tests crate 让同一套逻辑在仓库内可直接跑（本轮实测 30 passed）；真机执行 aarch64 单测
   作为额外佐证保留。建议 CI 同时跑 module-tests（宿主机）与真机二进制（发布前）。

## 10. 证据文件

| 文件 | 内容 |
| --- | --- |
| evidence-t18/verify-commands.log | git log、cargo check/fmt/clippy（aarch64）原始输出 + 宿主单测失败原因 + android 单测构建 |
| evidence-t18/unit-tests-on-device.log | 真机执行 aarch64 单测：30 passed（含两条降级用例与护栏用例） |
| evidence-t18/module-tests-in-repo.log | 仓库内 cargo test -p devmask_module_tests：30 passed（t21 交付形态，CI 可直接跑） |
| evidence-t18/t18-a-app-http.log | A 组（F1）：属性 18082 + fixture listener=18082 + hook fired/replay t=32ms |
| evidence-t18/t18-b-app-socks.log | B 组：socks5 属性/互斥 + SOCKS tunnel-request |
| evidence-t18/t18-c-direct.log | C 组（F2）：__DIRECT__ 属性全空 + app.fixture 命中 0 + 设备级仍 18080 |
| evidence-t18/t18-d-zero-injection.log | D 组：未配置/继承零注入（delta 0、无 [egress]、maps 0） |
| evidence-t18/t18-e-switch.log | 排障开关四条路径 + 真实 unavailable 日志（debug 门控、只降级） |
| evidence-t18/bypass-switch-grep.log | 旁路开关 grep 断言（源码 + 装机二进制） |
| evidence-t18/t18-f-maps.log | maps 可检出性 + companion 路径 + 保留映射行为判据 |
| evidence-t18/t18-g-regression.log | PUT/probe/DELETE + 脏状态自愈 |
| evidence-t18/t18-final-state.log | 收尾与最终状态实测 |

## 11. 复现步骤（最短路径）

~~~bash
# 0) 构建 + 装机
export ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk
RUSTFLAGS="-C default-linker-libraries" cargo +nightly ndk --platform 31 -t arm64-v8a build --release
adb push target/aarch64-linux-android/release/libzygisk.so /data/adb/modules/devmask/zygisk/arm64-v8a.so
adb reboot

# 0b) 单测（本 crate 不能在 macOS 跑，改在真机执行）
cargo ndk --platform 31 -t arm64-v8a test -p devmask --lib --no-run
adb push target/aarch64-linux-android/debug/deps/zygisk-<hash> /data/local/tmp/df_unit_tests
adb shell 'chmod 0755 /data/local/tmp/df_unit_tests && /data/local/tmp/df_unit_tests --test-threads=1'

# 1) fixture + 隧道（18080 设备级 / 18082 应用级 / 18081 SOCKS）
python3 docs/design/egress/fixtures/local_proxy.py &
adb reverse tcp:18080 tcp:18080; adb reverse tcp:18081 tcp:18081; adb reverse tcp:18082 tcp:18082
adb forward tcp:18686 tcp:8686

# 2) A/B/C：PUT /egress {"default":"mac_http","proxies":[...],"apps":[{probe,<mac_http2|mac_socks|__DIRECT__>}]}
adb install -r docs/design/egress/fixtures/probe-app/egress-probe.apk
adb shell 'am force-stop io.df.egressprobe; logcat -c; am start -W -n io.df.egressprobe/.MainActivity; sleep 5'
adb shell 'logcat -d -s EGRESSPROBE:I'; adb shell 'logcat -d | grep -a DevMask'

# 3) E：setprop debug.devmask.egress.disable_replay 1 / 0 / ""（配 debug=true 才生效）
# 4) 收尾：DELETE /egress?confirm=true；adb shell 'settings put global http_proxy :0'
~~~
