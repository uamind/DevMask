# 📝更新日志

## 🚀[v0.0.1] - 2026-11-25 · 更名首发

> 项目更名为 **DevMask**，版本号从零重开。以下历史章节属于 Device Faker 时期，保留备查。

### 🆕 新功能
- 🌐 带认证代理：设备内中继代填凭据，支持 HTTP Basic 与 SOCKS5（RFC 1929）
- 🔀 应用级出口代理：按包名指定出口，支持 __DIRECT__ 强制直连
- 🧭 Manager 代理池：绑定、批量下发、出口 IP 校验与审计
- 🛡️ fail-closed 防火墙：三链 iptables + uid 白名单，规则被外部清掉会自动复核

### ⚡ 改进
- 📦 模块 id / 运行路径 / API 头 / 存储键统一改为 devmask 前缀
- 📚 重写 README，按"设备侧 / 网络侧 / 管理侧"重新组织

### ⚠️ 已知限制
- 设备级不支持 SOCKS5；应用级注入只覆盖 Java 网络栈
- 严格 uid 归属待补（当前靠内核 owner match + 限频复核）

## ⬆️[v1.10.0 → v1.11.0] - 2026-12-02

### 🆕 新功能
- 🔐 **设备内认证中继（relay）**：带用户名/密码的代理由设备内一个只监听 `127.0.0.1` 的中继转发，替应用补 HTTP Basic / SOCKS5 RFC1929；设备级与应用级都覆盖，应用零改造
- 🔌 中继端口表：`egress.toml` 新增 `[relay]` 段与只读 `local_port`；区间 17800–17899，设备级占 `base_port`、应用级哈希分配，跨重启稳定
- 🛡️ uid 防火墙：三条 iptables 链（`DM_EGRESS_RLY` / `_ALLOW` / `_DENY`），只放行授权 uid；重建期间 **fail-closed**（DENY 兜底，不裸奔）
- 🔁 `POST /api/v1/egress/relay/restart`（`ops`，强制 apply）；`GET /egress` 增 `relay` 对象；`GET /egress/proxies` 增 `localPort` / `relayActive`
- 🔍 探测新增 `via=relay`（缺省仍 `upstream`）：一次覆盖「本地端口在监听 + 中继转发 + 上游认证 + 目标往返」；新增 `relay_not_listening` / `relay_connect` 两个 reason

### ⚡ 改进
- 🚫 **注入 fail-closed**：relay 视图非法（host 非回环 / 端口池外 / type 非法）时注入 `127.0.0.1:9` 并 arm replay，而不是「按未配置处理」——应用断网，但不会静默换出口
- 🧯 回退收窄：只有「中继明确未启用」（`[relay].enabled=false` 或文件未迁移）才回退上游；开启但缺 `local_port` 一律 fail-closed
- 🧹 凭据边界：v1.2 起**不再把凭据写进应用进程**（≤v1.1 会写 `java.net.socks.username/password`，实测不生效）
- 📊 Manager：`DeviceSummary.egress.relay = {enabled, active, listeners, firewallApplied}`；`/devices/{id}/egress` 透传 `relay`；`socks5` 探测真连（reqwest `socks` feature）
- 📚 文档：API.md / openapi.yaml（30 个操作）/ CONFIG.md / SERVER.md / Manager 契约与架构同步；新增 5 个 `egress_relay_*` 警告码（含设备级 socks5 的 `egress_relay_device_socks5_unsupported`）

### ⚠️ 已知限制
- 中继端口（`127.0.0.1:178xx`）与链名 `DM_EGRESS_RLY` 属**可检出项**；中继出网的是 **root** 进程，会绕过 AFWall+ / NetGuard 等按 uid 的防火墙
- **设备级依赖中继存活**：listener 挂掉期间整机代理链路不可用；`fail_open=true` 时下一次上游探测失败会**静默直连**（要「绝不直连」请把 `[probe].fail_open` 设为 `false`）
- 只有**带凭据**的代理才走中继；关闭中继没有 API（编辑 0600 的 `egress.toml` 的 `[relay].enabled` + restart）
- relay 只覆盖 **Java 层**：native 栈与 WebView 不受 fail-closed 保护
- 🪜 **分层 uid 策略（可用性与安全的折中）**：应用级端口在 accept 后先判 uid；uid 归属失败时，只有 `relay.firewall.applied=true`（同一次 `iptables -S` 采样确认三链生效）才放行，并记限频日志 `uid_unresolved_firewall_enforced`（≤1 次/秒）；`applied=false` / `firewall=false` 一律 fail-closed。放行前会限频复核 `-S`，规则被手工 `-F` 后 ≤1s 回到 fail-closed；代价是 root-trust（只约束谁能连端口，不约束以谁 uid 出网）
- ⚠️ **设备级 `default` 只支持 http**：合并后的有效 default 指向非 http 定义时 PUT 直接 `422 invalid_egress_default`（与 `[relay].enabled` / 凭据无关）；已存在的旧配置在 `enabled=true` 时 `settings` 黑洞 `127.0.0.1:9`（fail-closed），`enabled=false` 时按未配置（直连）；两种都**不会**在 `base_port` 建 SOCKS5 listener，且都报 `egress_relay_device_socks5_unsupported`。socks5 定义请只用于 `[[apps]].egress`
- ⚠️ **本地端口劫持窗口**：listener 释放到重绑之间端口可能被本机进程抢占；检测靠下一次 bind 失败（`addr_in_use`），应用级会重分配并在 `relay.portChanges` / `restartRequiredApps` 提示，设备级则不监听 + `egress_relay_bind_failed`

## ⬆️[v1.9.0 → v1.10.0] - 2026-12-01

### 🆕 新功能
- 🌐 **出口代理（egress）**：整机级 HTTP/HTTPS 代理写入 Android 全局设置（`settings global http_proxy`），应用流量按指定出口出网
- 🧩 应用级出口：Zygisk 在目标进程内注入 `http/https/socksProxy` 与 `ProxySelector`，支持 **SOCKS5** 与每应用独立出口；`__DIRECT__` 强制直连（会主动覆盖系统代理）
- 📡 出口 IP 探测：`POST /api/v1/egress/probe` 经当前链路请求可配置探测 URL，返回出口 IP / 延迟 / 时间；失败给结构化错误码
- 🗄️ Manager 代理池：`/relay/api/v1/egress-proxies` CRUD、绑定（整机 / 应用级）、批量下发（job `kind=egress`）、代理侧独立探测
- 🖥️ 双侧 WebUI：设备端设置页「网络出口」卡片（代理配置 + 探测 + 直连回滚），Manager 新增「代理池」页并在设备卡片显示出口 IP / 代理名
- 🔐 凭据保护：代理定义独立存 `egress.toml`（`0600`），API / 日志 / 审计 / WebUI 全程只给 `hasCredentials`；Manager 侧用 `devmask-manager.key` AES-256-GCM 加密落库

### ⚡ 改进
- 🧯 清除系统代理只用 `settings put global http_proxy :0`（`settings delete` 在 Android 17 上清不掉，实测记录在案）
- 🛟 fail-open 默认开启：探测失败或读回不一致时自动回落直连，一次配错不会让设备断网
- 🔁 幂等：期望状态与现状一致时不写盘、不备份、revision 不变（`changed:false`）；Manager 重放同内容记 `egress_unchanged` 而不是把 job 变成 partial
- 🧹 卸载 / 停用前清理：`uninstall.sh` 停服后清空系统代理并删运行时目录
- 📚 文档：新增 [API.md](API.md) egress 五端点、[CONFIG.md](CONFIG.md)「网络出口」章节、[openapi.yaml](openapi.yaml) 五个操作、Manager 契约与架构同步
- ⚖️ 应用级注入改为**只对显式写法**生效：只有 `[[apps]].egress` 写了代理名或 `__DIRECT__` 的进程才挂 hook / 起重放线程（设备级默认不再给每个应用加开销）
- ⚠️ 明确记录重放机制的三条代价：① 显式配置 egress 的进程会保留**常驻模块代码页**（重放线程与 PLT hook 都在 `.so` 里）——但**按名字扫 `/proc/<pid>/maps` 看不到**（Zygisk 用无名/匿名映射，实测 armed 进程 3170 行命中 0），真判断要比匿名可执行段；② 应用运行中修改设备级代理不落在 0–2000ms 重放窗口内，需重启应用；③ `setpriority` hook 未命中时退化为一次性注入（`replay hook unavailable`），注入值可能被框架覆盖
- ⚠️ **带用户名/密码的代理在 v1.10.0 时不可用**：SOCKS5 的 `java.net.socks.username/password` 进程内可读，但 libcore 不发送凭据（Android 17 实测代理侧 `methods=0002 user_ok=0 pass_ok=0` → `auth-failed`）；HTTP Basic 未实现（只注入 host/port，记 `unsupported-auth`）。带凭据的代理请用 **IP 白名单**授权（v1.11.0 起改为经设备内中继生效，见下方 v1.11.0 分节）

### 🐛 修复
- 🔁 修复应用级 HTTP 出口在目标进程内不生效：框架（`ActivityThread.handleBindApplication` → `Proxy.setHttpProxyConfiguration`）会在注入后约 28–95ms 按**设备级**代理整体重写 http/https 属性；现在改由 PLT hook（`setpriority`）在进程过渡完成后拉起重放线程，按 0–2000ms 时间表读回并重写（118 次探针、不早退）
- 🚫 修复 `__DIRECT__` 在设备级代理开启时失效（与上条同源：清空动作被框架随后写入覆盖，重放会把直连状态拉回来）
- 🧩 修复 `ProxySelector.of(...)` 在 Android 17（SDK 37）不存在导致的每进程告警：改为先探能力再调用，缺失时只记一行 `proxy_selector degraded`（selector 是加分项，属性路径才是权威载体）
- 🔒 修复配置读快照在跨文件事务中自我死锁（持排他 flock 时又取共享锁）
- 🔐 修复 `dry_run` / `diff` 响应回显代理密码：diff 里 `username` / `password` 行的值现在行级脱敏为 `<redacted>`（键名保留，旧值与新密码都不出现）
- 🎯 修复应用级探测失败误清设备级代理：`scope="app"` 的失败只记 `health.lastProbe`，`502` 带 `scope` 与 `egress_probe_scope_app` 警告，**不再**执行 `settings put global http_proxy :0`；只有设备级失败才触发 fail-open
- 📦 修复 `PUT /egress` 的 `apps` 数组语义：`apps: []` 与 `apps: null` 都清空全部 `[[apps]].egress`（整体替换），未列出的应用会删掉 `egress` 键

## ⬆️[v1.6.0 → v1.7.0] - 2026-11-09

### 🆕 新功能
- 🩺 app 域不可读的属性区也能伪装
- 🚀 WebUI 接入 KernelSU pm/io 接口，包枚举与文件读写全程零 shell fork
- 🔄 模板转换改纯前端，直接导入旧模块 ZIP / system.prop，无需 CLI 与临时文件
- 🎯 在线模板库支持按模板描述搜索，一次拉取聚合文件替代逐文件请求
- 🧩 补齐 custom_props 显式指令与 __DELETE__（soc_model / android_version 删族、sdk_int 可删除）
- 🪟 前台应用检测改用 TOP 集合，多窗口 / 小窗下不再误识别
- 🏷️ 日志新增版本 banner（版本号 + git hash + 构建时间）

### 🐛 修复
- 🛡️ 修复因 Zygisk Next 1.5.3 的 sepolicy.rule 大改导致读不到配置伪装全部失效（配置改由 companion 读取）
- 🎲 修复多模板声明同一包名时机型随机（命中改为 config 书写顺序，WebUI 提示争用）
- 🇨🇳 修复中文设备安装文案显示英文
- 🔍 修复 WebUI 分身配置解析错位、编辑分身会误删主应用条目
- 🧹 修复同一应用实例重复显示与「已配置」误判，并标注配置作用范围
- 🕵️ 属性区副本改为匿名映射伪装
- 🧪 修复 hybrid 模式冷启动窗口内属性未生效（companion_resetprop 也跑进程内 COW）

### ⚡ 改进
- 🏎️ companion 读取属性零 fork —— 存在性判定与备份直接查属性区
- 👻 属性写入「本机不存在就不写」，不再留下可枚举的幽灵属性
- 📦 模块不再打包 devmask_cli，安装包体积减小
- 🧱 cow_props 拆分为 11 个模块，职责分层便于维护
- ⬆️ 依赖全量升级：rsbinder 0.11、WebUI 依赖 20+ 项、CI 升 NDK r30

## ⬆️[v1.5.0 → v1.6.0] - 2026-09-26

### 🆕 新功能
- 🏷️ Build层新增 Build.BOARD / Build.SOC_MODEL 伪装
- 📅 新增 security_patch 安全补丁日期伪装字段
- 🆔 新增 display_id / incremental 专用伪装字段
- 📐 新增按应用 DPI 伪装支持
- ✏️ WebUI 自定义属性伪装开放编辑
- 📦 安装脚本 Zygisk 检测兼容独特 Zygisk 实现
- 🔗 关注模块弹窗新增官网入口

### 🐛 修复
- 🔄 修复前台门控两类失效 — 应用首启竞态与 packages.list 快照过期
- 🌐 修复在线模板库大量加载失败
- 🧹 修复开关改回默认后键值残留问题
- 🎯 修复多开包名「已配置」状态误判
- 🔒 属性区改用进程私有副本，消除 maps 中的 rw-p 指纹
- ⏱️ 切后台恢复提速至 0.5 秒

### ⚡ 改进
- 🏎️ 长属性值支持原地更新，无需删除重建
- 🤖 rsbinder 升级至 0.10，适配 Android 10~17
- 🗑️ 移除 CPU 伪装功能
- 🔇 非 debug 模式下模块与 companion 完全静默
- 🚀 WebUI 首屏路径瘦身与应用列表性能优化
- 💾 安装脚本默认保留已有配置

## ⬆️[v1.4.0 → v1.5.0] - 2026-08-01

### 🆕 新功能
- 🌍 新增土耳其语翻译
- 🧬 新增CPU伪装能力
- 👥 WebUI应用页支持显示多用户应用
- 📦 安装脚本新增Zygisk检测与中英双语自动切换
- 🔒 新增COW属性伪造引擎 — 替代full模式，per-process隔离零驻留
- 🔄 companion_resetprop 开关替代 resetprop 模式
- 🏷️ Build层新增 Build.HARDWARE 伪装

### ⚡ 改进
- ⚡ 优化模块底层性能（by @Tools-cx-app）
- 👆 WebUI支持侧滑返回逐级导航
- 🎬 优化WebUI页面衔接动画
- 🏗️ 日志系统重构
- 🎨 优化WebUI样式
- 📦 更新WebUI构建依赖
- 🏗️ 移除WebUI-X API，采用纯KernelSU WebUI API
- 🏎️ resetprop 启用 skip_svc 直写 + 属性区压缩
- 🏗️ 移除模式区分 — 统一为COW + companion唯一执行流
- 🔄 companion resetprop 恢复机制重构优化

## ⬆️[v1.3.5 → v1.4.0] - 2026-05-02

### 🆕 新功能
- 🖥️ WebUI应用页支持系统应用显示切换
- 🌐 WebUI加入在线模板GitHub源
- 🏷️ 为三种模式补充 build_id 与 Build.ID 伪装支持
- 📥 WebUI模板页新增导入/导出工具与本机模板导出功能
- 📤 WebUI模板页模板卡片添加导出按钮
- 👆 WebUI支持左右滑动切换页面
- ✏️ WebUI模板页支持编辑已存在模板名称

### 🐛 修复
- 🧹 修复WebUI模板编辑页清空部分字段后旧值残留问题
- 🔄 修复resetprop模式自定义属性伪装会话在前后台切换时的状态同步问题

### ⚡ 改进
- 🔗 更新适配jni 0.22 API
- ⚡ 大幅优化WebUI页面切换性能
- 🏎️ 优化WebUI应用列表加载性能
- 🏗️ 重构WebUI在线模板库全链路
- 📏 使用最前沿Web技术 pretext 优化WebUI在线模板虚拟列表文本高度测量
- 🚀 优化WebUI首屏切页响应
- 🎬 添加WebUI页面切换动画
- 🦀 将外部调用resetprop二进制重构为内置Rust库（by @Tools-cx-app）
- 💾 改进配置文件备份格式

## ⬆️[v1.3.0 → v1.3.5] - 2026-03-20

### 🆕 新功能
- 👁️ WebUI主页添加关注模块

### 🐛 修复
- 🔧 修复新版本Android System WebView渲染逻辑改变导致的WebUI无法滑动
- 🔧 修复旧版本Android System WebView不支持ES2022标准无法解析配置

### ⚡ 改进
- 🏗️ 使用xtask替换python脚本进行核心编译（by @Tools-cx-app）
- 🌐 WebUI优化在线模板库加载，增加wget加载方式并实现重试机制
- 🎨 优化WebUI应用配置编辑界面交互
- ⚡ 优化WebUI应用列表加载显示
- 📋 完善WebUI配置读取解析与编辑保存

## ⬆️[v1.2.0 → v1.3.0] - 2026-01-22

### 🆕 新功能
- 📦 WebUI支持显示未安装但已配置的包名
- 🏷️ WebUI在线模板库新增品牌分类
- 🤖 新增Android版本伪装和自定义属性功能
- 🔍 添加模板搜索功能
- 👥 WebUI支持显示区分多用户包名

### ⚡ 改进
- 🎨 优化WebUI样式及布局
- ⚡ 优化WebUI应用列表加载
- 🔧 characteristics 字段支持full模式
- 🔒 优化full模式__system_property_get 函数拦截
- 🔄 重构WebUI配置转换，支持直接选择

## ⬆️[v1.1.0 → v1.2.0] - 2025-12-17

### 🆕 新功能
- 🌐 WebUI添加在线模板库功能
- 🛠️ 添加CLI工具用于配置转换和在线模板加载
- 🔧 添加resetprop模式支持及characteristics属性
- 🌍 WebUI新增多语言支持
- 📋 添加FORCE_DENYLIST_UNMOUNT支持
- 👥 添加对多用户环境的配置支持

### ⚡ 改进
- 💾 安装脚本更改备份文件后缀为.bak以便于恢复
- 📱 WebUI适配KernelSU API 2.1.1以支持KernelSU 2.1.2及以上版本应用名称和图标显示
- 🎨 适配KernelSU WebUI沉浸标准
- 🌐 WebUI使用fetch API替代curl命令进行网络请求，提升兼容性
- 🏗️ 模块化架构重构Zygisk模块核心
- 🎨 优化WebUI显示和布局
- 🏗️ 重构WebUI模板页面为组件化结构
- 🔄 将C++ atexit实现迁移为Rust实现
- 📋 WebUI添加配置元数据支持

## ⬆️[v1.0.5 → v1.1.0] - 2025-11-09

### 🆕 新功能
- 🌐 新增WebUI界面，提供更友好的配置管理体验
- ⚙️ 安装脚本添加配置选择功能，支持个性化安装选项

### ⚡ 改进
- 🗺️ 优化配置字段属性映射逻辑，提升配置处理效率
- 📦 迁移到新的zygisk-api-rs库并更新依赖，兼容更多Zygisk实现
- 🔧 更新Rust edition至2024版本，利用最新语言特性
- 📚 优化配置文档结构，提升可读性和易用性

## ⬆️[v1.0.0 → v1.0.5] - 2025-11-04

### 🆕 新功能
- 📱 新增机型模板配置，便捷应用到多包名
- 🔄 新增lite和full双模式配置，满足不同使用场景
- 💾 新增模块安装备份配置机制，提升稳定性

### ⚡ 改进
- 🚀 优化核心性能和体积，运行更流畅
- 🧩 优化模块卸载逻辑，减少内存驻留时间
- 🎯 使用local_cxa_atexit_finalize_impl增强隐蔽性
