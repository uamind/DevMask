# 配置说明

为不同的应用配置不同的伪装机型与系统属性。配置文件使用 TOML 格式。

## 配置文件路径

- `/data/adb/device_faker/config/config.toml`
- `/data/adb/device_faker/config/egress.toml`（**0600**，代理定义与探测策略；不进导出、不进 API 明文，见[网络出口](#网络出口egress)）

配置为**热加载**：模块在每次应用启动时重新读取配置文件。修改配置后只需重启目标应用，**无需重启系统**。

- 配置文件缺失或解析失败时，该应用跳过伪装并卸载模块，不影响其他应用
- 未出现在任何配置中的应用不做任何伪装

## 全局设置

```toml
debug = false                        # 调试日志（默认关闭）
default_force_denylist_unmount = false
```

- `debug`：启用后输出 Info 级别日志（关闭时仅 Error），写入 `/data/adb/device_faker/logs/device_faker.log`；正常使用建议关闭以提高隐蔽性
- `default_force_denylist_unmount`：为目标应用启用 Zygisk 的 `FORCE_DENYLIST_UNMOUNT`，可在模板/应用里用 `force_denylist_unmount` 覆盖

## 编辑配置

> 多用户说明：支持在包名后追加 `@userId` 来只对指定用户生效。
> 
> - `userId` 对应路径 `/data/user/<userId>/...` 中的数字（例如 `0`、`999`）
> - 匹配优先级：先匹配 `com.example.app@userId`，找不到再回退匹配 `com.example.app`
> - 该写法同时适用于 `apps` 里的 `package` 和模板的 `packages` 列表
> - **裸包名（不带 `@userId`）是「user 0 的配置」+「所有分身/多开用户（非 0）的回退配置」双语义**。
>   因此想让主应用和它的分身用不同模板时，裸包名只写在一个模板里，分身在另一个模板里写
>   `com.example.app@999`；**不要把同一个裸包名同时写进两个模板**（运行时只认书写顺序第一个，
>   WebUI 会给出重复包名告警）。反过来，修改裸包名的配置会影响所有依赖回退的分身。
>
> 作用范围速查（运行时按 `pkg@uid` → 裸 `pkg` 顺序查找，命中即停）：
>
> | 写法 | 作用范围 |
> | --- | --- |
> | `com.example.app` | user 0 **+ 所有分身**（分身没单独配置时回退到这里） |
> | `com.example.app@0` | **仅 user 0**（精确键优先于裸名，分身不会命中） |
> | `com.example.app@999` | **仅该分身**（user 999） |
>
> 只想让主应用生效、不让分身继承时用 `@0`；想让主应用和所有分身都生效时用裸包名。

### 方式一：机型模板

在模板中定义 `packages` 列表，自动应用到所有包名：

```toml
[templates.redmagic_9_pro]
packages = [
    "com.mobilelegends.mi",
  # 仅对 userId=999 生效
  # "com.mobilelegends.mi@999",
    "com.supercell.brawlstars",
]
manufacturer = "ZTE"
brand = "nubia"
model = "NX769J"
device = "REDMAGIC 9 Pro"
fingerprint = "nubia/NX769J/NX769J:14/UKQ1.230917.001/20240813.173312:user/release-keys"
build_id = "UKQ1.230917.001"

# 无需写 [[apps]]，所有包名自动使用该模板
```

**优点**：
- ✅ 集中管理机型和包名
- ✅ 无需重复写 [[apps]]
- ✅ 一目了然地看到哪些应用使用哪个模板

### 方式二：直接配置

使用 `[[apps]]` 为单个应用指定设备信息：

```toml
[[apps]]
package = "com.omarea.vtools"
manufacturer = "Xiaomi"
brand = "Xiaomi"
model = "2509FPN0BC"
device = "Xiaomi 15 Pro"
product = "popsicle"
name = "popsicle"
```

**注意**：`package` 是唯一**必填**字段，缺失会导致整个配置解析失败。

### 优先级

```
[[apps]] 直接配置 > 模板 packages 列表 > 全局默认值
```

- **`[[apps]]` 是整记录匹配**：一旦包名命中 `[[apps]]`，该条记录**整体取代**模板——记录中未设置的字段**不会**回落模板，只套用全局默认值
- 未命中 `[[apps]]` 时，在模板的 `packages` 列表中查找
- 最终套用全局默认值：`force_denylist_unmount` ← `default_force_denylist_unmount`
- 未匹配任何配置的应用：不做伪装并卸载模块
- 出口代理（egress）不参与上面的机型优先级：它有自己的继承链 `[[apps]].egress` → `[egress].default` → 直连，见[网络出口](#网络出口egress)

**覆盖模板示例**：

```toml
[templates.redmagic_9_pro]
packages = ["com.mobilelegends.mi"]  # 默认使用这个模板
manufacturer = "ZTE"
brand = "nubia"

[[apps]]
package = "com.mobilelegends.mi"  # 命中后整记录取代模板
manufacturer = "Samsung"
# 模板中设置了但这里未写的字段（如 brand）不会生效
```

## 字段说明

### 设备信息字段

以下字段可在模板或 `[[apps]]` 中使用。每个字段会同时改写应用内读到的 `android.os.Build` 字段与对应的系统属性，两者保持一致：

| 字段 | Build 字段 | 系统属性 | 说明 |
|------|-----------|----------|------|
| `manufacturer` | `Build.MANUFACTURER` | `ro.product.manufacturer` 及分区变体 | 厂商 (如: Xiaomi, Samsung) |
| `brand` | `Build.BRAND` | `ro.product.brand` 及分区变体 | 品牌 (如: Redmi, nubia) |
| `marketname` | —（仅属性） | `ro.product.marketname` + `ro.vendor.oplus.market.name` | 市场名 (如: Nothing Phone (3))；OnePlus/OPPO 设备读取后者 |
| `model` | `Build.MODEL` | `ro.product.model` 及分区变体 | 型号 (如: NX769J, SM-S9280) |
| `name` | —（仅属性） | `ro.product.name` 及分区变体 | 产品内部名 (如: popsicle) |
| `device` | `Build.DEVICE` | `ro.product.device` 及分区变体 | 代号 (如: xuanyuan)；未设置时依次回落到 `product` / `name` |
| `product` | `Build.PRODUCT` | `ro.product.name` 及分区变体 + `ro.build.product` | 产品代号 (如: xuanyuan)；与 `name` 同源，未设置时两者互相回填 |
| `hardware` | `Build.HARDWARE` | `ro.hardware` | 硬件名 (如: qcom)，无分区变体 |
| `board` | `Build.BOARD` | `ro.product.board` | 主板名 (如: kalama)，无分区变体 |
| `soc_model` | `Build.SOC_MODEL` | `ro.soc.model` | SoC 型号 (如: SM8550)，无分区变体 |
| `fingerprint` | `Build.FINGERPRINT` | `ro.build.fingerprint` | 指纹 |
| `build_id` | `Build.ID` | `ro.build.id`、`ro.system.build.id`、`ro.vendor.build.id`、`ro.product.build.id` | Build ID (如: UKQ1.230917.001) |
| `characteristics` | —（仅属性） | `ro.build.characteristics` | 特性 (如: tablet) |

> **「分区变体」**指 `ro.product.{odm,vendor,system,system_ext,product,bootimage,system_dlkm,vendor_dlkm,odm_dlkm}.<字段>`：不同厂商的 ROM 会从不同分区读取机型信息，整族写入才能让各条读取路径拿到一致的值。
>
> 设备上**原本不存在**的属性不会被写入——例如部分 ROM 没有 `ro.product.system_dlkm.model`，非 OPPO 机型没有 `ro.vendor.oplus.market.name`。这样不会凭空多出真机没有的属性，也不影响伪装效果。**例外**：`custom_props` 里显式写出的 key 按你的要求照写。

> **指纹会自动补全其它字段**：只写 `fingerprint` 也能得到一致的伪装——模块解析指纹后，把**未显式填写**的 `brand`、`product`、`device`、`android_version`、`build_id`、`incremental` 自动补上（已写的字段不会被覆盖）。补出来的 `incremental` 会同时驱动 `Build.VERSION.INCREMENTAL` 与 `ro.build.version.incremental`。

### Android 版本与构建信息伪装

| 字段 | Build 字段 | 系统属性 | 示例 |
|------|-----------|----------|------|
| `android_version` | `Build.VERSION.RELEASE` | `ro.build.version.release`、`ro.system.build.version.release`、`ro.vendor.build.version.release`、`ro.product.build.version.release` | `"15"`, `"14"`, `"13"` |
| `sdk_int` | `Build.VERSION.SDK_INT`（整数） | `ro.build.version.sdk`、`ro.system.build.version.sdk`、`ro.vendor.build.version.sdk`、`ro.product.build.version.sdk` | `35`, `34`, `33` |
| `display_id` | `Build.DISPLAY` | `ro.build.display.id` | `PD2546_16.0.9.400(CN01)`，无分区变体 |
| `incremental` | `Build.VERSION.INCREMENTAL` | `ro.build.version.incremental` 及分区变体 | `compiler260228234011` |
| `security_patch` | `Build.VERSION.SECURITY_PATCH` | `ro.build.version.security_patch` 及分区变体 | `2025-06-05` |

> `security_patch` 会连同各分区副本一起写入。

### DPI伪装

在模板或 `[[apps]]` 中设置 `dpi`（范围 `120`–`640`），例如 `dpi = 420`。模块先保存当前的 DPI；目标应用退到后台或退出后恢复原值，回到前台时重新应用。原本没有保存时走 DPI 重置。

### 自定义属性

`custom_props` 可设置任意系统属性（直接写入，无分区变体展开），支持在模板与 `[[apps]]` 中使用，也支持[特殊标记值](#特殊标记值)：

```toml
[[apps]]
package = "com.custom.app"
manufacturer = "Custom"

[apps.custom_props]
"ro.custom.property" = "custom_value"
"ro.debug.mode" = "__DELETE__"    # 删除该属性
"ro.empty.value" = "__EMPTY__"    # 设为空字符串
```

### 特殊标记值

| 标记值 | 含义 |
|--------|------|
| 普通字符串 | 设置为该值 |
| `""` | 设置为空字符串（等价于 `__EMPTY__`） |
| `"__EMPTY__"` | 设置为空字符串 |
| `"__DELETE__"` | 删除该属性（结构化字段则删除对应属性族；**仅在 `companion_resetprop = true` 时真正执行**） |

> **结构化字段**（`brand`、`model`、`fingerprint`、`android_version` 等）写 `__DELETE__` 时：该字段不再写入，并删除它对应的整组属性；`__EMPTY__` 则按空字符串写入（Build 字段同样写成空串）。少数字段会一并处理厂商别名（如 `marketname`、`security_patch`）。
>
> `sdk_int` 的写法是 `sdk_int = "__DELETE__"`（数值字段的字符串标记，WebUI 表单只接受整数，需手改 TOML）——**删除后相关读取会失败，属高风险操作**。
>
> ⚠️ **删除只在 `companion_resetprop = true` 时执行**：默认模式下删除列表会被跳过（`__DELETE__` 与「只影响本应用」天然冲突）。`__EMPTY__` 在默认模式下也能正常生效。

### 功能开关

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `companion_resetprop` | `false` | `true` 时伪装值对**全系统生效**（`getprop`、其它应用也能读到），应用切到后台或退出后自动恢复真值；`false`（默认）时只在应用自己的进程内生效。需要全局生效时再开启，详见 [生效方式](#生效方式) |
| `dpi` | — | 临时修改系统显示密度（范围 `120`–`640`）；应用退到后台或退出后自动恢复原值 |
| `force_denylist_unmount` | 继承 `default_force_denylist_unmount` | 对该应用强制启用 Zygisk `FORCE_DENYLIST_UNMOUNT`；优先级：应用 > 模板 > 全局默认 |

**注意**：
- 除 `package` 外所有字段均为可选
- 使用模板的 `packages` 时无需写 `[[apps]]`（自动应用）
- `[[apps]]` 中的字段整记录取代模板，未设置字段不回落（见[优先级](#优先级)）
- 不在上表中的字段会被静默忽略（不影响解析）

## 网络出口（egress）

> 需要 `device_faker_server` v1.10.0 及以上（用于 WebUI「网络出口」卡片与探测）。只装模块不装服务时配置照样生效，只是没有探测与读回校验。

出口代理决定「应用流量从哪个 IP 出去」，与机型伪装是两条独立链路。两种粒度：

| 粒度 | 实现 | 覆盖范围 | 协议 |
| --- | --- | --- | --- |
| 设备级（方案 A） | 写 Android 全局代理 `settings global http_proxy` | 尊重系统代理的应用（OkHttp / HttpURLConnection / WebView / 多数 SDK） | 仅 HTTP/HTTPS CONNECT，**不承载认证** |
| 应用级（方案 B） | Zygisk 在目标进程内注入 `http/https/socksProxy` 与 `ProxySelector` | 只有被配置的那个应用，不动系统设置 | HTTP + SOCKS5；**带认证的代理本期不可用**（见[已知限制](#已知限制)） |

### config.toml：只写引用

```toml
[egress]
default = "office_http"        # 设备级默认；省略或留空 = 直连

[[apps]]
package = "com.example.a"
egress  = "lab_socks"          # 应用级覆盖（这里用 SOCKS5）

[[apps]]
package = "com.example.b"
egress  = "__DIRECT__"         # 强制直连，覆盖设备级代理
```

| 位置 | 键 | 取值 | 说明 |
| --- | --- | --- | --- |
| `[egress]` | `default` | 代理名 | 设备级出口。**只接受 `default` 一个键**：写 `host` / `port` / `username` / `password` 等 → `422 unknown_field`（凭据只存在 `egress.toml`） |
| `[[apps]]` | `egress` | 代理名 或 `"__DIRECT__"` | 应用级覆盖。省略 = 继承 `[egress].default`；`__DIRECT__` = 强制直连 |

继承链（只有这两级）：

```
[[apps]].egress  >  [egress].default  >  直连
```

代理名规则：`^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$`（1–64 字符，区分大小写）；`__DIRECT__` 是保留值，不能当代理名。引用了**尚未定义**的代理名允许保存（跨文件解耦），运行期返回 `egress_proxy_not_found` 警告并按直连处理。

> **模板不支持 `egress`**：模板被多个包共享，语义上不是「每应用出口」。给模板内的应用配应用级出口会为它建立内联记录，而内联记录**整记录覆盖模板**（机型字段会丢），写入时返回 `egress_inline_shadows_template` 警告。想同时保留机型与应用级出口，请把机型字段一起写进该 `[[apps]]` 记录。

### egress.toml：代理定义与探测策略

路径 `/data/adb/device_faker/config/egress.toml`，**权限 0600**（root only）。`config.toml` 仍是 0644、可分享可导出，所以**密码只放这里**。文件由服务维护，手工编辑后重启服务生效。

```toml
version = 1

[proxies.office_http]
type     = "http"              # http | socks5
host     = "198.51.100.10"     # 主机名或 IPv4；不含 scheme / 端口 / 路径
port     = 8080
# username = "<USERNAME>"      # 可选
# password = "<PASSWORD>"      # 可选；永不出现在任何 API / 日志 / 审计 / WebUI

[proxies.lab_socks]
type     = "socks5"
host     = "198.51.100.20"
port     = 1080
username = "<USERNAME>"
password = "<PASSWORD>"

[probe]
url             = "http://checkip.example.com/"   # 只接受 http://（不引 TLS 栈）
timeout_seconds = 5                                # 1–30，默认 5
fail_open       = true                             # 代理不可用时放行直连（默认）
```

- ≤64 条定义；`host` ≤253 字符、`username` ≤128、`password` ≤256。
- 权限被改宽时服务会自愈回 0600 并给 `egress_perm_fixed` 警告；写入配置前的备份 `backups/egress.toml.<stamp>-<rev8>.bak` 同为 0600（`backupId` 即 `<stamp>-<rev8>`）。
- `egress.toml` **不进** `GET /config?format=toml` 导出、不进 WebUI 导出；`GET /api/v1/egress` 只回脱敏摘要（唯一凭据信号是 `hasCredentials`）。
- `username` / `password` 会被**安全存储**（0600、API 只回 `hasCredentials`），但**应用流量不会发送认证**：SOCKS5 的凭据属性可读而 libcore 不发送，HTTP Basic 未实现。带凭据的代理请用 **IP 白名单**授权，或等后续「设备内本地认证中继」（P2）。细节见[已知限制](#已知限制)。

### fail-open 与降级

`fail_open = true`（默认）时，以下三个时点判定代理不可用后，服务会把系统代理切成直连（`settings put global http_proxy :0`）并标记降级——**配置本身不变**，改回去或探测成功即恢复：

1. 显式探测失败，且原因是 `connect` / `dns` / `timeout` / `tls`（`http_status` / `invalid_response` 不触发：链路可能是通的）；
2. 写入后读回与期望不一致；
3. 服务启动自愈时读回不一致。

降级状态在 `GET /api/v1/egress` 的 `health`（`degraded` / `degradedSince` / `degradedReason`）与日志里可见；一次成功探测**且读回校验通过**才恢复（探测成功但 `settings` 补不回去时保持降级）。`fail_open = false`（fail-closed）时判定照做但**不动**系统代理——宁可断网也不换出口，适合「IP 一致性优先」的场景。

两条粒度约定：

- **只有设备级链路才降级**：`POST /api/v1/egress/probe` 带 `package`（`scope=app`）时，失败只记 `health.lastProbe` 并在错误 `details.scope="app"` + `details.warnings[].code="egress_probe_scope_app"`，**不会**清设备级系统代理。
- **降级动作要真的落地才算数**：清直连后按 `settings get` + `dumpsys` 读回校验；没落地时 `health.degradedReason = "settings_unavailable"`、`effectiveMode` 报**实际**模式（不会谎报 `direct`），日志里 `cleared=false`。

### 生效方式

| 粒度 | 生效时机 |
| --- | --- |
| 设备级 | **立即**：写 `settings`，新建立的连接即走代理（已建立的连接需应用重连） |
| 应用级 | **目标应用冷启动后**（与机型伪装一致）；注入在进程启动早期完成，随后由后置重放修正框架覆盖，见下节；必要时 `POST /api/v1/apps/{pkg}/restart` |

```sh
# 看当前出口（服务在跑时）
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8686/api/v1/egress \
  | jq '{mode, effectiveMode, applied, health}'

# 一键回滚直连（也可用 WebUI「网络出口」卡片的开关）
REV=$(curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8686/api/v1/egress | jq -r .revision)
curl -s -X DELETE -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$REV\"" \
  "http://127.0.0.1:8686/api/v1/egress?confirm=true" | jq '{changed, mode}'
```

清除系统代理**只能**用 `settings put global http_proxy :0`（`settings delete global http_proxy` 在 Android 17 上清不掉，实测见 [SERVER.md](SERVER.md)）。服务已封装这条规则，手工操作时请照做。

### 应用级注入如何生效（后置重放）

应用级出口（`[[apps]].egress`）是**进程内注入 + 后置重放**，不是「写一次就完事」。理解这段时序，才能解释「配了却不生效」这类现象。

1. **注入**发生在 Zygisk 的 `pre_app_specialize`（进程启动早期，`Application` 构造之前）：写 `http.proxyHost/Port`、`https.proxyHost/Port`、`socksProxyHost/Port` 与凭据载体，并尝试 `ProxySelector.setDefault(...)`。
2. **框架随后会覆盖**：应用绑定进程时走 `ActivityThread.handleBindApplication` → `ActivityThread.updateHttpProxy(app)` → `Proxy.setHttpProxyConfiguration(cm.getDefaultProxy())`，按**设备级** `ProxyInfo` 把 http/https 代理属性（含 nonProxyHosts 与 ProxySelector）**整体重写**；设备级是直连时就把它们清空。真机实测这次覆盖发生在注入后 **约 28–95ms**（t9 见 95ms，本轮 Pixel 9 见 28ms）。
3. **所以必须重放**：注入之后起一个短命线程，按固定时间表读回属性，一发现与期望不符就把**整套**期望重写一遍（不是只补那一个键）。

重放窗口 **0–2000ms，不做早退**——我们无法知道框架究竟哪一刻写，提前收工就会漏掉后一次覆盖：

| 区间（相对注入时刻） | 节奏 | 用途 |
| --- | --- | --- |
| `0..=160ms` | 每 2ms 一次 | 热点区：抢在框架覆盖前后立刻修回 |
| `165..=300ms` | 每 5ms 一次 | 覆盖更慢的机型 / 更慢的冷启动 |
| `300..=2000ms` | 几何退避（340 起，×5/4+10，末点 2000） | 慢冷启动兜底 |

整窗共 **118 次探针**，绝大部分时间在睡，CPU 代价只是几十次属性读取。

**触发点为什么不在 specialize 里起线程**：`pre_app_specialize` 阶段创建线程会让 zygote 的 `selinux_android_setcontext`（要求进程单线程）失败，进程直接 SIGABRT（Pixel 9 / Android 17 实测 tombstone：`JNI FatalError called: ... selinux_android_setcontext(...) failed`）。因此改为「等进程过渡完成，再由 PLT hook 拉起线程」：在 `libandroid_runtime.so` 上 hook `setpriority`（AOSP 在 `ZygoteHooks.postForkChild`/`SetProcessState` 之后**必然**调用一次 `setpriority(PRIO_PROCESS, 0, PROCESS_PRIORITY_DEFAULT)`），备用触发点 `__system_property_get`。

**只对显式写法注入**：只有 `[[apps]].egress` 写了代理名或 `__DIRECT__` 的进程才会挂 hook、起重放线程。未写 / 空串 → **不注入**（设备级行为本来就由框架写进每个进程；重复注入只会给每个进程多一个 hook、一条线程，还要留住模块映射）。显式写的代理名在 `egress.toml` 里不存在时**不注入也不回落到设备级默认**——宁可直连，也不悄悄换出口。

观测（需要 `debug = true`；同时进 logcat 与模块日志）：

```
[egress] replay hook armed image=libandroid_runtime device=... inode=... symbols=setpriority,__system_property_get   # 模块日志（debug=true）
[egress] replay hook armed package=com.example.a                                                                  # logcat（DeviceFaker 标签）
[egress] replay start package=com.example.a trigger=post_zygote_setpriority
[egress] replay package=com.example.a t=28ms reason=framework_overwrite applied=6/6 changed=[http.proxyHost=-->127.0.0.1, http.proxyPort=-->18082]
[egress] replay done package=com.example.a probes=118 replays=1 divergences=1 state=match elapsed=2003ms
[egress] replay hook unavailable reason=commit_failed error=...
[egress] replay hook not fired within 2000ms (no replay started); module retained (code pages stay mapped for this process) package=com.example.a waited=2000ms
[egress] proxy_selector degraded reason=of_method_missing sdk=Some(37)
```

`state=match` 表示窗口结束时属性都在期望态；`state=mismatch` 表示仍没拉回（例如线程被长时间挂起）；`replay hook unavailable` 表示没挂上 hook，本次退化为「一次性注入」。

`replay hook not fired within …` 是「武装成功、但两个触发点一次都没命中」的诊断行。它有**两个触发时机**（都不依赖 hook，也不建线程）：

1. **重放窗口到点的定时器**：arm 成功后挂一个 `alarm(⌈REPLAY_WINDOW_MS/1000⌉)`（默认窗口 2000ms），到点仍未 spawn 就打一行，并**立刻**把 `SIGALRM` 还原成先前的处置；
2. **进程退出兜底**：进程在窗口内就退出时由 `atexit` 回调补一行（`waited=` 是从 arm 到退出的实际毫秒数，定时器那条则是窗口值）。

**正常路径不打印**：hook 通常 ~26ms 就命中并拉起重放线程，命中时会 `alarm(0)` 撤销定时器、还原信号处置，并把「已上报」置位——所以正常进程里这一行**完全不会出现**（日志与未加诊断前一致）。诊断只打一次，两个时机共用同一开关。

**打印后模块代码页仍然保留**：`armed` 的进程本来就跳过 `dlclose`，而且这里**不能**卸载——重放线程与 hook 的代码就在这个 `.so` 里，卸载即崩。所以行内写明 `module retained (code pages stay mapped for this process)`；此时该进程按名字扫 `/proc/<pid>/maps` 仍然看不到模块（匿名映射，见「已知限制」）。

看到这行说明本次进程只做了 pre-specialize 的**一次性注入**，应用级 HTTP 出口很可能随后被框架覆盖：优先检查 ROM 是否改了 `SpecializeCommon` 的调用顺序、hook 是否被拦。另注意 **SIGKILL**（AMS 杀进程的主要方式）下定时器与退出兜底都不会执行——那种情况只能靠「logcat 里没有 `replay start`」判断。

### 已知限制

- **`dry_run` / `diff` 里的凭据是掩码的**：`PUT /api/v1/egress?diff=true` 或 `?dry_run=true` 返回的 unified diff 会把 `username` / `password` 行的值替换成 `<redacted>`（键名保留），所以看不到密码原文、也看不到新旧密码。这是**有意的安全行为**，不是 diff 缺数据。
- **带用户名 / 密码的代理本期不可用（认证不支持）**：`egress.toml` 里的凭据会被安全存储，但**应用流量不会发送认证**——SOCKS5 注入的 `java.net.socks.username/password` 属性进程内可读，但 libcore 的 SOCKS 客户端不会用它发认证（Android 17 实测：代理侧 `methods=0002 user_ok=0 pass_ok=0`，最终 `auth-failed`）；HTTP 代理只注入 host/port，Basic 认证未实现（只留一行 `[egress] unsupported-auth package=... type=http`）。带凭据的代理请用 **IP 白名单**授权，或等后续「设备内本地认证中继」（P2，尚未实现）。注意**探测**走服务自己实现的客户端：设备端探测在 SOCKS5 下可以带认证、Manager 侧探测只支持 HTTP 代理，所以「探测成功」不等于「应用能出网」。

  为了避免「配了密码却静默不生效」，服务端会在生效代理带凭据时给显式警告：`GET/PUT/DELETE /api/v1/egress` 与 `POST /api/v1/egress/probe` 的 `warnings` 带 `egress_auth_unsupported`（`details={scope:"device"|"app", proxy, field:"username"|"password"}`，同一代理只报一次）。看到它说明**这份凭据不会被使用**。
- **显式配置了 `egress` 的应用会保留模块代码页**：重放线程与 PLT hook 都在模块 `.so` 里，zygisk 不能在它们运行时 `dlclose`，所以这类进程会常驻这段代码（重放跑满 2001ms 后进程继续存活、hook 调用也不崩，说明映射确实保留）。但**按名字扫 `/proc/<pid>/maps` 看不到它**：Zygisk 把模块代码加载成无名/匿名映射（实测 armed 进程 3170 行 maps 里 `device_faker`/`arm64-v8a`/`libzygisk` 命中 0；只有 companion 进程的 maps 能看到 `/data/adb/modules/device_faker/zygisk/arm64-v8a.so`）。真正的可检出性判断是**匿名可执行映射**——对比同机未配置 egress 的进程与本进程的 `r-xp` 匿名段（数量 / 大小 / 偏移），而不是按文件名 grep。**没有**应用级出口的进程照旧立即卸载模块。
- **运行中修改设备级代理不会重放**：重放窗口只覆盖应用冷启动后的 0–2000ms。应用已经在跑时再改设备级 `default`（AMS 广播 `updateHttpProxy`）落在窗口之外，需要重启该应用才会按新值生效。
- **`setpriority` hook 未命中时退化为一次性注入**：`libandroid_runtime.so` 找不到或 PLT hook 提交失败时只记一行 `[egress] replay hook unavailable reason=...`，本次进程没有重放——注入值仍会写入，但可能被框架随后的 `updateHttpProxy` 覆盖（表现为应用级 HTTP 出口不生效、`__DIRECT__` 拉不回直连）。用 `adb logcat -s DeviceFaker` 能直接看到这一行。
- `ProxySelector.of(...)` 在 Android 17（SDK 37）不存在：属性路径是权威载体，selector 只是加分项；缺失时记 `[egress] proxy_selector degraded reason=of_method_missing`（每进程一次），不影响属性注入与重放。
- 设备级只覆盖 HTTP/HTTPS CONNECT（SOCKS5 需方案 B），且**不承载认证**：给设备级指向带凭据的代理会返回 `egress_credentials_ignored` 警告，仍只按 `host:port` 生效。
- UDP / QUIC 不受 HTTP 代理约束（QUIC 可能绕过代理直连）；已建立的连接不会切链路；VPN 类应用会覆盖系统代理。
- WebView 走系统代理（设备级有效、应用级无效）；native 栈（cronet、自研 C++、QUIC）反之。
- **卸载或禁用模块前请先执行 `DELETE /api/v1/egress`**：系统代理写在 Android settings 里，模块被禁用后没有进程能替你清掉。`uninstall.sh` 会在停服后自动清（`settings put global http_proxy :0`），但手动「禁用模块」不会触发它。
- 探测默认请求 `http://checkip.amazonaws.com/`；只接受 `http://`，且探测在**服务进程**内完成（`scope:"app"` 时结果是近似值）。

## 生效方式

所有应用统一走同一执行流（无需选择模式）：

```
① 覆写 Build 字段 → ② 修改系统属性 → ③ DPI 伪装 → ④ 卸载模块
```

- **默认（推荐）**：伪装只在**应用自己的进程内**生效——应用里读到的都是伪装值，其它应用、系统服务和 `getprop` 读到的仍是真值；模块改完立即卸载，不常驻、不残留。
- **`companion_resetprop = true`**：伪装值**对全系统生效**（`getprop`、其它应用也能读到）。应用切到后台或退出后自动恢复真值，回到前台重新应用。

需要让检测工具（多数通过 `getprop` 子进程读取属性）也看到伪装值时，才需要开启 `companion_resetprop`；代价是应用运行期间伪装值对全局可见。

## 完整配置示例

```toml
# ── 全局设置 ──────────────────────────────────────────────
debug = false                        # 调试日志（默认关闭）
default_force_denylist_unmount = false

# ── 机型模板 ──────────────────────────────────────────────
[templates.redmagic_9_pro]
packages = [
    "com.mobilelegends.mi",
  # 仅对 userId=999 生效
  # "com.mobilelegends.mi@999",
    "com.supercell.brawlstars",
]
manufacturer = "Nubia"
brand = "REDMAGIC"
model = "NX809J"
device = "REDMAGIC 11 PRO"
product = "NX809J"
fingerprint = "REDMAGIC/NX809J-UN/NX809J:16/BP2A.250605.031.A3/20251017.000000:user/release-keys"
build_id = "BP2A.250605.031.A3"

# ── 网络出口（可选；详见「网络出口」章节）────────────────
[egress]
default = "office_http"          # 设备级出口；代理定义写在 egress.toml（0600）

# ── 直接配置 ──────────────────────────────────────────────
[[apps]]
package = "com.omarea.vtools"
manufacturer = "Xiaomi"
brand = "Xiaomi"
model = "2509FPN0BC"
device = "Xiaomi 15 Pro"
product = "popsicle"
name = "popsicle"
android_version = "15"
sdk_int = 35
dpi = 420                       # 临时屏幕密度（120–640）
force_denylist_unmount = true  # 覆盖全局默认，仅对该应用启用

[[apps]]
package = "com.example.detected.app"
companion_resetprop = true     # 全系统属性一致（应用退出后自动恢复）
manufacturer = "Custom"

[apps.custom_props]
"ro.custom.property" = "custom_value"
"ro.debug.mode" = "__DELETE__"
```
