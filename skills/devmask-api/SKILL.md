---
name: devmask-api
description: 通过 DevMask 原生 /api/v1 管理 Android 机型伪装：查看状态、模板、应用配置，创建/绑定模板，开启外部访问，重启应用；用户要改某个 App 的伪装机型、排查未生效、或自动化调用 devmask HTTP API 时使用。
whenToUse: 用户提到 devmask、机型伪装、修改 Build 字段、模板绑定、外部访问/API Token，或要求用 HTTP API 批量管理伪装配置时。
metadata:
  api: http://127.0.0.1:8686/api/v1
  docs: docs/API.md
  auth: Bearer token
  requires: 已安装 devmask 1.8+ 的 Root Android 设备
---

# DevMask /api/v1 使用手册

DevMask 是 Zygisk 机型伪装模块。devmask_server 随模块启动，默认只监听
127.0.0.1:8686；在管理器 WebUI 的设置 → External Access 里可改为 0.0.0.0，让局域网访问。
所有 API 都要 Bearer Token。

本 Skill 覆盖第一增量：配置、模板、应用、包、设备、校验。MCP、plan/apply、应用名/图标、
TLS 尚未实现，见第 9 节。

## 1. 环境检查

    adb devices
    adb shell 'cat /data/adb/modules/devmask/module.prop | head -5'
    adb shell 'sh /data/adb/modules/devmask/service.sh status'

期望 supervisor 与 server 都是 running。旧版模块没有 service.sh/bin 时先安装新模块 zip。

## 2. 连接与 Token

本机调试（推荐）：

    adb forward tcp:8686 tcp:8686
    export DF_BASE=http://127.0.0.1:8686/api/v1

局域网：设置 → External Access 打开开关、设置端口并保存；页面显示 http://手机IP:端口。
手机与电脑需同网段，路由器不能开 AP 隔离。

Token：

- 默认是 admin123（首次启动写入；日志横幅同样会打印一次）。
- 模块 WebUI → 设置 → External Access → 「访问 Token」输入框可改成任意值；留空不动，Reset 恢复 admin123。
- 请求头：Authorization: Bearer <token>（默认 admin123）
- 作用域 read / write / ops / admin，互不隐含；常规 Agent 用 read + write，重启应用再加 ops。
- 只放环境变量，不写脚本、日志或 git：

    export DF_TOKEN=admin123

401 = Token 缺失/无效；403 = 作用域不足（details.required）。

## 3. 数据模型与 revision

- 唯一事实来源：/data/adb/devmask/config/config.toml。
- templates：命名机型模板，packages 是绑定包名。
- apps：内联应用配置，package 唯一；同一包既有内联又命中模板时内联优先。
- revision：配置文件原始字节的 SHA-256（裸 64 位小写十六进制，可带 sha256: 前缀）。
- 写操作带 If-Match: "<revision>"；缺 428，不匹配 412，不自动合并；首次创建可用 *。
- 写入是 flock + 临时文件 + rename + fsync，自动备份最近 10 份。
- 改配置后 companion 按 mtime 重渲染 spec；目标应用下次启动才生效，用 restart 接口重启。
- 不要在 API 之外直接改 config.toml 再让 API 写，revision 会冲突；统一走 API。

## 4. 只读检查

    curl -s -H "Authorization: Bearer $DF_TOKEN" "$DF_BASE/status"
    curl -s -H "Authorization: Bearer $DF_TOKEN" "$DF_BASE/overview"
    curl -s -H "Authorization: Bearer $DF_TOKEN" "$DF_BASE/config?format=json"
    curl -s -H "Authorization: Bearer $DF_TOKEN" "$DF_BASE/config/revision"
    curl -s -H "Authorization: Bearer $DF_TOKEN" "$DF_BASE/templates"
    curl -s -H "Authorization: Bearer $DF_TOKEN" "$DF_BASE/apps"
    curl -s -H "Authorization: Bearer $DF_TOKEN" "$DF_BASE/packages?type=user"

GET /overview 一次给出设备 fingerprint、全部模板和每个已配置包的 source/resolved/warnings；
GET /apps/{pkg} 是单包排查入口；没有独立 /explain 端点。

## 5. 写操作流程（示例：把 com.example.app 伪装成 Pixel 9）

### 5.1 取 revision

    REV=$(curl -s -H "Authorization: Bearer $DF_TOKEN" \
      "$DF_BASE/config/revision" | python3 -c 'import json,sys; print(json.load(sys.stdin)["revision"])')

### 5.2 新建/整体替换模板

PUT 是整体替换：未出现的已知字段会被删除；packages 不出现等价于 []。

    curl -s -X PUT "$DF_BASE/templates/pixel_9" \
      -H "Authorization: Bearer $DF_TOKEN" \
      -H "Content-Type: application/json" \
      -H "If-Match: \"$REV\"" \
      -d '{
        "packages": ["com.example.app"],
        "manufacturer": "Google",
        "brand": "google",
        "model": "Pixel 9",
        "device": "tokay",
        "android_version": "15",
        "sdk_int": 35,
        "dpi": 420
      }'

响应 WriteResult 含新 revision、previousRevision、backupId、warnings；用它更新 REV。

### 5.3 绑定模板到应用

    curl -s -X PUT "$DF_BASE/apps/com.example.app" \
      -H "Authorization: Bearer $DF_TOKEN" \
      -H "Content-Type: application/json" \
      -H "If-Match: \"$REV\"" \
      -d '{"template": "pixel_9", "detachFromTemplates": true}'

detachFromTemplates 默认 true：从其它模板移除该包，并删除同包内联记录（内联优先会盖掉模板）。

也可以 POST /templates/pixel_9/packages 批量绑定。

### 5.4 重启并验证

    curl -s -X POST "$DF_BASE/apps/com.example.app/restart" \
      -H "Authorization: Bearer $DF_TOKEN" \
      -H "Content-Type: application/json" \
      -d '{"forceStop": true, "launch": false, "user": 0}'

    curl -s -H "Authorization: Bearer $DF_TOKEN" "$DF_BASE/apps/com.example.app"

需要 ops 作用域。验证 source.type=template、resolved 字段正确、warnings 为空。

### 5.5 内联配置

    curl -s -X PUT "$DF_BASE/apps/com.example.two" \
      -H "Authorization: Bearer $DF_TOKEN" \
      -H "Content-Type: application/json" \
      -H "If-Match: \"$REV\"" \
      -d '{"config": {
        "package": "com.example.two",
        "brand": "Redmi",
        "model": "2509FPN0BC",
        "android_version": "15",
        "sdk_int": 35,
        "dpi": 420
      }}'

内联是整记录替换，不是字段合并；改一个字段也要写全。

## 6. 端点速查

| 方法 | 路径 | 作用域 | If-Match | 说明 |
| --- | --- | --- | --- | --- |
| GET | /healthz | 无 | 否 | 存活探针 |
| GET | /status | read | 否 | 模块/设备/服务/配置状态 |
| GET | /overview | read | 否 | 设备 fingerprint + 模板 + 每个包的 resolved |
| GET | /config?format=json\|toml | read | 否 | 读配置；toml 原文 |
| GET | /config/revision | read | 否 | 取 revision |
| PUT | /config | write | 是 | 整体替换配置 |
| POST | /config/validate | read | 否 | 校验文本，不落盘 |
| GET | /templates | read | 否 | 模板列表 |
| GET | /templates/{name} | read | 否 | 详情 {revision,position,name,template} |
| PUT | /templates/{name} | write | 是 | 新建/整体替换 |
| PATCH | /templates/{name} | write | 是 | 字段级合并；null 删除 |
| DELETE | /templates/{name}?confirm=true | write | 是 | 删除模板 |
| POST | /templates/{name}/packages | write | 是 | 批量绑定包名 |
| DELETE | /templates/{name}/packages/{pkg} | write | 是 | 解绑包名 |
| GET | /apps | read | 否 | 已配置应用列表 |
| GET | /apps/{pkg} | read | 否 | 来源/解析结果/警告 |
| PUT | /apps/{pkg} | write | 是 | 绑定模板或写内联 |
| DELETE | /apps/{pkg}?confirm=true | write | 是 | 删除应用配置 |
| POST | /apps/{pkg}/restart | ops | 否 | force-stop，可选拉起 |
| GET | /packages | read | 否 | 已安装包元数据 |
| GET | /device | read | 否 | 设备与 Root 信息 |
| GET | /device/props?keys=... | read | 否 | getprop 白名单 |
| GET | /device/users | read | 否 | 用户列表 |
| GET | /schema/config | read | 否 | 配置 JSON Schema（draft-07） |

写端点普遍支持 dry_run=true / diff=true；删除要 confirm=true；重复包名默认 409，
需显式 allow_duplicate_package=true。

## 7. 错误与重试

    {"error":{"code":"...","message":"...","details":{...},"retryable":false,"requestId":"..."}}

| 状态 | code | 处理 |
| --- | --- | --- |
| 401 | unauthorized | 重新设置 Token |
| 403 | forbidden_scope | 换有对应作用域的 Token |
| 404 | not_found | 检查模板名/包名/路径 |
| 409 | conflict_duplicate_package | 展示冲突；确认后加 allow_duplicate_package=true |
| 412 | config_revision_mismatch | 重新取 revision，重建变更；不要原样重试 |
| 422 | validation_failed | 看 details.errors[].path/code |
| 428 | precondition_required | 补 If-Match；首次创建用 * |
| 429 | rate_limited | 当前版本不会返回；按 Retry-After 退避 |
| 5xx | internal_error / upstream_unavailable | 可退避重试，带 requestId 查日志 |

## 8. 操作纪律

- 写前必取 revision；每次写响应都返回新 revision，后续写用它。
- 破坏性操作先展示变更并得到用户确认；能用 PATCH 就不要 PUT 整条。
- Token 只放环境变量，不写入 Skill、脚本、日志或 git。
- 不提供任意 shell/文件接口；只有本文列出的端点。
- 失败保留 requestId，查 /data/adb/devmask/logs/server.log 与 api-audit.jsonl。

## 9. 当前限制

- 没有 MCP、plan/apply、应用名/图标、TLS；MCP 是后续增量。
- 没有 429 限流，不要依赖服务端限流保护。
- packages 的应用名/图标为空，用包名。
- PUT /config 整体替换 TOML 不保留注释；模板/应用写操作保留注释。
- 局域网暴露需要可信网络；公网用 Tailscale/WireGuard 或反代加 TLS。

## 10. 参考

- docs/API.md：完整契约与字段
- docs/openapi.yaml：OpenAPI 3.1
- docs/SERVER.md：部署、Token、外部访问、排障
- docs/llms.txt：AI 短摘要
