// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 配置校验（docs/dev/server-api-plan.md 9.4）。
//!
//! 规则刻意保持「解析层不管、写入侧把关」的分工：[`crate::config`] 的解析与合并语义
//! 一行不改（例如 dpi 超出 120-640 时 merge 仍然静默忽略），校验只把这类问题**显式报出来**，
//! 让 WebUI / AI 在写之前就知道会丢字段。
//!
//! 严重级别：
//!
//! - `error`：写入应当被拒绝（包名格式、dpi 越界、sdk_int 越界、custom_props 键不在白名单、
//!   egress 代理名不合法）。egress 的 error 码直接用契约 §2.6 的 ValidationError 码
//!   `invalid_egress_name` / `invalid_egress_default`，server 侧可原样透传成 422；
//! - `warning`：可以写入但要提示（重复包名冲突、值超过 Android 属性上限、未知字段）。

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::config::{
    AppConfig, Config, DELETE_MARKER, DIRECT_MARKER, DPI_MAX, DPI_MIN, DeviceTemplate,
    EGRESS_PROXY_NAME_MAX, EGRESS_PROXY_NAME_PATTERN, EMPTY_MARKER, EgressSection, NumberSetting,
    SDK_INT_MAX, SDK_INT_MIN, is_egress_name_shaped,
};

/// custom_props 允许的属性名前缀白名单。
pub const PROP_KEY_PREFIXES: [&str; 2] = ["ro.", "persist."];
/// Android 属性值的硬上限（PROP_VALUE_MAX，92 字节）；超出会被截断。
pub const PROP_VALUE_MAX: usize = 92;
/// 本项目的防御性上限：超过即视为配置错误。
pub const PROP_VALUE_HARD_MAX: usize = 4096;
/// 属性名长度上限（Android 本身是 32，但 `ro.product.system_ext.marketname` 已超 32，故放宽）。
pub const PROP_KEY_MAX: usize = 128;
/// 包名（含 `pkg@userId`）总长度上限。
pub const PACKAGE_NAME_MAX: usize = 255;
/// 模板名长度上限。
pub const TEMPLATE_NAME_MAX: usize = 128;

/// 校验严重级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// 写入应被拒绝。
    Error,
    /// 可写入，但需要提示。
    Warning,
}

/// 一条校验问题。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// 严重级别。
    pub severity: Severity,
    /// 稳定的机器可读错误码（如 `dpi.out_of_range`）。
    pub code: &'static str,
    /// 出错位置（如 `templates.pixel_9.dpi`）。
    pub path: String,
    /// 人类可读说明。
    pub message: String,
}

/// 校验结果集合。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ValidationReport {
    /// 全部问题，按发现顺序。
    pub issues: Vec<Issue>,
}

impl ValidationReport {
    /// 空报告。
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一条问题。
    pub fn push(
        &mut self,
        severity: Severity,
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.issues.push(Issue {
            severity,
            code,
            path: path.into(),
            message: message.into(),
        });
    }

    /// 追加一条 error。
    pub fn error(
        &mut self,
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.push(Severity::Error, code, path, message);
    }

    /// 追加一条 warning。
    pub fn warning(
        &mut self,
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.push(Severity::Warning, code, path, message);
    }

    /// 合并另一份报告。
    pub fn merge(&mut self, other: ValidationReport) {
        self.issues.extend(other.issues);
    }

    /// 没有 error（warning 不影响通过）。
    pub fn is_ok(&self) -> bool {
        !self
            .issues
            .iter()
            .any(|issue| issue.severity == Severity::Error)
    }

    /// 全部 error。
    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error)
    }

    /// 全部 warning。
    pub fn warnings(&self) -> impl Iterator<Item = &Issue> {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Warning)
    }

    /// error 数量。
    pub fn error_count(&self) -> usize {
        self.errors().count()
    }

    /// warning 数量。
    pub fn warning_count(&self) -> usize {
        self.warnings().count()
    }
}

/// 同一条包名被多个模板声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuplicatePackage {
    /// 冲突的包名。
    pub package: String,
    /// 声明了该包名的模板（书写顺序）。
    pub templates: Vec<String>,
}

/// 校验可调参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationOptions {
    /// 同一包名被多个模板声明时的级别（默认 warning；严格模式可设为 error）。
    pub duplicate_package_severity: Severity,
}

impl Default for ValidationOptions {
    fn default() -> Self {
        Self {
            duplicate_package_severity: Severity::Warning,
        }
    }
}

/// 校验模板名：非空、长度受限、不含控制字符。
pub fn validate_template_name(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        return Some("模板名不能为空".to_owned());
    }
    if name.len() > TEMPLATE_NAME_MAX {
        return Some(format!("模板名超过 {TEMPLATE_NAME_MAX} 字节"));
    }
    if name.chars().any(char::is_control) {
        return Some("模板名不能包含换行或控制字符".to_owned());
    }
    None
}

/// 校验包名（支持 `pkg@userId`）；返回错误说明。
pub fn validate_package_key(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some("包名不能为空".to_owned());
    }
    if value.len() > PACKAGE_NAME_MAX {
        return Some(format!("包名超过 {PACKAGE_NAME_MAX} 字节"));
    }

    let (package, user) = match value.rsplit_once('@') {
        Some((package, user)) => (package, Some(user)),
        None => (value, None),
    };

    if let Some(user) = user
        && (user.is_empty() || user.len() > 3 || !user.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Some(format!("@{user} 不是合法 userId（0-999）"));
    }

    if package.is_empty() {
        return Some("包名不能为空".to_owned());
    }

    let segments: Vec<&str> = package.split('.').collect();
    if segments.len() < 2 {
        return Some("包名至少要有两段（如 com.example）".to_owned());
    }
    for segment in segments {
        if !is_java_identifier(segment) {
            return Some(format!("包名段 {segment:?} 不是合法 Java 标识符"));
        }
    }
    None
}

/// 包名是否合法（含 `pkg@userId` 形式）。
pub fn is_valid_package_key(value: &str) -> bool {
    validate_package_key(value).is_none()
}

/// 校验代理名（契约 §2.5：字符集/长度 + 保留值）；返回错误说明。
///
/// 只用于 `egress.toml` 的代理名与 `[egress].default`。`[[apps]].egress` 额外允许
/// `__DIRECT__`，见 [`validate_app`]。
pub fn validate_egress_proxy_name(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some("代理名不能为空".to_owned());
    }
    if value.len() > EGRESS_PROXY_NAME_MAX {
        return Some(format!(
            "代理名 {value:?} 超过 {EGRESS_PROXY_NAME_MAX} 字符上限"
        ));
    }
    if value == DIRECT_MARKER {
        return Some(format!(
            "{DIRECT_MARKER} 是保留值（应用级强制直连），不能作为代理名"
        ));
    }
    if !is_egress_name_shaped(value) {
        return Some(format!(
            "代理名 {value:?} 不合法：必须以字母/数字/下划线开头，其后只允许字母、数字、下划线、点、减号（正则 {EGRESS_PROXY_NAME_PATTERN}）"
        ));
    }
    None
}

/// 校验 `[egress]` 段（设备级）：空串视为未设置；`__DIRECT__` 不接受（直连请删键）。
pub fn validate_egress(section: &EgressSection) -> ValidationReport {
    let mut report = ValidationReport::new();
    let Some(value) = section.default.as_deref().filter(|value| !value.is_empty()) else {
        return report;
    };
    if value == DIRECT_MARKER {
        // 契约 §2.3 / §2.6：设备级 default 不接受保留值，直连应当删键。
        report.error(
            "invalid_egress_default",
            "egress.default",
            format!("设备级 default 不能是 {DIRECT_MARKER}；直连请删除 [egress].default"),
        );
    } else if let Some(message) = validate_egress_proxy_name(value) {
        report.error("invalid_egress_name", "egress.default", message);
    }
    report
}

fn is_java_identifier(segment: &str) -> bool {
    let mut bytes = segment.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Android 版本号：数字，可带点分小段（`15` / `14` / `8.1`）。
pub fn is_android_version(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// 校验一个模板（不含跨条目冲突）。
pub fn validate_template(name: &str, template: &DeviceTemplate) -> ValidationReport {
    let mut report = ValidationReport::new();
    let path = format!("templates.{name}");

    if let Some(message) = validate_template_name(name) {
        report.error("template.name_invalid", path.clone(), message);
    }

    let mut seen: HashSet<&str> = HashSet::new();
    for package in &template.packages {
        if let Some(message) = validate_package_key(package) {
            report.error(
                "package.invalid",
                format!("{path}.packages"),
                format!("包名 {package:?}：{message}"),
            );
        } else if !seen.insert(package.as_str()) {
            report.warning(
                "package.duplicate_in_template",
                format!("{path}.packages"),
                format!("包名 {package} 在同一个模板里出现了多次"),
            );
        }
    }

    validate_common_fields(
        &mut report,
        &path,
        template.dpi,
        template.sdk_int,
        template.android_version.as_deref(),
        template.custom_props.as_ref(),
    );

    report
}

/// 校验一条 `[[apps]]`（不含跨条目冲突）。
pub fn validate_app(app: &AppConfig) -> ValidationReport {
    let mut report = ValidationReport::new();
    let path = format!("apps.{}", app.package);

    if let Some(message) = validate_package_key(&app.package) {
        report.error(
            "package.invalid",
            path.clone(),
            format!("package {:?}：{message}", app.package),
        );
    }

    // 应用级出口：空串 = 未设置（继承设备级），__DIRECT__ = 强制直连，其余必须是合法代理名。
    if let Some(value) = app.egress.as_deref().filter(|value| !value.is_empty())
        && value != DIRECT_MARKER
        && let Some(message) = validate_egress_proxy_name(value)
    {
        report.error(
            "invalid_egress_name",
            format!("{path}.egress"),
            format!(
                "egress 只能是代理名（{EGRESS_PROXY_NAME_PATTERN}）或 {DIRECT_MARKER}：{message}"
            ),
        );
    }

    validate_common_fields(
        &mut report,
        &path,
        app.dpi,
        app.sdk_int,
        app.android_version.as_deref(),
        app.custom_props.as_ref(),
    );

    report
}

fn validate_common_fields(
    report: &mut ValidationReport,
    path: &str,
    dpi: Option<u32>,
    sdk_int: Option<NumberSetting>,
    android_version: Option<&str>,
    custom_props: Option<&std::collections::HashMap<String, String>>,
) {
    if let Some(dpi) = dpi
        && !(DPI_MIN..=DPI_MAX).contains(&dpi)
    {
        report.error(
            "dpi.out_of_range",
            format!("{path}.dpi"),
            format!("dpi 必须在 {DPI_MIN}-{DPI_MAX} 之间，当前 {dpi}（超出范围会在合并时被忽略）"),
        );
    }

    if let Some(NumberSetting::Value(value)) = sdk_int
        && !(SDK_INT_MIN..=SDK_INT_MAX).contains(&value)
    {
        report.error(
            "sdk_int.out_of_range",
            format!("{path}.sdk_int"),
            format!("sdk_int 必须在 {SDK_INT_MIN}-{SDK_INT_MAX} 之间，当前 {value}"),
        );
    }

    if let Some(version) = android_version
        && version != DELETE_MARKER
        && version != EMPTY_MARKER
        && !is_android_version(version)
    {
        report.error(
            "android_version.invalid",
            format!("{path}.android_version"),
            format!("android_version 必须是数字版本号（如 15、14），当前 {version:?}"),
        );
    }

    if let Some(props) = custom_props {
        for (key, value) in props {
            let prop_path = format!("{path}.custom_props.{key}");
            if key.is_empty() {
                report.error("custom_props.key_empty", prop_path, "属性名不能为空");
                continue;
            }
            if !PROP_KEY_PREFIXES
                .iter()
                .any(|prefix| key.starts_with(prefix))
            {
                report.error(
                    "custom_props.key_not_allowed",
                    prop_path.clone(),
                    format!(
                        "属性名必须以 {} 开头（当前 {key:?}）",
                        PROP_KEY_PREFIXES.join(" 或 ")
                    ),
                );
            }
            if key.len() > PROP_KEY_MAX {
                report.error(
                    "custom_props.key_too_long",
                    prop_path.clone(),
                    format!("属性名超过 {PROP_KEY_MAX} 字节"),
                );
            }
            if value == DELETE_MARKER || value == EMPTY_MARKER {
                continue;
            }
            if value.len() > PROP_VALUE_HARD_MAX {
                report.error(
                    "custom_props.value_too_long",
                    prop_path.clone(),
                    format!("属性值超过 {PROP_VALUE_HARD_MAX} 字节上限"),
                );
            } else if value.len() > PROP_VALUE_MAX {
                report.warning(
                    "custom_props.value_too_long",
                    prop_path,
                    format!("属性值超过 Android 上限 {PROP_VALUE_MAX} 字节，可能被 resetprop 截断"),
                );
            }
        }
    }
}

/// 找出被多个模板声明的包名（按包名排序，结果确定）。
pub fn duplicate_packages(config: &Config) -> Vec<DuplicatePackage> {
    let mut claims: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, template) in &config.templates {
        for package in &template.packages {
            let templates = claims.entry(package.as_str()).or_default();
            if !templates.contains(&name.as_str()) {
                templates.push(name.as_str());
            }
        }
    }

    claims
        .into_iter()
        .filter(|(_, templates)| templates.len() > 1)
        .map(|(package, templates)| DuplicatePackage {
            package: package.to_owned(),
            templates: templates.into_iter().map(str::to_owned).collect(),
        })
        .collect()
}

/// 校验整份配置：逐条目 + 跨条目冲突。
pub fn validate_config(config: &Config, options: &ValidationOptions) -> ValidationReport {
    let mut report = ValidationReport::new();

    if let Some(section) = &config.egress {
        report.merge(validate_egress(section));
    }

    for (name, template) in &config.templates {
        report.merge(validate_template(name, template));
    }
    for app in &config.apps {
        report.merge(validate_app(app));
    }

    for duplicate in duplicate_packages(config) {
        let first = duplicate
            .templates
            .first()
            .cloned()
            .unwrap_or_else(String::new);
        report.push(
            options.duplicate_package_severity,
            "package.duplicate",
            format!("templates.{first}.packages"),
            format!(
                "包名 {} 同时出现在模板 [{}] 中；只有书写顺序最靠前的模板生效，其余被静默忽略",
                duplicate.package,
                duplicate.templates.join(", ")
            ),
        );
    }

    let mut seen_apps: HashSet<&str> = HashSet::new();
    for app in &config.apps {
        if !seen_apps.insert(app.package.as_str()) {
            report.warning(
                "package.duplicate_app",
                format!("apps.{}", app.package),
                format!("包名 {} 有多条 [[apps]] 条目，只有第一条生效", app.package),
            );
        }

        let claimants = config.conflicting_templates_for_package(&app.package);
        if !claimants.is_empty() {
            report.warning(
                "package.app_overrides_template",
                format!("apps.{}", app.package),
                format!(
                    "包名 {} 同时被模板 [{}] 绑定；apps 条目优先，模板绑定不会生效",
                    app.package,
                    claimants.join(", ")
                ),
            );
        }
    }

    report
}

/// 文档里出现的未知字段（会被原样保留，但不在模型白名单里）。
#[cfg(feature = "edit")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnknownField {
    /// 所属位置：`config` / `templates.<name>` / `apps[<index>]`。
    pub path: String,
    /// 未知键名。
    pub key: String,
}

/// 扫描文档中的未知键（顶层 / 模板 / apps 三层）。
#[cfg(feature = "edit")]
pub fn unknown_fields(document: &toml_edit::DocumentMut) -> Vec<UnknownField> {
    let mut fields = Vec::new();
    let table = document.as_table();

    for (key, _) in table.iter() {
        if !crate::config::is_known_config_key(key) {
            fields.push(UnknownField {
                path: "config".to_owned(),
                key: key.to_owned(),
            });
        }
    }

    // [egress] 只允许 default：host/port/username/password 等凭据键出现在 0644 的
    // config.toml 里必须被看见（server 会升级成 422 unknown_field）。
    if let Some(egress) = table.get("egress").and_then(toml_edit::Item::as_table) {
        for (key, _) in egress.iter() {
            if !crate::config::is_known_egress_key(key) {
                fields.push(UnknownField {
                    path: "egress".to_owned(),
                    key: key.to_owned(),
                });
            }
        }
    }

    if let Some(templates) = table.get("templates").and_then(toml_edit::Item::as_table) {
        for (name, item) in templates.iter() {
            let Some(template) = item.as_table() else {
                continue;
            };
            for (key, _) in template.iter() {
                if !crate::config::is_known_template_key(key) {
                    fields.push(UnknownField {
                        path: format!("templates.{name}"),
                        key: key.to_owned(),
                    });
                }
            }
        }
    }

    if let Some(apps) = table
        .get("apps")
        .and_then(toml_edit::Item::as_array_of_tables)
    {
        for (index, app) in apps.iter().enumerate() {
            for (key, _) in app.iter() {
                if !crate::config::is_known_app_key(key) {
                    fields.push(UnknownField {
                        path: format!("apps[{index}]"),
                        key: key.to_owned(),
                    });
                }
            }
        }
    }

    fields
}

/// 校验整个文档：解析 + 逐条目校验 + 未知字段 warning。
#[cfg(feature = "edit")]
pub fn validate_document(
    document: &toml_edit::DocumentMut,
    options: &ValidationOptions,
) -> ValidationReport {
    let mut report = ValidationReport::new();

    match Config::from_toml(&document.to_string()) {
        Ok(config) => report.merge(validate_config(&config, options)),
        Err(error) => report.error("config.parse", "config", format!("配置解析失败：{error:#}")),
    }

    for field in unknown_fields(document) {
        report.warning(
            "config.unknown_field",
            field.path,
            format!("未知字段 {} 会原样保留，但不会被模块使用", field.key),
        );
    }

    report
}
