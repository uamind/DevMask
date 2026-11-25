// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

use std::{
    ffi::{c_int, c_void},
    sync::{
        Mutex, Once,
        atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use jni::{
    Env, EnvUnowned, JavaVM, Outcome, jni_sig, jni_str,
    objects::{JClass, JObject, JString, JValue, JValueOwned},
    strings::JNIStr,
};
use log::{info, warn};
use zygisk_api::api::{V4, ZygiskApi};

use crate::config::{MergedAppConfig, NumberSetting};
use crate::egress::{
    Divergence, EgressProxy, EgressType, InjectionPlan, PropertyOp, ReplayExpectation,
    ReplaySummary, ReplayTick, SelectorSpec, replay_expectations, run_replay_loop,
};

/// Build 字段的有效伪装值：与 config::field_value 相同的归一化——
/// 未设置/空串/`__DELETE__` 返回 None（跳过 hook），`__EMPTY__` 归一化为空串。
fn field_str(field: &Option<String>) -> Option<&str> {
    match field.as_deref() {
        Some(value) if !value.is_empty() && value != "__DELETE__" => {
            Some(if value == "__EMPTY__" { "" } else { value })
        }
        _ => None,
    }
}

/// 根据合并配置 Hook android.os.Build 的静态字段。
pub fn hook_build_fields(
    env: &mut EnvUnowned,
    merged_config: &MergedAppConfig,
) -> anyhow::Result<()> {
    env.with_env(|jenv| -> Result<(), jni::errors::Error> {
        let build_class = jenv.find_class(jni_str!("android/os/Build"))?;

        if let Some(manufacturer) = field_str(&merged_config.manufacturer) {
            set_build_field(jenv, &build_class, jni_str!("MANUFACTURER"), manufacturer)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(brand) = field_str(&merged_config.brand) {
            set_build_field(jenv, &build_class, jni_str!("BRAND"), brand)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(model) = field_str(&merged_config.model) {
            set_build_field(jenv, &build_class, jni_str!("MODEL"), model)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(device) = field_str(&merged_config.device) {
            set_build_field(jenv, &build_class, jni_str!("DEVICE"), device)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(product) = field_str(&merged_config.product) {
            set_build_field(jenv, &build_class, jni_str!("PRODUCT"), product)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        // HARDWARE 字段
        if let Some(hardware) = field_str(&merged_config.hardware) {
            set_build_field(jenv, &build_class, jni_str!("HARDWARE"), hardware)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        // BOARD 字段
        if let Some(board) = field_str(&merged_config.board) {
            set_build_field(jenv, &build_class, jni_str!("BOARD"), board)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        // SOC_MODEL 字段（API 29+ 存在；Build.SOC_MODEL 的属性来源是 ro.soc.model）
        if let Some(soc_model) = field_str(&merged_config.soc_model) {
            set_build_field(jenv, &build_class, jni_str!("SOC_MODEL"), soc_model)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(fingerprint) = field_str(&merged_config.fingerprint) {
            set_build_field(jenv, &build_class, jni_str!("FINGERPRINT"), fingerprint)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(build_id) = field_str(&merged_config.build_id) {
            set_build_field(jenv, &build_class, jni_str!("ID"), build_id)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        // Build.DISPLAY 的属性来源是 ro.build.display.id（单一属性，无分区副本）
        if let Some(display_id) = field_str(&merged_config.display_id) {
            set_build_field(jenv, &build_class, jni_str!("DISPLAY"), display_id)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        hook_version_fields(jenv, &build_class, merged_config)
            .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;

        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
    Ok(())
}

fn hook_version_fields(
    env: &mut Env,
    _build_class: &JClass,
    merged_config: &MergedAppConfig,
) -> anyhow::Result<()> {
    let version_class = env
        .find_class(jni_str!("android/os/Build$VERSION"))
        .context("Failed to find Build.VERSION class")?;

    if let Some(android_version) = field_str(&merged_config.android_version) {
        set_build_field(env, &version_class, jni_str!("RELEASE"), android_version)?;
    }

    // `__DELETE__`（NumberSetting::Delete）不 hook 该字段：与字符串字段的
    // `__DELETE__` 语义一致——只由 delete_props 列表在 companion 侧删除属性，
    // Java 层字段保持系统原值。
    if let Some(sdk_int) = merged_config.sdk_int.and_then(NumberSetting::value) {
        set_build_int_field(env, &version_class, jni_str!("SDK_INT"), sdk_int as i32)?;
    }

    // Build.VERSION.INCREMENTAL 的属性来源是 ro.build.version.incremental
    if let Some(incremental) = field_str(&merged_config.incremental) {
        set_build_field(env, &version_class, jni_str!("INCREMENTAL"), incremental)?;
    }

    // Build.VERSION.SECURITY_PATCH 的属性来源是 ro.build.version.security_patch
    if let Some(security_patch) = field_str(&merged_config.security_patch) {
        set_build_field(
            env,
            &version_class,
            jni_str!("SECURITY_PATCH"),
            security_patch,
        )?;
    }

    Ok(())
}

fn set_build_field(
    env: &mut Env,
    build_class: &JClass,
    field_name: &JNIStr,
    value: &str,
) -> anyhow::Result<()> {
    let _field_id = env
        .get_static_field_id(build_class, field_name, jni_sig!("Ljava/lang/String;"))
        .with_context(|| "Failed to get field ID".to_string())?;

    let new_value = env
        .new_string(value)
        .with_context(|| format!("Failed to create string for {value}"))?;

    env.set_static_field(
        build_class,
        field_name,
        jni_sig!("Ljava/lang/String;"),
        JValue::Object(&new_value),
    )
    .with_context(|| "Failed to set field".to_string())?;

    Ok(())
}

fn set_build_int_field(
    env: &mut Env,
    build_class: &JClass,
    field_name: &JNIStr,
    value: i32,
) -> anyhow::Result<()> {
    let _field_id = env
        .get_static_field_id(build_class, field_name, jni_sig!("I"))
        .with_context(|| "Failed to get field ID".to_string())?;

    env.set_static_field(build_class, field_name, jni_sig!("I"), JValue::Int(value))
        .with_context(|| "Failed to set field".to_string())?;

    Ok(())
}

// ── 应用级出口代理注入（方案 B，docs/plans/egress-contract.md §5）───────────────
//
// 注入发生在 `pre_app_specialize`，早于任何应用代码执行；只动 Java 层（进程内
// 系统属性 + `ProxySelector`），不改 native（cronet / 自研 C++ 栈不在覆盖范围内）。
//
// **为什么「写一次」不够（t9 真机验收 F1/F2，docs/design/egress/t9-acceptance.md §5.2/§5.3）**：
// 框架在 `ActivityThread` 绑定应用时按**设备级** `ProxyInfo` 调
// `android.net.Proxy.setHttpProxySystemProperty(...)`，把 http/https 代理属性整体重写一遍
// （设备级直连则直接清空）。实测注入后 ~95ms 就被覆盖：应用级 http 代理读不到、`__DIRECT__`
// 也拉不回直连。zygisk 的 pre/post_app_specialize 都早于它，所以必须**再写一次**——
// 见 [arm_egress_replay]：注入后起一个短命线程，按 [crate::egress::replay_offsets_ms]
// 的节奏读回属性，一发现不符（= 框架刚写过）就重放，直到稳定或窗口用尽。
// 真机时间序证据走 logcat（标签 `DeviceFaker`），见 [emit_logcat]。
//
// 三条硬约束：
//   1. **未配置 = 零动作**：[apply_egress_proxy] 的第一个分支就是 return，连一次 JNI
//      调用、一个线程都不会多发——老配置（没有 `[egress]` / `[[apps]].egress`）的行为
//      与本次改动之前完全一致；
//   2. **失败只记日志**：每一步都是「尝试 → 记一行 → 继续下一项」，最外层还有
//      `catch_unwind`（jni 的 `with_env` 自带），重放线程里的错误同样只进日志——
//      绝不把错误抛给 `app_specialize`（契约 §5.5）；
//   3. **密码不进日志**：只输出 type/host/port/creds=0|1、失败步骤名，以及凭据载体的
//      `set:true|false`；[EgressProxy] 的 `Debug` 也已对密码打码。

/// `java.net.ProxySelector` 注入的结果（契约 §5.1 第 6 项）。
///
/// Android 的 libcore **没有** `ProxySelector.of(Proxy)`（t9：SDK 37 上 `Method not found`，
/// 见 F3）；应用进程里又没法自己子类化这个抽象类（native 侧没有 Java 类可加载，也没有
/// dex 可用）。因此**属性路径是唯一权威载体**——平台自己的 `sun.net.spi.DefaultProxySelector`
/// 就是按这几个属性选路的——selector 只是「这个平台的 libcore 恰好有工厂方法」时的加分项，
/// F1/F2 不依赖它。
enum SelectorAttempt {
    /// 装上了（libcore 有 `of()` 工厂）
    Applied,
    /// 该 libcore 没有工厂方法：**预期内**降级，不是失败
    Unsupported,
    /// 其它失败（类加载/反射/setDefault 抛异常）
    Failed(String),
}

/// 本进程是否需要保持模块映射（= 挂了重放）。lib.rs 据此**不**设 `DlCloseModuleLibrary`：
/// 重放线程与 PLT hook 的代码都在这个 .so 里，zygisk 一旦 dlclose，它们执行到的下一条指令
/// 就是野指针。只有显式配了 `[[apps]].egress` 的应用才会置位（设备级继承不会）。
static EGRESS_REPLAY_ACTIVE: AtomicBool = AtomicBool::new(false);

/// selector 降级原因每个进程只解释一次（不要每次注入刷一行）。
static SELECTOR_DEGRADED: Once = Once::new();

/// logcat 优先级（android/log.h）。
const LOG_PRIORITY_INFO: i32 = 4;
const LOG_PRIORITY_WARN: i32 = 5;

/// 见 [EGRESS_REPLAY_ACTIVE]。
pub(crate) fn egress_replay_active() -> bool {
    EGRESS_REPLAY_ACTIVE.load(Ordering::Acquire)
}

/// `pre_app_specialize` 这一次「立即注入」的全部产出，也是重放线程的输入。
struct ImmediateOutcome {
    report: InjectionReport,
    selector: SelectorAttempt,
    sdk_int: Option<i32>,
    vm: Option<JavaVM>,
}

/// 一次注入的逐项结果：成功计数 + 每项失败一行 `warn`（契约 §5.5 的日志格式）。
#[derive(Default)]
struct InjectionReport {
    applied: usize,
    failures: Vec<(&'static str, String)>,
}

impl InjectionReport {
    fn record(&mut self, step: &'static str, result: std::result::Result<(), anyhow::Error>) {
        match result {
            Ok(()) => self.applied += 1,
            Err(err) => self.failures.push((step, err.to_string())),
        }
    }
}

/// 在目标进程内落地应用级出口代理。
///
/// `proxy` 为 `None`（未配置 / 代理定义缺失）时**不做任何事**：既不设置属性，也不清
/// 属性、不装 ProxySelector、不起重放线程——「未配置」的语义是继承设备级行为（契约 §5.2 末），
/// 顺手清一遍反而会破坏「设备级对该应用生效」的预期。
///
/// 返回 `Some(ReplayArming)` 表示「这个进程还需要后置重放」（F1/F2）：调用方（lib.rs）拿
/// 到之后必须调 [arm_egress_replay] 注册过渡完成钩子——**不要**在 specialize 里直接起线程，
/// 见 [arm_egress_replay] 的注释（那会让 zygote 的 setcontext 失败并 abort 进程）。
pub(crate) fn apply_egress_proxy(
    env: &mut EnvUnowned,
    package: &str,
    debug: bool,
    proxy: Option<&EgressProxy>,
) -> Option<ReplayArming> {
    let proxy = proxy?;

    let Some(plan) = crate::egress::injection_plan(proxy) else {
        warn!(
            "[egress] inject failed package={package} step=plan error=missing endpoint type={}",
            proxy.kind.as_str()
        );
        return None;
    };

    if plan.unsupported_auth {
        // 契约 §5.1 第 7 项：HTTP Basic 只能靠全局 `java.net.Authenticator`，影响面过大
        // （同一进程里**所有**连接都会被问到凭据，包括应用自己的流量），本任务明确不装。
        // 因此只注入主机端口，认证交给应用自己；这一行就是 unsupported-auth 标记。
        info!(
            "[egress] unsupported-auth package={package} type=http \
             (host/port injected only, credentials ignored)"
        );
    }

    let outcome = env.with_env(|jenv| -> anyhow::Result<ImmediateOutcome> {
        let report = apply_injection_plan(jenv, &plan);
        let selector = match &plan.selector {
            Some(spec) => install_proxy_selector(jenv, spec),
            None => SelectorAttempt::Applied,
        };
        Ok(ImmediateOutcome {
            report,
            selector,
            sdk_int: android_sdk_int(jenv),
            vm: jenv.get_java_vm().ok(),
        })
    });

    // 顶层 catch-all：with_env 已经 catch_unwind 了闭包里的 panic，这里只负责记录。
    match outcome.into_outcome() {
        Outcome::Ok(immediate) => {
            for (step, error) in &immediate.report.failures {
                warn!("[egress] inject failed package={package} step={step} error={error}");
            }

            let selector_state = match &immediate.selector {
                SelectorAttempt::Applied => "applied",
                SelectorAttempt::Unsupported => {
                    // 不静默：把「为什么没有 selector」写清楚，并说明属性路径才是权威载体。
                    SELECTOR_DEGRADED.call_once(|| {
                        warn!(
                            "[egress] proxy_selector degraded reason=of_method_missing sdk={:?} \
                             (libcore has no ProxySelector.of(Proxy); properties are authoritative \
                             and kept in place by the post-bind replay, so F1/F2 do not depend on it)",
                            immediate.sdk_int
                        );
                    });
                    "degraded"
                }
                SelectorAttempt::Failed(error) => {
                    warn!(
                        "[egress] inject failed package={package} step=proxy_selector error={error}"
                    );
                    "failed"
                }
            };

            info!(
                "[egress] injected package={package} type={} host={} port={} creds={} ok={} failed={} selector={selector_state}",
                proxy.kind.as_str(),
                proxy.host.as_deref().unwrap_or("-"),
                proxy
                    .port
                    .map(|port| port.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                u8::from(proxy.has_credentials()),
                immediate.report.applied,
                immediate.report.failures.len()
            );

            // F1/F2：框架随后会按设备级 ProxyInfo 重写 http/https 属性，必须再写一次。
            // 这里只把「要重放什么」交回去，真正的线程由 lib.rs 在 specialize 末尾用
            // PLT hook 挂上（见 [arm_egress_replay]）。
            let expectations = replay_expectations(&plan);
            if expectations.is_empty() {
                return None;
            }
            match immediate.vm {
                Some(vm) => Some(ReplayArming {
                    vm,
                    package: package.to_string(),
                    expectations,
                    debug,
                }),
                None => {
                    warn!(
                        "[egress] replay skipped package={package} step=java_vm error=unavailable"
                    );
                    None
                }
            }
        }
        Outcome::Err(err) => {
            warn!("[egress] inject failed package={package} step=apply error={err}");
            None
        }
        Outcome::Panic(_) => {
            warn!("[egress] inject failed package={package} step=apply error=panic");
            None
        }
    }
}

/// 按计划逐项执行**属性**部分（selector 由 [install_proxy_selector] 单独处理，这样
/// 「本平台没有 of() 工厂」这种预期内降级不会被算成注入失败）。
/// 任何单项失败都不影响后续项（契约 §5.5 的降级表）。
fn apply_injection_plan(env: &mut Env, plan: &InjectionPlan) -> InjectionReport {
    let mut report = InjectionReport::default();

    for op in &plan.property_ops {
        match op {
            PropertyOp::Set { key, value } => {
                report.record(key, set_system_property(env, key, value));
            }
            PropertyOp::Clear { key } => {
                report.record(key, clear_system_property(env, key));
            }
        }
    }

    // socks5 的凭据载体：这两个属性由 Java 的 socks 实现自己读，不需要全局 Authenticator。
    if let Some((username, password)) = &plan.socks_credentials {
        report.record(
            "java.net.socks.username",
            set_system_property(env, "java.net.socks.username", username),
        );
        report.record(
            "java.net.socks.password",
            set_system_property(env, "java.net.socks.password", password),
        );
    }

    report
}

/// `java.lang.System.setProperty(String, String)`。
fn set_system_property(env: &mut Env, key: &str, value: &str) -> anyhow::Result<()> {
    let class = env.find_class(jni_str!("java/lang/System"))?;
    let key = env.new_string(key)?;
    let value = env.new_string(value)?;
    env.call_static_method(
        &class,
        jni_str!("setProperty"),
        jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
        &[JValue::Object(&key), JValue::Object(&value)],
    )?;
    Ok(())
}

/// `java.lang.System.clearProperty(String)`：清掉父进程可能继承的同名属性。
fn clear_system_property(env: &mut Env, key: &str) -> anyhow::Result<()> {
    let class = env.find_class(jni_str!("java/lang/System"))?;
    let key = env.new_string(key)?;
    env.call_static_method(
        &class,
        jni_str!("clearProperty"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
        &[JValue::Object(&key)],
    )?;
    Ok(())
}

/// 装一个进程级 `java.net.ProxySelector`（契约 §5.1 第 6 项）。
///
/// 先用 `get_static_method_id` **探能力**再调用，以区分两种完全不同的情况：
/// - `MethodNotFound` → [SelectorAttempt::Unsupported]：`ProxySelector.of` 在这个 libcore 里
///   根本不存在（Android 17 / SDK 37 实测，见 t9 F3）。**这是预期内降级**，不是注入失败；
/// - 其它错误 → [SelectorAttempt::Failed]：真失败（类加载 / 反射 / setDefault 抛异常）。
///
/// 为什么不去「自己造一个 ProxySelector」：它是**抽象类**，`java.lang.reflect.Proxy` 只能代理
/// 接口；native 侧没有 Java 类可加载（没有 dex），所以进程内无法子类化。属性路径
/// （`http.proxyHost` 等）本来就是平台 `sun.net.spi.DefaultProxySelector` 的输入，配合
/// [arm_egress_replay] 已足以让 F1/F2 成立——selector 只是「libcore 恰好有工厂方法」时的
/// 加分项，**不是**权威载体。
fn install_proxy_selector(env: &mut Env, spec: &SelectorSpec) -> SelectorAttempt {
    let Ok(selector_class) = env.find_class(jni_str!("java/net/ProxySelector")) else {
        return SelectorAttempt::Failed("find_class java/net/ProxySelector".to_string());
    };

    match env.get_static_method_id(
        &selector_class,
        jni_str!("of"),
        jni_sig!("(Ljava/net/Proxy;)Ljava/net/ProxySelector;"),
    ) {
        Ok(_) => {}
        Err(jni::errors::Error::MethodNotFound { .. }) => return SelectorAttempt::Unsupported,
        Err(err) => return SelectorAttempt::Failed(format!("probe of(): {err}")),
    }

    let proxy = match build_proxy_object(env, spec) {
        Ok(proxy) => proxy,
        Err(err) => return SelectorAttempt::Failed(format!("build proxy: {err:#}")),
    };

    let selector = match env
        .call_static_method(
            &selector_class,
            jni_str!("of"),
            jni_sig!("(Ljava/net/Proxy;)Ljava/net/ProxySelector;"),
            &[JValue::Object(&proxy)],
        )
        .and_then(|value| value.l())
    {
        Ok(selector) => selector,
        Err(err) => return SelectorAttempt::Failed(format!("of(): {err}")),
    };

    match env.call_static_method(
        &selector_class,
        jni_str!("setDefault"),
        jni_sig!("(Ljava/net/ProxySelector;)V"),
        &[JValue::Object(&selector)],
    ) {
        Ok(_) => SelectorAttempt::Applied,
        Err(err) => SelectorAttempt::Failed(format!("setDefault(): {err}")),
    }
}

/// 造 `java.net.Proxy`：`Proxy.NO_PROXY`（直连）或
/// `new Proxy(Proxy.Type.HTTP|SOCKS, InetSocketAddress.createUnresolved(host, port))`。
fn build_proxy_object<'local>(
    env: &mut Env<'local>,
    spec: &SelectorSpec,
) -> anyhow::Result<JObject<'local>> {
    let proxy_class = env.find_class(jni_str!("java/net/Proxy"))?;

    let proxy = match spec {
        SelectorSpec::Direct => env
            .get_static_field(
                &proxy_class,
                jni_str!("NO_PROXY"),
                jni_sig!("Ljava/net/Proxy;"),
            )?
            .l()?,
        SelectorSpec::Proxy { kind, host, port } => {
            let type_class = env.find_class(jni_str!("java/net/Proxy$Type"))?;
            let type_name = match kind {
                EgressType::Socks5 => jni_str!("SOCKS"),
                _ => jni_str!("HTTP"),
            };
            let proxy_type = env
                .get_static_field(&type_class, type_name, jni_sig!("Ljava/net/Proxy$Type;"))?
                .l()?;

            // createUnresolved：不在注入侧解析域名，避免把 DNS 行为带进 zygote 之后的
            // 早期启动阶段，也让 SOCKS 的远端解析语义保持不变。
            let address_class = env.find_class(jni_str!("java/net/InetSocketAddress"))?;
            let host = env.new_string(host)?;
            let address = env
                .call_static_method(
                    &address_class,
                    jni_str!("createUnresolved"),
                    jni_sig!("(Ljava/lang/String;I)Ljava/net/InetSocketAddress;"),
                    &[JValue::Object(&host), JValue::Int(i32::from(*port))],
                )?
                .l()?;

            env.new_object(
                &proxy_class,
                jni_sig!("(Ljava/net/Proxy$Type;Ljava/net/SocketAddress;)V"),
                &[JValue::Object(&proxy_type), JValue::Object(&address)],
            )?
        }
    };

    Ok(proxy)
}

/// `android.os.Build.VERSION.SDK_INT`——只为在 selector 降级那行日志里写清机型代次。
fn android_sdk_int(env: &mut Env) -> Option<i32> {
    let class = env.find_class(jni_str!("android/os/Build$VERSION")).ok()?;
    match env
        .get_static_field(&class, jni_str!("SDK_INT"), jni_sig!("I"))
        .ok()?
    {
        JValueOwned::Int(sdk_int) => Some(sdk_int),
        _ => None,
    }
}

// ── F1/F2：框架覆盖之后的重放 ───────────────────────────────────────────────
//
// 真机时间序（t9 §5.3）：注入 21:22:58.454（type=direct ok=6）→ 21:22:58.549 进程内读到
// http.proxyHost=127.0.0.1/18080（设备级值）。中间 95ms 只有框架会写这些属性。写一次
// 必被吃掉，所以注入之后按 crate::egress::replay_offsets_ms() 的节奏**读回并重放**。

/// 一次重放任务的全部输入：由 [apply_egress_proxy] 产出，由 [arm_egress_replay] 装填，
/// 最后由 [setpriority_hook] 里 spawn。
pub(crate) struct ReplayArming {
    vm: JavaVM,
    package: String,
    expectations: Vec<ReplayExpectation>,
    debug: bool,
}

/// 待启动的重放任务（`setpriority` hook 里取走）。
static REPLAY_PENDING: Mutex<Option<ReplayArming>> = Mutex::new(None);
/// 本进程已经 spawn 过重放线程（hook 会命中多次，只做一次）。
static REPLAY_SPAWNED: AtomicBool = AtomicBool::new(false);
/// zygisk PLT hook 返回的原始 `setpriority`。
static REAL_SETPRIORITY: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
/// 备选触发点 __system_property_get 的原始函数。
static REAL_SYSTEM_PROPERTY_GET: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
/// 钩子第一次命中时打一行日志（之后走快路径，不再碰 logcat）。
static HOOK_FIRED_LOGGED: AtomicBool = AtomicBool::new(false);
/// 本次重放是否允许打 logcat（= 配置里的 debug；钩子在 extern "C" 边界里拿不到它）。
static REPLAY_DEBUG: AtomicBool = AtomicBool::new(false);

/// 注册「进程过渡完成」钩子并登记重放任务。
///
/// **为什么不在 specialize 里起线程**：真机实测（Pixel 9 / Android 17）在
/// `pre_app_specialize` 里创建线程会让 zygote 的 `selinux_android_setcontext` 失败
/// （该调用要求进程单线程），进程直接 SIGABRT，tombstone 原文：
/// `Abort message: 'JNI FatalError called: ... selinux_android_setcontext(...) failed'`。
/// 因此改成「等框架把过渡做完，再由 PLT hook 里创建线程」。
///
/// 钩子选点：AOSP `com_android_internal_os_Zygote.cpp` 的 `SpecializeCommon` 在
/// `setresuid` → `selinux_android_setcontext` → `ZygoteHooks.postForkChild`
/// （`Runtime::SetProcessState`，进程从此不再是 zygote）之后，**必然**调用一次
/// `setpriority(PRIO_PROCESS, 0, PROCESS_PRIORITY_DEFAULT)`。这是最早能安全创建线程的
/// 时刻，也是本模块唯一稳定拿到的「过渡完成」回调（android14/15/16/main 四个分支
/// 的调用顺序一致，均在本文件的 `SpecializeCommon` 里）。
pub(crate) fn arm_egress_replay(api: &mut ZygiskApi<V4>, arming: ReplayArming) {
    let debug = arming.debug;
    let package = arming.package.clone();

    // ① 决策：debug 护栏 + 排障开关 + 镜像定位。判定与日志文案都在 crate::egress
    //    (plan_replay_arm / ReplayArmFailure::log_line)，两条真实降级分支有宿主单测直接驱动。
    //
    //    排障开关（**仅排障 / QA 用**）：真机上「hook unavailable」的两条触发条件
    //    （镜像缺失 / PLT commit 失败）都不可制造，所以留一个显式开关，让链路**完全**退到
    //    「没挂上 hook」的状态——不登记任务、不置位、不保留模块代码页，只记一行同族日志。
    //       adb shell setprop debug.device_faker.egress.disable_replay 1    # 重启应用生效
    //       adb shell setprop debug.device_faker.egress.disable_replay ""   # 恢复（空值 = 关闭）
    //    护栏：必须同时 config.toml 里 debug = true 才生效（debug 关闭时不改变行为）；
    //    值只认 1/true/yes/on（大小写不敏感），空值与其它值一律视为关闭；需要 root/shell 写；
    //    测试完务必 setprop "" 并**重启应用**；属性名会残留在进程属性表里、可被枚举。
    let (device, inode) = match crate::egress::plan_replay_arm(
        debug,
        replay_disabled_by_debug_prop,
        android_runtime_image,
    ) {
        crate::egress::ArmPlan::Proceed { device, inode } => (device, inode),
        crate::egress::ArmPlan::DisabledByDebugProp => {
            // 走进这里时 debug 必然为真，logcat 通道直接点亮，便于 QA 在 logcat 里 grep。
            replay_unavailable(crate::egress::ArmOutcome::DisabledByDebugProp);
            emit_logcat(
                LOG_PRIORITY_WARN,
                &format!(
                    "[egress] replay hook unavailable package={package} reason=debug_disable_replay"
                ),
            );
            return;
        }
        crate::egress::ArmPlan::Unavailable(failure) => {
            replay_unavailable(crate::egress::ArmOutcome::Unavailable(failure));
            return;
        }
    };

    // ② 先占位再提交：hook 一旦 commit 就可能立刻命中，待启动任务必须已经在等。
    EGRESS_REPLAY_ACTIVE.store(true, Ordering::Release);
    REPLAY_DEBUG.store(debug, Ordering::Release);
    {
        let Ok(mut slot) = REPLAY_PENDING.lock() else {
            replay_unavailable(crate::egress::ArmOutcome::Unavailable(
                crate::egress::ReplayArmFailure::PltHookCommitFailed(
                    "pending_lock_poisoned".to_string(),
                ),
            ));
            return;
        };
        *slot = Some(arming);
    }

    // ③ 注册两个触发点并提交。
    let mut setpriority_original: *const () = std::ptr::null();
    let mut property_get_original: *const () = std::ptr::null();
    unsafe {
        api.plt_hook_register(
            device as libc::dev_t,
            inode as libc::ino_t,
            c"setpriority",
            setpriority_hook as *const (),
            &mut setpriority_original,
        );
        // 备选触发点：__system_property_get 在 postForkChild（SetProcessState 之后）里被
        // ART 读 dalvik.vm.* 时调用，比 setpriority 更早命中；它很热，所以钩子里只做一次
        // 原子读，之后完全透明转发。
        api.plt_hook_register(
            device as libc::dev_t,
            inode as libc::ino_t,
            c"__system_property_get",
            system_property_get_hook as *const (),
            &mut property_get_original,
        );
    }
    REAL_SETPRIORITY.store(
        setpriority_original.cast_mut().cast::<c_void>(),
        Ordering::Release,
    );
    REAL_SYSTEM_PROPERTY_GET.store(
        property_get_original.cast_mut().cast::<c_void>(),
        Ordering::Release,
    );

    // ④ 提交结果的映射与日志同样走纯逻辑（finish_replay_arm / ArmOutcome），保证被单测
    //    覆盖的就是线上这条路径。
    let commit = api.plt_hook_commit().map_err(|err| err.to_string());
    match crate::egress::finish_replay_arm(commit) {
        crate::egress::ArmOutcome::Armed => {
            // 只有真的 armed 才登记「hook 没命中」诊断：commit 失败时模块会被 dlclose，
            // 那时再挂信号处理器/atexit 回调就是在已卸载的代码上留指针（进程退出即崩）。
            install_never_fired_probe(&package);
            if debug {
                info!(
                    "[egress] replay hook armed image=libandroid_runtime device={device} inode={inode} symbols=setpriority,__system_property_get"
                );
                emit_logcat(
                    LOG_PRIORITY_INFO,
                    &format!("[egress] replay hook armed package={package}"),
                );
            }
        }
        crate::egress::ArmOutcome::Unavailable(failure) => {
            REAL_SETPRIORITY.store(std::ptr::null_mut(), Ordering::Release);
            REAL_SYSTEM_PROPERTY_GET.store(std::ptr::null_mut(), Ordering::Release);
            replay_unavailable(crate::egress::ArmOutcome::Unavailable(failure));
        }
        crate::egress::ArmOutcome::DisabledByDebugProp => {}
    }
}

// ── M-5：armed 但 hook 一直没命中的诊断 ─────────────────────────────────────
//
// armed 之后 lib.rs 故意不 dlclose（hook / 重放线程的代码都在这个 .so 里），所以一旦触发
// hook 始终没命中，进程会白留一张代码页、而且什么日志都没有。诊断必须落在**晚于
// setpriority** 的点上，而 post_app_specialize 早于它（在那里判定会误杀正常武装），因此
// 这里用两个都不依赖 hook 的观测点：
//   ① 定时器（alarm，不创建线程）：arm 成功后等一个重放窗口，到点仍未 spawn → 打一行
//      `replay hook not fired within N ms; module retained ...`，并**立刻**把 SIGALRM 恢复
//      成默认处置。正常路径 hook ~26ms 就命中，会提前 alarm(0) 撤销——也就是说这个定时器
//      在正常情况下只活几十毫秒，进程几乎看不到它长期存在；
//   ② 进程退出兜底：进程在窗口内就退出时补一行（正常路径 spawned=true → 静默）。
// SIGKILL（AMS 杀进程的主要方式）下 ①② 都不会执行——那种情况只能靠 logcat 里没有
// `replay start` 判断；别再假设「永远有兜底」。
//
// 为什么不另起线程：specialize 里创建线程会让 zygote 的 selinux_android_setcontext 失败
// 并 SIGABRT（见 arm_egress_replay 的时序说明）。信号处理器里只做原子读 + 预先缓存的
// `__android_log_write` 指针，不分配、不加锁。

/// 一次性的「hook 没命中」诊断任务（arm 成功时登记）。
struct NeverFiredProbe {
    package: String,
    window_ms: u64,
    armed_at: Instant,
}

static NEVER_FIRED: Mutex<Option<NeverFiredProbe>> = Mutex::new(None);
/// 诊断只打一次（定时器与退出兜底共用）。
static NEVER_FIRED_REPORTED: AtomicBool = AtomicBool::new(false);
/// 装定时器前的 SIGALRM 处置。**不能**用 null 当哨兵：`SIG_DFL` 本身就是 0。
static SIGALRM_PREV: AtomicUsize = AtomicUsize::new(0);
/// 我们自己是否装着 SIGALRM 处理器（决定撤销时要不要还原）。
static SIGALRM_INSTALLED: AtomicBool = AtomicBool::new(false);
/// 信号上下文里用的预构建 C 字符串（进程级生命周期，不释放）。
static SIGNAL_TAG: AtomicPtr<std::ffi::c_char> = AtomicPtr::new(std::ptr::null_mut());
static SIGNAL_MSG: AtomicPtr<std::ffi::c_char> = AtomicPtr::new(std::ptr::null_mut());
/// 预先解析好的 `__android_log_write`（信号上下文里不能再 dlsym）。
static ANDROID_LOG_WRITE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// arm 成功后登记诊断：定时器 + 退出兜底。
fn install_never_fired_probe(package: &str) {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    // commit 与这里之间 hook 可能已经命中（它就在应用主线程上跑）：那就没必要再装诊断，
    // 否则一步慢就会留下一个永远不会撤销的 SIGALRM 处理器。
    if REPLAY_SPAWNED.load(Ordering::Acquire) {
        return;
    }

    let window_ms = crate::egress::REPLAY_WINDOW_MS;
    if let Ok(mut slot) = NEVER_FIRED.lock() {
        *slot = Some(NeverFiredProbe {
            package: package.to_string(),
            window_ms,
            armed_at: Instant::now(),
        });
    }

    // 信号上下文不能分配：tag 与整行消息在这里预构建成 C 字符串（进程级生命周期）。
    let line = crate::egress::replay_not_fired_line(window_ms, window_ms, package);
    if let (Ok(tag), Ok(message)) = (
        std::ffi::CString::new("DeviceFaker"),
        std::ffi::CString::new(line.as_str()),
    ) {
        SIGNAL_TAG.store(tag.into_raw(), Ordering::Release);
        SIGNAL_MSG.store(message.into_raw(), Ordering::Release);
    }
    ANDROID_LOG_WRITE.store(android_log_write_symbol(), Ordering::Release);

    // 退出兜底：登记一个 extern "C" 回调（代码在 .so 里，armed 期间不会 dlclose）。
    // SAFETY: 只是登记回调，没有前置条件。
    unsafe { libc::atexit(replay_never_fired_at_exit) };

    // 定时器：alarm 粒度是秒，向上取整并至少 1 秒。
    let seconds = window_ms.div_ceil(1000).max(1) as libc::c_uint;
    // SAFETY: 安装我们自己的处理器，旧处置留给撤销时还原。
    let previous = unsafe {
        libc::signal(
            libc::SIGALRM,
            never_fired_alarm_handler as *const () as libc::sighandler_t,
        )
    };
    SIGALRM_PREV.store(previous as usize, Ordering::Release);
    SIGALRM_INSTALLED.store(true, Ordering::Release);
    // SAFETY: alarm 只设置内核定时器，不创建线程。
    unsafe { libc::alarm(seconds) };
    // 注意：这里**不打日志**——正常路径（hook 命中）的日志必须与改动前完全一致；
    // 诊断只在「到点仍未 spawn」时出现（另见 replay_never_fired_at_exit 的退出兜底）。
}

/// hook 命中（或收尾）时撤销诊断：撤掉定时器、还原 SIGALRM 处置，并压住退出兜底。
fn cancel_never_fired_probe() {
    NEVER_FIRED_REPORTED.store(true, Ordering::Release);
    // SAFETY: alarm(0) 取消定时器；signal() 还原先前处置。
    unsafe {
        libc::alarm(0);
        if SIGALRM_INSTALLED.swap(false, Ordering::AcqRel) {
            libc::signal(
                libc::SIGALRM,
                SIGALRM_PREV.load(Ordering::Acquire) as libc::sighandler_t,
            );
        }
    }
    if let Ok(mut slot) = NEVER_FIRED.lock() {
        *slot = None;
    }
}

/// 定时器到点：hook 还没命中就写一行诊断（**信号上下文**：只做原子读 + 一次预缓存调用）。
extern "C" fn never_fired_alarm_handler(_signal: c_int) {
    if !REPLAY_SPAWNED.load(Ordering::Acquire) && !NEVER_FIRED_REPORTED.swap(true, Ordering::AcqRel)
    {
        let writer = ANDROID_LOG_WRITE.load(Ordering::Acquire);
        let tag = SIGNAL_TAG.load(Ordering::Acquire);
        let message = SIGNAL_MSG.load(Ordering::Acquire);
        if !writer.is_null() && !tag.is_null() && !message.is_null() {
            // SAFETY: writer 是 android_log_write_symbol() 解析出的 __android_log_write；
            // tag/message 是 install_never_fired_probe 里 into_raw 的 CString，永不释放。
            let writer: AndroidLogWrite = unsafe { std::mem::transmute(writer) };
            unsafe { writer(LOG_PRIORITY_WARN, tag, message) };
        }
    }
    // 只该响一次：立刻还原先前的处置（通常是 SIG_DFL），不把 SIGALRM 长期占住。
    // SAFETY: signal() 是异步信号安全的。
    if SIGALRM_INSTALLED.swap(false, Ordering::AcqRel) {
        unsafe {
            libc::signal(
                libc::SIGALRM,
                SIGALRM_PREV.load(Ordering::Acquire) as libc::sighandler_t,
            )
        };
    }
}

/// 进程退出兜底：进程在窗口内就退出时补一行（正常路径 spawned=true → 静默）。
extern "C" fn replay_never_fired_at_exit() {
    let spawned = REPLAY_SPAWNED.load(Ordering::Acquire);
    let active = EGRESS_REPLAY_ACTIVE.load(Ordering::Acquire);
    let reported = NEVER_FIRED_REPORTED.load(Ordering::Acquire);
    if !crate::egress::should_report_never_fired(active, spawned, reported) {
        return;
    }
    // 退出阶段别的线程可能已经没了：拿不到锁就放弃（定时器那条路已经覆盖）。
    let probe = match NEVER_FIRED.try_lock() {
        Ok(mut slot) => slot.take(),
        Err(_) => None,
    };
    let Some(probe) = probe else {
        return;
    };
    if NEVER_FIRED_REPORTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let waited_ms = probe.armed_at.elapsed().as_millis() as u64;
    let line = crate::egress::replay_not_fired_line(probe.window_ms, waited_ms, &probe.package);
    warn!("{line}");
    emit_logcat(LOG_PRIORITY_WARN, &line);
}
/// 挂不上 hook 时统一收尾：丢掉待启动任务、撤销「保留模块映射」的置位，并记一行日志。
///
/// 撤销置位很关键：hook 没挂上就不会有重放线程，模块就没必要常驻（lib.rs 看到
/// `egress_replay_active() == false` 会照旧 `dlclose`），否则等于白白多留一张代码页。
fn replay_unavailable(outcome: crate::egress::ArmOutcome) {
    if let Ok(mut slot) = REPLAY_PENDING.lock() {
        *slot = None;
    }
    // 结论自证：只有 Armed 才该保留模块映射，这里由 outcome 反推，避免手写 false 漂移。
    EGRESS_REPLAY_ACTIVE.store(outcome.keeps_module_mapped(), Ordering::Release);
    if let Some(line) = outcome.log_line() {
        warn!("{line}");
    }
}

/// 读 Android 系统属性 `debug.device_faker.egress.disable_replay`：`1/true/yes/on` 视为开启。
///
/// 用 Android 属性（`setprop`）而不是 Java 系统属性：arm 发生在 specialize 早期，读 Java
/// 属性要走 JNI；这里只需要一个进程级开关。`setprop` 之后**重启应用**才生效（每个进程
/// 在 specialize 时读一次）。
#[cfg(target_os = "android")]
fn replay_disabled_by_debug_prop() -> bool {
    const NAME: &std::ffi::CStr = c"debug.device_faker.egress.disable_replay";
    // PROP_VALUE_MAX（92 字节）：`__system_property_get` 要求缓冲区至少这么大。
    let mut buffer = [0 as libc::c_char; 92];
    // SAFETY: 缓冲区长度符合要求，成功时写入以 NUL 结尾。
    let len = unsafe { libc::__system_property_get(NAME.as_ptr(), buffer.as_mut_ptr()) };
    if len <= 0 {
        return false;
    }
    // SAFETY: 成功返回时缓冲区是以 NUL 结尾的字符串。
    let value = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
    matches!(
        value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(not(target_os = "android"))]
fn replay_disabled_by_debug_prop() -> bool {
    false
}

/// `libandroid_runtime.so` 的 (dev, inode)：zygisk 的 PLT hook 用 dev/ino 定位镜像。
fn android_runtime_image() -> Option<(u64, u64)> {
    for path in [
        c"/system/lib64/libandroid_runtime.so",
        c"/system/lib/libandroid_runtime.so",
    ] {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::stat(path.as_ptr(), &mut stat) } == 0 {
            // 统一成 u64 交给 crate::egress 的纯逻辑（hooks 侧再转回 libc 类型）。
            return Some((stat.st_dev as u64, stat.st_ino as u64));
        }
    }
    None
}

/// `setpriority` 的 PLT hook：进程过渡完成后第一次被调用时，把重放线程拉起来。
///
/// 这是 `extern "C"` 边界：内部不得 panic（本 crate 是 `panic = "abort"`），也不得阻塞——
/// 只做一次原子判断 + 取走任务 + spawn，然后把调用原样转发给 zygisk 给的原始函数。
unsafe extern "C" fn setpriority_hook(which: c_int, who: libc::id_t, prio: c_int) -> c_int {
    note_hook_fired("setpriority");
    start_pending_replay();

    let original = REAL_SETPRIORITY.load(Ordering::Acquire);
    if original.is_null() {
        // 理论上不可达（arm 时先写 original 再 commit）；退化为直接调 libc。
        return unsafe { libc::setpriority(which, who, prio) };
    }
    let original: unsafe extern "C" fn(c_int, libc::id_t, c_int) -> c_int =
        unsafe { std::mem::transmute(original) };
    unsafe { original(which, who, prio) }
}

/// 备选触发点：__system_property_get（签名与 bionic 一致）。
///
/// 只在「本进程还没 spawn 重放线程」时做一次原子读，之后完全透明转发 —— 它在启动期会被
/// 调用上千次，钩子里不能有任何重活。
unsafe extern "C" fn system_property_get_hook(
    name: *const std::ffi::c_char,
    value: *mut std::ffi::c_char,
) -> c_int {
    note_hook_fired("__system_property_get");
    start_pending_replay();

    let original = REAL_SYSTEM_PROPERTY_GET.load(Ordering::Acquire);
    if original.is_null() {
        return unsafe { libc::__system_property_get(name, value) };
    }
    let original: unsafe extern "C" fn(*const std::ffi::c_char, *mut std::ffi::c_char) -> c_int =
        unsafe { std::mem::transmute(original) };
    unsafe { original(name, value) }
}

/// 钩子第一次命中时打一行（证明 hook 真的生效了，真机排障第一现场）。
fn note_hook_fired(symbol: &str) {
    if !REPLAY_DEBUG.load(Ordering::Relaxed) {
        return;
    }
    if HOOK_FIRED_LOGGED.swap(true, Ordering::AcqRel) {
        return;
    }
    emit_logcat(
        LOG_PRIORITY_INFO,
        &format!("[egress] replay hook fired symbol={symbol}"),
    );
}

/// 取走待启动任务并 spawn 重放线程（一个进程只做一次）。
fn start_pending_replay() {
    if REPLAY_SPAWNED.load(Ordering::Acquire) {
        return;
    }
    let arming = match REPLAY_PENDING.lock() {
        Ok(mut slot) => slot.take(),
        Err(_) => None,
    };
    let Some(arming) = arming else {
        return;
    };
    REPLAY_SPAWNED.store(true, Ordering::Release);
    // 触发点已命中：撤掉「hook 没命中」的定时器并还原 SIGALRM（正常路径下它只活了几十毫秒）。
    cancel_never_fired_probe();

    let ReplayArming {
        vm,
        package,
        expectations,
        debug,
    } = arming;

    let thread_package = package.clone();
    match std::thread::Builder::new()
        .name("df-egress-replay".to_string())
        .spawn(move || run_egress_replay(vm, &thread_package, &expectations, debug))
    {
        Ok(_) => {
            if debug {
                emit_logcat(
                    LOG_PRIORITY_INFO,
                    &format!(
                        "[egress] replay start package={package} trigger=post_zygote_setpriority"
                    ),
                );
            }
        }
        Err(err) => {
            if debug {
                emit_logcat(
                    LOG_PRIORITY_WARN,
                    &format!("[egress] replay failed package={package} step=spawn error={err}"),
                );
            }
        }
    }
}

/// 重放线程主体：先带退避 attach 到 JVM，再跑完 [run_replay_loop]，把时间序写进 logcat。
fn run_egress_replay(vm: JavaVM, package: &str, expectations: &[ReplayExpectation], debug: bool) {
    let started = Instant::now();

    // zygisk 的 `pre_app_specialize` 期间 ART 仍处在 zygote 的 specialization 窗口里，这时从
    // 侧线程 AttachCurrentThread 会**直接失败**（真机实测：Goole Pixel 9 / Android 17 上
    // `error=JNI call failed`，replay 一次都没跑）。所以先等 specialize 返回，再带退避重试；
    // 预算约 100ms，仍失败就放弃并记一行——应用照常运行，属性注入仍在，只是不再重放。
    let mut attach_attempts = 0u32;
    let attached = loop {
        if attach_attempts >= 10 {
            break false;
        }
        attach_attempts += 1;
        std::thread::sleep(Duration::from_millis(if attach_attempts == 1 {
            8
        } else {
            10
        }));
        match vm.attach_current_thread(|_env| Ok::<(), anyhow::Error>(())) {
            Ok(()) => break true,
            Err(err) => {
                // 过渡刚结束时偶发失败（ART 还在收尾），重试即可；只记第一次，避免刷屏。
                if debug && attach_attempts == 1 {
                    emit_logcat(
                        LOG_PRIORITY_INFO,
                        &format!("[egress] replay attach retry package={package} error={err:?}"),
                    );
                }
            }
        }
    };
    if !attached {
        if debug {
            emit_logcat(
                LOG_PRIORITY_WARN,
                &format!(
                    "[egress] replay failed package={package} step=attach error=retries_exhausted"
                ),
            );
        }
        return;
    }

    let outcome =
        vm.attach_current_thread(|env| -> anyhow::Result<(Vec<ReplayTick>, ReplaySummary)> {
            let mut sleep_until = |offset: u64| {
                let target = started + Duration::from_millis(offset);
                if let Some(remaining) = target.checked_duration_since(Instant::now()) {
                    std::thread::sleep(remaining);
                }
            };

            let mut ctx: &mut Env = env;
            Ok(run_replay_loop(
                &mut ctx,
                expectations,
                // 每次读/写各开一个本地引用帧：整轮有上百次 JNI 调用，局部引用不能在一个帧里
                // 线性累积（ART 的局部引用表会一路涨到帧结束）。
                |ctx: &mut &mut Env, key: &str| {
                    ctx.with_local_frame(8, |env: &mut Env| -> anyhow::Result<Option<String>> {
                        Ok(read_system_property(env, key))
                    })
                    .ok()
                    .flatten()
                },
                |ctx: &mut &mut Env, expectation: &ReplayExpectation| {
                    ctx.with_local_frame(8, |env: &mut Env| -> anyhow::Result<bool> {
                        Ok(match &expectation.want {
                            Some(value) => set_system_property(env, expectation.key, value).is_ok(),
                            None => clear_system_property(env, expectation.key).is_ok(),
                        })
                    })
                    .unwrap_or(false)
                },
                &mut sleep_until,
            ))
        });

    match outcome {
        Ok((ticks, summary)) => {
            if !debug {
                return;
            }
            for tick in &ticks {
                if let ReplayTick::Diverged {
                    at_ms,
                    found,
                    applied,
                } = tick
                {
                    let changed: Vec<String> = found.iter().map(Divergence::describe).collect();
                    emit_logcat(
                        LOG_PRIORITY_WARN,
                        &format!(
                            "[egress] replay package={package} t={at_ms}ms \
                             reason=framework_overwrite applied={applied}/{} changed=[{}]",
                            expectations.len(),
                            changed.join(", ")
                        ),
                    );
                }
            }
            emit_logcat(
                if summary.state_matches {
                    LOG_PRIORITY_INFO
                } else {
                    LOG_PRIORITY_WARN
                },
                &format!(
                    "[egress] replay done package={package} probes={} replays={} divergences={} \
                     state={} elapsed={}ms",
                    summary.probes,
                    summary.replays,
                    summary.divergences,
                    if summary.state_matches {
                        "match"
                    } else {
                        "mismatch"
                    },
                    started.elapsed().as_millis()
                ),
            );
        }
        Err(err) => {
            if debug {
                emit_logcat(
                    LOG_PRIORITY_WARN,
                    &format!("[egress] replay failed package={package} step=run error={err:?}"),
                );
            }
        }
    }
}

/// 读回一个 Java 系统属性；`None` = 不存在 / null。
fn read_system_property(env: &mut Env, key: &str) -> Option<String> {
    let class = env.find_class(jni_str!("java/lang/System")).ok()?;
    let key = env.new_string(key).ok()?;
    let value = env
        .call_static_method(
            &class,
            jni_str!("getProperty"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
            &[JValue::Object(&key)],
        )
        .ok()?;
    let object = value.l().ok()?;
    if object.is_null() {
        return None;
    }
    let text = env.cast_local::<JString>(object).ok()?;
    Some(text.mutf8_chars(env).ok()?.into())
}

/// `__android_log_write` 的函数签名（与 android/log.h 一致）。
#[cfg(target_os = "android")]
type AndroidLogWrite =
    unsafe extern "C" fn(i32, *const std::ffi::c_char, *const std::ffi::c_char) -> i32;

/// 解析 `__android_log_write` 的地址（只查一次）。
///
/// 单独抽出来是因为**信号上下文里不能重新 dlsym**：装定时器时先解析一次存进
/// [ANDROID_LOG_WRITE]，处理器里直接复用。`__android_log_write` 是 NDK 公开 API，
/// liblog 在每个 Android 进程里都由 libart 加载，因此 RTLD_DEFAULT 能找到。
/// 裸指针进 `OnceLock` 的包装：符号地址在进程生命周期内不变，跨线程只读。
#[cfg(target_os = "android")]
struct SymbolAddr(*mut c_void);

// SAFETY: `dlsym` 返回的符号地址在进程生命周期内是常量，所有线程只读取它。
#[cfg(target_os = "android")]
unsafe impl Send for SymbolAddr {}
// SAFETY: 同上，没有可变状态。
#[cfg(target_os = "android")]
unsafe impl Sync for SymbolAddr {}

#[cfg(target_os = "android")]
fn android_log_write_symbol() -> *mut c_void {
    use std::sync::OnceLock;

    static SYMBOL: OnceLock<SymbolAddr> = OnceLock::new();
    SYMBOL
        .get_or_init(|| {
            // SAFETY: 纯符号查找，没有前置条件。
            SymbolAddr(unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"__android_log_write".as_ptr()) })
        })
        .0
}

/// 往 logcat 写一行。
///
/// 重放发生在 `pre_app_specialize` **返回之后**，那时模块的日志缓冲已经 flush 给 companion
/// 了——之后 `log::` 宏写出的行不会再有人取走。所以重放的时间序证据直接走 logcat
/// （`adb logcat -s DeviceFaker`），可与探针应用的 logcat 读数对齐时间。
#[cfg(target_os = "android")]
fn emit_logcat(priority: i32, message: &str) {
    use std::ffi::CString;

    let symbol = android_log_write_symbol();
    if symbol.is_null() {
        return;
    }
    // SAFETY: 符号已按 android/log.h 的 __android_log_write(int, const char*, const char*) 解析。
    let writer: AndroidLogWrite = unsafe { std::mem::transmute(symbol) };
    let Ok(tag) = CString::new("DeviceFaker") else {
        return;
    };
    let Ok(text) = CString::new(message.replace('\0', " ")) else {
        return;
    };
    // SAFETY: 两个 CString 在调用期间存活且以 NUL 结尾。
    unsafe { writer(priority, tag.as_ptr(), text.as_ptr()) };
}

#[cfg(not(target_os = "android"))]
fn emit_logcat(_priority: i32, _message: &str) {}
