# 测试 fixture（tests/fixtures）

供 core / server 单元测试、集成测试与 PATH shim 使用的静态样本。
全部为手工构造的最小样本，不含真实用户数据。

## 目录

| 路径 | 用途 |
| --- | --- |
| `config/minimal.toml` | 最小合法配置：全局开关 + 1 个模板 + 1 个内联应用；TOML 往返 / 解析 / 读写测试基线 |
| `config/commented.toml` | 注释、行内注释、未知字段、未知表的往返保留测试；校验「toml_edit 不丢注释/未知字段」 |
| `config/conflict.toml` | 同一裸包名出现在两个模板中，用于 `conflicts` / `conflict_duplicate_package` 测试 |
| `config/invalid.toml` | TOML 合法但语义非法（`dpi=9999`、`sdk_int="abc"`、非法包名），用于 `validation_failed` 测试 |
| `module/module.prop` | 模块元数据样本，`/status` 的 `module` 字段解析测试 |
| `server/server.default.toml` | server 默认配置 + 两个 Token（owner / readonly）；见下方明文 token |
| `api/template.json` | `PUT /templates/{name}` 请求体样本（无 `name` 字段，名字在路径里） |
| `api/app.json` | `PUT /apps/{pkg}` 请求体样本（绑定 `pixel_9` 模板） |
| `android/pm-list-packages*.txt` | `pm list packages`（默认/`-f`/`-U`/`-3`/`-s`/`-d`）输出样本 |
| `android/dumpsys-package-*.txt` | `dumpsys package <pkg>` 输出样本（versionName、userId、安装状态、用户列表） |
| `android/getprop.txt` | `getprop` 输出样本（Pixel 9 / Android 15） |
| `bin/pm`、`bin/dumpsys`、`bin/getprop` | PATH shim：把上面的样本模拟成 Android 命令 |

## server fixture 中的 Token

`server.default.toml` 只存 sha256，明文仅用于测试：

| 名称 | 明文 | scopes |
| --- | --- | --- |
| owner | `df_test_token_0123456789` | read, write, ops, admin |
| readonly | `df_readonly_token_9876543210` | read |

重新生成摘要：

```bash
python3 -c 'import hashlib;print("sha256:"+hashlib.sha256(b"<token>").hexdigest())'
```

## PATH shim 用法

server 在 host 上跑测试时没有 Android 的 `pm` / `dumpsys`，可以把 shim 目录加到 PATH 前面：

```bash
export PATH="$PWD/tests/fixtures/bin:$PATH"
pm list packages -U        # package:com.example.app uid:10123
dumpsys package com.example.app | head
getprop ro.product.model   # Pixel 9
```

Rust 测试里可以这样用：

```rust
let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("../tests/fixtures");
let config = std::fs::read_to_string(fixtures.join("config/minimal.toml"))?;
let shim_path = format!("{}:{}", fixtures.join("bin").display(), std::env::var("PATH")?);
std::process::Command::new("pm")
    .arg("list").arg("packages").arg("-U")
    .env("PATH", shim_path)
    .output()?;
```

`tests/api/smoke.sh` 也会读取 `api/template.json` 生成写请求体，确保 fixture
与契约测试保持同源。

## 约定

- 所有包名使用 `com.example.*`（保留域名），不要写入真实应用配置；
- 修改 fixture 结构时同步检查 `tests/check.sh` 的解析断言；
- fixture 是测试输入，不是运行时配置；不要把设备上的真实 `/data/adb/device_faker/config/config.toml` 复制进来。
