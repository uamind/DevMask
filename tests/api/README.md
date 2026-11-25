# DevMask API 测试（tests/api）

面向 devmask_server `/api/v1` 的可重复契约测试。**契约以
[docs/API.md](../../docs/API.md) 为准**。脚本不依赖 jq，JSON 解析统一用 `python3`；
bash 兼容 3.2（macOS 自带）与 5.x。

## 文件

| 文件 | 作用 |
| --- | --- |
| `smoke.sh` | 冒烟：healthz → status → config → templates CRUD → apps（绑定 + 内联 + explain）→ packages → 清理 |
| `concurrency.sh` | 同一 If-Match 并发两次 PUT，断言一个 200、一个 412 |
| `common.sh` | 公共库：请求、JSON 取值、断言、服务启动/清理 |
| `mock_server.py` | 按 docs/API.md 第一增量实现的最小契约 mock，用于无 host server 时自测脚本本身 |

## 依赖

- `curl`
- `python3`（3.8+，只用到标准库；`mock_server.py` 在 3.11+ 额外支持 TOML 请求体）
- `bash` 3.2+

## 启动方式

### 1. 已运行的服务（最常用）

```bash
# 设备侧 service.sh 已启动，且已 adb forward tcp:8686 tcp:8686
TOKEN=df_xxx bash tests/api/smoke.sh

# 自定义地址
BASE=http://127.0.0.1:18686/api/v1 TOKEN=df_xxx bash tests/api/smoke.sh
```

### 2. 本机二进制（host server / 交叉编译后的本机构建）

```bash
SERVER_BIN=target/debug/devmask_server \
SERVER_ARGS='--config /tmp/df/server.toml --static-dir module/webroot' \
TOKEN=df_xxx \
bash tests/api/smoke.sh
```

`SERVER_BIN` 会在临时目录里以后台进程启动，脚本退出时自动结束。
`SERVER_ARGS` 会按空格拆分；需要 shell 语法（管道、重定向）时改用：

```bash
SERVER_CMD='./target/debug/devmask_server --config ./server.toml' TOKEN=df_xxx bash tests/api/smoke.sh
```

### 3. 契约自测（不需要真实 server）

```bash
MOCK=1 bash tests/api/smoke.sh
MOCK=1 ROUNDS=2 bash tests/api/concurrency.sh
```

`MOCK=1` 且未设置 `BASE` 时，默认 `http://127.0.0.1:18686/api/v1`，并自动把
`TOKEN` 设为 fixture token `df_test_token_0123456789`。
mock 只用于验证测试脚本与断言，不代替真实 server 验收。

### 4. 一键自检

```bash
bash tests/check.sh              # bash -n + fixture 校验 + mock e2e
bash tests/check.sh --no-e2e     # 只做静态检查
```

## 环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `BASE` | `http://127.0.0.1:8686/api/v1` | API 根地址（含 `/api/v1`） |
| `TOKEN` | 空 | Bearer Token；为空时不发送 Authorization |
| `MOCK` | `0` | `1` = 服务不可达时启动 `mock_server.py` |
| `MOCK_PORT` | `18686` | `MOCK=1` 且未设置 BASE 时使用的端口 |
| `SERVER_BIN` | 空 | 服务不可达时启动的本机二进制 |
| `SERVER_ARGS` | 空 | 传给 `SERVER_BIN` 的参数（按空格拆分） |
| `SERVER_CMD` | 空 | 完全自定义启动命令（优先级高于 `SERVER_BIN`） |
| `ASSERT_HEADERS` | `1` | 是否断言 `X-Request-Id` / `X-DM-Revision` / `ETag` |
| `API_TIMEOUT` | `20` | 单请求超时（秒） |
| `VERBOSE` | `0` | `1` = 打印每次请求的状态码 |
| `KEEP_TMP` | `0` | `1` = 保留临时目录与 `server.log` 便于排查 |
| `ROUNDS` | `1` | `concurrency.sh` 的并发轮数 |

## smoke.sh 覆盖范围

1. `GET /healthz` 无鉴权 200（`status=ok`）；
2. `GET /status` 200，`module.id=devmask`、`config.revision`；
3. `GET /config` 200：`config` 对象、`X-DM-Revision`、带引号 `ETag`；
4. `GET /config/revision` 200：revision 与 status/config 三者一致；
5. `GET /templates` 200：`items` 列表 + `total`（兼容 `items` / `templates` / `data.templates`）；
6. `PUT /templates/{name}`（If-Match，201/200）→ WriteResult.revision 必须变化；
7. `GET /templates/{name}`：顶层 `name` + `template.model` + `ETag`；
8. `PATCH /templates/{name}`（If-Match）：`previousRevision` 等于旧 revision，
   revision 变化；再 GET 断言 `template.model` 已更新；
9. `GET /apps`（内联记录列表）、`PUT /apps/{pkg}` 绑定模板 →
   `GET /apps/{pkg}`（explain）断言 `source.type=template`、`source.template`、
   `resolved.model`；
10. `PUT /apps/{pkg}` 写内联配置 → GET 断言 `source.type=inline`、
    `source.alsoInline=true`、`resolved.model`；
11. `GET /packages`：`items` + `source`；
12. 清理：`DELETE /apps/{pkg}?confirm=true&detach_templates=true` →
    `GET /apps/{pkg}` 返回 200 `source.type=none`；
    `DELETE /templates/{name}?confirm=true` → 再 GET 404 `not_found`；
    最后回读 `/config/revision` 与最后一次写一致。

> 本增量没有 `/apps/{pkg}/explain` 与 `/templates/{name}/clone`：
> 「解释生效来源」由 `GET /apps/{pkg}` 提供，「CRUD」只覆盖列表/详情/创建/合并/删除。

### 期望输出（节选）

    [api-test] == 2/8 status ==
    [api-test]   PASS GET /status -> HTTP 200
    [api-test]   PASS status.module.id (module.id=devmask)
    ...
    [api-test]   PASS 创建模板 (revision sha256:... -> sha256:...)
    ...
    [api-test]   PASS 内联覆盖模板时 source.alsoInline (source.alsoInline=true)
    ...
    [api-test] SMOKE PASS：status → config → templates CRUD → apps → packages → explain 全部通过

退出码：`0` 全部通过；`1` 任一断言失败（失败时会打印请求、状态码、响应头/体与服务日志尾部）。

## concurrency.sh 语义

- 先建模板（seed），使随后的并发 PUT 是「更新」而不是「创建」，契约上稳定返回 200；
- 读取当前 `revision`，再用**同一个 If-Match** 并发发起两次
  `PUT /api/v1/templates/{name}`，两次请求体不同；
- 断言状态码排序后恰为 `200 412`；
- 胜者响应必须带新 `revision`，且与随后 `GET /config/revision` 一致；
- 败者必须是统一错误信封：`error.code=config_revision_mismatch`，
  `error.details.expected/actual` 分别等于旧 revision 与胜者 revision；
- 控制组：过期 If-Match 必须 412；
- 清理模板（`confirm=true`），GET 断言 404；
- `ROUNDS=N` 可重复多轮。

    [api-test]   并发结果：A=412 B=200
    [api-test]   PASS 恰好一个 200、一个 412
    [api-test]   PASS 败者错误码 = config_revision_mismatch
    [api-test] CONCURRENCY PASS：1 轮均满足「一个 200、一个 412」

## mock_server.py 的边界

- 实现本次测试用到的端点：healthz / status / config / config-revision / templates（含包名绑定、解绑）/
  apps / packages；返回与 docs/API.md 一致的 `items`、`total`、`nextCursor`、
  `WriteResult`（`previousRevision`）与错误信封；
- 不实现鉴权作用域（只有单 Token 校验）、限流、审计、静态资源、MCP；
- 配置模型是 JSON 内存态 + 落盘文件（revision = 文件 sha256，含自增 seq 保证每次写都变化），
  不是真实 TOML，也不验证注释保留；
- 写锁是进程内 `threading.Lock`，用于验证「一个 200、一个 412」的测试逻辑，
  不能证明真实 server 的 `flock` 实现；
- 缺少 If-Match 返回 `428 precondition_required`，与 docs/API.md §2.5 一致；
- 未实现 `/apps/{pkg}/explain`、`/templates/{name}/clone`、`/config/validate` 等
  本增量之外的路径（返回 404 信封）。

## 常见失败

| 现象 | 排查 |
| --- | --- |
| BASE 不可达 | 服务未启动 / 未 `adb forward`；或设置 `SERVER_BIN` / `MOCK=1` |
| 401 `unauthorized` | `TOKEN` 未设置或与服务端 `server.toml` 不匹配 |
| 403 `forbidden_scope` | Token 只有 read 作用域，写用例需要 write |
| 400 `confirmation_required` | DELETE 缺少 `confirm=true` |
| 412 `config_revision_mismatch` | 有外部写入（管理器 WebUI 直写）改变了配置，重跑即可 |
| 428 `precondition_required` | 写请求缺少 `If-Match` |
| 缺少 `X-DM-Revision` / `ETag` | server 未实现响应头；可用 `ASSERT_HEADERS=0` 临时跳过断言定位问题 |
| `GET /config` 非 JSON | server 默认返回 TOML；本测试要求 JSON（`Accept: application/json`） |
