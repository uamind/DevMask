// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! API 响应 DTO 与 core 校验结果到契约结构的映射（docs/API.md 6）。
//!
//! 配置内容字段一律直接用 core 的模型（snake_case，与 config.toml 同名）；
//! 只有 API 元数据（revision / position / fieldsSet / nextCursor …）是 camelCase。

use std::collections::HashMap;

use devmask_core::config::{AppConfig, Config, DeviceTemplate, MergedAppConfig, NumberSetting};
use devmask_core::validate::{Severity, ValidationReport, duplicate_packages};
use serde::Serialize;
use serde_json::{Value, json};

use crate::store::TxOutcome;

/// 写入/校验警告（docs/API.md 6.6）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Warning {
    /// 稳定警告码。
    pub code: &'static str,
    /// 人类可读说明。
    pub message: String,
    /// 结构化上下文。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl Warning {
    /// 构造警告。
    pub fn new(code: &'static str, message: impl Into<String>, details: Option<Value>) -> Self {
        Self {
            code,
            message: message.into(),
            details,
        }
    }
}

/// 字段级校验错误（docs/API.md 6.7）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationError {
    /// 字段路径，如 templates.pixel_9.dpi。
    pub path: String,
    /// 稳定错误码（docs/API.md 8.2）。
    pub code: &'static str,
    /// 人类可读说明。
    pub message: String,
}

impl ValidationError {
    /// 构造一条错误。
    pub fn new(path: impl Into<String>, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            code,
            message: message.into(),
        }
    }

    /// 转成 422 details.errors 的 JSON 数组元素。
    pub fn to_value(&self) -> Value {
        json!({ "path": self.path, "code": self.code, "message": self.message })
    }
}

/// 把 core 的校验报告映射成契约结构：
///
/// - error → [`ValidationError`]，错误码收敛到 docs/API.md 8.2 的 9 个稳定值；
/// - warning → [`Warning`]，只保留契约里存在的码（未知字段/重复包名）；
/// - `package.duplicate_app` 由 warning 提升为 error `duplicate_app_record`（契约 6.3）。
///
/// 契约里没有对应码的 core warning（`custom_props.value_too_long`、
/// `package.app_overrides_template`）不进入 API，避免客户端遇到未知枚举值。
pub fn map_validation_report(report: &ValidationReport) -> (Vec<ValidationError>, Vec<Warning>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    for issue in &report.issues {
        match issue.severity {
            Severity::Error => errors.push(ValidationError::new(
                issue.path.clone(),
                map_issue_code(issue.code),
                issue.message.clone(),
            )),
            Severity::Warning => match issue.code {
                "config.unknown_field" => warnings.push(Warning::new(
                    "unknown_field_preserved",
                    issue.message.clone(),
                    Some(json!({ "fields": [issue.path] })),
                )),
                "package.duplicate" | "package.duplicate_in_template" => {
                    warnings.push(Warning::new(
                        "duplicate_package",
                        issue.message.clone(),
                        Some(json!({ "path": issue.path })),
                    ));
                }
                "package.duplicate_app" => {
                    // 契约要求同一 package 多条内联记录是 422 duplicate_app_record。
                    errors.push(ValidationError::new(
                        issue.path.clone(),
                        "duplicate_app_record",
                        issue.message.clone(),
                    ));
                }
                _ => {}
            },
        }
    }

    (errors, warnings)
}

fn map_issue_code(code: &str) -> &'static str {
    match code {
        "config.parse" => "syntax_error",
        "template.name_invalid" => "invalid_template_name",
        "package.invalid" => "invalid_package",
        "package.duplicate_app" => "duplicate_app_record",
        // egress 的两个校验码（契约 §1.8）core 已按契约命名，server 原样透传，不要收敛成 invalid_value
        "invalid_egress_name" => "invalid_egress_name",
        "invalid_egress_default" => "invalid_egress_default",
        "dpi.out_of_range" | "sdk_int.out_of_range" => "out_of_range",
        "android_version.invalid"
        | "security_patch.invalid"
        | "custom_props.key_empty"
        | "custom_props.key_not_allowed"
        | "custom_props.key_too_long"
        | "custom_props.value_too_long" => "invalid_value",
        _ => "invalid_value",
    }
}

/// 同一包名出现在多个模板的警告（details 带 package 与 templates）。
pub fn duplicate_package_warnings(config: &Config) -> Vec<Warning> {
    duplicate_packages(config)
        .into_iter()
        .map(|duplicate| {
            Warning::new(
                "duplicate_package",
                format!(
                    "{} 同时被 {} 声明",
                    duplicate.package,
                    duplicate.templates.join(" 与 ")
                ),
                Some(json!({
                    "package": duplicate.package,
                    "templates": duplicate.templates,
                })),
            )
        })
        .collect()
}

/// 空 packages 模板的警告。
pub fn empty_packages_warnings(config: &Config) -> Vec<Warning> {
    config
        .templates
        .iter()
        .filter(|(_, template)| template.packages.is_empty())
        .map(|(name, _)| {
            Warning::new(
                "empty_packages",
                format!("模板 {name} 没有绑定任何包名"),
                Some(json!({ "template": name })),
            )
        })
        .collect()
}

/// 未安装包名的警告。`installed` 返回 None 表示 PackageManager 不可用，跳过。
pub fn package_not_installed_warnings(
    packages: impl IntoIterator<Item = String>,
    installed: impl Fn(&str) -> Option<bool>,
) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for package in packages {
        if !seen.insert(package.clone()) {
            continue;
        }
        if installed(&package) == Some(false) {
            warnings.push(Warning::new(
                "package_not_installed",
                format!("包名 {package} 当前未安装，配置仍会保留"),
                Some(json!({ "package": package })),
            ));
        }
    }
    warnings
}

/// 写入结果（docs/API.md 6.5）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteResult {
    /// 写入后的配置 revision（dryRun 时为当前值）。
    pub revision: String,
    /// 写入前 revision；首次创建为 null。
    pub previous_revision: Option<String>,
    /// 是否演练。
    pub dry_run: bool,
    /// 自动备份 ID；dryRun 时为 null。
    pub backup_id: Option<String>,
    /// 仅在 diff=true 或 dry_run=true 时出现。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// diff 是否因超过 64 KiB 被截断。
    pub diff_truncated: bool,
    /// 警告列表。
    pub warnings: Vec<Warning>,
}

impl WriteResult {
    /// 由事务结果构造；`diff` 由调用方按需传入。
    pub fn new(
        outcome: &TxOutcome,
        dry_run: bool,
        diff: Option<(String, bool)>,
        warnings: Vec<Warning>,
    ) -> Self {
        let (diff, diff_truncated) = match diff {
            Some((diff, truncated)) => (Some(diff), truncated),
            None => (None, false),
        };
        Self {
            revision: outcome.revision.clone(),
            previous_revision: outcome.previous_revision.clone(),
            dry_run,
            backup_id: outcome.backup_id.clone(),
            diff,
            diff_truncated,
            warnings,
        }
    }
}

/// GET /config 的响应（format=json）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigResponse {
    /// 当前 revision。
    pub revision: String,
    /// 文件 mtime（UTC）；不存在时为 null。
    pub mtime: Option<String>,
    /// 文件字节数。
    pub size: u64,
    /// 配置文件是否存在。
    pub exists: bool,
    /// 警告（config_missing / unknown_field_preserved）。
    pub warnings: Vec<Warning>,
    /// 配置文档（snake_case，与 config.toml 同名）。
    pub config: Config,
}

/// GET /config/revision 的响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigRevisionResponse {
    /// 当前 revision。
    pub revision: String,
    /// 文件 mtime（UTC）；不存在时为 null。
    pub mtime: Option<String>,
    /// 文件字节数。
    pub size: u64,
    /// 配置文件是否存在。
    pub exists: bool,
}

/// POST /config/validate 的响应（校验失败也是 200）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateResponse {
    /// 是否通过（无 error）。
    pub valid: bool,
    /// 校验时的当前 revision。
    pub revision: String,
    /// 字段级错误。
    pub errors: Vec<ValidationError>,
    /// 警告。
    pub warnings: Vec<Warning>,
}

/// 模板列表项。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListItem {
    /// 模板名（config.toml 键）。
    pub name: String,
    /// 0 基书写位置。
    pub position: usize,
    /// 绑定的包名（保序）。
    pub packages: Vec<String>,
    /// 包名数量。
    pub package_count: usize,
    /// 已设置的字段名（按字母序，不含 packages）。
    pub fields_set: Vec<String>,
}

/// GET /templates 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListResponse {
    /// 当前 revision。
    pub revision: String,
    /// 过滤后的总数。
    pub total: usize,
    /// 下一页游标；null 表示没有下一页。
    pub next_cursor: Option<String>,
    /// 当前页。
    pub items: Vec<TemplateListItem>,
}

/// GET /templates/{name} 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateResponse {
    /// 当前 revision。
    pub revision: String,
    /// 0 基书写位置。
    pub position: usize,
    /// 模板名。
    pub name: String,
    /// 模板对象（不含模板名）。
    pub template: DeviceTemplate,
}

/// 应用来源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSource {
    /// inline | template | none。
    #[serde(rename = "type")]
    pub source_type: &'static str,
    /// 模板名；inline / none 为 null。
    pub template: Option<String>,
    /// 内联记录存在但同时命中模板。
    pub also_inline: bool,
}

/// 应用列表项。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppListItem {
    /// 包名。
    pub package: String,
    /// 0 基数组位置。
    pub position: usize,
    /// 来源。
    pub source: AppSource,
    /// 已设置的字段名（字母序，不含 package）。
    pub fields_set: Vec<String>,
}

/// GET /apps 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppListResponse {
    /// 当前 revision。
    pub revision: String,
    /// 过滤后的总数。
    pub total: usize,
    /// 下一页游标。
    pub next_cursor: Option<String>,
    /// 当前页。
    pub items: Vec<AppListItem>,
}

/// 最终生效值（docs/API.md 6.4）：字符串只在显式设置时出现，布尔/数组给继承后的值。
#[derive(Debug, Clone, Default, Serialize)]
pub struct AppResolved {
    /// Build.MANUFACTURER
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// Build.BRAND
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    /// 市场名
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marketname: Option<String>,
    /// Build.MODEL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// ro.product.name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Build.DEVICE
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Build.PRODUCT
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    /// Build.HARDWARE
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardware: Option<String>,
    /// Build.BOARD
    #[serde(skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    /// Build.SOC_MODEL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soc_model: Option<String>,
    /// Build.FINGERPRINT
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Build.ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    /// Build.DISPLAY
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_id: Option<String>,
    /// Build.VERSION.INCREMENTAL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incremental: Option<String>,
    /// Build.VERSION.SECURITY_PATCH
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_patch: Option<String>,
    /// Build.CHARACTERISTICS
    #[serde(skip_serializing_if = "Option::is_none")]
    pub characteristics: Option<String>,
    /// Build.VERSION.RELEASE
    #[serde(skip_serializing_if = "Option::is_none")]
    pub android_version: Option<String>,
    /// Build.VERSION.SDK_INT 或 __DELETE__
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdk_int: Option<NumberSetting>,
    /// 显示密度
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
    /// 自定义属性（不含指纹派生的 ro.build.type / ro.build.tags）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_props: Option<HashMap<String, String>>,
    /// 是否强制 FORCE_DENYLIST_UNMOUNT（继承后的有效值）
    pub force_denylist_unmount: bool,
    /// maps 清理模式（继承后的有效值）
    pub hide_maps: Vec<String>,
    /// 是否全部走 companion resetprop（继承后的有效值）
    pub companion_resetprop: bool,
}

impl AppResolved {
    /// 从 core 的合并结果构造；派生属性不暴露给客户端。
    pub fn from_merged(merged: &MergedAppConfig) -> Self {
        let custom_props = merged.custom_props.as_ref().map(|props| {
            props
                .iter()
                .filter(|(key, _)| !merged.derived_props.contains(*key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<HashMap<_, _>>()
        });
        Self {
            manufacturer: merged.manufacturer.clone(),
            brand: merged.brand.clone(),
            marketname: merged.marketname.clone(),
            model: merged.model.clone(),
            name: merged.name.clone(),
            device: merged.device.clone(),
            product: merged.product.clone(),
            hardware: merged.hardware.clone(),
            board: merged.board.clone(),
            soc_model: merged.soc_model.clone(),
            fingerprint: merged.fingerprint.clone(),
            build_id: merged.build_id.clone(),
            display_id: merged.display_id.clone(),
            incremental: merged.incremental.clone(),
            security_patch: merged.security_patch.clone(),
            characteristics: merged.characteristics.clone(),
            android_version: merged.android_version.clone(),
            sdk_int: merged.sdk_int,
            dpi: merged.dpi,
            custom_props,
            force_denylist_unmount: merged.force_denylist_unmount,
            hide_maps: merged.hide_maps.clone(),
            companion_resetprop: merged.companion_resetprop,
        }
    }
}

/// GET /apps/{pkg} 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppResponse {
    /// 包名（与路径一致，含 @userId 时保留）。
    pub package: String,
    /// 当前 revision。
    pub revision: String,
    /// 是否已安装；pm 不可用时为 null。
    pub installed: Option<bool>,
    /// 配置来源。
    pub source: AppSource,
    /// 最终生效值；未配置时为 {}。
    pub resolved: Value,
    /// 警告（no_config / duplicate_package / package_not_installed）。
    pub warnings: Vec<Warning>,
}

/// 契约里「集合规模 / 字符串长度」的上限（docs/API.md 8.1），core 校验不覆盖这些，
/// 由 API 层在写入前统一把关。
pub const MAX_TEMPLATE_PACKAGES: usize = 200;
/// 内联应用记录上限。
pub const MAX_APPS: usize = 500;
/// hide_maps 条目上限。
pub const MAX_HIDE_MAPS: usize = 64;
/// custom_props 条目上限。
pub const MAX_CUSTOM_PROPS: usize = 128;
/// 普通字符串字段长度上限。
pub const MAX_STRING_FIELD: usize = 512;
/// hide_maps 单条长度上限。
pub const MAX_HIDE_MAP_ENTRY: usize = 256;
/// custom_props 值长度上限。
pub const MAX_PROP_VALUE: usize = 1024;

/// 契约 8.1 的规模/长度校验。
pub fn limit_issues(config: &Config) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    if config.apps.len() > MAX_APPS {
        errors.push(ValidationError::new(
            "apps",
            "out_of_range",
            format!("apps 最多 {MAX_APPS} 条，当前 {}", config.apps.len()),
        ));
    }
    check_hide_maps(&config.default_hide_maps, "default_hide_maps", &mut errors);
    for (name, template) in &config.templates {
        let path = format!("templates.{name}");
        if template.packages.len() > MAX_TEMPLATE_PACKAGES {
            errors.push(ValidationError::new(
                format!("{path}.packages"),
                "out_of_range",
                format!(
                    "单个模板最多 {MAX_TEMPLATE_PACKAGES} 个包名，当前 {}",
                    template.packages.len()
                ),
            ));
        }
        if let Some(hide_maps) = &template.hide_maps {
            check_hide_maps(hide_maps, &format!("{path}.hide_maps"), &mut errors);
        }
        if let Some(props) = &template.custom_props {
            check_custom_props(props, &format!("{path}.custom_props"), &mut errors);
        }
        check_string_fields(template, &path, &mut errors);
    }
    for (index, app) in config.apps.iter().enumerate() {
        let path = format!("apps[{index}]");
        if let Some(hide_maps) = &app.hide_maps {
            check_hide_maps(hide_maps, &format!("{path}.hide_maps"), &mut errors);
        }
        if let Some(props) = &app.custom_props {
            check_custom_props(props, &format!("{path}.custom_props"), &mut errors);
        }
        check_string_fields(app, &path, &mut errors);
    }
    errors
}

fn check_hide_maps(entries: &[String], path: &str, errors: &mut Vec<ValidationError>) {
    if entries.len() > MAX_HIDE_MAPS {
        errors.push(ValidationError::new(
            path,
            "out_of_range",
            format!("最多 {MAX_HIDE_MAPS} 项，当前 {}", entries.len()),
        ));
    }
    for entry in entries {
        if entry.chars().count() > MAX_HIDE_MAP_ENTRY {
            errors.push(ValidationError::new(
                path,
                "invalid_value",
                format!("hide_maps 每项不超过 {MAX_HIDE_MAP_ENTRY} 字符"),
            ));
            break;
        }
    }
}

fn check_custom_props(
    props: &HashMap<String, String>,
    path: &str,
    errors: &mut Vec<ValidationError>,
) {
    if props.len() > MAX_CUSTOM_PROPS {
        errors.push(ValidationError::new(
            path,
            "out_of_range",
            format!("最多 {MAX_CUSTOM_PROPS} 项，当前 {}", props.len()),
        ));
    }
    for value in props.values() {
        if value == "__DELETE__" || value == "__EMPTY__" {
            continue;
        }
        if value.chars().count() > MAX_PROP_VALUE {
            errors.push(ValidationError::new(
                path,
                "invalid_value",
                format!("属性值不超过 {MAX_PROP_VALUE} 字符"),
            ));
            break;
        }
    }
}

fn check_string_fields<T: Serialize>(value: &T, path: &str, errors: &mut Vec<ValidationError>) {
    let Value::Object(map) = serde_json::to_value(value).unwrap_or(Value::Null) else {
        return;
    };
    for (key, field) in map {
        let Value::String(text) = field else {
            continue;
        };
        if key == "package" {
            continue;
        }
        if text.chars().count() > MAX_STRING_FIELD {
            errors.push(ValidationError::new(
                format!("{path}.{key}"),
                "invalid_value",
                format!("{key} 不超过 {MAX_STRING_FIELD} 字符"),
            ));
        }
    }
}
/// `[egress]` 段只允许 `default`：出现 host/port/username/password 等键即 422 unknown_field。
///
/// core 把这类键报成 warning（`config.unknown_field`），但契约 §2.3 硬约束 1 要求 422：
/// 0644 的 config.toml 里不允许出现任何凭据键（凭据只能进 0600 的 egress.toml）。
pub fn egress_section_issues(document: &toml_edit::DocumentMut) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let Some(table) = document.get("egress").and_then(toml_edit::Item::as_table) else {
        return errors;
    };
    for (key, _) in table.iter() {
        if !devmask_core::config::is_known_egress_key(key) {
            errors.push(ValidationError::new(
                format!("egress.{key}"),
                "unknown_field",
                format!("[egress] 只允许 default，未知字段 {key}"),
            ));
        }
    }
    errors
}

/// 从配置文本收集「文件级」警告：未知字段会被原样保留。
pub fn document_warnings(text: &str) -> Vec<Warning> {
    let Ok(document) = text.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };
    let fields: Vec<String> = devmask_core::validate::unknown_fields(&document)
        .into_iter()
        .map(|field| format!("{}.{}", field.path, field.key))
        .collect();
    if fields.is_empty() {
        return Vec::new();
    }
    vec![Warning::new(
        "unknown_field_preserved",
        format!("文件里有 {} 个未知字段，已原样保留", fields.len()),
        Some(json!({ "fields": fields })),
    )]
}

/// 把 ConfigDocument 渲染成 config.toml 文本（PUT /config 的 JSON 路径）。
///
/// 复用 core 的 upsert_template / set_app 保证字段写法与结构化端点一致；
/// custom_props 排序后写入，避免 HashMap 迭代顺序让同样内容产生不同 revision。
pub fn document_from_config(config: &Config) -> Result<String, devmask_core::edit::EditError> {
    use devmask_core::edit::{set_app, upsert_template};
    use toml_edit::{DocumentMut, Item, Value as TomlValue};

    let mut document = DocumentMut::new();
    document["debug"] = Item::Value(TomlValue::from(config.debug));
    document["default_force_denylist_unmount"] =
        Item::Value(TomlValue::from(config.default_force_denylist_unmount));
    if !config.default_hide_maps.is_empty() {
        let mut array = toml_edit::Array::new();
        for entry in &config.default_hide_maps {
            array.push(entry.clone());
        }
        document["default_hide_maps"] = Item::Value(TomlValue::Array(array));
    }
    for (name, template) in &config.templates {
        upsert_template(&mut document, name, template)?;
    }
    for app in &config.apps {
        set_app(&mut document, app)?;
    }
    sort_custom_props(&mut document);
    Ok(document.to_string())
}

fn sort_custom_props(document: &mut toml_edit::DocumentMut) {
    use toml_edit::Item;

    if let Some(templates) = document.get_mut("templates").and_then(Item::as_table_mut) {
        for (_, item) in templates.iter_mut() {
            if let Some(table) = item.as_table_mut() {
                sort_table_key(table, "custom_props");
            }
        }
    }
    if let Some(apps) = document
        .get_mut("apps")
        .and_then(Item::as_array_of_tables_mut)
    {
        for table in apps.iter_mut() {
            sort_table_key(table, "custom_props");
        }
    }
}

fn sort_table_key(table: &mut toml_edit::Table, key: &str) {
    use toml_edit::{Item, Value};

    match table.get_mut(key) {
        Some(Item::Value(Value::InlineTable(inline))) => inline.sort_values(),
        Some(Item::Table(sub)) => sub.sort_values(),
        _ => {}
    }
}
/// 模板已设置字段名（字母序，不含 packages）。
pub fn template_fields_set(template: &DeviceTemplate) -> Vec<String> {
    fields_set_from_value(template, &["packages"])
}

/// 应用已设置字段名（字母序，不含 package）。
pub fn app_fields_set(app: &AppConfig) -> Vec<String> {
    fields_set_from_value(app, &["package"])
}

fn fields_set_from_value<T: Serialize>(value: &T, exclude: &[&str]) -> Vec<String> {
    let Value::Object(map) = serde_json::to_value(value).unwrap_or(Value::Null) else {
        return Vec::new();
    };
    let mut fields: Vec<String> = map
        .keys()
        .filter(|key| !exclude.contains(&key.as_str()))
        .cloned()
        .collect();
    fields.sort();
    fields
}
