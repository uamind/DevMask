# DevMask 📱

**一个应用，一个身份。** DevMask 按包名伪造机型、Build 指纹、系统属性与 DPI，并可为每个应用单独指定出口网络；未配置的应用读取的仍是真机值。

## 它解决什么

十个应用装在一台手机上，平台侧靠机型、Build、属性与 DPI 就能把你串成同一个人；换一台设备，出口 IP 往往还是同一条宽带。DevMask 把这两件事拆开：

- **设备侧**：按包名伪造机型与系统属性，只影响配置过的应用
- **网络侧**：为设备或单个应用指定出口代理，需要认证的代理由设备内中继代填凭据
- **管理侧**：多台设备聚合到 Manager / Relay，统一机型、代理池与批量下发

## 能力一览

| 能力 | 说明 |
| --- | --- |
| 机型 / 模板 | manufacturer、brand、model、device、product、board、SoC、fingerprint、build id 整套字段，按模板批量套用 |
| 系统属性 | 属性族展开（含 ro.product 多分区副本）、自定义属性白名单、__DELETE__ / __EMPTY__ 语义 |
| 版本与 DPI | Android 版本、SDK、安全补丁、Build.DISPLAY、增量号；按应用调整显示密度（120–640） |
| 应用级出口 | 按包名指定 HTTP 或 SOCKS5 代理，不影响其它应用 |
| 认证中继 | 需要用户名密码的代理由设备内中继代填，支持 HTTP Basic 与 SOCKS5（RFC 1929） |
| 批量与审计 | Manager 代理池、绑定、job 下发、逐设备结果、出口 IP 校验、操作日志 |

## 快速开始

1. 安装模块（需要 Zygisk 实现，如 ZygiskNext）
2. 打开 WebUI，在「模板」里建一个机型模板，或直接编辑某个应用
3. 在「应用」里把模板绑到包名，重启该应用生效
4. 需要出口代理时，在「网络」里配置（设置页只留一个入口）；带账号密码的代理会自动走设备内中继
5. 多设备场景：启动 Manager，添加设备（地址 + 设备 Token），之后用代理池与批量下发统一管理

## WebUI

- **概览**：模块版本、伪装应用数、模板数、出口状态与操作入口
- **模板**：新建、编辑、导入导出，支持在线模板库检索与套用
- **应用**：按包名配置身份，分身可单独配置或继承主应用
- **网络**：设备级出口、代理定义、出口 IP 探测、本机中继与「一键直连」
- **设置**：主题 / 语言、模块与工具、外部访问与 API 访问、关于

更多细节见 [WebUI 指南](./webui.md)、[配置说明](./configuration/index.md) 与 [常见问题](./faq.md)。

## 在线模板库

在线模板库的机型配置来自 DevMask 维护的 [uamind/devmask-config](https://github.com/uamind/devmask-config)，可直接在 WebUI 里预览并套用到包名；自有源不可用时会回落到上游社区仓。

## 致谢 🙏

本项目在开发过程中参考了以下优秀项目：

- [zygisk-dump-dex](https://github.com/ri-char/zygisk-dump-dex) - 提供了 Rust 开发 Zygisk 模块的原型参考
- [zygisk-api-rs](https://github.com/rmnscnce/zygisk-api-rs) - 提供了 Zygisk API 的 Rust 依赖支持
- [MiPushZygisk](https://github.com/wushidia/MiPushZygisk) - 提供了 Zygisk 机型伪装的方案参考

---

**📱 让设备不为应用的机型限制所困！** 🚀
