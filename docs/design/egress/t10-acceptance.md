# t10 管理端验收：代理池 → 绑定 → 批量下发 → 探测

- 时间：2026-11-24 22:47–22:58（CST）
- 本轮构建：cargo build -p devmask_manager → target/debug/devmask_manager（22:47，version 0.2.0）
- 启动方式（另起端口 + 临时 data-dir，未触碰 8765）：
  ./target/debug/devmask_manager --bind 127.0.0.1:18765 --data-dir /tmp/dm-t10 --static-dir manager/webui/dist --token admin123
- /healthz：status=ok, version=0.2.0, **db.schemaVersion=2**（evidence-t10/healthz.json）
- 8765 旧实例：验收前后 lsof 均为 device_fa PID 48145（未动）
- 前端：manager/webui 构建产物（dist，build 22:47，含 ProxiesPage chunk），由 manager 静态托管；
  UI 全部请求打到 http://127.0.0.1:18765/relay/api/v1/*，**没有使用 VITE_MOCK**
- 未修改实现代码；新增测试装置 docs/design/egress/fixtures/mock_device_egress.py

## 结论总表（8 条）

| # | 验收项 | 结论 | 证据 |
| --- | --- | --- | --- |
| 1 | 本轮构建 + schemaVersion=2 + 18765 + 临时 data-dir，未动 8765 | PASS | healthz.json + lsof 记录 |
| 2 | 代理池 CRUD；删除被绑定代理 409 egress_proxy_in_use | PASS | t10-1-crud.log、delete-bound-409.json |
| 3 | PUT 整机 + 应用级各一次，GET 读回一致（含 deviceState） | PASS | bind-1/bind-2 日志与 JSON |
| 4 | apply 产生 job kind=egress，含 ok/skipped/failed 与 egress_unchanged、device_offline | PASS | apply-1..4 job JSON |
| 5 | UI 冒烟（真实后端）+ 截图 | PASS | docs/design/manager-egress/01..13 |
| 6 | 全量响应与审计无密码明文（含 PATCH/apply） | PASS | t10-6-security.log（35 文件 + DB/WAL/key + 日志，命中 0） |
| 7 | 报告 | PASS | 本文件 |
| 8 | 回归 | PASS | cargo test -p devmask_manager：142 passed / 0 failed |

## 1. 测试装置

| 组件 | 说明 |
| --- | --- |
| mock 设备（本轮新增） | docs/design/egress/fixtures/mock_device_egress.py：按 egress-contract §1 实现 /api/v1/status、/overview、GET/PUT/DELETE /api/v1/egress、POST /api/v1/egress/probe、GET /api/v1/egress/proxies |
| 4 台 mock | A: mock-a-ok（18686, ok）、B: mock-b-ok（18687, ok）、C: mock-c-fail（18688, GET/PUT /egress 返回 500）、D: mock-d-offline（18689，验收中途 kill 制造离线） |
| fixture 代理 | docs/design/egress/fixtures/local_proxy.py：HTTP 代理 18080（Manager 侧探测经它）、SOCKS5 18081；虚拟主机 probe.fixture → 198.51.100.7 |
| 说明 | 仓库自带的 manager/docs/mock_device.py 没有 /api/v1/egress，用它做 egress 下发只会得到 egress_unsupported(404)，因此本轮补了一个带 egress 的 mock（建议纳入 manager/docs 或 tests harness，见第 8 节观察） |

设备添加（4 台均 HTTP 201、online=true、model=Pixel 9）：

    curl -s -X POST -H "Authorization: Bearer admin123" -H 'Content-Type: application/json' \
      -d '{"name":"Mock-A-ok","baseUrl":"http://127.0.0.1:18686","token":"mocktoken","tags":["t10"]}' \
      http://127.0.0.1:18765/relay/api/v1/devices

## 2. 第 1 项：代理池 CRUD + 409（PASS）

创建（HTTP 201，脱敏字段齐全）：

    curl -s -X POST ... -d '{"name":"t10-http","type":"http","host":"127.0.0.1","port":18080,"region":"local","tags":["t10"]}' \
      $BASE/egress-proxies
    {
      "id": "egp_b663eb1e23c04887ba66bf1e0405d00c", "name": "t10-http", "type": "http",
      "host": "127.0.0.1", "port": 18080, "hasCredentials": false, "region": "local",
      "tags": ["t10"], "enabled": true, "boundDevices": 0, "boundApps": 0,
      "lastCheck": null, "createdAt": "...", "updatedAt": "..."
    }

- 列表：total=3（t10-http / t10-socks / t10-tmp），响应只有 hasCredentials，没有 password/username 值。
- 详情：GET /egress-proxies/{id} → 200。
- PATCH 双态：{"name":"t10-tmp2","username":"tmpuser","password":"<PW>","tags":["t10","patched"]} → 200、hasCredentials=true；
  再 PATCH {"username":null,"password":null} → 200、hasCredentials=false（清除）。
- DELETE 未绑定代理 → 204。
- **DELETE 被绑定代理**（t10-http 已绑到 mock-a-ok）→ **409**：

    {"error":{"code":"egress_proxy_in_use",
      "details":{"apps":[],"devices":["mock-a-ok"],"name":"t10-http",
                 "proxyId":"egp_b663…","total":1},
      "message":"代理 t10-http 仍被 1 条绑定引用","retryable":false}}

## 3. 第 2 项：整机与应用级绑定 + 读回（PASS）

    # 整机（app 为空串）
    curl -s -X PUT ... -d '{"bindings":[{"app":"","proxyId":"egp_b663…"}]}' \
      $BASE/devices/mock-a-ok/egress
    {"deviceId":"mock-a-ok","changed":true,
     "bindings":[{"app":"","mode":"proxy","proxyId":"egp_b663…","proxyName":"t10-http","updatedAt":"…"}],
     "pendingApply":true,"warnings":[]}

    # 整机 + 应用级各一条
    -d '{"bindings":[{"app":"","proxyId":"egp_b663…"},{"app":"com.xingin.xhs","proxyId":"egp_72c0…"}]}'
    → 200，bindings 2 条（整机 t10-http + com.xingin.xhs → t10-socks）

GET /devices/mock-a-ok/egress 读回与写入完全一致，并带 **deviceState 透传**（来自设备端 GET /api/v1/egress）：

    deviceState keys = [applied, apps, configRevision, configured, default, effectiveMode,
                        egressFile, exclusionList, failOpen, health, mode,
                        probeTimeoutSeconds, probeUrl, proxies, readback, revision,
                        upstream, warnings]
    deviceState.mode=direct  applied=true  readback.settingsValue=":0"   ← 绑定尚未下发前的设备真实态

## 4. 第 3 项：批量下发 job kind=egress（PASS）

### apply#1（A/B/C/D，probe=true）→ HTTP 202

    {"jobId":"job_01a20e07e9a4…","status":"queued","kind":"egress","total":5,...}
    job 终态：status=partial  total=5  succeeded=4  skipped=0  failed=1
     - mock-a-ok        app=''                 ok      code=—                 （整机下发成功）
     - mock-a-ok        app='com.xingin.xhs'   ok      code=—                 （应用级下发成功）
     - mock-b-ok        app=''                 ok      code=egress_unchanged  （已是期望态，未重复下发）
     - mock-c-fail      app=''                 failed  code=device_error      （设备返回 500）
     - mock-d-offline  app=''                  ok      code=egress_unchanged

### apply#2（A/B，probe=false）→ 幂等重放

    status=succeeded total=3 succeeded=3
     - mock-a-ok ''、mock-a-ok 'com.xingin.xhs'、mock-b-ok '' → 全部 ok + code=egress_unchanged

### apply#3（kill 掉 mock-d 后只下发 D）

    status=partial total=1 skipped=1
     - mock-d-offline ''  skipped  code=device_offline  message=连接失败

### UI 触发的 apply（job_01a20e0fbc42…，见第 6 节截图 13）

    status=partial kind=egress total=5 ok=3 skipped=1 failed=1
     - mock-a-ok ''                ok      egress_unchanged
     - mock-a-ok 'com.xingin.xhs'  ok      egress_unchanged
     - mock-b-ok ''                ok      egress_unchanged
     - mock-c-fail ''              failed  device_error
     - mock-d-offline ''           skipped device_offline

**ok / skipped / failed 三种状态 + egress_unchanged + device_offline 全部覆盖。**

## 5. 第 4 项：探测（PASS）

    # HTTP 代理（url 覆盖为 fixture 虚拟主机）
    curl -s -X POST ... -d '{"url":"http://probe.fixture/"}' $BASE/egress-proxies/<http>/probe
    {"proxyId":"egp_b663…","ok":true,"exitIp":"198.51.100.7","latencyMs":0,
     "checkedAt":"2026-11-24T14:48:49Z","url":"http://probe.fixture/","upstreamStatus":200}

    # 代理侧日志证明真的经代理出去
    2026-11-24T22:48:49+0800 proto=http listener=18080 GET http://probe.fixture/ host=probe.fixture auth=0

    # egress_checks 落库（GET /egress-proxies/{id}.lastCheck）
    http  lastCheck = {source:"manager", ok:true,  exitIp:"198.51.100.7", latencyMs:0, errorCode:null}
    socks lastCheck = {source:"manager", ok:false, exitIp:null, latencyMs:null, errorCode:"unsupported_proxy_type"}

    # socks5 探测 → HTTP 422
    {"error":{"code":"validation_failed","details":{"errors":[
      {"code":"unsupported_proxy_type","message":"Manager 侧探测只支持 HTTP 代理","path":"proxyId"}]}}}

即：HTTP 探测返回 exitIp 且写 egress_checks(ok=1)；socks5 返回 422 unsupported_proxy_type 且落 ok=0。

## 6. 第 5 项：UI 冒烟（真实后端，PASS）

前端由 manager 静态托管（http://127.0.0.1:18765/），所有请求命中 /relay/api/v1/*；Token 存 localStorage.dm_token。

| 截图（docs/design/manager-egress/） | 内容 |
| --- | --- |
| 01-dashboard.png | 总览：设备 4/4、代理池 2 |
| 02-proxies-list.png | 代理池列表（代理定义、绑定数、最近探测列） |
| 03-proxy-create.png | 新建代理弹窗（名称/类型/主机/端口/只写凭据/地区/标签/启用） |
| 04-proxies-after-create.png | UI 创建 ui-proxy 后 3/3 个代理（POST 201） |
| 05-probe-result-toast.png | UI 行内「探测」→ toast「出口 IP <public-egress-ip> · 544 ms（刚刚）」（POST …/probe 200） |
| 06-edit-credentials-state.png | 编辑弹窗：类型 socks5「应用级；Manager 探测返回 422」、密码框为空、「清除已保存的凭据（当前状态：已设置）」 |
| 07-devices-egress-card.png | 设备卡「出口代理」行：t10-http + 出口 198.51.100.7 + 最近探测时间；toolbar「未配代理」筛选与计数（4 台 · 3 台未配代理） |
| 08-unbound-filter.png | 勾选「未配代理」后列表 4 → 3 台（只剩未绑定的 B/C/D） |
| 09-device-drawer-fail-device.png | 设备详情抽屉（C）：出口代理「未配置」+ 设备返回 HTTP 500 的错误呈现 |
| 10-device-drawer-bound.png | 设备详情抽屉（A）：设备级 t10-http / 出口 IP / 绑定行（整机 + com.xingin.xhs→t10-socks）/ 设备端 mode=proxy、已生效、读回一致、fail-open、settings 值 |
| 11-batch-bind-dialog.png | 「绑定设备」弹窗：应用留空=整机、probe 开关、只看未配代理、已选 3 台、当前已绑定 1 条 |
| 12-batch-dryrun.png | 预演（dryRun）计划表：3 项（设备/应用/模式/代理） |
| 13-job-results.png | 下发结果表：成功 3 / 跳过 1 / 失败 1，逐行 code（egress_unchanged / device_error / device_offline） |

UI 侧动作与后端对应（浏览器 network 面板可见）：
POST /egress-proxies(201) → GET /egress-proxies(200) → POST /egress-proxies/{id}/probe(200)
→ PUT /devices/{id}/egress → POST /devices/egress/apply(200 dryRun / 202 job) → GET /jobs/{id}。

## 7. 第 6 项：无密码明文（PASS）

- evidence-t10 下 35 个文件（响应 JSON + 命令日志）全文扫描密码：命中 **0**。
- manager 数据目录：devmask-manager.db / devmask-manager.db-wal / devmask-manager.db-shm / devmask-manager.key 全部命中 **0**（凭据以 AES-GCM 存）。
- manager 进程日志（/tmp/t10-manager.out）：命中 **0**。
- 所有响应里**没有 "password" 字段**（grep -l '"password"' → 无）；PATCH 响应只有 hasCredentials=true/false。
- 审计（GET /audit?limit=200）：egress.proxy_create/update/delete/bind/apply/proxy_probe 共 14 条，
  detail 只有 proxyId/name/deviceId/total/changed，没有凭据。

## 8. 观察（非阻塞）

1. **仓库自带 mock 缺 egress 端点**：manager/docs/mock_device.py 没有 /api/v1/egress，用它跑 egress 下发只会得到
   skipped/egress_unsupported(404)。本轮补的 docs/design/egress/fixtures/mock_device_egress.py 已覆盖 egress 子集，
   建议把它并入 manager/docs（或 manager/tests 的 mock harness），否则后续 egress 冒烟容易被误判成「下发不了」。
2. **UI Token 首登细节**：首次打开若 localStorage 无 dm_token，会弹「Manager Token」对话框；本轮在该对话框里
   填值后点「保存并重试」，localStorage 未写入（随后改用 localStorage.dm_token 预置才进入已登录态）。
   可能与自动化填值方式有关，建议 t6 侧复核该对话框的提交路径（真实用户手输不一定会复现）。
3. **批量绑定弹窗默认勾选「只看未配代理的设备」**：当所有设备都已绑定时，弹窗显示「没有匹配的设备 / 已选 0 台」，
   需手动取消该勾选才能再选设备。属交互取舍，建议加一句空态提示。

## 9. 回归

    $ cargo test -p devmask_manager
    64 + 14 + 14 + 4 + 6 + 17 + 8 + 15 = 142 passed; 0 failed；TEST_EXIT=0
    （evidence-t10/cargo-test-full.log）

前端构建：npm --prefix manager/webui run build → WEBUI_BUILD_EXIT=0（dist 22:47）。

## 10. 证据文件

| 文件 | 内容 |
| --- | --- |
| evidence-t10/build-and-tests.log | cargo build / cargo test / webui build |
| evidence-t10/cargo-test-full.log | cargo test -p devmask_manager 全量输出（142 passed） |
| evidence-t10/healthz.json | /healthz（schemaVersion=2） |
| evidence-t10/t10-1-crud.log | 第 1 项：代理池 CRUD + 409 |
| evidence-t10/t10-2-bindings.log | 第 2 项：整机/应用级绑定与读回 |
| evidence-t10/t10-2-4-bind-apply-probe.log | 第 2/3/4 项：绑定、apply、probe 的关键输出 |
| evidence-t10/t10-3-apply.log | 第 3 项：apply#1/#2/#3 逐设备结果 |
| evidence-t10/t10-6-security.log | 第 6 项：密码扫描（0 命中）+ 审计 + PATCH 响应 |
| evidence-t10/apply-{1,2,3,4}-job*.json | 各次 job 的完整响应 |
| evidence-t10/{bind-1,bind-2}-get.json | GET /devices/{id}/egress 读回（含 deviceState） |
| evidence-t10/delete-bound-409.json | 409 egress_proxy_in_use 原文 |
| evidence-t10/probe-{http,socks}.json + proxy-*-detail.json | 探测响应与 lastCheck |
| evidence-t10/proxies-list.json / proxy-*.json | 代理定义（脱敏）与 PATCH 双态 |
| evidence-t10/audit.json | 审计条目（egress.* 14 条） |
| evidence-t10/fixture-proxy.log | 代理侧日志（证明 Manager 探测真的经代理） |
| docs/design/manager-egress/*.png | 13 张 UI 截图（见第 6 节表格） |

## 11. 复现步骤

    # 1) 构建
    cargo build -p devmask_manager
    npm --prefix manager/webui run build

    # 2) mock 设备 + fixture 代理
    python3 docs/design/egress/fixtures/mock_device_egress.py --port 18686 --mode ok &
    python3 docs/design/egress/fixtures/mock_device_egress.py --port 18688 --mode fail &
    python3 docs/design/egress/fixtures/mock_device_egress.py --port 18689 --mode ok &
    python3 docs/design/egress/fixtures/local_proxy.py &     # 18080 HTTP / 18081 SOCKS

    # 3) Manager（新端口 + 临时 data-dir）
    ./target/debug/devmask_manager --bind 127.0.0.1:18765 --data-dir /tmp/dm-t10 \
      --static-dir manager/webui/dist --token admin123

    # 4) 依次执行第 2–5 节的 curl；UI 打开 http://127.0.0.1:18765/（localStorage.dm_token=admin123）

    # 5) 收尾
    kill <manager/mocks/fixture pid>; rm -rf /tmp/dm-t10
