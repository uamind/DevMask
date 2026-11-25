// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 模块侧（Zygisk `.so`）**纯逻辑**的宿主机测试载体。
//!
//! # 为什么必须是独立 crate
//!
//! 被测试的代码在根 crate `device_faker`（Zygisk 模块）里，而它依赖
//! `prop-rs-android` / `rsbinder` / `jni` / `zygisk-api` 这些 **Android-only** crate，
//! 在宿主机上根本编不过——所以 `cargo test -p device_faker` 永远跑不起来，模块侧的
//! 单测此前只能靠一个 `/tmp` 里的临时 harness（t14/t17 就是这么跑的）。临时 harness
//! 不进仓库 = CI 覆盖不到 = 迟早腐坏。
//!
//! 这个 crate 用 `#[path]` 把**真实的** `src/egress.rs` 与 `src/spec.rs` 原样编进来
//! （不是复制、不是重新实现），于是 `cargo test -p device_faker_module_tests` 在宿主机/CI
//! 上执行的就是那两份源码里的 `#[cfg(test)] mod tests`——实现改坏了，这里立刻红。
//!
//! # 依赖面（有意压到最小，且必须保持宿主机可编译）
//!
//! - `log`：被测代码用它写 `warn`（本 crate 不装 logger，测试时静默）；
//! - `toml`：spec 本身就是 TOML 编解码，换掉它就变成「测副本」而不是测真实实现；
//! - `device_faker_core`：`Config` 继承链的类型来源（core 自己也是纯 Rust、宿主可编译）。
//!
//! **严禁**在本 crate 引入 `prop-rs-android` / `rsbinder` / `jni` / `zygisk-api` 或任何
//! 会拉起这些依赖的东西。`cargo tree -p device_faker_module_tests` 应当只有
//! `log` + `toml` + `device_faker_core` 及其纯 Rust 传递依赖；一旦出现
//! Android-only crate，宿主机编译失败就是最好的门禁（这也是不要把它合并回主 crate 的原因）。
//!
//! # 哪些东西**不**在这里
//!
//! 只有「纯逻辑」在 `src/egress.rs` / `src/spec.rs`：属性注入计划、重放时间表与循环、
//! arming 的三条退化路径、代理表解析与渲染（含中继 `[relay]` / `local_port` 的
//! 「带凭据 → 127.0.0.1:local_port」改写）、per-package spec 渲染。真正碰 JNI/PLT hook
//! 的部分在 `src/hooks.rs`（`setpriority_hook` / `arm_egress_replay` 的 JNI 适配），
//! 只能在真机上验证，见 docs/design/egress/ 的验收记录。

/// `src/egress.rs` 通过 `crate::config::{Config, DIRECT_MARKER}` 取值；这里把共享配置
/// crate 的导出原样挂到相同路径下，让真实源码无需任何改动就能编译。
pub mod config {
    pub use device_faker_core::config::*;
}

/// `src/spec.rs` 只在 companion（root 域）的 IO 路径上用这两个符号：
/// `render_stamp` / `maybe_render`。宿主机测试只驱动**纯渲染**（`render_map` /
/// `render_doc`），因此这里给同签名桩，且 `load_config` 恒返回「没有配置」——
/// 绝不会真的去读 `/data/adb`。
pub const CONFIG_PATH: &str = "/data/adb/device_faker/config/config.toml";

pub fn load_config() -> Result<Option<config::Config>, String> {
    // 生产签名是 `anyhow::Result`；这里用 `Result<_, String>` 只为不把 anyhow 拉进本 crate
    // （`{err:#}` 对 String 同样成立，且这个 Err 分支在宿主测试里永不构造）。
    Ok(None)
}

/// 真实的模块侧纯逻辑源码（含它自己的 `#[cfg(test)]` 用例）。
///
/// 用 `#[path]` 而不是复制：复制出来的副本测的是副本，实现改了测试也不会红。
#[path = "../../src/egress.rs"]
pub mod egress;

/// spec 渲染的真实源码：中继改写（`127.0.0.1:local_port`）、`__DIRECT__` 与
/// 「未配置」的逐字节不变性、`local_port` 缺失的降级都在这里被驱动。
#[path = "../../src/spec.rs"]
pub mod spec;

/// 源码级守卫：契约 §9.1 K1（v1.2）要求凭据载体**彻底移除**，而不是「留着代码恒 None」。
///
/// 这里直接扫两份注入源码：socks 凭据属性名一旦被重新加回来（哪怕只是注释里的示例），
/// 宿主机测试立刻红——比人工 grep 更可靠。
#[cfg(test)]
mod source_guards {
    const EGRESS_SRC: &str = include_str!("../../src/egress.rs");
    const HOOKS_SRC: &str = include_str!("../../src/hooks.rs");

    #[test]
    fn credential_carrier_keys_are_absent_from_injection_sources() {
        // 分段拼接：本测试自己的源码也会被 include_str! 扫进去，完整字面量会自我命中。
        let keys = [
            ["java.net.socks", "username"].join("."),
            ["java.net.socks", "password"].join("."),
        ];
        for key in keys {
            assert!(
                !EGRESS_SRC.contains(&key),
                "src/egress.rs 仍引用凭据载体 {key}"
            );
            assert!(
                !HOOKS_SRC.contains(&key),
                "src/hooks.rs 仍引用凭据载体 {key}"
            );
        }
    }
}
