// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 基于 toml_edit 的文档级增量编辑。
//!
//! 设计目标（docs/dev/server-api-plan.md 9.5）：
//!
//! - **只改目标键**：注释、空行、键顺序、未知字段（顶层 `[cpu_presets]`、模板的
//!   `author` / `description` 等）全部原样保留；
//! - **幂等**：目标模型与文档一致时返回 `changed = false`，调用方据此跳过落盘；
//! - **可预测**：写入顺序沿用 [`crate::config`] 结构体字段顺序；已在文档里的键保持原位，
//!   新键追加到所在表的末尾。
//!
//! 语义约定：`upsert_*` 是**整体替换**（PUT 语义）——模型里为 `None` 的已知字段会从
//! 文档里删除，`packages` 为空时删除 `packages` 键。未知键从不删除。
//!
//! 所有函数都作用在内存中的 [`DocumentMut`] 上，不碰文件系统；落盘见 `crate::revision::ConfigStore`。

use std::collections::HashMap;

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use crate::config::{AppConfig, DELETE_MARKER, DeviceTemplate, EgressSection, NumberSetting};

/// 编辑失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// TOML 语法错误。
    Parse { message: String },
    /// 入参不合法（空模板名、空包名等）。
    Invalid {
        field: &'static str,
        message: String,
    },
    /// 目标模板不存在。
    NotFound { table: &'static str, name: String },
    /// 文档结构与预期不符（例如 `apps` 不是数组表）。
    Structure {
        path: String,
        expected: &'static str,
        found: &'static str,
    },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { message } => write!(formatter, "invalid TOML: {message}"),
            Self::Invalid { field, message } => write!(formatter, "invalid {field}: {message}"),
            Self::NotFound { table, name } => {
                write!(formatter, "{table} entry '{name}' not found")
            }
            Self::Structure {
                path,
                expected,
                found,
            } => write!(formatter, "expected {expected} at '{path}', found {found}"),
        }
    }
}

impl std::error::Error for EditError {}

/// 解析 TOML 文本为可编辑文档（保留全部格式信息）。
pub fn parse_document(text: &str) -> Result<DocumentMut, EditError> {
    text.parse::<DocumentMut>()
        .map_err(|error| EditError::Parse {
            message: error.to_string(),
        })
}

/// 创建或整体更新模板 `name`。
///
/// - 模板不存在：新建 `[templates.<name>]` 表；
/// - 模板已存在：已知字段按模型替换（`None` 即删除该键），未知字段保留原位；
/// - 返回文档是否真的发生了变化。
pub fn upsert_template(
    document: &mut DocumentMut,
    name: &str,
    template: &DeviceTemplate,
) -> Result<bool, EditError> {
    if name.trim().is_empty() {
        return Err(EditError::Invalid {
            field: "template name",
            message: "模板名不能为空".to_owned(),
        });
    }

    let before = document.to_string();
    let templates = templates_table_mut(document)?;

    if !templates.contains_key(name) {
        templates.insert(name, Item::Table(Table::new()));
    }
    let Some(item) = templates.get_mut(name) else {
        unreachable!("template table inserted above");
    };
    let table = match item {
        Item::Table(table) => table,
        other => {
            return Err(EditError::Structure {
                path: format!("templates.{name}"),
                expected: "table",
                found: item_kind(other),
            });
        }
    };

    apply_template_fields(table, template);
    Ok(document.to_string() != before)
}

/// 删除模板 `name`；不存在时返回 `false`（幂等删除）。
pub fn delete_template(document: &mut DocumentMut, name: &str) -> bool {
    document
        .as_table_mut()
        .get_mut("templates")
        .and_then(Item::as_table_mut)
        .is_some_and(|templates| templates.remove(name).is_some())
}

/// 把 `package` 绑定到模板 `name`（已绑定则为空操作）。
pub fn bind_package(
    document: &mut DocumentMut,
    name: &str,
    package: &str,
) -> Result<bool, EditError> {
    if package.trim().is_empty() {
        return Err(EditError::Invalid {
            field: "package",
            message: "包名不能为空".to_owned(),
        });
    }

    let before = document.to_string();
    let templates = templates_table_mut(document)?;
    let Some(item) = templates.get_mut(name) else {
        return Err(EditError::NotFound {
            table: "templates",
            name: name.to_owned(),
        });
    };
    let table = match item {
        Item::Table(table) => table,
        other => {
            return Err(EditError::Structure {
                path: format!("templates.{name}"),
                expected: "table",
                found: item_kind(other),
            });
        }
    };

    if !table.contains_key("packages") {
        let mut array = Array::new();
        array.set_trailing(" ");
        let mut value = Value::from(package);
        // 与项目现有配置风格一致：packages = [ "a", "b" ]
        value.decor_mut().set_prefix(" ");
        array.push_formatted(value);
        table.insert("packages", Item::Value(Value::Array(array)));
    } else {
        let Some(item) = table.get_mut("packages") else {
            unreachable!("packages checked above");
        };
        match item {
            Item::Value(Value::Array(array)) => {
                let bound = array.iter().any(|item| item.as_str() == Some(package));
                if !bound {
                    push_string(array, package);
                }
            }
            other => {
                return Err(EditError::Structure {
                    path: format!("templates.{name}.packages"),
                    expected: "array",
                    found: item_kind(other),
                });
            }
        }
    }

    Ok(document.to_string() != before)
}

/// 把 `package` 从模板 `name` 解绑（删除全部重复项；未绑定则为空操作）。
pub fn unbind_package(
    document: &mut DocumentMut,
    name: &str,
    package: &str,
) -> Result<bool, EditError> {
    let before = document.to_string();
    let templates = templates_table_mut(document)?;
    let Some(item) = templates.get_mut(name) else {
        return Err(EditError::NotFound {
            table: "templates",
            name: name.to_owned(),
        });
    };
    let table = match item {
        Item::Table(table) => table,
        other => {
            return Err(EditError::Structure {
                path: format!("templates.{name}"),
                expected: "table",
                found: item_kind(other),
            });
        }
    };

    match table.get_mut("packages") {
        Some(Item::Value(Value::Array(array))) => {
            let mut index = 0;
            while index < array.len() {
                if array.get(index).and_then(Value::as_str) == Some(package) {
                    array.remove(index);
                } else {
                    index += 1;
                }
            }
        }
        Some(other) => {
            return Err(EditError::Structure {
                path: format!("templates.{name}.packages"),
                expected: "array",
                found: item_kind(other),
            });
        }
        None => {}
    }

    Ok(document.to_string() != before)
}

/// 创建或整体更新一条 `[[apps]]`（按 `package` 匹配）。
pub fn set_app(document: &mut DocumentMut, app: &AppConfig) -> Result<bool, EditError> {
    if app.package.trim().is_empty() {
        return Err(EditError::Invalid {
            field: "package",
            message: "应用配置的 package 不能为空".to_owned(),
        });
    }

    let before = document.to_string();
    ensure_apps_array(document)?;
    let Some(Item::ArrayOfTables(apps)) = document.as_table_mut().get_mut("apps") else {
        unreachable!("apps array ensured above");
    };

    let index = apps.iter().position(|table| {
        table.get("package").and_then(Item::as_str) == Some(app.package.as_str())
    });
    match index {
        Some(index) => {
            let Some(table) = apps.get_mut(index) else {
                unreachable!("index from position()");
            };
            apply_app_fields(table, app);
        }
        None => {
            let mut table = Table::new();
            apply_app_fields(&mut table, app);
            apps.push(table);
        }
    }

    Ok(document.to_string() != before)
}

/// 删除 `package` 对应的全部 `[[apps]]` 条目；未命中返回 `false`。
pub fn delete_app(document: &mut DocumentMut, package: &str) -> Result<bool, EditError> {
    ensure_apps_array(document)?;
    let Some(Item::ArrayOfTables(apps)) = document.as_table_mut().get_mut("apps") else {
        unreachable!("apps array ensured above");
    };

    let mut removed = false;
    let mut index = 0;
    while index < apps.len() {
        let Some(table) = apps.get(index) else {
            break;
        };
        let matches = table.get("package").and_then(Item::as_str) == Some(package);
        if matches {
            apps.remove(index);
            removed = true;
        } else {
            index += 1;
        }
    }
    Ok(removed)
}

/// 创建或整体更新顶层 `[egress]` 段（契约 §2.3 / §2.7）。
///
/// - `default` 为 `Some(非空代理名)`：写入 `[egress] default = "…"`（表不存在时新建）；
/// - `None` 或空串：删除 `default` 键；表因此变空时整段删除（删除整段等价于直连）；
/// - `[egress]` 里的未知键与注释原样保留（未知键会让表不再为空，故整段不会被删）。
///
/// 幂等：文档已与模型一致时返回 `false`，调用方据此跳过落盘。
pub fn set_egress(document: &mut DocumentMut, section: &EgressSection) -> Result<bool, EditError> {
    let before = document.to_string();
    let value = non_empty(section.default.as_deref());

    if document.as_table().get("egress").is_none() {
        if let Some(value) = value {
            let mut table = Table::new();
            // 已有内容时让新段与上文空一行，符合项目配置的书写风格。
            if !document.as_table().is_empty() {
                table.decor_mut().set_prefix("\n");
            }
            set_string(&mut table, "default", value);
            document.as_table_mut().insert("egress", Item::Table(table));
        }
        return Ok(document.to_string() != before);
    }

    let mut remove_table = false;
    match document.as_table_mut().get_mut("egress") {
        Some(Item::Table(table)) => match value {
            Some(value) => set_string(table, "default", value),
            None => {
                table.remove("default");
                remove_table = table.is_empty();
            }
        },
        Some(other) => {
            return Err(EditError::Structure {
                path: "egress".to_owned(),
                expected: "table",
                found: item_kind(other),
            });
        }
        None => unreachable!("egress checked above"),
    }
    if remove_table {
        document.as_table_mut().remove("egress");
    }

    Ok(document.to_string() != before)
}

// ── 内部实现 ────────────────────────────────────────────────────────────────

fn item_kind(item: &Item) -> &'static str {
    match item {
        Item::None => "nothing",
        Item::Value(Value::Array(_)) => "array",
        Item::Value(Value::InlineTable(_)) => "inline table",
        Item::Value(_) => "value",
        Item::Table(_) => "table",
        Item::ArrayOfTables(_) => "array of tables",
    }
}

/// 取（必要时创建隐式）顶层 `templates` 表。
fn templates_table_mut(document: &mut DocumentMut) -> Result<&mut Table, EditError> {
    if document.as_table().get("templates").is_none() {
        let mut table = Table::new();
        table.set_implicit(true);
        document
            .as_table_mut()
            .insert("templates", Item::Table(table));
    }

    match document.as_table_mut().get_mut("templates") {
        Some(Item::Table(table)) => Ok(table),
        Some(other) => Err(EditError::Structure {
            path: "templates".to_owned(),
            expected: "table",
            found: item_kind(other),
        }),
        None => unreachable!("templates inserted above"),
    }
}

/// 保证 `apps` 是数组表。`apps = [ { ... } ]` 的内联写法会被等价改写为 `[[apps]]`
/// （TOML 语义完全相同；内联数组里的注释无法保留，这种写法在真实配置里没有出现过）。
fn ensure_apps_array(document: &mut DocumentMut) -> Result<(), EditError> {
    match document.as_table().get("apps") {
        None => {
            document
                .as_table_mut()
                .insert("apps", Item::ArrayOfTables(ArrayOfTables::new()));
            Ok(())
        }
        Some(Item::ArrayOfTables(_)) => Ok(()),
        Some(Item::Value(Value::Array(_))) => {
            let Some(item) = document.as_table_mut().remove("apps") else {
                unreachable!("apps exists");
            };
            let Item::Value(Value::Array(array)) = item else {
                unreachable!("matched array above");
            };

            let mut apps = ArrayOfTables::new();
            for value in array.iter() {
                match value {
                    Value::InlineTable(inline) => apps.push(inline.clone().into_table()),
                    other => {
                        return Err(EditError::Structure {
                            path: "apps".to_owned(),
                            expected: "array of inline tables",
                            found: item_kind(&Item::Value(other.clone())),
                        });
                    }
                }
            }
            document
                .as_table_mut()
                .insert("apps", Item::ArrayOfTables(apps));
            Ok(())
        }
        Some(other) => Err(EditError::Structure {
            path: "apps".to_owned(),
            expected: "array of tables",
            found: item_kind(other),
        }),
    }
}

fn apply_template_fields(table: &mut Table, template: &DeviceTemplate) {
    if template.packages.is_empty() {
        table.remove("packages");
    } else {
        set_string_array(table, "packages", &template.packages);
    }
    set_opt_string(table, "manufacturer", template.manufacturer.as_deref());
    set_opt_string(table, "brand", template.brand.as_deref());
    set_opt_string(table, "marketname", template.marketname.as_deref());
    set_opt_string(table, "model", template.model.as_deref());
    set_opt_string(table, "name", template.name.as_deref());
    set_opt_string(table, "device", template.device.as_deref());
    set_opt_string(table, "product", template.product.as_deref());
    set_opt_string(table, "hardware", template.hardware.as_deref());
    set_opt_string(table, "board", template.board.as_deref());
    set_opt_string(table, "soc_model", template.soc_model.as_deref());
    set_opt_string(table, "fingerprint", template.fingerprint.as_deref());
    set_opt_string(table, "build_id", template.build_id.as_deref());
    set_opt_string(table, "display_id", template.display_id.as_deref());
    set_opt_string(table, "incremental", template.incremental.as_deref());
    set_opt_string(table, "security_patch", template.security_patch.as_deref());
    set_opt_string(
        table,
        "characteristics",
        template.characteristics.as_deref(),
    );
    set_opt_string(
        table,
        "android_version",
        template.android_version.as_deref(),
    );
    set_opt_number_setting(table, "sdk_int", template.sdk_int);
    set_opt_integer(table, "dpi", template.dpi);
    set_custom_props(table, template.custom_props.as_ref());
    set_opt_bool(
        table,
        "force_denylist_unmount",
        template.force_denylist_unmount,
    );
    match template.hide_maps.as_deref() {
        // None = 继承全局 default_hide_maps；Some([]) = 显式关掉，两者语义不同。
        None => {
            table.remove("hide_maps");
        }
        Some(values) => set_string_array(table, "hide_maps", values),
    }
    set_opt_bool(table, "companion_resetprop", template.companion_resetprop);
}

fn apply_app_fields(table: &mut Table, app: &AppConfig) {
    set_string(table, "package", &app.package);
    set_opt_string(table, "manufacturer", app.manufacturer.as_deref());
    set_opt_string(table, "brand", app.brand.as_deref());
    set_opt_string(table, "marketname", app.marketname.as_deref());
    set_opt_string(table, "model", app.model.as_deref());
    set_opt_string(table, "name", app.name.as_deref());
    set_opt_string(table, "device", app.device.as_deref());
    set_opt_string(table, "product", app.product.as_deref());
    set_opt_string(table, "hardware", app.hardware.as_deref());
    set_opt_string(table, "board", app.board.as_deref());
    set_opt_string(table, "soc_model", app.soc_model.as_deref());
    set_opt_string(table, "fingerprint", app.fingerprint.as_deref());
    set_opt_string(table, "build_id", app.build_id.as_deref());
    set_opt_string(table, "display_id", app.display_id.as_deref());
    set_opt_string(table, "incremental", app.incremental.as_deref());
    set_opt_string(table, "security_patch", app.security_patch.as_deref());
    set_opt_string(table, "characteristics", app.characteristics.as_deref());
    set_opt_string(table, "android_version", app.android_version.as_deref());
    set_opt_number_setting(table, "sdk_int", app.sdk_int);
    set_opt_integer(table, "dpi", app.dpi);
    set_custom_props(table, app.custom_props.as_ref());
    set_opt_bool(table, "force_denylist_unmount", app.force_denylist_unmount);
    match app.hide_maps.as_deref() {
        None => {
            table.remove("hide_maps");
        }
        Some(values) => set_string_array(table, "hide_maps", values),
    }
    set_opt_bool(table, "companion_resetprop", app.companion_resetprop);
    // 空串在 egress 语义里等价于「未设置」（契约 §2.4），按删键处理以保持幂等。
    set_opt_string(table, "egress", non_empty(app.egress.as_deref()));
}

/// egress 语义里空串等于「未设置」：写文档时按删除处理。
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn string_value(value: &str, decor_source: Option<&Value>) -> Value {
    let mut new_value = Value::from(value);
    if let Some(source) = decor_source
        && let Some(prefix) = source.decor().prefix()
    {
        new_value.decor_mut().set_prefix(prefix.clone());
    }
    new_value
}

/// 追加一个数组元素：复用最后一个元素的缩进（多行数组保持对齐）。
fn push_string(array: &mut Array, value: &str) {
    let decor_source = array.iter().last().cloned();
    array.push_formatted(string_value(value, decor_source.as_ref()));
}

/// 写入标量值，并把旧值上的 decor（行内注释、前后空白）搬到新值上。
///
/// 已存在的键走 `get_mut` 原位赋值：`Table::insert` 会用 `Key::fmt()` 重排 key 的
/// 原始写法（例如把 `"一加15"` 重新编码），破坏「键顺序与写法不变」的承诺。
fn set_value(table: &mut Table, key: &str, value: Value) {
    let mut item = Item::Value(value);
    if let Some(existing) = table.get_mut(key) {
        if let Some(new_value) = item.as_value_mut()
            && let Some(old) = existing.as_value()
        {
            *new_value.decor_mut() = old.decor().clone();
        }
        *existing = item;
    } else {
        table.insert(key, item);
    }
}

fn set_string(table: &mut Table, key: &str, value: &str) {
    set_value(table, key, Value::from(value));
}

fn set_opt_string(table: &mut Table, key: &str, value: Option<&str>) {
    match value {
        Some(value) => set_string(table, key, value),
        None => {
            table.remove(key);
        }
    }
}

fn set_opt_bool(table: &mut Table, key: &str, value: Option<bool>) {
    match value {
        Some(value) => set_value(table, key, Value::from(value)),
        None => {
            table.remove(key);
        }
    }
}

fn set_opt_integer(table: &mut Table, key: &str, value: Option<u32>) {
    match value {
        Some(value) => set_value(table, key, Value::from(i64::from(value))),
        None => {
            table.remove(key);
        }
    }
}

fn set_opt_number_setting(table: &mut Table, key: &str, value: Option<NumberSetting>) {
    match value {
        Some(NumberSetting::Value(value)) => {
            set_value(table, key, Value::from(i64::from(value)));
        }
        Some(NumberSetting::Delete) => set_string(table, key, DELETE_MARKER),
        None => {
            table.remove(key);
        }
    }
}

/// 整体替换字符串数组：语义一致时不动（避免格式抖动），否则按位置复用每个元素的缩进。
fn set_string_array(table: &mut Table, key: &str, values: &[String]) {
    if let Some(Item::Value(Value::Array(existing))) = table.get(key) {
        let same = existing.len() == values.len()
            && existing
                .iter()
                .zip(values)
                .all(|(item, value)| item.as_str() == Some(value.as_str()));
        if same {
            return;
        }
    }

    let existing = match table.get(key) {
        Some(Item::Value(Value::Array(array))) => Some(array.clone()),
        _ => None,
    };

    let mut array = Array::new();
    match &existing {
        Some(old) => {
            array.set_trailing(old.trailing().clone());
            array.set_trailing_comma(old.trailing_comma());
        }
        None => {
            if !values.is_empty() {
                array.set_trailing(" ");
            }
        }
    }
    for (index, value) in values.iter().enumerate() {
        let mut new_value = Value::from(value.as_str());
        // 优先复用同一位置元素的缩进；新增元素沿用最后一个元素的缩进（多行数组保持对齐）。
        let prefix = existing.as_ref().and_then(|old| {
            old.get(index)
                .or_else(|| old.iter().last())
                .and_then(|item| item.decor().prefix().cloned())
        });
        new_value
            .decor_mut()
            .set_prefix(prefix.unwrap_or_else(|| " ".into()));
        array.push_formatted(new_value);
    }

    set_value(table, key, Value::Array(array));
}

fn set_custom_props(table: &mut Table, props: Option<&HashMap<String, String>>) {
    let Some(props) = props else {
        table.remove("custom_props");
        return;
    };

    let mut entries: Vec<(&str, &str)> = props
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    // HashMap 迭代顺序不定，写文档时按 key 排序，保证输出可复现。
    entries.sort_unstable_by(|left, right| left.0.cmp(right.0));

    match table.get("custom_props") {
        Some(Item::Value(Value::InlineTable(_))) => {
            let Some(Item::Value(Value::InlineTable(inline))) = table.get_mut("custom_props")
            else {
                unreachable!("matched inline table above");
            };
            apply_inline_props(inline, &entries);
        }
        Some(Item::Table(_)) => {
            let Some(Item::Table(section)) = table.get_mut("custom_props") else {
                unreachable!("matched table above");
            };
            apply_section_props(section, &entries);
        }
        _ => {
            let mut inline = InlineTable::new();
            for (key, value) in &entries {
                inline.insert(*key, Value::from(*value));
            }
            set_value(table, "custom_props", Value::InlineTable(inline));
        }
    }
}

fn apply_inline_props(inline: &mut InlineTable, entries: &[(&str, &str)]) {
    let stale: Vec<String> = inline
        .iter()
        .map(|(key, _)| key.to_string())
        .filter(|key| !entries.iter().any(|(entry, _)| entry == key))
        .collect();
    for key in stale {
        inline.remove(&key);
    }

    for (key, value) in entries {
        let mut new_value = Value::from(*value);
        if let Some(existing) = inline.get_mut(key) {
            *new_value.decor_mut() = existing.decor().clone();
            *existing = new_value;
        } else {
            inline.insert(*key, new_value);
        }
    }
}

fn apply_section_props(section: &mut Table, entries: &[(&str, &str)]) {
    let stale: Vec<String> = section
        .iter()
        .map(|(key, _)| key.to_string())
        .filter(|key| !entries.iter().any(|(entry, _)| entry == key))
        .collect();
    for key in stale {
        section.remove(&key);
    }

    for (key, value) in entries {
        set_string(section, key, value);
    }
}
