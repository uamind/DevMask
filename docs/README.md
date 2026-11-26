**简体中文** | [English](en/README.md) | [Türkçe](tr/README.md)

# DevMask 📱

<img src="logo.png" style="width: 96px;" alt="logo">

**一个应用，一个身份。** 按包名伪造机型、Build 指纹、系统属性与 DPI，出口网络也能按应用独立指定——包括需要账号密码的住宅代理。

[![Version](https://img.shields.io/github/v/release/uamind/DevMask?logo=github)](https://github.com/uamind/DevMask/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/uamind/DevMask/total?logo=github&logoColor=green)](https://github.com/uamind/DevMask/releases)
[![Language](https://img.shields.io/badge/language-Rust-orange?logo=rust&logoColor=orange)](https://www.rust-lang.org/)

> v0.0.1 是 DevMask 的第一个版本，版本号从零重开。历史见 [CHANGELOG](CHANGELOG.md)。

DevMask 是**独立维护的项目**，不是上游仓库的分支镜像。早期版本基于 [Device Faker](https://github.com/Seyud/device_faker)，沿用其配置模型与 Zygisk 注入思路；感谢原作者 Seyud 与社区，原项目的版权与许可声明保留在 [NOTICES](../NOTICES) 与各源文件头。

## 官方源

| 用途 | 地址 |
| --- | --- |
| 主仓库 | [uamind/DevMask](https://github.com/uamind/DevMask) |
| 下载 / 更新 | [Releases](https://github.com/uamind/DevMask/releases/latest) · [Update.json](../Update.json) |
| 机型模板库（我们维护） | [uamind/devmask-config](https://github.com/uamind/devmask-config) |
| 文档站 | [uamind.github.io/DevMask](https://uamind.github.io/DevMask/) |
| 问题反馈 | [Issues](https://github.com/uamind/DevMask/issues) |

## 它解决什么

同一台手机上装十个应用，平台侧能通过机型、Build 指纹、属性、DPI 把你串成一个人；换台设备，出口 IP 又是同一条宽带。DevMask 把这两件事拆开解决：

- **设备侧**：按包名伪造机型与系统属性，只影响配置过的应用，其它应用读到的仍是真值
- **网络侧**：给设备或单个应用指定出口代理，需要认证的代理由设备内中继代填凭据
- **管理侧**：多台设备聚合到一个 Relay，批量下发统一机型，AI 只需要对接一个入口

## 能力一览

| 能力 | 说明 |
| --- | --- |
| 机型 / 模板 | manufacturer、brand、model、device、product、board、SoC、fingerprint、build id 等整套字段，可按模板批量套用 |
| 系统属性 | 属性族展开（含 ro.product 多分区副本）、自定义属性白名单、__DELETE__ / __EMPTY__ 语义 |
| 版本伪装 | Android 版本、SDK、安全补丁、Build.DISPLAY、增量号 |
| DPI | 按应用临时改显示密度（120–640） |
| 应用级出口 | 按包名指定 HTTP 或 SOCKS5 代理，不影响其它应用 |
| 认证中继 | 需要用户名密码的代理由设备内中继代填，支持 HTTP Basic 与 SOCKS5（RFC 1929） |
| 设备级出口 | 整机 HTTP 代理，走系统 settings |
| 批量与聚合 | Manager 代理池、绑定、job 下发、逐设备结果、出口 IP 校验、审计 |
| 观测 | 状态 / 概览 / 模板 / 应用矩阵 / 出口探测 / 操作日志 |

## 三个入口

1. **设备端 WebUI**（单机）：装好模块后在管理器里打开，配置模板与应用，开启"外部访问"即可局域网访问
2. **Manager / Relay**（多设备）：一台机器纳管全部设备，统一机型、代理池、批量下发，浏览器打开即用
3. **AI / 脚本**：设备端 /api/v1 与 Manager /relay/api/v1 都是 HTTP + Bearer，契约分别见 [API.md](API.md) 与 [manager/docs/API.md](../manager/docs/API.md)

## 快速开始

1. 安装模块（需要 Zygisk 实现，如 ZygiskNext；不支持 Magisk 自带 Zygisk）
2. 打开 WebUI，在"模板"里建一个机型模板、从在线模板库套用（模板来自 [uamind/devmask-config](https://github.com/uamind/devmask-config)），或直接编辑某个应用的字段
3. 在"应用"里把模板绑到包名，重启该应用生效
4. 需要出口代理时，在"网络"页里配置；带账号密码的代理会自动走设备内中继
5. 多设备场景：启动 Manager，添加设备（地址 + 设备 Token），之后用代理池与批量下发统一管理

配置文件在 /data/adb/devmask/config/config.toml，字段说明见 [CONFIG.md](CONFIG.md)；服务与外部访问见 [SERVER.md](SERVER.md)。

## 已知限制

- 带认证的代理需要设备内中继（v0.0.1 起内建）；设备级**不支持** SOCKS5，只支持 HTTP
- 应用级注入只覆盖 Java 网络栈，native / WebView 不受控；配了应用级代理的进程会常驻模块代码页
- 出口代理的严格 uid 归属仍待补（当前靠内核 owner match + 限频复核兜底）
- 真机验证覆盖 Pixel 9 / Android 17 / KernelSU，其它机型需自行验证

## 文档

- [配置说明](CONFIG.md) · [服务与 API](SERVER.md) · [API 契约](API.md)
- [出口代理计划](plans/proxy-support-plan.md) · [认证中继计划](plans/authenticated-proxy-plan.md) · [中继契约](plans/relay-contract.md)
- Manager：[API](../manager/docs/API.md) · [架构](../manager/docs/ARCH.md) · [运行](../manager/docs/RUN.md)
- 项目迁移与品牌说明：[repo-migration.md](plans/repo-migration.md)（仓库/模板源/发布通道）

## 致谢

DevMask 早期版本基于 [Device Faker](https://github.com/Seyud/device_faker)（作者 Seyud）开发，配置模型与 Zygisk 注入思路来自该项目。感谢 Seyud 与社区打下的基础；当前仓库由 DevMask 团队独立维护，WebUI、网络出口与多设备管理在此基础上重构。Zygisk 生态参考项目见[文档站致谢页](https://uamind.github.io/DevMask/guide/#致谢)。

原项目的版权与许可声明保留在 [NOTICES](../NOTICES) 与各源文件头。

## 许可与二次分发

GPL-3.0-or-later。二次分发时请保留源文件中的 SPDX 标识、版权行与许可声明，并提供对应完整源码。
