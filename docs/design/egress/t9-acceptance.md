# t9 真机验收：设备端出口代理（Pixel 9）

- 设备：Pixel 9（`tokay`），序列号 `4A190DLAQ002QR`，Android 17（SDK 37），KernelSU root
- 模块：`/data/adb/modules/devmask`，`module.prop` version=v1.10.0 / versionCode=20261201
- 设备端服务：`devmask_server` 1.10.0，bind `127.0.0.1:8686`，capabilities 含 `egress`
- 构建：工作区 HEAD `822bbc7` + 本增量未提交改动；`cargo ndk --platform 29 -t arm64-v8a build -p devmask_server --release`、
  `RUSTFLAGS="-C default-linker-libraries" cargo +nightly ndk --platform 31 -t arm64-v8a build --release`（两条均 exit 0）
- 验收动作全部通过 `/api/v1/egress*` 与 `adb shell` 完成；**未修改任何实现代码**（server/、src/、core/、manager/、webui/ 均未触碰）

## 结论总表

| # | 验收点 | 结论 | 关键证据 |
| --- | --- | --- | --- |
| 1 | `PUT /egress` 设置 HTTP 代理 → `settings get global http_proxy` 返回 host:port，dumpsys 显示 Global proxy | **PASS** | `127.0.0.1:18080`；`Global proxy: [127.0.0.1] 18080 xl=`；`applied:true` |
| 2 | `POST /egress/probe` 返回 exitIp 与延迟；断开代理后 5s 内 `egress.probe_failed` | **PASS** | `exitIp=198.51.100.7 latencyMs=5`；断开后 `502 egress.probe_failed` 用时 **0.021s** |
| 3 | `DELETE /egress` → 读回 `:0`，dumpsys none，无残留 `global_http_proxy_host/port` | **PASS** | `http_proxy=:0`、`host=`、`port=0`、`Global proxy: none` |
| 4 | 反向用例：手工制造脏状态，重启服务 reconcile 自愈 | **PASS** | `settings delete global http_proxy` 与错值 `10.9.9.9:1` 两次都被 `wrote=true` 修回 |
| 5 | 应用级 http 与 socks5，重启应用后进程内属性生效 | **PARTIAL FAIL** | socks5 生效（`socksProxyHost=127.0.0.1:18081`，含凭据）；**http 不生效**（进程内 `http.proxyHost=null`）；`__DIRECT__` 也拉不回直连 —— 见「缺陷」 |
| 6 | 执行 `module/uninstall.sh` 后系统代理清成 `:0` | **PASS** | `http_proxy=:0`、`Global proxy: none`、运行时目录已删除 |
| 7 | 服务日志与所有 API 响应无密码明文 | **PASS** | 28 个文件（日志/审计/API 响应/证据）扫描，两个 fixture 密码命中数 = **0** |

最终状态：`http_proxy=:0` / `Global proxy: none`（见文末「收尾状态」）。

## 测试装置（fixture）

| 组件 | 说明 |
| --- | --- |
| `fixtures/local_proxy.py` | 临时出口代理：HTTP 代理（绝对 URI + CONNECT）与 SOCKS5（含用户名/密码认证），同一进程双端口 18080 / 18081 |
| 虚拟主机 `probe.fixture` / `198.51.100.7` | 代理对这两个目标直接回 `198.51.100.7`（文档地址段），用来制造**可判定的假出口 IP**，不依赖外网 |
| 其他目标 | 真实转发（如 `checkip.amazonaws.com`），用于验证真实链路 |
| `adb reverse` | `tcp:18080`、`tcp:18081`，让设备用 `127.0.0.1` 访问 Mac 上的 fixture |
| `fixtures/probe-app/` | 最小探针 APK（`io.df.egressprobe`，javac+d8+aapt2+apksigner 构建）：冷启动时把 `System.getProperties()` 中的代理属性、`ProxySelector` 选择结果、以及一次真实 HTTP 请求结果写进 logcat 与文件；凭据属性只输出「是否已设置」 |
| 临时 Token | 为免动既有 `default` Token（可能仍被其它脚本使用），向 `server.toml` **追加**一个 `id="t9-qa"` 的 Token；验收结束后已恢复 `server.toml` 备份并重启服务 |

fixture 自检（Mac 侧，验证代理本身可用）：

```
$ curl -s -x http://127.0.0.1:18080 http://probe.fixture/
198.51.100.7
$ curl -s --socks5-hostname 127.0.0.1:18081 -U "$FIXTURE_SOCKS_USER:$FIXTURE_SOCKS_PASS" http://probe.fixture/
198.51.100.7
```

## 1) 设备级 HTTP 代理写入（PASS）

```bash
REV=$(curl -s -H "Authorization: Bearer $QA_TOKEN" $DF_API/egress | python3 -c 'import json,sys;print(json.load(sys.stdin)["revision"])')
curl -s -H "Authorization: Bearer $QA_TOKEN" -H 'Content-Type: application/json' \
  -H "If-Match: \"$REV\"" -X PUT --data @/tmp/put1.json $DF_API/egress
adb shell 'settings get global http_proxy'
adb shell 'dumpsys connectivity | grep -i proxy'
```

请求体（密码在日志里一律替换为 `<redacted>`）：

```json
{
  "default": "mac_http",
  "failOpen": true,
  "probeUrl": "http://probe.fixture/",
  "probeTimeoutSeconds": 5,
  "proxies": [
    { "name": "mac_http", "type": "http", "host": "127.0.0.1", "port": 18080 },
    { "name": "mac_socks", "type": "socks5", "host": "127.0.0.1", "port": 18081,
      "username": "dfsocks", "password": "<redacted>" }
  ]
}
```

输出（节选）：

```
HTTP/1.1 200 OK
x-dm-egress-revision: 4d3f84eef9cc7df20a165723ab8836163197a1ee431ddc890d26c16cbf9c902c
{
  "changed": true,
  "backupId": "toml.20261124T131637.080853588Z-f9f80df8",
  "mode": "proxy", "effectiveMode": "proxy", "default": "mac_http", "applied": true,
  "upstream": {"name":"mac_http","type":"http","host":"127.0.0.1","port":18080,"hasCredentials":false},
  "readback": {"settingsValue":"127.0.0.1:18080","dumpsysGlobalProxy":"Global proxy: [127.0.0.1] 18080 xl=","matches":true,"at":"2026-11-24T13:16:37Z"},
  "warnings": []
}

$ adb shell 'settings get global http_proxy'
127.0.0.1:18080

$ adb shell 'dumpsys connectivity | grep -i proxy'
mMultiProxyEnabled: false
ProxyTracker:
  Global proxy: [127.0.0.1] 18080 xl=
  Default proxy: none

$ adb shell 'settings list global | grep -i proxy'
global_http_proxy_exclusion_list=
global_http_proxy_host=127.0.0.1
global_http_proxy_port=18080
global_proxy_pac_url=
http_proxy=127.0.0.1:18080
```

设备侧落盘：`egress.toml` 权限 `-rw-------`（0600），`[probe]` 与两条 `[proxies.*]`；`config.toml` 出现
`[egress]\ndefault = "mac_http"`。**结论：写入 → settings → dumpsys 三层一致，PASS。**

## 2) 出口 IP 探测与断链失败（PASS）

```bash
curl -s -H "Authorization: Bearer $QA_TOKEN" -H 'Content-Type: application/json' \
  -X POST --data '{}' $DF_API/egress/probe
kill "$(cat /tmp/egress-fixture.pid)"          # 断开代理
curl -s -H "Authorization: Bearer $QA_TOKEN" -H 'Content-Type: application/json' \
  -X POST --data '{}' $DF_API/egress/probe -w '%{time_total}s'
```

成功：

```
HTTP 200 total=0.025278s
{"ok": true, "exitIp": "198.51.100.7", "latencyMs": 5, "checkedAt": "2026-11-24T13:16:58Z",
 "scope": "device", "via": "proxy", "proxy": "mac_http", "url": "http://probe.fixture/",
 "upstreamStatus": 200, "approximate": false, "warnings": []}
```

**代理真的被用到了**（fixture 日志；同一时刻还能看到其它系统应用经该代理发起 CONNECT，说明系统代理确实对整机生效）：

```
2026-11-24T21:16:45+0800 proto=http CONNECT android.googleapis.com:443 auth=0 auth_ok=1
2026-11-24T21:16:46+0800 proto=http CONNECT notifications-pa.googleapis.com:443 auth=0 auth_ok=1
2026-11-24T21:16:59+0800 proto=http GET http://probe.fixture/ host=probe.fixture auth=0 auth_ok=1
```

断开代理后 **0.021 秒**返回失败（远小于 5s 上限）：

```
HTTP 502 total=0.020945s
{"error":{"code":"egress.probe_failed",
          "details":{"reason":"invalid_response","timeoutMs":5000,"upstreamStatus":null,
                     "url":"http://probe.fixture/"},
          "message":"出口探测失败（invalid_response）","retryable":true}}
```

补充：断开时 `adb reverse` 仍在监听，所以设备侧连接是被「接受后立刻关闭」，reason 落到 `invalid_response`；
把 default 指向一个**没有监听**的端口（127.0.0.1:18099）则得到 `reason=connect`，耗时 0.64s，并附带 fail-open 警告：

```
HTTP 502 total=0.642905s
{"error":{"code":"egress.probe_failed","details":{"reason":"connect","timeoutMs":5000,
  "warnings":[{"code":"egress_degraded_to_direct","details":{"reason":"connect"},
               "message":"已按 fail-open 切为直连（配置保持不变）"}]}}}
```

**结论：探测返回出口 IP 与延迟；断链后 5s 内给出 `egress.probe_failed`，PASS。**

## 3) 回滚到直连（PASS，含历史坑复核）

```bash
curl -s -H "Authorization: Bearer $QA_TOKEN" -H "If-Match: \"$REV\"" -X DELETE $DF_API/egress   # 无 confirm → 400
curl -s -H "Authorization: Bearer $QA_TOKEN" -H "If-Match: \"$REV\"" -X DELETE "$DF_API/egress?confirm=true"
adb shell 'settings get global http_proxy; settings list global | grep -i proxy'
adb shell 'dumpsys connectivity | grep -i proxy'
```

```
# 缺 confirm=true
HTTP 400 {"error":{"code":"confirmation_required","details":{"action":"egress_clear"},...}}

# 正常 DELETE
{"changed": true, "mode": "direct", "effectiveMode": "direct", "default": null, "upstream": null,
 "applied": true,
 "readback": {"settingsValue":":0","dumpsysGlobalProxy":"Global proxy: none","matches":true}}

$ adb shell 'settings get global http_proxy'
:0
$ adb shell 'dumpsys connectivity | grep -i proxy'
  Global proxy: none
  Default proxy: none
$ adb shell 'settings list global | grep -i proxy'
global_http_proxy_exclusion_list=
global_http_proxy_host=
global_http_proxy_port=0
global_proxy_pac_url=
http_proxy=:0
```

**历史坑复核**：写代理后 `global_http_proxy_host/port` 会被系统同步成 `127.0.0.1/18080`；执行 `DELETE /egress`
之后这两个派生键回到空 / `0`，**没有残留指向旧代理**。重复 DELETE（新 revision）返回 `changed:false`（幂等）。
`egress.toml` 的 `[proxies.*]` 与 `config.toml` 的 `[egress]` 段均被清除，`[probe]` 段按契约保留。
**PASS。**

## 4) 反向用例：脏状态 reconcile 自愈（PASS）

### A. `settings delete global http_proxy`（历史坑：删除键不生效）

```
$ adb shell 'settings delete global http_proxy; settings get global http_proxy; settings list global | grep -i proxy'
Deleted 1 rows
null
global_http_proxy_exclusion_list=
global_http_proxy_host=127.0.0.1      # ← 脏状态：键没了，host/port 仍在
global_http_proxy_port=18080
global_proxy_pac_url=

$ adb shell 'dumpsys connectivity | grep -i proxy'
  Global proxy: [127.0.0.1] 18080 xl=     # ← 代理仍然生效，正是「delete 清不掉」的坑
```

重启服务：

```
$ adb shell 'sh /data/adb/modules/devmask/service.sh restart'
[2026-11-24T13:17:29.092738Z] INFO egress reconcile 完成（读回与期望一致） wrote=true effective_mode="from_config"
$ adb shell 'settings get global http_proxy'
127.0.0.1:18080                      # ← 自愈
$ adb shell 'dumpsys connectivity | grep -i proxy'
  Global proxy: [127.0.0.1] 18080 xl=
```

### B. `settings put global http_proxy 10.9.9.9:1`（错值）

```
$ adb shell 'settings put global http_proxy 10.9.9.9:1; settings get global http_proxy'
10.9.9.9:1
$ adb shell 'sh /data/adb/modules/devmask/service.sh restart'
[2026-11-24T13:17:40.372598Z] INFO egress reconcile 完成（读回与期望一致） wrote=true effective_mode="from_config"
$ adb shell 'settings get global http_proxy'
127.0.0.1:18080
$ curl -s -H "Authorization: Bearer $QA_TOKEN" $DF_API/egress     | python3 -c 'import json,sys;d=json.load(sys.stdin);print(json.dumps({k:d[k] for k in ("mode","effectiveMode","default","applied","readback","health")},ensure_ascii=False))'
{"mode": "proxy", "effectiveMode": "proxy", "default": "mac_http", "applied": true,
 "readback": {"settingsValue": "127.0.0.1:18080", "dumpsysGlobalProxy": "Global proxy: [127.0.0.1] 18080 xl=", "matches": true},
 "health": {"degraded": false, "degradedSince": null, "degradedReason": null, "lastProbe": null}}
```

**结论：两种脏状态都被启动 reconcile 修回期望值，PASS。**

## 5) 应用级注入（PARTIAL FAIL）

> **后续状态（2026-12-01 补注）**：本节 F1/F2/F3 已由 t14 修复并通过真机回归——注入改为「只对显式写法 + 进程过渡完成后由 PLT hook 引重放」，机制与三条代价见 [CONFIG.md 的「应用级注入如何生效」](../../CONFIG.md#应用级注入如何生效后置重放)。本文其余内容保留为**当时的失败证据**，不要按此判断当前行为。

装置：探针 APK `io.df.egressprobe`，`am force-stop` 后 `am start -W` 冷启动，读取进程内属性并落 logcat / 文件。

### 5.1 SOCKS5：生效（PASS）

```bash
curl -s -H "Authorization: Bearer $QA_TOKEN" -H 'Content-Type: application/json' \
  -H "If-Match: \"$REV\"" -X PUT \
  --data '{"apps":[{"package":"io.df.egressprobe","egress":"mac_socks"}]}' $DF_API/egress
adb shell 'am force-stop io.df.egressprobe; am start -W -n io.df.egressprobe/.MainActivity'
adb shell 'logcat -d -s EGRESSPROBE:I'
```

```
11-24 21:22:51.021 14267 14267 I EGRESSPROBE: http.proxyHost=<null>
11-24 21:22:51.021 14267 14267 I EGRESSPROBE: socksProxyHost=127.0.0.1
11-24 21:22:51.021 14267 14267 I EGRESSPROBE: socksProxyPort=18081
11-24 21:22:51.021 14267 14267 I EGRESSPROBE: java.net.socks.username=dfsocks
11-24 21:22:51.021 14267 14267 I EGRESSPROBE: java.net.socks.password=<set:true>
11-24 21:22:51.021 14267 14267 I EGRESSPROBE: selector.probe.fixture=SOCKS @ 127.0.0.1/<unresolved>:18081
```

模块侧日志（`debug = true` 时才会落盘）：

```
[2026-11-24 21:22:47.899] [INFO] zygisk::hooks - [egress] injected package=io.df.egressprobe type=socks5 host=127.0.0.1 port=18081 creds=1 ok=8 failed=1
[2026-11-24 21:22:47.899] [WARN] zygisk::hooks - [egress] inject failed package=io.df.egressprobe step=proxy_selector error=Method not found: of (Ljava/net/Proxy;)Ljava/net/ProxySelector;
```

### 5.2 HTTP：进程内不生效（FAIL）

```
# 设备级直连（default=null）+ 应用级 mac_http2(127.0.0.1:18082)
[2026-11-24 21:22:22.238] [WARN] zygisk::hooks - [egress] inject failed package=io.df.egressprobe step=proxy_selector error=Method not found: of (Ljava/net/Proxy;)Ljava/net/ProxySelector;
[2026-11-24 21:22:22.238] [INFO] zygisk::hooks - [egress] injected package=io.df.egressprobe type=http host=127.0.0.1 port=18082 creds=0 ok=6 failed=1
# 同一进程内读到的却是：
11-24 21:22:05.329 14028 I EGRESSPROBE: http.proxyHost=<null>
11-24 21:22:05.329 14028 I EGRESSPROBE: http.proxyPort=<null>
11-24 21:22:05.329 14028 I EGRESSPROBE: selector.probe.fixture=DIRECT
```

反证（设备级代理打开时，应用级配了 18082，但进程内看到的是设备级的 18080）：

```
# 设备级 default=mac_http(18080) + 应用级 mac_http2(18082)
11-24 21:21:16.310 13785 I EGRESSPROBE: http.proxyHost=127.0.0.1
11-24 21:21:16.310 13785 I EGRESSPROBE: http.proxyPort=18080        # ← 不是 18082
11-24 21:21:16.310 13785 I EGRESSPROBE: selector.probe.fixture=HTTP @ 127.0.0.1/<unresolved>:18080
```

### 5.3 `__DIRECT__`：拉不回直连（FAIL）

```
# 设备级 default=mac_http(18080) + 应用级 __DIRECT__
[2026-11-24 21:22:58.454] [INFO] zygisk::hooks - [egress] injected package=io.df.egressprobe type=direct host=- port=- creds=0 ok=6 failed=1
11-24 21:22:58.549 14390 I EGRESSPROBE: http.proxyHost=127.0.0.1      # ← 95ms 后又变回设备级代理
11-24 21:22:58.549 14390 I EGRESSPROBE: http.proxyPort=18080
11-24 21:22:58.549 14390 I EGRESSPROBE: selector.probe.fixture=HTTP @ 127.0.0.1/<unresolved>:18080
```

### 缺陷（按失败上报，未自行修改实现）

**F1（high，契约 §5.1 第 1–4 项）应用级 HTTP 代理在目标进程内不生效。**
注入点 `ZYGISK pre_app_specialize`（`src/lib.rs:242`）早于框架的每进程代理初始化：
`android.app.ActivityThread` 绑定应用时会按**设备级** `ProxyInfo` 写/清 `http.proxyHost`、`http.proxyPort`、
`https.proxyHost`、`https.proxyPort`，把我们刚写进去的值覆盖掉（设备级有代理 → 覆盖成设备级值；
设备级直连 → 直接清空）。证据是同一进程的时间顺序：`21:22:58.454` 注入 `type=direct ok=6`、
`21:22:58.549` 读到 `http.proxyHost=127.0.0.1`（设备级值），中间 95ms 只有框架会写这些属性。

**F2（high，契约 §5.2/§6「`__DIRECT__` 必须显式直连」）`__DIRECT__` 在设备级代理开启时无效。**
与 F1 同源：清空动作被框架随后的写入覆盖，结果配置了 `__DIRECT__` 的应用仍走设备级代理。

**F3（medium，契约 §5.5 的降级路径在这台设备上必然触发）`ProxySelector.of(Proxy)` 在 Android 17 (SDK 37) 不存在。**
每次注入都记录 `step=proxy_selector error=Method not found: of`（socks/http/direct 三种类型都失败），
因此「属性刷新失败时靠 ProxySelector 兜底」这条路在本机型上不存在，F1/F2 也没有兜底可言。
属性注入本身按契约「失败只记日志、不影响启动」处理正确（`ok=6/8`，应用正常冷启动）。

**影响**：设备级（方案 A）链路完全可用；应用级只有 SOCKS5 可用；应用级 HTTP 与 `__DIRECT__` 目前无效。
**未修复原因**：本任务限定只写测试产物，不改 `src/`。修复方向（供下游参考，未实施）：把注入点后移到
框架写完代理属性之后（例如 `ActivityThread.handleBindApplication` 之后的首个应用代码入口、或对
`android.net.Proxy`/属性写入做 hook 后置重放），并为 `ProxySelector` 提供不依赖 `of()` 的反射/
子类化替代方案。

## 6) 卸载清理（PASS）

```bash
adb shell 'tar -czf /data/local/tmp/devmask-runtime.t9.tgz -C /data/adb devmask'  # 先备份运行时目录
adb shell 'sh /data/adb/modules/devmask/uninstall.sh; echo exit=$?'
adb shell 'settings get global http_proxy; settings list global | grep -i proxy'
adb shell 'dumpsys connectivity | grep -i proxy'
```

```
exit=0
:0
global_http_proxy_exclusion_list=
global_http_proxy_host=
global_http_proxy_port=0
global_proxy_pac_url=
http_proxy=:0
  Global proxy: none
  Default proxy: none
$ adb shell 'ls -la /data/adb/devmask'
ls: /data/adb/devmask: No such file or directory
```

随后把备份解回并重启服务，配置与代理一起恢复（说明 uninstall 只清运行时，不动模块本体）：

```
$ adb shell 'sh /data/adb/modules/devmask/service.sh restart'
$ adb shell 'settings get global http_proxy'
127.0.0.1:18080
$ curl -s -H "Authorization: Bearer $QA_TOKEN" $DF_API/egress     | python3 -c 'import json,sys;d=json.load(sys.stdin);print(json.dumps({k:d[k] for k in ("mode","default","applied","readback")},ensure_ascii=False,indent=2))'
{"mode": "proxy", "default": "mac_http", "applied": true,
 "readback": {"settingsValue": "127.0.0.1:18080", "dumpsysGlobalProxy": "Global proxy: [127.0.0.1] 18080 xl=", "matches": true}}
```

## 7) 凭据边界（PASS）

扫描对象：`server.log`、`api-audit.jsonl`、`devmask.log`、以及本次全部 API 响应
（`GET /egress`、`GET /egress/proxies`、`GET /config`、`GET /overview`、`GET /status`、`PUT/DELETE /egress`、
`POST /egress/probe`）与 `docs/design/egress/evidence/` 下所有证据文件，共 28 个文件。

```
扫描文件数: 28
secret=FIXTURE_HTTP_PASS length=24 命中文件数=0
secret=FIXTURE_SOCKS_PASS length=24 命中文件数=0
结论：命中文件数为 0 即表示日志/API 响应/证据文件里没有密码明文。
```

- 响应里**没有任何 `password` 字段**，只出现 `hasCredentials`：

```
$ curl -s -H "Authorization: Bearer $QA_TOKEN" $DF_API/egress/proxies
{"revision":"...","total":3,"items":[
  {"name":"mac_http","type":"http","host":"127.0.0.1","port":18080,"hasCredentials":false,"usedAsDefault":true,"apps":[]},
  {"name":"mac_http2","type":"http","host":"127.0.0.1","port":18082,"hasCredentials":false,"usedAsDefault":false,"apps":[]},
  {"name":"mac_socks","type":"socks5","host":"127.0.0.1","port":18081,"hasCredentials":true,"usedAsDefault":false,"apps":[]}],
 "warnings":[]}
```

- 唯一出现明文的位置是设备侧 `egress.toml`（契约允许，权限 0600）：`-rw------- 1 root root 349 ... egress.toml`。
- 探针应用对 `java.net.socks.password` 也只输出 `<set:true>`（上面 5.1 的 logcat 可见）。
- 本报告与证据日志中的密码一律写为 `<redacted>` / `$FIXTURE_SOCKS_PASS`。

## 8) 收尾状态（必须项）

```
$ adb devices
List of devices attached
4A190DLAQ002QR	device

$ adb shell 'settings list global | grep -i proxy'
global_http_proxy_exclusion_list=
global_http_proxy_host=
global_http_proxy_port=0
global_proxy_pac_url=
http_proxy=:0

$ adb shell 'settings get global http_proxy'
:0

$ adb shell 'dumpsys connectivity | grep -i proxy'
mMultiProxyEnabled: false
ProxyTracker:
  Global proxy: none
  Default proxy: none
```

清理动作（均已执行并核验）：

1. `DELETE /egress?confirm=true` → `settings http_proxy=:0`；
2. 删除测试期间产生的 `egress.toml`（恢复为「文件不存在」），确认 `config.toml` 里无 `[egress]`、无测试
   `[[apps]]` 残留、无 `debug = true`（`grep` 三项均空）；
3. `server.toml` 恢复备份（临时 `t9-qa` Token 已移除，服务重启后 `tokens=1`）；
4. 卸载探针应用 `io.df.egressprobe`；删除 `/data/local/tmp` 下本次临时文件；
5. 关停 fixture 代理，`adb reverse --remove-all`、`adb forward --remove tcp:18686`。

已知无害残留：`config.toml` 因 `PUT /egress` 的 TOML 重写从 9361 字节变成 9374 字节（`toml_edit` 的
空白/结尾规范化，无 `[egress]`、无测试 app 记录）；`config/backups/` 下多了本次验收产生的备份文件
（服务自身保留最近 10 份，会自动老化）。

## 9) 证据文件索引

| 文件 | 内容 |
| --- | --- |
| `evidence/t0-egress-baseline.json` | 升级后首次 `GET /egress`（未配置态） |
| `evidence/t1-put-http-proxy.log` | 第 1 项：PUT → settings → dumpsys → 落盘 |
| `evidence/t2-probe.log` | 第 2 项：探测成功 / 断链失败 / connect 失败 / fail-open 警告 |
| `evidence/t3-delete.log` | 第 3 项：confirm 校验、DELETE、残留键复核、幂等 |
| `evidence/t4-reconcile.log` | 第 4 项：脏状态 A/B + 重启自愈 |
| `evidence/t5-app-injection.log` | 第 5 项：探针应用安装、应用级 http/socks5/`__DIRECT__` 冷启动读数 |
| `evidence/t5d-http-vs-platform.log` | 第 5 项：区分「注入没跑」与「被框架覆盖」的对照实验（18080 vs 18082） |
| `evidence/t5e-debug-injection.log` | 第 5 项：打开模块 debug 后的 `[egress] injected/inject failed` 原始日志 |
| `evidence/t5f-injection-truth.log` | 第 5 项：socks5 成功 / `__DIRECT__` 失效 的最终证据 |
| `evidence/t6-uninstall.log` | 第 6 项：uninstall.sh 前后 settings/dumpsys/目录 |
| `evidence/t7-credentials.log` | 第 7 项：凭据扫描（命中数 0）、`hasCredentials` 输出、`egress.toml` 0600 |
| `evidence/t9-final-state.log` | 收尾：DELETE、恢复 server.toml、最终 settings |
| `evidence/fixture-proxy.log` | fixture 代理请求日志（含设备真实经代理的 CONNECT/GET） |
| `fixtures/local_proxy.py` | 临时 HTTP + SOCKS5 代理（含虚拟主机 `probe.fixture` → 198.51.100.7） |
| `fixtures/probe-app/` | 探针 APK 源码 + `build.sh` + 已签名 `egress-probe.apk` |

## 10) 复现步骤（最短路径）

```bash
# 0) 构建并装到设备（本仓库根目录）
ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk cargo ndk --platform 29 -t arm64-v8a build -p devmask_server --release
RUSTFLAGS="-C default-linker-libraries" cargo +nightly ndk --platform 31 -t arm64-v8a build --release
adb push target/aarch64-linux-android/release/devmask_server /data/adb/modules/devmask/bin/
adb push target/aarch64-linux-android/release/libzygisk.so /data/adb/modules/devmask/zygisk/arm64-v8a.so
adb reboot   # zygisk .so 需要开机重新加载

# 1) 起 fixture 与隧道（Mac 侧）
. /tmp/egress-fixture.env      # FIXTURE_HTTP_PORT/FIXTURE_SOCKS_PORT/FIXTURE_LOG/账号密码
python3 docs/design/egress/fixtures/local_proxy.py &
adb reverse tcp:18080 tcp:18080; adb reverse tcp:18081 tcp:18081
adb forward tcp:18686 tcp:8686

# 2) 第 1~3 项：见上面各节命令
# 3) 第 5 项：adb install -r docs/design/egress/fixtures/probe-app/egress-probe.apk
#    adb shell 'am force-stop io.df.egressprobe; am start -W -n io.df.egressprobe/.MainActivity; logcat -d -s EGRESSPROBE:I'
# 4) 收尾：DELETE /egress?confirm=true + adb shell 'settings put global http_proxy :0'
```
