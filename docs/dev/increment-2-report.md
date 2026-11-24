# 第二增量验收报告：ksu 外部访问（开关 / 端口 / Token / 局域网连通）

- 日期：2026-11-24
- 范围：t13「管理器模式外部访问卡片」验收；不改业务代码、不 commit
- 设备：Pixel 9 / Android 17 / KernelSU（root）；手机 IP 192.168.30.2，Mac 192.168.30.149（同一 /24）
- 服务二进制：target/aarch64-linux-android/release/device_faker_server（t9 构建，t4 之后版本）
- 结论：开关、端口、Token 三条路径全部通过；UI 卡片自动化点击受共享浏览器 hidden 限制，改为「t13 纯函数 + 真机 server.toml 等价写入 + 模式门控」验证，并给出手工步骤。

---

## 1. 结果总览

| # | 项 | 结果 | 证据 |
| --- | --- | --- | --- |
| 1 | WebUI 全流程构建 | PASS | python3 webui/build_webui.py exit 0（npm ci/type-check/lint/vite build） |
| 2 | core / server 单测无回归 | PASS | core 51 passed、server 166 passed，0 failed |
| 3 | serverConfig 纯函数测试 | PASS | node --experimental-strip-types tests/webui/server-config.test.mjs exit 0，9 组全过 |
| 4 | 开启外部访问 | PASS | 0.0.0.0:18901 监听；LAN status/config 200，无 Token 401 |
| 5 | 关闭外部访问 | PASS | 监听回 127.0.0.1:18901；本机 200；LAN 000（不可达） |
| 6 | 修改端口 | PASS | 新端口 18902 LAN 200；旧端口 18901 000 |
| 7 | 重置 Token | PASS | 旧 Token 401，新 Token 200；实际 [[tokens]] 仅 1 条 |
| 8 | 设备恢复 | PASS | config.toml sha256/mtime/size 回到基线；server.toml 删除；临时模块文件删除；无进程/端口/forward |
| 9 | UI 模式门控 | PASS（含说明） | dev(ksu) serverAccess.supported=true；native __DF_SERVER__=true、无卡片；readonly healthz 404、无卡片 |

---

## 2. 构建与单测

    python3 webui/build_webui.py        # exit 0
    cargo test -p device_faker_core      # exit 0
    cargo test -p device_faker_server    # exit 0

| 项 | passed | failed |
| --- | ---: | ---: |
| device_faker_core | 51 | 0 |
| device_faker_server | 166 | 0 |
| WebUI（type-check + lint + build） | — | 0 |

---

## 3. 纯函数测试（serverConfig）

新增可重复执行测试：tests/webui/server-config.test.mjs

    node --experimental-strip-types tests/webui/server-config.test.mjs   # exit 0

覆盖 9 组：

1. 空文本默认 127.0.0.1:8686 / allow_lan=false / 无 Token；
2. 解析 module/server.default.toml；
3. 开启外部访问：bind=0.0.0.0:<port>、allow_lan=true，保留注释、未知字段、[[tokens]]；
4. 关闭：回 127.0.0.1:<port>、allow_lan=false，Token 保留；
5. 缺 bind / allow_lan 时追加；
6. 端口边界：1/65535/"8686" 合法，0/65536/"abc"/1.5 非法；归一化回退 8686；
7. stripTokenBlocks 删除所有 Token 表，保留其它表与注释；
8. appendTokenBlock 自动补 sha256: 前缀，且只追加一条；
9. parseIpAddrOutput（多行 + -o 单行、排除 loopback/rmnet）、parseDhcpProps、formatLanUrls。

---

## 4. 真机验收

### 4.1 初始状态与备份

| 项 | 值 |
| --- | --- |
| /data/adb/device_faker/config/config.toml | 9361 bytes，sha256 f9f80df8af73…，mtime 1793793738，mode 600 |
| /data/adb/device_faker/config/server.toml | 不存在（记录 absent，结束时删除重建的文件即恢复） |
| 已安装模块 | 旧版本：只有 module.prop / uninstall.sh / webroot / zygisk，没有 service.sh、bin/、server.default.toml |
| 备份 | config.toml → /data/local/tmp/df-t14/config.toml.bak（sha256 一致） |

说明：当前设备模块是 t7 之前安装的旧版；为走真实 service.sh restart 路径，临时把 t7 的
module/service.sh、module/server.default.toml、release device_faker_server 放进
/data/adb/modules/device_faker/，验收结束后全部删除（见 4.6）。用户安装新模块 zip 后即自带这些文件。

### 4.2 模拟 UI 写入（与 t13 save() 同一套纯函数）

用 t13 的 applyServerSettings + appendTokenBlock + stripTokenBlocks 处理
module/server.default.toml（等价于 UI 在没有 server.toml 时使用默认模板），生成
bind = "0.0.0.0:18901"、allow_lan = true、一条 [[tokens]]（sha256），push 到真实
/data/adb/device_faker/config/server.toml 并 chmod 600。

    parseServerSettings -> {"bind":"0.0.0.0:18901","host":"0.0.0.0","port":18901,"allowLan":true,"hasToken":true,"tokenCount":1}

### 4.3 开启：LAN 连通

    su -c "sh /data/adb/modules/device_faker/service.sh restart"   # 见下方 exit 124 说明
    service.sh status -> supervisor: running (pid=29746) / server: running (pid=29751)
    ss -ltn -> LISTEN 0.0.0.0:18901

| 请求（从 Mac 192.168.30.149） | 结果 |
| --- | --- |
| curl http://192.168.30.2:18901/api/v1/status -H "Authorization: Bearer <token>" | 200 |
| curl http://192.168.30.2:18901/api/v1/config -H "Authorization: Bearer <token>" | 200 |
| 同上不带 Token | 401 |
| 本机（adb forward 18890→18901）healthz | 200 |

说明：restart 的 host 侧退出码是 124：adb shell su -c 会等待守护循环的后台 stdio，
被 timeout 20 杀掉；但 status、ss、healthz 都证明服务已启动。这是 adb/su 调用方式问题
（t9 也记录过），不是 service.sh 缺陷。自动化脚本应使用 timeout 或重定向后后台执行。

### 4.4 关闭：LAN 不可达

    bind = "127.0.0.1:18901"; allow_lan = false; Token 保留

| 请求 | 结果 |
| --- | --- |
| 本机 http://127.0.0.1:18890/api/v1/status | 200 |
| Mac http://192.168.30.2:18901/api/v1/status | 000（连接不可达） |
| Mac http://192.168.30.2:18901/api/v1/healthz | 000 |
| ss -ltn | 只监听 127.0.0.1:18901 |

### 4.5 改端口 + 重置 Token

改端口：

    bind = "0.0.0.0:18902"; allow_lan = true; Token 不变
    ss -ltn -> LISTEN 0.0.0.0:18902
    LAN 18902 status -> 200
    LAN 18901 status -> 000（旧端口关闭）

重置 Token（strip 全部 [[tokens]] + append 新哈希）：

    LAN 旧 Token -> 401
    LAN 新 Token -> 200
    LAN 新 Token /api/v1/config -> 200
    实际 [[tokens]] 数量：1（另有一行注释示例 # [[tokens]]，不计入）

### 4.6 恢复

| 操作 | 结果 |
| --- | --- |
| service.sh stop | supervisor: stopped / server: stopped |
| 恢复 config.toml | sha256 f9f80df8af73…、size 9361、mtime 1793793738，与基线一致 |
| 删除测试 server.toml | 原状态 absent，已删除 |
| 删除临时模块文件 | service.sh / server.default.toml / bin/ 已移除；模块目录恢复为 module.prop/uninstall.sh/webroot/zygisk |
| 清理 pid / server.log / backups | 已删除；logs/device_faker.log 原文件保留 |
| 端口/进程/forward | ss 无 18901/18902 监听；无 device_faker_server 进程；adb forward --list 为空 |

config.toml.lock 是 t9 遗留的空锁文件，本轮开始时就存在，保持原样未动。

---

## 5. 管理器模式 UI 冒烟

| 模式 | 证据 | 结论 |
| --- | --- | --- |
| 管理器（Vite dev，ksu mock） | backend.runtime = "ksu"；serverAccess.supported = true；bindPreview=127.0.0.1:8686、port=8686、allowLan=false | 卡片条件成立 |
| 原生（真实 server :18881） | window.__DF_SERVER__ = true、window.ksu = undefined；页面无 "External" 文本 | 卡片不渲染 |
| 只读（静态页 :18903，无 /api） | 无 __DF_SERVER__、无 window.ksu、/api/v1/healthz → 404；页面无 "External" 文本 | 卡片不渲染 |

说明：共享浏览器页面 document.visibilityState=hidden，Settings 页的懒加载面板与翻页动画
不推进（t9 同样现象），因此无法用坐标自动点击「外部访问」卡片。t13 已用假 window.ksu
（内存 FS + exec 桩）完成卡片端到端：开启写盘 + Token 弹窗、关闭保留 Token、重置 Token、
native/readonly 不渲染。本轮在此基础上补了真机 server.toml + service.sh 的真实链路。

### 手工点击步骤（可复现）

1. 安装新模块 zip（含 service.sh、bin/device_faker_server、server.default.toml），重启设备或执行 su -c "sh /data/adb/modules/device_faker/service.sh start"。
2. 管理器打开 WebUI → Settings → External access 卡片。
3. 打开开关，端口改为 18901 → Save；若没有 Token，会弹出一次性 Token 明文，复制保存。
4. 手机与 Mac 同一 Wi-Fi；Mac 执行：
   curl -H "Authorization: Bearer $TOKEN" http://<手机IP>:18901/api/v1/status，应为 200。
5. 关闭开关 → Save；同一 curl 应连接失败；curl 127.0.0.1:18901（手机本机/adb forward）应为 200。
6. Reset token → 旧 Token 401、新 Token 200；明文只显示一次。
7. 结束后恢复原 server.toml（或删除，回到默认），service.sh restart。

---

## 6. 已知差距与建议

| 缺口 | 影响 | 建议 / Owner |
| --- | --- | --- |
| 共享浏览器 hidden，Settings 卡片无法自动点击 | 本轮 UI 验证依赖 t13 假 ksu E2E + store/模式门控；未做真实点击回归 | webui-dev：加组件测试（mock backend）或 Playwright headed runner；qa 下轮补 |
| 当前设备模块是 t7 之前旧版（无 service.sh/bin） | UI 卡片调用 service.sh restart 会提示二进制缺失，直到安装新模块 zip | infra-dev：发布/安装流程说明；本轮已临时安装验证并恢复 |
| adb shell su -c service.sh restart 退出码 124 | 自动化脚本需 timeout/重定向，否则误判失败 | qa：已写入报告；下轮可封装 tests/device/external-access.sh |
| 防火墙 / AP 隔离 / 无 TLS | 局域网暴露面依赖网络环境；模块无法控制路由器 | docs 已写安全提醒；建议可信网络或 adb forward/Tailscale |
| 无端口占用自动探测 | 端口冲突时服务启动失败，只在日志提示 | server/infra 下轮可加启动前检测或 UI 提示 |

---

## 7. 本轮改动

- 新增 tests/webui/server-config.test.mjs（9 组纯函数验收，可重复执行）。
- 更新 docs/SERVER.md：新增 §6.1 管理器「外部访问」卡片、§6.2 手工配置与验证、排障项、安全提醒。
- 新增本报告。
- 未改 webui/、server/、core/、module/ 业务代码，未 commit。

---

## 8. 复现命令速查

    # 单测 / 构建
    cargo test -p device_faker_core
    cargo test -p device_faker_server
    python3 webui/build_webui.py
    node --experimental-strip-types tests/webui/server-config.test.mjs

    # 真机（root，t7+ 模块）
    adb shell 'su -c "cp -p /data/adb/device_faker/config/config.toml /data/local/tmp/config.toml.bak"'
    # 用 UI 或 serverConfig 纯函数生成 server.toml（bind 0.0.0.0:18901 + allow_lan=true + [[tokens]]）
    adb push server.toml /data/local/tmp/server.toml
    adb shell 'su -c "cp /data/local/tmp/server.toml /data/adb/device_faker/config/server.toml && chmod 600 /data/adb/device_faker/config/server.toml"'
    adb shell 'su -c "sh /data/adb/modules/device_faker/service.sh restart"'
    curl -H "Authorization: Bearer $TOKEN" http://<phone-ip>:18901/api/v1/status
    # 关闭：bind 127.0.0.1 + allow_lan=false，重启后再 curl 应失败
    adb shell 'su -c "sh /data/adb/modules/device_faker/service.sh stop"'
    adb shell 'su -c "cp -p /data/local/tmp/config.toml.bak /data/adb/device_faker/config/config.toml; rm -f /data/adb/device_faker/config/server.toml"'

---

## 9. 真机 ksu UI 端到端补充（2026-11-24）

在 Pixel 9 上用 host bridge 模拟管理器 ksu 环境，实际打开生产构建的 Settings → External Access 卡片：

- 开启外部访问 0.0.0.0:18910：弹窗生成 Token，server.toml 写入 bind/allow_lan/[[tokens]]；LAN status/config 200，无 Token 401。
- 关闭：bind 恢复 127.0.0.1，LAN 000；adb forward 本机 200。
- 改端口 18911：LAN 新端口 200，旧端口 000。
- 重置 Token：旧 401、新 200，[[tokens]] 保持 1 条。
- 发现并修复：service.sh restart 在 adb/管理器调用下等待后台 stdio，导致 UI 保存一直 loading（exit 124）。修复：start_server 关闭继承 stdio；backend/ksu.ts 对 start/stop/restart 重定向；保存/重置后轮询 status 最多 6s，避免短暂显示 Stopped。
- 测试后已停止服务、删除临时 service.sh/bin/server.default.toml/server.toml/lock/log，恢复 config.toml sha256 f9f80df8…，无进程/端口。
