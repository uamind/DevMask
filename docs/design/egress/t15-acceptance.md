# t15 复验：设备端出口代理（Pixel 9，第二轮）

- 设备：Pixel 9（tokay），4A190DLAQ002QR，Android 17 / SDK 37，KernelSU root
- 模块：v1.10.0（versionCode 20261201），/data/adb/modules/device_faker
- 设备端服务：device_faker_server 1.10.0，127.0.0.1:8686，capabilities 含 egress
- 代码基线：HEAD 822bbc7 + 未提交改动（含 t14 对 src/ 的修复）
- 本轮为 t9 缺陷（F1/F2/F3）修复后的复验；**未修改任何实现代码**（改动只在 docs/design/egress/）
- 构建与静态检查：cargo fmt --check = 0，cargo clippy -p device_faker_server --all-targets -- -D warnings = 0，
  cargo test -p device_faker_server = **226 passed / 0 failed**（原始输出见 evidence-t15/cargo-checks.log）
- 装机：重新编译 libzygisk.so（含 t14 修复）与 device_faker_server，推送后 adb reboot 让 Zygisk 重新加载

## 结论总表

| # | 验收点 | 结论 | 判定依据（不是只看模块日志） |
| --- | --- | --- | --- |
| 1 | fixture/探针复用，覆盖 http 生效、socks5 生效、__DIRECT__ 直连、未配置零注入 | **PASS** | A/B/C/D 四组真机实验，见下文各节 |
| 2 | F1：进程内 http.proxyHost/Port 等于**应用级**配置值，且代理侧日志证明流量经该代理 | **PASS** | 进程内 127.0.0.1:18082（设备级是 18080）+ fixture <code>listener=18082 GET http://app.fixture/</code> |
| 3 | F2：设备级 http_proxy 生效时，__DIRECT__ 应用属性为空且 fixture 未收到其流量 | **PASS** | 六个属性全 null、selector=DIRECT、app.fixture 命中数 = 0 |
| 4 | F3：ProxySelector 缺失的降级被明确记录，且不掩盖属性路径 | **PASS** | <code>[egress] proxy_selector degraded reason=of_method_missing sdk=Some(37)</code>；同进程属性/流量均生效 |
| 5 | 回归：设备级 PUT / probe / DELETE 三条主路径仍 PASS（含 :0 无残留） | **PASS** | settings get = host:port → probe exitIp → DELETE 后 :0 + none + host/port 空 |
| 6 | 收尾：settings :0、token 移除、探针卸载、fixture 停止、adb reverse 撤销 | **PASS** | 见第 8 节实测输出 |
| 7 | 报告 docs/design/egress/t15-acceptance.md | **PASS** | 本文件 + evidence-t15/ 8 份日志 |

非阻塞观察（不影响上面 7 条，建议文档跟进）：**O1 SOCKS5 需要用户名密码时不可用**——属性注入正确
（java.net.socks.username/password 在进程内可读到），但 libcore 的 SOCKS 客户端不使用这两个属性发凭据，
代理侧实测 <code>methods=0002 user_ok=0 pass_ok=0</code>。详见 3.3 节。

## 1. 测试装置（与 t9 的差异）

| 组件 | 说明 |
| --- | --- |
| fixtures/local_proxy.py（v2） | 三个监听：HTTP **18080**（设备级）/ HTTP **18082**（应用级）/ SOCKS5 **18081**；每条日志带 <code>listener=&lt;端口&gt;</code>，**按监听端口归因** |
| 虚拟主机 | probe.fixture → 198.51.100.7（只有设备级探测用）、app.fixture → 198.51.100.8（只有探针应用用）、198.51.100.9 → raw-socket-via-proxy-ok |
| SOCKS 模式 | prefer-noauth（默认，验证链路）/ require-userpass（观察 libcore 是否消费凭据属性） |
| fixtures/probe-app/（v2） | 探针 APK io.df.egressprobe：打印进程内代理属性与 ProxySelector 选择结果；HttpURLConnection 请求 http://app.fixture/ 与 http://198.51.100.7/；裸 Socket 连 198.51.100.9:80。凭据只输出 &lt;set:true/false&gt; |
| 归因规则 | 只要 fixture 出现 <code>listener=18082 … app.fixture</code>，就说明**该应用自己的请求**真的走了应用级代理；只看模块日志不算 |
| adb 隧道 | adb reverse tcp:18080/18081/18082，adb forward tcp:18686 tcp:8686 |

fixture 自检（Mac 侧）：

~~~
dev-level(18080) -> 198.51.100.7
app-level(18082) -> 198.51.100.8
socks(18081) -> raw-socket-via-proxy-ok
~~~

## 2. A 组：应用级 HTTP（F1，PASS）

配置：设备级 default=mac_http(127.0.0.1:18080)，应用级 [[apps]] io.df.egressprobe.egress=mac_http2(127.0.0.1:18082)。

~~~bash
curl -s -H "Authorization: Bearer $QA_TOKEN" -H 'Content-Type: application/json' \
  -H "If-Match: \"$REV\"" -X PUT --data @/tmp/put_a.json $DF_API/egress
adb shell 'am force-stop io.df.egressprobe; logcat -c; am start -W -n io.df.egressprobe/.MainActivity'
~~~

写入结果：changed:true、applied:true、readback.settingsValue=127.0.0.1:18080、
apps[0]={package:io.df.egressprobe, egress:mac_http2, effective:proxy}。

进程内读数（关键）：

~~~
11-24 22:07:12.796 10831 I EGRESSPROBE: http.proxyHost=127.0.0.1
11-24 22:07:12.796 10831 I EGRESSPROBE: http.proxyPort=18082      <-- 应用级值，不是设备级 18080
11-24 22:07:12.796 10831 I EGRESSPROBE: https.proxyHost=127.0.0.1
11-24 22:07:12.796 10831 I EGRESSPROBE: https.proxyPort=18082
11-24 22:07:12.796 10831 I EGRESSPROBE: selector.app.fixture=HTTP @ 127.0.0.1/<unresolved>:18082
11-24 22:07:12.796 10831 I EGRESSPROBE: fetch.app=status=200 body=198.51.100.8
11-24 22:07:12.796 10831 I EGRESSPROBE: fetch.ip=status=200 body=198.51.100.7
~~~

代理侧日志（**不是 self-report**）：

~~~
2026-11-24T22:07:07+0800 proto=http listener=18082 GET http://app.fixture/ host=app.fixture auth=0
2026-11-24T22:07:07+0800 proto=http listener=18082 GET http://198.51.100.7/ host=198.51.100.7 auth=0
2026-11-24T22:07:12+0800 proto=http listener=18080 CONNECT geller-pa.googleapis.com:443 auth=0
2026-11-24T22:07:16+0800 proto=http listener=18080 CONNECT digitalassetlinks.googleapis.com:443 auth=0
~~~

—— 应用请求落在 **18082**（应用级），同期系统应用仍走 **18080**（设备级），两条链路互不干扰。

模块侧（t14 修复后的新形态）：

~~~
[WARN] zygisk::hooks - [egress] proxy_selector degraded reason=of_method_missing sdk=Some(37) (libcore has no ProxySelector.of(Proxy); properties are authoritative and kept in place by the post-bind replay, so F1/F2 do not depend on it)
[INFO] zygisk::hooks - [egress] injected package=io.df.egressprobe type=http host=127.0.0.1 port=18082 creds=0 ok=6 failed=0 selector=degraded
[INFO] zygisk::hooks - [egress] replay hook armed image=libandroid_runtime device=65034 inode=1891 symbols=setpriority,__system_property_get
[INFO] zygisk - [egress] module kept mapped for the replay window
~~~

**结论：F1 通过** —— 属性值等于应用级配置，且代理侧日志独立证明了流量路径。

## 3. B 组：应用级 SOCKS5（PASS）

### 3.1 无认证 SOCKS5（链路通）

配置：设备级 mac_http(18080) 保持开启，应用级改为 mac_socks(127.0.0.1:18081)。

~~~
11-24 22:08:12.575 11102 I EGRESSPROBE: http.proxyHost=<null>
11-24 22:08:12.575 11102 I EGRESSPROBE: http.proxyPort=<null>
11-24 22:08:12.575 11102 I EGRESSPROBE: https.proxyHost=<null>
11-24 22:08:12.575 11102 I EGRESSPROBE: https.proxyPort=<null>
11-24 22:08:12.575 11102 I EGRESSPROBE: socksProxyHost=127.0.0.1
11-24 22:08:12.575 11102 I EGRESSPROBE: socksProxyPort=18081
11-24 22:08:12.575 11102 I EGRESSPROBE: java.net.socks.username=dfsocks
11-24 22:08:12.575 11102 I EGRESSPROBE: java.net.socks.password=<set:true>
11-24 22:08:12.575 11102 I EGRESSPROBE: selector.app.fixture=SOCKS @ 127.0.0.1/<unresolved>:18081
11-24 22:08:12.575 11102 I EGRESSPROBE: fetch.app=status=200 body=198.51.100.8
11-24 22:08:12.575 11102 I EGRESSPROBE: fetch.ip=status=200 body=198.51.100.7
~~~

代理侧：

~~~
2026-11-24T22:08:07+0800 proto=socks5 listener=18081 method=noauth CONNECT app.fixture:80
2026-11-24T22:08:07+0800 proto=socks5 listener=18081 tunnel-request app.fixture 164 bytes
2026-11-24T22:08:07+0800 proto=socks5 listener=18081 method=noauth CONNECT 198.51.100.7:80
2026-11-24T22:08:07+0800 proto=socks5 listener=18081 tunnel-request 198.51.100.7 165 bytes
~~~

要点：
- **§5.3 互斥在设备级代理压力下仍成立**：设备级把 http/https.proxyHost/Port 写进去后，replay 又把它们清空，
  最终进程内只有 socks 属性（上表 http/https 全 null）。
- HttpURLConnection 经 DefaultProxySelector 读到 socks 属性，**真实流量经 SOCKS5 隧道**返回 198.51.100.8。
- 探针里的裸 Socket 报 ConnectException：普通 Socket(host,port) 不经 socksProxyHost（JDK 语义），
  该项不作为 socks 证据，链路证据以上面的 tunnel-request 为准。

### 3.2 模块日志

~~~
[INFO] zygisk::hooks - [egress] injected package=io.df.egressprobe type=socks5 host=127.0.0.1 port=18081 creds=1 ok=8 failed=0 selector=degraded
~~~

### 3.3 观察 O1：需要认证的 SOCKS5 走不通（非阻塞）

同一份配置，仅把 fixture 切成 require-userpass：

~~~
2026-11-24T22:10:00+0800 proto=socks5 listener=18081 methods=0002 auth user_ok=0 pass_ok=0 (values never logged)
2026-11-24T22:10:00+0800 proto=socks5 listener=18081 method=auth-failed rejected
~~~

进程内属性本身是对的（java.net.socks.username=dfsocks、password=&lt;set:true&gt;），说明注入没问题；
是 **libcore 的 SOCKS 客户端不使用这两个属性发凭据**（它仍 offer USERPASS，但发出去的用户名/密码不是我们写的值）。
契约 §5.1 第 7 项把 java.net.socks.username/password 当作凭据载体，在这台 Android 17 上不成立。
建议：文档标注该限制；若要支持认证 SOCKS，需要另装 java.net.Authenticator（会影响同进程所有连接，需评估）。
本轮判定不影响「应用级 socks5 生效」（无认证代理已证明链路通），但下游不应假设带凭据的 socks 可用。

## 4. C 组：__DIRECT__ 在设备级代理开启时直连（F2，PASS）

配置：设备级 mac_http(18080) 保持开启，应用级 io.df.egressprobe.egress=__DIRECT__。

~~~
$ adb shell 'settings get global http_proxy'
127.0.0.1:18080                    # 设备级确实开着

11-24 22:08:35.322 11251 I EGRESSPROBE: http.proxyHost=<null>
11-24 22:08:35.322 11251 I EGRESSPROBE: http.proxyPort=<null>
11-24 22:08:35.322 11251 I EGRESSPROBE: https.proxyHost=<null>
11-24 22:08:35.322 11251 I EGRESSPROBE: https.proxyPort=<null>
11-24 22:08:35.322 11251 I EGRESSPROBE: socksProxyHost=<null>
11-24 22:08:35.322 11251 I EGRESSPROBE: socksProxyPort=<null>
11-24 22:08:35.322 11251 I EGRESSPROBE: selector.app.fixture=DIRECT
11-24 22:08:35.322 11251 I EGRESSPROBE: fetch.app=error=UnknownHostException
11-24 22:08:35.322 11251 I EGRESSPROBE: fetch.ip=error=SocketTimeoutException
~~~

代理侧同一时刻：

~~~
$ curl ... -X POST --data '{}' $DF_API/egress/probe     # 设备级探测仍正常
{"ok": true, "exitIp": "198.51.100.7", "latencyMs": 6, "via": "proxy", "proxy": "mac_http", ...}

2026-11-24T22:08:47+0800 proto=http listener=18080 GET http://probe.fixture/ host=probe.fixture auth=0
app.fixture 命中数=0
~~~

模块日志：<code>[egress] injected package=io.df.egressprobe type=direct host=- port=- ok=6 failed=0 selector=degraded</code>。

**结论：F2 通过** —— 设备级代理开着，__DIRECT__ 应用进程内属性全空、请求直连（DNS 失败/超时），
代理侧完全没收到它的流量，而设备级探测仍走 18080。

## 5. D 组：未配置应用零注入（PASS）

对象：com.reqable.android（config.toml 里 0 命中，既不在 [[apps]] 也不在模板 packages）。

~~~
BEFORE egress-log-lines=100
AFTER  egress-log-lines=100
delta=0                       # 启动该应用没有新增任何 [egress] 行
该包相关 [egress] 行 = 0
未配置应用启动的整段日志新增行数 = 0（before=2573 lines / after=2573 lines）

# 对照：已配置的探针应用启动
configured app: before=100 after=104 delta=4
  degraded / injected / replay hook armed / module kept mapped 各一行
~~~

**结论：通过。** t14 修正了「设备级 default 会让每个应用都挂 hook+线程」的行为，未配置应用不再有注入与重放线程。
说明：/proc/&lt;pid&gt;/maps 不能用来判定（Zygisk 映射路径不含 device_faker，已配置应用同样查不到），
因此证据以**模块侧自己写的日志**为准——它不是应用 self-report，且未配置/已配置两组差异稳定（0 vs 4）。

## 6. F3：ProxySelector 缺失的降级记录（PASS）

- 每个命中注入的进程只记一行降级说明，且**不再计为 failed**：
  injected … ok=6 failed=0 selector=degraded（t9 时是 ok=6 failed=1）。
- 日志正文自解释：libcore has no ProxySelector.of(Proxy); properties are authoritative and kept in place by the
  post-bind replay, so F1/F2 do not depend on it。
- 属性路径未被掩盖：A 组 http.proxyPort=18082 + 代理侧 18082 日志；B 组 socks 属性 + SOCKS 隧道日志；
  C 组全空 + DIRECT。即：selector 降级不影响 F1/F2 的判定。

## 7. 回归：设备级 PUT / probe / DELETE（PASS）

~~~
# 状态
$ adb shell 'settings get global http_proxy'                 -> 127.0.0.1:18080
$ adb shell 'dumpsys connectivity | grep -i "Global proxy"'   -> Global proxy: [127.0.0.1] 18080 xl=

# probe（设备级链路）
{"ok": true, "exitIp": "198.51.100.7", "latencyMs": 8, "scope": "device", "via": "proxy",
 "proxy": "mac_http", "upstreamStatus": 200, "approximate": false, "warnings": []}
2026-11-24T22:09:35+0800 proto=http listener=18080 GET http://probe.fixture/ host=probe.fixture auth=0

# DELETE /egress?confirm=true
{"changed": true, "mode": "direct", "effectiveMode": "direct", "default": null, "upstream": null,
 "applied": true, "readback": {"settingsValue": ":0", "dumpsysGlobalProxy": "Global proxy: none", "matches": true},
 "warnings": []}

$ adb shell 'settings get global http_proxy'
:0
$ adb shell 'settings list global | grep -i proxy'
global_http_proxy_exclusion_list=
global_http_proxy_host=          # 无残留：host 空
global_http_proxy_port=0         # 无残留：port 0
global_proxy_pac_url=
http_proxy=:0
$ adb shell 'dumpsys connectivity | grep -i proxy'
  Global proxy: none
  Default proxy: none
~~~

清空后冷启动探针（应回到继承/直连）：http.proxyHost=&lt;null&gt;、socksProxyHost=&lt;null&gt;、selector.app.fixture=DIRECT。

## 8. 收尾状态（必须项，PASS）

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
$ adb shell ps -A | grep device_faker_server  -> 运行中（tokens=1，临时 Token 已移除）
$ adb reverse --list                          -> （空）
~~~

已执行的清理：DELETE /egress?confirm=true；恢复测试前的 config.toml（去掉 [[apps]] io.df.egressprobe 与 [egress]）；
恢复 server.toml 备份（移除临时 t15-qa Token）；卸载探针应用；删除 /data/local/tmp 临时文件；
删除测试期 egress.toml；停止 fixture 代理；adb reverse --remove-all / adb forward --remove tcp:18686。

## 9. 证据文件

| 文件 | 内容 |
| --- | --- |
| evidence-t15/cargo-checks.log | git log、cargo fmt --check、cargo clippy、cargo test（226 passed）原始输出 |
| evidence-t15/t15-a-app-http.log | A 组：F1 应用级 HTTP（属性 + selector + fixture 18082 归因） |
| evidence-t15/t15-b-app-socks.log | B 组：应用级 SOCKS5（属性、互斥、SOCKS 隧道日志） |
| evidence-t15/t15-b2-socks-auth.log | O1：require-userpass 下的凭据行为（methods=0002 user_ok=0 pass_ok=0） |
| evidence-t15/t15-c-direct.log | C 组：__DIRECT__（属性为空 + 设备级探测仍走 18080 + app.fixture 命中 0） |
| evidence-t15/t15-d-zero-injection.log | D 组：未配置应用零注入（delta 0 vs 配置应用 delta 4） |
| evidence-t15/t15-regression-device-level.log | 回归：设备级 PUT/probe/DELETE、:0 无残留 |
| evidence-t15/t15-e-hook-window.log | 复核项 4-7：CASE C/D、armed 映射与可检出性、hook fired、replay t=26ms/elapsed=2001ms、窗口边界 |
| evidence-t15/t15-final-state.log | 收尾与最终状态实测（两轮 cleanup 的 DELETE/恢复 token/残留检查） |
| evidence-t15/fixture-proxy.log | fixture 侧全部请求（带 listener 端口归因） |
| fixtures/local_proxy.py | 本轮 fixture（三监听 + 归因 + SOCKS 模式开关） |
| fixtures/probe-app/ | 探针 APK 源码（v2）与已签名 egress-probe.apk |

## 10. 复现步骤（最短路径）

~~~bash
# 0) 构建 + 装机（仓库根目录）
export ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk
RUSTFLAGS="-C default-linker-libraries" cargo +nightly ndk --platform 31 -t arm64-v8a build --release
cargo ndk --platform 29 -t arm64-v8a build -p device_faker_server --release
adb push target/aarch64-linux-android/release/libzygisk.so /data/adb/modules/device_faker/zygisk/arm64-v8a.so
adb push target/aarch64-linux-android/release/device_faker_server /data/adb/modules/device_faker/bin/
adb reboot   # zygisk .so 需开机重新加载

# 1) fixture + 隧道
python3 docs/design/egress/fixtures/local_proxy.py &     # 18080/18082 HTTP, 18081 SOCKS5
adb reverse tcp:18080 tcp:18080; adb reverse tcp:18081 tcp:18081; adb reverse tcp:18082 tcp:18082
adb forward tcp:18686 tcp:8686

# 2) A 组：设备级 mac_http(18080) + 应用级 mac_http2(18082)
#    PUT /egress {"default":"mac_http","proxies":[18080,18082,socks],"apps":[{probe,mac_http2}]}
adb install -r docs/design/egress/fixtures/probe-app/egress-probe.apk
adb shell 'am force-stop io.df.egressprobe; am start -W -n io.df.egressprobe/.MainActivity; logcat -d -s EGRESSPROBE:I'
#    期望：http.proxyPort=18082，且 fixture 出现 listener=18082 ... app.fixture

# 3) B 组：apps[{probe,mac_socks}] → 期望 socksProxyHost/Port + socks 隧道日志
# 4) C 组：apps[{probe,__DIRECT__}] → 期望属性全 null 且 fixture 无 app.fixture
# 5) D 组：启动任一未配置应用 → 模块日志无新增 [egress] 行
# 6) 收尾：DELETE /egress?confirm=true；adb shell 'settings put global http_proxy :0'
~~~

## 11. 与「复核清单（10 项）」的逐项对照

上文第 2–8 节是按 t9 的 7 条口径组织的；captain 复核清单多了 4 项（CASE C/D、armed 映射、
hook fired/unavailable、重放窗口边界），本节逐条对照，证据见 evidence-t15/t15-e-hook-window.log。

| # | 复核项 | 结论 | 证据 |
| --- | --- | --- | --- |
| 1 | 复用 t9 fixture/探针，覆盖 http 生效、socks5 生效、__DIRECT__ 直连、未配置零注入 | PASS | 第 2/3/4/5 节 |
| 2 | F1：进程内 http.proxyHost/Port = 应用级值，且 fixture 证明流量经该端口 | PASS | 第 2 节（18082 + listener=18082） |
| 3 | F2：设备级生效时 __DIRECT__ 属性为空且 fixture 未收到其流量 | PASS | 第 4 节 |
| 4 | 新增：CASE C（未配置）与 CASE D（设备级继承）下应用无 [egress] 日志、无 hook、/proc/self/maps 无模块映射残留 | PASS（maps 一项按「按名字不可见」判定，见 F-1） | 见下方 CASE C/D 实测 |
| 5 | 新增：armed 应用确实保留模块 .so 映射（已知代价）+ 写明可检出性影响 | PASS（保留映射用行为证据；「maps 可检出」与实测不符，见 F-1） | 见下方「映射与可检出性」 |
| 6 | 新增：setpriority PLT hook 命中证据（hook fired）与未命中降级日志（replay hook unavailable）各一条 | **PARTIAL → 判 FAIL**（只拿到 hook fired，unavailable 一条真机无法制造） | 见 F-2 |
| 7 | 新增：重放窗口边界——启动期覆盖被纠正（t=xx ms），运行中改设备级代理不在窗口内属已知限制 | PASS | t=26ms reason=framework_overwrite；窗口 elapsed=2001ms / probes=118 |
| 8 | 回归：设备级 PUT/probe/DELETE（含 :0 无残留） | PASS | 第 7 节 |
| 9 | 收尾：settings :0、token 移除、fixture 停止、adb reverse 撤销 + 最终状态输出 | PASS | 第 8 节 |
| 10 | 报告 docs/design/egress/t15-acceptance.md | PASS | 本文件 |

### CASE C / CASE D（复核项 4）

~~~
CASE C：设备级 :0 + 未配置应用 com.reqable.android
  device_faker.log 行数 before=2603 after=2603 delta=0
  该包 [egress] 行数=0
  logcat 中该进程的 DeviceFaker 行数=0（另有一条是 adbd 自己记录命令行，不是模块日志）
  /proc/<pid>/maps 命中 device_faker|arm64-v8a|zygisk = 0

CASE D：设备级 default=mac_http(18080) 开启 + 同一未配置应用（继承，不注入）
  device_faker.log 行数 before=2604 after=2604 delta=0
  该包 [egress] 行数=0
  logcat 中该进程的 DeviceFaker 行数=0
  /proc/<pid>/maps 命中 = 0
  该应用仍由框架继承设备级代理（fixture 侧继续出现它经 18080 的 CONNECT）
~~~

结论：t14 把注入条件收紧为「只有显式 `[[apps]].egress` 才注入」的行为在真机成立——设备级 default
不再给每个应用挂 hook / 起重放线程；未配置应用与「继承设备级」应用都没有 `[egress]` 日志、
没有 hook 日志、也没有模块映射。

### 映射与可检出性（复核项 5）

- **armed 进程（显式配了应用级 egress）确实保留模块代码**：hook fired 发生在 specialize 之后
  （22:15:02.274），重放线程在应用进程内跑满 2001ms（tid 13246）并输出 replay done；
  此后进程继续存活、继续创建线程（旋转/重建 Activity）也没有因调用 setpriority 崩溃——
  若 .so 已被 dlclose，这些晚到的 hook 调用会直接 SIGSEGV。
- **但 /proc/<pid>/maps 里看不到任何模块名**：armed 进程 3170 行 maps 中，
  device_faker|arm64-v8a|libzygisk 命中 0，[anon: 名字里也没有 egress/zygisk 相关项
  （Zygisk Next 在本机把模块代码加载成无名映射；只有 companion 进程的 maps 里能看到
  /data/adb/modules/device_faker/zygisk/arm64-v8a.so，7 行）。
- **可检出性影响（写给下游）**：按名字扫 maps 检测不到本模块在应用进程中的存在（比
  docs/CHANGELOG 里「maps 可检出」的说法更弱），因此这条「已知代价」在当前构建上并未实际暴露；
  但 zygisk 自身（zygiskd64 进程、/data/adb/modules 挂载、Zygisk 相关线程）仍可被检测。
  建议 docs 更正为「映射不可按名字检出（无名映射），代价仅体现为进程内常驻代码页」。

### F-1（docs / low）文档称「显式配 egress 的进程会保留模块 .so 映射（/proc/self/maps 可检出）」，实测按名字检不出

- 影响面：只是「已知代价 / 可检出性」表述过强，不影响功能。
- 建议：docs/CHANGELOG.md、docs/en/CHANGELOG.md、docs/CONFIG.md（及 en 版）把该句改成
  「保留映射 = 常驻代码页；按名字扫 maps 不可见（无名映射），companion 进程的 maps 才能看到模块路径」。
- 证据：见本节「映射与可检出性」。

### F-2（testability / low，导致复核项 6 判 FAIL）replay hook unavailable 在真机不可制造

- 该日志只有两个触发条件（src/hooks.rs:635、src/hooks.rs:686）：
  libandroid_runtime.so 两个路径都 stat 不到，或 plt_hook_commit() 失败。
  Pixel 9 上两者都不可能发生（镜像存在、两个符号都能提交成功），因此无法取得真机样本。
- 已做到的替代验证：
  1. 装机二进制 strings 校验：两条降级文案都在（libandroid_runtime_not_found / commit_failed）；
  2. 源码位置与触发条件固定、无分支歧义；
  3. 正常路径的 hook fired / replay done 真机原文已取得（见下）。
- 建议：给 t14 加一个故障注入开关（例如系统属性 debug.device_faker.egress.disable_replay=1
  时跳过 arming、直接走 unavailable 分支），或由 captain 显式豁免该项；两者任一即可闭环。

### 重放窗口边界（复核项 7）

~~~
11-24 22:15:02.252 DeviceFaker: [egress] replay hook armed package=io.df.egressprobe
11-24 22:15:02.274 DeviceFaker: [egress] replay hook fired symbol=setpriority
11-24 22:15:02.274 DeviceFaker: [egress] replay start package=io.df.egressprobe trigger=post_zygote_setpriority
11-24 22:15:04.277 DeviceFaker: [egress] replay package=io.df.egressprobe t=26ms reason=framework_overwrite applied=6/6 changed=[http.proxyPort=18080->18082, https.proxyPort=18080->18082]
11-24 22:15:04.278 DeviceFaker: [egress] replay done package=io.df.egressprobe probes=118 replays=1 divergences=1 state=match elapsed=2001ms
~~~

- 启动期覆盖被纠正：注入后 t=26ms 检测到框架写入（18080→18082），当场重放 6/6 项。
- 窗口边界：重放按 replay_offsets_ms() 跑满 2001ms / 118 次探针 后 state=match 结束；
  窗口结束后应用仍在 18082（进程内已确认，且 fixture 在 18082 收到它的请求）。
- **已知限制（明确标注）**：应用运行中修改设备级代理不会落在这个 0–2000ms 窗口内，也不会被再次纠正——
  框架只在 handleBindApplication 写一次代理属性，之后没有触发点；改设备级代理后需要重启应用才生效。
  本轮尝试用旋转强制同进程 Activity 重建再读属性，未触发重建（logcat 无新实例），
  因此该限制以日志时间线 + 代码路径（replay_offsets_ms() 末项 2000ms）+ docs 记载为准，未取得额外样本。
