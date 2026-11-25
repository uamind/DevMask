// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

#[cfg(target_os = "android")]
mod atexit;
mod companion;
mod config;
mod cow_props;
mod egress;
#[cfg(target_os = "android")]
mod fg_observer;
#[cfg(target_os = "android")]
mod file_logger;
mod hooks;
mod prop_backend;
mod spec;

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use anyhow::Context;
use companion::{handle_companion_request, spoof_system_props_via_companion};
use config::Config;
use hooks::{apply_egress_proxy, arm_egress_replay, egress_replay_active, hook_build_fields};
use jni::{EnvUnowned, errors::ThrowRuntimeExAndDefault};
use log::{LevelFilter, error, info, warn};
use zygisk_api::{
    ZygiskModule,
    api::{V4, ZygiskApi, v4::ZygiskOption},
    raw::ZygiskRaw,
};

pub(crate) const CONFIG_PATH: &str = "/data/adb/devmask/config/config.toml";

/// 版本 banner 用的构建标识，格式为
/// `v<版本> (<git hash>, built <YYYY-MM-DD HH:MM:SS ±HH:MM>) <上游仓库>`，例如
/// `v1.7.0 (16e269f, built 2026-10-31 04:28:53 +08:00) <https://github.com/uamind/DevMask>`。
///
/// - `DF_GIT_HASH` / `DF_BUILD_EPOCH` 由 `build.rs` 在编译期注入（工作区脏会带 `-dirty`）；
/// - 时间按**设备本地时区**渲染并附 UTC 偏移——真机上排查「时钟/时区」相关问题时，
///   能一眼看出这份日志是哪个构建、在什么本地时间编出来的；
/// - 用 `localtime_r` 而不是引 `chrono`：这里只需要一个时间点，不值得为它加依赖。
pub(crate) fn build_ident() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let hash = option_env!("DF_GIT_HASH").unwrap_or("unknown");
    let built = option_env!("DF_BUILD_EPOCH")
        .and_then(format_epoch_local)
        .unwrap_or_else(|| "unknown".to_string());

    format!("v{version} ({hash}, built {built}) <https://github.com/uamind/DevMask>")
}

/// epoch 秒 → 设备本地时间字符串（`YYYY-MM-DD HH:MM:SS ±HH:MM`），失败返回 `None`。
#[cfg(any(target_os = "android", target_os = "linux"))]
fn format_epoch_local(epoch: &str) -> Option<String> {
    let secs = epoch.trim().parse::<libc::time_t>().ok()?;

    // SAFETY: `tm` 全零对 libc 是合法输入，`localtime_r` 会覆盖它；返回值非空即写入成功。
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::localtime_r(&secs, &mut tm) };
    if result.is_null() {
        return None;
    }

    let offset = tm.tm_gmtoff;
    let sign = if offset < 0 { '-' } else { '+' };
    let offset_secs = offset.abs();

    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        sign,
        offset_secs / 3600,
        (offset_secs % 3600) / 60,
    ))
}

/// 非 android/linux 目标（本地 `cargo check` 主机侧）不做本地时间格式化。
#[cfg(not(any(target_os = "android", target_os = "linux")))]
fn format_epoch_local(_epoch: &str) -> Option<String> {
    None
}

#[derive(Default)]
struct MyModule;

impl ZygiskModule for MyModule {
    type Api = V4;

    fn on_load(&self, mut api: ZygiskApi<V4>, _env: EnvUnowned) {
        #[cfg(target_os = "android")]
        file_logger::init_buffer_only();
        // 尽早拉起 companion：它负责把用户配置预渲染成 per-package spec，
        // 这样第一批 app 启动时 spec 已经就绪。
        let _ = companion::send_companion_command(&mut api, &companion::CompanionRequest::Ping);
    }

    fn pre_app_specialize(
        &self,
        mut api: ZygiskApi<V4>,
        mut env: EnvUnowned,
        args: &mut <V4 as ZygiskRaw>::AppSpecializeArgs,
    ) {
        if let Err(err) = self.handle_app_specialize(&mut api, &mut env, args) {
            error!("pre_app_specialize failed: {err:?}");
        }
    }

    fn post_app_specialize(
        &self,
        mut api: ZygiskApi<V4>,
        _env: EnvUnowned,
        _args: &<V4 as ZygiskRaw>::AppSpecializeArgs,
    ) {
        // 重放线程仍在跑时不能 dlclose 模块（见 hooks::egress_replay_active）。
        //
        // 注意：这里**不能**判定「replay hook 没命中」。本模块挂在 setpriority 上，而框架
        // 是在 zygisk 的 specialize 回调之后才调用它（hooks.rs 的时序说明），在 post 阶段
        // 查会把正常武装误判成失败。诊断改由两个更晚的点负责
        // （hooks::install_never_fired_probe）：重放窗口到点的 alarm 定时器 + 进程退出兜底；
        // 正常命中路径两者都静默。
        if !egress_replay_active() {
            api.set_option(ZygiskOption::DlCloseModuleLibrary);
        }
    }

    fn pre_server_specialize(
        &self,
        mut api: ZygiskApi<V4>,
        _env: EnvUnowned,
        _args: &mut <V4 as ZygiskRaw>::ServerSpecializeArgs,
    ) {
        api.set_option(ZygiskOption::DlCloseModuleLibrary);
    }
}

impl MyModule {
    fn handle_app_specialize(
        &self,
        api: &mut ZygiskApi<V4>,
        env: &mut EnvUnowned,
        args: &mut <V4 as ZygiskRaw>::AppSpecializeArgs,
    ) -> anyhow::Result<()> {
        let result = self.do_handle_app_specialize(api, env, args);

        // 在 pre_app_specialize 退出前统一 flush，确保 on_load + specialize 期间
        // 产生的所有日志都能发给 companion 落盘。
        if let Err(e) = flush_log_buffer_to_companion(api) {
            // 这里不能用 error!，否则会产生新的日志又无法 flush。
            // 静默失败，日志将丢失。
            let _ = e;
        }

        result
    }

    fn do_handle_app_specialize(
        &self,
        api: &mut ZygiskApi<V4>,
        env: &mut EnvUnowned,
        args: &mut <V4 as ZygiskRaw>::AppSpecializeArgs,
    ) -> anyhow::Result<()> {
        let package_name = Self::extract_package_name(env, args)?;
        let user_id = Self::extract_android_user_id(args);
        let package_with_user = format!("{package_name}@{user_id}");

        // companion 侧现在自己管理会话状态和恢复逻辑；
        // Zygisk 模块侧不再需要跨应用恢复（ACTIVE_RESET_SESSION 已移除）。

        let (config, egress_proxy) = match load_config_for(api, &package_with_user, &package_name) {
            Ok(Some((cfg, egress_proxy))) => (cfg, egress_proxy),
            Ok(None) => {
                api.set_option(ZygiskOption::DlCloseModuleLibrary);
                return Ok(());
            }
            Err(err) => {
                error!("Failed to load config: {err:#}");
                api.set_option(ZygiskOption::DlCloseModuleLibrary);
                return Ok(());
            }
        };

        configure_log_level(config.debug);

        if config.debug {
            info!(
                "Config loaded with {} apps and {} templates",
                config.apps.len(),
                config.templates.len()
            );
        }

        // 同一包名被多个模板声明时，只有书写顺序最先的那个生效，其余被静默丢弃。
        // 这是最常见的「配了 A 机型却生效了 B 机型」成因，必须留痕。
        // 注意：非 debug 下日志级别为 Off，此告警只在 debug = true 时落盘；
        // 用户侧的主要提示在 WebUI（保存/列表处）。
        if config.debug {
            for key in [package_with_user.as_str(), package_name.as_str()] {
                let claimants = config.conflicting_templates_for_package(key);
                if claimants.len() > 1 {
                    warn!(
                        "Package {key} is claimed by {} templates [{}]; only '{}' takes effect, \
                         the rest are silently ignored. Assign a package to at most one template.",
                        claimants.len(),
                        claimants.join(", "),
                        claimants[0]
                    );
                }
            }
        }

        let merged = config
            .get_merged_config(&package_with_user)
            .or_else(|| config.get_merged_config(&package_name));

        let Some(merged) = merged else {
            if config.debug {
                info!("App {package_name} (user {user_id}) not in config, unloading module");
            }
            api.set_option(ZygiskOption::DlCloseModuleLibrary);
            return Ok(());
        };

        if merged.force_denylist_unmount {
            api.set_option(ZygiskOption::ForceDenylistUnmount);
            if config.debug {
                info!("Force denylist unmount enabled for {package_name}");
            }
        }

        // ── 统一执行流（按需调度）──────────────────────────────────────────
        // ① JNI 字段覆写（始终执行）
        hook_build_fields(env, &merged)?;
        if config.debug {
            info!("Build fields faked successfully");
        }

        // ①-ter 应用级出口代理（方案 B，docs/plans/egress-contract.md §5）
        //   companion 已经把该包实际的代理（含 __DIRECT__）解析好随 spec 下发；
        //   未配置时 egress_proxy 为 None，apply_egress_proxy 立刻返回，不碰任何属性、
        //   不起线程——老配置零回归。注入失败只记日志，绝不影响应用启动（契约 §5.5）。
        //
        //   注意「写一次不够」：框架在 ActivityThread 绑定应用时会按设备级 ProxyInfo
        //   重写 http/https 属性（t9 F1/F2，注入后 ~95ms），所以还要挂一个「过渡完成」钩子，
        //   在 zygote 完成 SELinux/uid 过渡之后拉起重放线程把属性钉住。
        //   不能在 specialize 里直接起线程：那会让 zygote 的 selinux_android_setcontext
        //   失败并 SIGABRT 整个应用进程（真机 tombstone 已确认）。
        if let Some(arming) =
            apply_egress_proxy(env, &package_name, config.debug, egress_proxy.as_ref())
        {
            arm_egress_replay(api, arming);
        }

        // ①-bis 清除 /proc/self/maps 中匹配模式的属性映射（anti-detection）
        if !merged.hide_maps.is_empty() {
            cow_props::unmap_prop_areas(&merged.hide_maps);
        }

        // ② COW 属性伪造（per-process，覆盖 native 读取，零模块驻留）
        //    companion_resetprop = true 时跳过 COW，全部交给 companion resetprop（全局生效）
        let (prop_map, explicit_props) = Config::build_merged_property_map_with_explicit(&merged);
        if config.debug {
            info!(
                "Property map: {} entries ({} explicit custom_props)",
                prop_map.len(),
                explicit_props.len()
            );
        }

        if merged.companion_resetprop {
            // 全属性走 companion resetprop（getprop 和进程内读取一致）。
            //
            // 同时在进程内做一遍 COW（hybrid）：
            //   1. 消除 D1 冷启动窗口 —— 全局 properties 要等 `FG == 包名` 事件才 apply，
            //      在这之前 app 早期读取会看到真值；进程内 patch 发生在
            //      `pre_app_specialize`，早于任何 app 代码。
            //   2. 修复「app 域不可读的属性区」（如 HyperOS 的 vendor_default_prop）：
            //      companion 写得进文件但 app 读不到，只有进程内私有副本可见。
            //   3. COW 克隆会把被克隆节点的 `context_` 改写成 app 可读 context
            //      （见 `PropBackend::repoint_area_and_relabel`），否则 bionic 在
            //      specialize 后的 SELinux 复查会 munmap 掉克隆（真机实测）。
            // 全局与进程内写的是同一批值，getprop 与进程内读取仍然一致。
            let _ = cow_props::apply_cow_spoof(&prop_map, &explicit_props);

            let delete_props = Config::build_delete_props_list(&merged);
            if !prop_map.is_empty() || !delete_props.is_empty() || merged.dpi.is_some() {
                if let Err(e) = spoof_system_props_via_companion(
                    api,
                    &prop_map,
                    &explicit_props,
                    &delete_props,
                    &package_name,
                    merged.dpi,
                    config.debug,
                ) {
                    error!("Companion resetprop (full) failed: {e:?}");
                } else if config.debug {
                    info!(
                        "Companion resetprop (full): {} set + {} delete for {package_name}",
                        prop_map.len(),
                        delete_props.len()
                    );
                }
            }
        } else {
            // 严格 COW：COW 无法处理的属性不再降级为全局 resetprop——
            // 自动 fallback 会修改全局属性（getprop/其他 app 可见），破坏
            // COW 模式的 per-process 隔离承诺。需要全局生效（getprop 子进程
            // 可见、__DELETE__ 删除属性）时显式开启 companion_resetprop。
            let unfound_props = match cow_props::apply_cow_spoof(&prop_map, &explicit_props) {
                Ok(unfound) => unfound,
                Err(e) => {
                    error!("COW spoof failed: {e:?}");
                    Vec::new()
                }
            };

            for (key, value) in &unfound_props {
                warn!(
                    "Strict COW cannot apply '{key}' ({} bytes) for {package_name}; \
                     enable companion_resetprop for global fallback",
                    value.len()
                );
            }

            let delete_props = Config::build_delete_props_list(&merged);
            for key in &delete_props {
                warn!(
                    "Strict COW cannot delete '{key}' for {package_name}; \
                     enable companion_resetprop to use __DELETE__"
                );
            }

            // dpi 与属性伪装无关（companion 经 wm density 修改全局 settings），
            // 配置了仍走 companion。
            if merged.dpi.is_some() {
                let empty: HashMap<String, String> = HashMap::new();
                let no_explicit: HashSet<String> = HashSet::new();
                if let Err(e) = spoof_system_props_via_companion(
                    api,
                    &empty,
                    &no_explicit,
                    &[],
                    &package_name,
                    merged.dpi,
                    config.debug,
                ) {
                    error!("Companion dpi failed: {e:?}");
                } else if config.debug {
                    info!("Companion dpi applied for {package_name}");
                }
            }
        }

        // ②-bis 不可读 context 的进程内合成（S4）
        //   有些属性区对 app 域不可读（如 HyperOS 的 vendor_default_prop）：companion
        //   全局 resetprop 写得进文件但 app 读不到，严格 COW 也会因「路由区未映射」
        //   跳过。这里对 custom_props 里**显式**写出的这类 key，在进程内合成匿名
        //   属性区并挂到对应 context 节点，使 SystemProperties.get 在本进程可见。
        //   两种模式都执行：它只补「本进程永远读不到」的缺口，不改任何真实映射。
        if let Err(e) =
            cow_props::apply_unreadable_ctx_synth(&prop_map, &explicit_props, &merged.hide_maps)
        {
            warn!("Unreadable-context property synth failed: {e:#}");
        }

        // ③ DlClose —— 但应用级代理的「后置重放」还在跑时必须保留模块映射：
        //    重放线程的代码就在这个 .so 里，zygisk 一旦 dlclose，线程执行到的下一条指令
        //    就是野指针。只对配置了 egress 的应用生效；未配置 egress 的应用照旧 dlclose
        //    （启动路径与内存足迹零回归）。
        if egress_replay_active() {
            if config.debug {
                info!("[egress] module kept mapped for the replay window");
            }
        } else {
            api.set_option(ZygiskOption::DlCloseModuleLibrary);
        }
        Ok(())
    }

    fn extract_android_user_id(args: &<V4 as ZygiskRaw>::AppSpecializeArgs) -> u32 {
        // Android 的 app UID = userId * 100000 + appId
        // 这里的 userId 对应 /data/user/<userId>/... 里的数字
        const AID_USER_OFFSET: u32 = 100_000;
        let uid = *args.uid;
        if uid <= 0 {
            return 0;
        }
        (uid as u32) / AID_USER_OFFSET
    }

    fn extract_package_name(
        env: &mut EnvUnowned,
        args: &<V4 as ZygiskRaw>::AppSpecializeArgs,
    ) -> anyhow::Result<String> {
        let result: String = env
            .with_env(|_jenv| -> Result<String, jni::errors::Error> {
                let app_data_dir = args.app_data_dir.to_string();

                if let Some(package) = app_data_dir.rsplit('/').next()
                    && !package.is_empty()
                {
                    return Ok(package.to_string());
                }

                let nice_name = args.nice_name.to_string();

                let mut nice_name: String = nice_name;
                if let Some(idx) = nice_name.find(':') {
                    nice_name.truncate(idx);
                }

                Ok(nice_name)
            })
            .resolve::<ThrowRuntimeExAndDefault>();
        Ok(result)
    }
}

/// 从 companion 取某个包的预渲染 spec（内存，不落盘），按 `pkg@uid` → 裸包名顺序尝试。
///
/// 返回 `(机型配置, 该包的出口代理)`：两者同源于同一份 spec TOML，但解析失败的影响面
/// 完全不同——机型配置解析失败 = 整个 spec 坏了（返回 Err，卸载模块）；出口代理小节
/// 解析失败只等于「没配代理」（[egress::from_spec_doc] 自己吞掉错误返回 None）。
fn load_config_for(
    api: &mut ZygiskApi<V4>,
    package_with_user: &str,
    package: &str,
) -> anyhow::Result<Option<(Config, Option<egress::EgressProxy>)>> {
    let request = companion::CompanionRequest::GetSpec(companion::SpecRequest {
        keys: vec![package_with_user.to_string(), package.to_string()],
    });
    let response = companion::send_companion_command(api, &request)?;
    if response.status != 0 {
        return Ok(None);
    }
    let Some(text) = response.spec else {
        return Ok(None);
    };

    // 出口代理小节与机型字段同源：先单独取它，解析失败只丢代理、不影响机型。
    // 包名只用于日志（非法 relay 视图的 warn 需要它定位到应用）。
    let egress_proxy = egress::from_spec_doc(&text, package);

    let config = Config::from_toml(&text).map_err(|err| {
        // 契约 §7.1：spec 里可能带出口代理的 password，而 toml 错误的 Display 会附上
        // 出错行的**原文**——这里只保留不含源码片段的摘要。
        let detail = err
            .downcast_ref::<toml::de::Error>()
            .map(|err| err.message().to_string())
            .unwrap_or_else(|| "unclassified parse error".to_string());
        anyhow::anyhow!("spec TOML is invalid: {detail}")
    })?;

    Ok(Some((config, egress_proxy)))
}

/// 只在 companion 侧使用：读取 `/data/adb/devmask/config/config.toml` 用于渲染 spec。
/// 模块进程（zygote 域）读不到该路径，不会走到这里。
pub(crate) fn load_config() -> anyhow::Result<Option<Config>> {
    if !Path::new(CONFIG_PATH).exists() {
        return Ok(None);
    }

    let config_content = fs::read_to_string(CONFIG_PATH)
        .with_context(|| format!("Failed to read config at {CONFIG_PATH}"))?;
    let config = Config::from_toml(&config_content)?;
    Ok(Some(config))
}

fn configure_log_level(debug_enabled: bool) {
    let level = if debug_enabled {
        LevelFilter::Info
    } else {
        // 非 debug 不打印任何日志（Error 也不落盘）。
        LevelFilter::Off
    };
    log::set_max_level(level);
}

fn flush_log_buffer_to_companion(api: &mut ZygiskApi<V4>) -> anyhow::Result<()> {
    let lines = file_logger::drain_lines();
    if lines.is_empty() {
        return Ok(());
    }

    let request = companion::CompanionRequest::WriteLog(companion::WriteLogRequest { lines });
    let response = companion::send_companion_command(api, &request)?;
    if response.status != 0 {
        anyhow::bail!(
            response
                .message
                .unwrap_or_else(|| "companion write log failed".to_string())
        );
    }
    Ok(())
}

// Note: The register_module macro should handle the EnvUnowned properly
// The unwrap_unchecked issue is a macro expansion problem in jni 0.22
// We'll let the macro handle this internally
zygisk_api::register_module!(MyModule);
zygisk_api::register_companion!(handle_companion_request);
