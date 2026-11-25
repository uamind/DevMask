// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! egress 配置面回归（docs/plans/egress-contract.md §2.3 / §2.4 / §2.5 / §2.7）：
//! `[egress].default` 与 `[[apps]].egress` 的解析、序列化、校验与增量编辑。

use devmask_core::config::{
    AppConfig, Config, DIRECT_MARKER, EGRESS_PROXY_NAME_MAX, EGRESS_PROXY_NAME_PATTERN,
    EgressSection,
};
use devmask_core::edit::{
    EditError, bind_package, delete_app, parse_document, set_app, set_egress, upsert_template,
};
use devmask_core::schema::bundle;
use devmask_core::validate::{
    Severity, ValidationOptions, unknown_fields, validate_app, validate_config, validate_document,
    validate_egress_proxy_name,
};

const SAMPLE: &str = r#"# 顶部注释：必须保留
debug = true

[egress]
default = "us_la_01"    # 设备级默认

[templates.pixel]
# 模板注释
packages = [ "com.example.a", "com.example.b" ]
model = "Pixel 9"    # 机型
author = "someone"

[[apps]]
package = "com.example.a"
brand = "google"    # 应用注释
egress = "jp_tokyo_02"    # 应用级出口
extra_app_key = 1

[[apps]]
package = "com.example.b"
egress = "__DIRECT__"

# 未知顶层表
[cpu_presets.kirin]
name = "Kirin 9000"
"#;

fn sample_config() -> Config {
    Config::from_toml(SAMPLE).expect("fixture must parse")
}

fn codes(report: &devmask_core::validate::ValidationReport) -> Vec<&'static str> {
    report.issues.iter().map(|issue| issue.code).collect()
}

#[test]
fn parses_device_and_app_egress() {
    let config = sample_config();
    assert_eq!(
        config.egress,
        Some(EgressSection {
            default: Some("us_la_01".to_owned()),
        })
    );
    assert_eq!(config.apps.len(), 2);
    assert_eq!(config.apps[0].egress.as_deref(), Some("jp_tokyo_02"));
    assert_eq!(config.apps[1].egress.as_deref(), Some(DIRECT_MARKER));
    assert_eq!(DIRECT_MARKER, "__DIRECT__");
}

#[test]
fn toml_and_json_roundtrip_are_lossless() {
    let config = sample_config();

    // TOML 序列化 → 重新解析必须等价（egress 表与 app 字段都在）。
    let text = toml::to_string(&config).expect("serialize config to TOML");
    assert!(text.contains("[egress]"), "{text}");
    assert!(text.contains("default = \"us_la_01\""), "{text}");
    assert!(text.contains("egress = \"jp_tokyo_02\""), "{text}");
    let reread = Config::from_toml(&text).expect("serialized TOML must parse");
    assert_eq!(reread, config);

    // JSON 序列化（API 形态）：None 字段不出现，egress 保留。
    let json = serde_json::to_value(&config).expect("serialize config to JSON");
    assert_eq!(json["egress"]["default"], "us_la_01");
    assert_eq!(json["apps"][0]["egress"], "jp_tokyo_02");
    assert_eq!(json["apps"][1]["egress"], "__DIRECT__");
    let back: Config = serde_json::from_value(json).expect("deserialize config JSON");
    assert_eq!(back, config);
}

#[test]
fn rewriting_parsed_document_is_byte_identical() {
    let config = sample_config();
    let mut document = parse_document(SAMPLE).expect("parse fixture");

    let mut changed = set_egress(
        &mut document,
        config.egress.as_ref().expect("egress section"),
    )
    .expect("set egress");
    for app in &config.apps {
        changed |= set_app(&mut document, app).expect("set app");
    }
    for (name, template) in &config.templates {
        changed |= upsert_template(&mut document, name, template).expect("upsert template");
    }

    assert!(!changed, "把解析结果原样写回必须是无操作");
    assert_eq!(document.to_string(), SAMPLE);
    assert_eq!(
        Config::from_toml(&document.to_string()).expect("rewritten document must parse"),
        config
    );
}

#[test]
fn valid_names_and_direct_marker_produce_no_issues() {
    // SAMPLE 里 apps 与模板绑定了同一批包名，那是既有的 package 告警；egress 本身必须零 issue。
    let report = validate_config(&sample_config(), &ValidationOptions::default());
    assert!(report.is_ok(), "{:?}", report.issues);
    assert!(
        !codes(&report)
            .iter()
            .any(|code| code.starts_with("invalid_egress")),
        "{:?}",
        report.issues
    );

    let long_but_ok = "a".repeat(EGRESS_PROXY_NAME_MAX);
    for name in [
        "a",
        "_x",
        "US_LA_01",
        "a.b-c_9",
        "0start",
        long_but_ok.as_str(),
        DIRECT_MARKER,
        "",
    ] {
        let app = AppConfig {
            package: "com.example.x".to_owned(),
            egress: Some(name.to_owned()),
            ..Default::default()
        };
        let report = validate_app(&app);
        assert!(
            report.issues.is_empty(),
            "egress={name:?} 不应产生任何 issue：{:?}",
            report.issues
        );
    }

    // 设备级：普通代理名合法，空串 = 未设置。
    for default in ["us_la_01", "jp.tokyo-02", "", "A"] {
        let config = Config::from_toml(&format!("[egress]\ndefault = \"{default}\"\n"))
            .expect("device egress fixture must parse");
        let report = validate_config(&config, &ValidationOptions::default());
        assert!(
            report.issues.is_empty(),
            "default={default:?} 不应产生任何 issue：{:?}",
            report.issues
        );
    }

    // 没有 [egress]、没有 apps.egress 时同样零 issue。
    let bare = Config::from_toml("debug = true\n").unwrap();
    assert!(
        validate_config(&bare, &ValidationOptions::default())
            .issues
            .is_empty()
    );
}

#[test]
fn invalid_app_names_report_invalid_egress_name() {
    let long = "a".repeat(EGRESS_PROXY_NAME_MAX + 1);
    for bad in [
        "bad name",
        "-leading",
        ".dot",
        "a/b",
        "a:b",
        "a@b",
        "a\\b",
        "代理",
        "a\nb",
        "a b",
        long.as_str(),
    ] {
        let app = AppConfig {
            package: "com.example.x".to_owned(),
            egress: Some(bad.to_owned()),
            ..Default::default()
        };
        let report = validate_app(&app);
        assert_eq!(
            codes(&report),
            vec!["invalid_egress_name"],
            "egress={bad:?} 必须恰好产生一条 invalid_egress_name"
        );
        let issue = &report.issues[0];
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(issue.path, "apps.com.example.x.egress");
        assert!(
            issue.message.contains("__DIRECT__"),
            "错误信息要说明合法取值：{}",
            issue.message
        );
    }

    // __DIRECT__ 是保留值，永远不能当代理名（egress.toml 的 [proxies] 表名）。
    let reserved = validate_egress_proxy_name(DIRECT_MARKER).expect("保留值必须被拒绝");
    assert!(reserved.contains("保留值"), "{reserved}");
    assert!(validate_egress_proxy_name("us_la_01").is_none());

    // 文档级校验同样报 error（server 的 PUT /config 路径复用 validate_document）。
    let source = "[egress]\ndefault = \"bad name\"\n\n[[apps]]\npackage = \"com.example.x\"\negress = \"a/b\"\n";
    let document = parse_document(source).unwrap();
    let report = validate_document(&document, &ValidationOptions::default());
    assert_eq!(
        codes(&report),
        vec!["invalid_egress_name", "invalid_egress_name"],
        "{:?}",
        report.issues
    );
    assert_eq!(report.error_count(), 2);
}

#[test]
fn invalid_device_default_distinguishes_codes() {
    let long = "a".repeat(EGRESS_PROXY_NAME_MAX + 1);
    for bad in ["bad name", "-leading", "a/b", "代理", long.as_str()] {
        let config = Config::from_toml(&format!("[egress]\ndefault = \"{bad}\"\n"))
            .expect("device egress fixture must parse");
        let report = validate_config(&config, &ValidationOptions::default());
        assert_eq!(
            codes(&report),
            vec!["invalid_egress_name"],
            "default={bad:?}"
        );
        let issue = &report.issues[0];
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(issue.path, "egress.default");
    }

    // 设备级不接受 __DIRECT__：直连应当删键，码与「名字非法」区分（契约 §2.6）。
    let config = Config::from_toml("[egress]\ndefault = \"__DIRECT__\"\n").unwrap();
    let report = validate_config(&config, &ValidationOptions::default());
    assert_eq!(codes(&report), vec!["invalid_egress_default"]);
    assert_eq!(report.issues[0].severity, Severity::Error);
    assert_eq!(report.issues[0].path, "egress.default");
    assert!(report.issues[0].message.contains("删除"));
}

#[test]
fn set_app_writes_and_clears_egress_byte_exactly() {
    let mut app = sample_config().apps[0].clone();

    // 写：只改 brand 与 egress 两行，行内注释跟随，其它行逐字节不动。
    app.brand = Some("samsung".to_owned());
    app.egress = Some("new_proxy".to_owned());
    let mut document = parse_document(SAMPLE).unwrap();
    assert!(set_app(&mut document, &app).expect("set app"));
    let expected = SAMPLE
        .replace(
            "brand = \"google\"    # 应用注释",
            "brand = \"samsung\"    # 应用注释",
        )
        .replace(
            "egress = \"jp_tokyo_02\"    # 应用级出口",
            "egress = \"new_proxy\"    # 应用级出口",
        );
    assert_eq!(document.to_string(), expected);

    // 重复写入同一值：changed=false 且文档不动。
    let text = document.to_string();
    assert!(!set_app(&mut document, &app).expect("set app again"));
    assert_eq!(document.to_string(), text);

    // 清除：只删除 egress 行，其它注释/未知键逐字节保留。
    let mut cleared = app.clone();
    cleared.egress = None;
    let mut document = parse_document(SAMPLE).unwrap();
    let mut cleared_app = sample_config().apps[0].clone();
    cleared_app.egress = None;
    assert!(set_app(&mut document, &cleared_app).expect("clear egress"));
    let expected = SAMPLE.replace("egress = \"jp_tokyo_02\"    # 应用级出口\n", "");
    assert_eq!(document.to_string(), expected);
    let reread = Config::from_toml(&document.to_string()).expect("cleared document must parse");
    assert_eq!(reread.apps[0].egress, None);
    assert_eq!(
        reread.apps[1].egress.as_deref(),
        Some(DIRECT_MARKER),
        "清除一条应用不得影响另一条"
    );

    assert!(!set_app(&mut document, &cleared_app).expect("clear twice"));
    assert_eq!(document.to_string(), expected);

    // 空串 = 未设置：与删除键完全等价（也保持幂等）。
    let mut empty = sample_config().apps[0].clone();
    empty.egress = Some(String::new());
    let mut document = parse_document(SAMPLE).unwrap();
    assert!(set_app(&mut document, &empty).expect("empty egress removes key"));
    assert_eq!(document.to_string(), expected);
    let mut again = parse_document(&document.to_string()).unwrap();
    assert!(!set_app(&mut again, &empty).expect("empty egress is idempotent"));
    assert_eq!(again.to_string(), expected);
}

#[test]
fn set_egress_creates_updates_and_clears_the_section() {
    // 新建 [egress]：表头与其它段用空行隔开。
    let without_egress = SAMPLE.replace("[egress]\ndefault = \"us_la_01\"    # 设备级默认\n\n", "");
    let mut document = parse_document(&without_egress).unwrap();
    let section = EgressSection {
        default: Some("jp_tokyo_02".to_owned()),
    };
    assert!(set_egress(&mut document, &section).expect("create egress section"));
    let text = document.to_string();
    assert!(
        text.contains("\n[egress]\ndefault = \"jp_tokyo_02\"\n"),
        "{text}"
    );
    assert_eq!(
        Config::from_toml(&text).unwrap().egress,
        Some(section.clone())
    );

    // 幂等：同一值第二次无操作。
    let mut again = parse_document(&text).unwrap();
    assert!(!set_egress(&mut again, &section).expect("set same egress"));
    assert_eq!(again.to_string(), text);

    // 改值：行内注释跟随，其它内容不动。
    let changed = EgressSection {
        default: Some("jp_tokyo_02".to_owned()),
    };
    let mut document = parse_document(SAMPLE).unwrap();
    assert!(set_egress(&mut document, &changed).expect("update egress"));
    assert_eq!(
        document.to_string(),
        SAMPLE.replace("default = \"us_la_01\"", "default = \"jp_tokyo_02\"")
    );

    // 清除：只有 default 时整段删除。
    let mut document = parse_document(SAMPLE).unwrap();
    assert!(set_egress(&mut document, &EgressSection::default()).expect("clear egress"));
    let expected = SAMPLE.replace("[egress]\ndefault = \"us_la_01\"    # 设备级默认\n\n", "");
    assert_eq!(document.to_string(), expected);
    let mut again = parse_document(&document.to_string()).unwrap();
    assert!(!set_egress(&mut again, &EgressSection::default()).expect("clear twice"));
    assert_eq!(again.to_string(), expected);

    // 没有 [egress] 时清除：无操作。
    let mut document = parse_document(&without_egress).unwrap();
    assert!(!set_egress(&mut document, &EgressSection::default()).expect("nothing to clear"));
    assert_eq!(document.to_string(), without_egress);
}

#[test]
fn set_egress_keeps_unknown_keys_and_comments() {
    let source = r#"# 文件注释
[egress]
default = "us_la_01"
proxy_host = "203.0.113.10"    # 手工写进去的未知键

[[apps]]
package = "com.example.a"
"#;
    let mut document = parse_document(source).unwrap();
    assert!(
        set_egress(&mut document, &EgressSection::default()).expect("clear default"),
        "删除 default 必须算改动"
    );
    let expected = source.replace("default = \"us_la_01\"\n", "");
    assert_eq!(document.to_string(), expected);
    assert!(
        document.to_string().contains("[egress]"),
        "还有未知键，段必须保留"
    );
    assert!(
        document
            .to_string()
            .contains("proxy_host = \"203.0.113.10\"    # 手工写进去的未知键")
    );
    let config = Config::from_toml(&document.to_string()).unwrap();
    assert_eq!(config.egress, Some(EgressSection { default: None }));
}

#[test]
fn other_edits_keep_egress_intact() {
    let mut document = parse_document(SAMPLE).unwrap();
    let mut template = sample_config().templates["pixel"].clone();
    template.model = Some("Pixel 10".to_owned());
    assert!(upsert_template(&mut document, "pixel", &template).expect("upsert template"));
    assert!(bind_package(&mut document, "pixel", "com.example.c").expect("bind package"));
    assert!(delete_app(&mut document, "com.example.b").expect("delete app"));

    let text = document.to_string();
    assert!(text.contains("[egress]\ndefault = \"us_la_01\"    # 设备级默认"));
    assert!(text.contains("egress = \"jp_tokyo_02\"    # 应用级出口"));
    assert!(text.contains("# 顶部注释：必须保留"));
    assert!(text.contains("author = \"someone\""));
    assert!(text.contains("[cpu_presets.kirin]"));

    let config = Config::from_toml(&text).expect("edited document must parse");
    assert_eq!(
        config.egress,
        Some(EgressSection {
            default: Some("us_la_01".to_owned()),
        })
    );
    assert_eq!(config.apps.len(), 1);
    assert_eq!(config.apps[0].egress.as_deref(), Some("jp_tokyo_02"));
}

#[test]
fn egress_keys_are_known_and_unknown_keys_are_reported() {
    // 合法的 egress 配置不得被当成未知字段。
    let document = parse_document(SAMPLE).unwrap();
    let fields = unknown_fields(&document);
    assert!(
        !fields
            .iter()
            .any(|field| field.key == "egress" || field.key == "default"),
        "{fields:?}"
    );
    assert!(
        fields
            .iter()
            .any(|field| { field.path == "apps[0]" && field.key == "extra_app_key" })
    );

    let report = validate_document(&document, &ValidationOptions::default());
    assert!(
        report.is_ok(),
        "egress 配置不得产生 error：{:?}",
        report.errors().collect::<Vec<_>>()
    );
    assert!(
        !report
            .issues
            .iter()
            .any(|issue| issue.code == "config.unknown_field" && issue.path == "egress"),
        "合法的 [egress].default 不是未知字段：{:?}",
        report.issues
    );

    // 但 [egress] 里的凭据键必须被看见（server 会升级成 422）。
    let source = r#"[egress]
default = "us_la_01"
type = "http"
host = "203.0.113.10"
port = 1080
username = "user"
password = "secret"

[[apps]]
package = "com.example.a"
"#;
    let document = parse_document(source).unwrap();
    let fields = unknown_fields(&document);
    for credential in ["type", "host", "port", "username", "password"] {
        assert!(
            fields
                .iter()
                .any(|field| field.path == "egress" && field.key == credential),
            "{credential} 必须被当成未知键：{fields:?}"
        );
    }
    let report = validate_document(&document, &ValidationOptions::default());
    assert!(report.is_ok(), "未知字段是 warning，不阻断");
    let egress_issues: Vec<_> = report
        .issues
        .iter()
        .filter(|issue| issue.code == "config.unknown_field" && issue.path == "egress")
        .collect();
    assert_eq!(egress_issues.len(), 5, "{egress_issues:?}");
    assert!(
        egress_issues
            .iter()
            .all(|issue| issue.severity == Severity::Warning),
        "未知字段只告警，不阻断"
    );
    for credential in ["type", "host", "port", "username", "password"] {
        assert!(
            egress_issues
                .iter()
                .any(|issue| issue.message.contains(credential)),
            "{credential} 必须在 warning 里出现：{egress_issues:?}"
        );
    }

    // 模板不支持 egress（契约 §2.4）：词法上仍是未知字段，写路径原样保留。
    let document = parse_document("[templates.x]\negress = \"us_la_01\"\n").unwrap();
    let fields = unknown_fields(&document);
    assert!(
        fields
            .iter()
            .any(|field| field.path == "templates.x" && field.key == "egress"),
        "{fields:?}"
    );
}

#[test]
fn none_empty_and_direct_are_distinct() {
    // 模型层保留三种状态：缺键 = None、空串 = Some("")、__DIRECT__ = 独立取值。
    // 前两者在 core 语义里都表示「未设置」（继承设备级），后者是主动直连（契约 §2.4）。
    let source = "[[apps]]\npackage = \"com.example.none\"\n\n[[apps]]\npackage = \"com.example.empty\"\negress = \"\"\n\n[[apps]]\npackage = \"com.example.direct\"\negress = \"__DIRECT__\"\n";
    let config = Config::from_toml(source).unwrap();
    assert_eq!(config.apps[0].egress, None);
    assert_eq!(config.apps[1].egress.as_deref(), Some(""));
    assert_eq!(config.apps[2].egress.as_deref(), Some(DIRECT_MARKER));

    let report = validate_config(&config, &ValidationOptions::default());
    assert!(report.issues.is_empty(), "{:?}", report.issues);

    // 序列化不丢信息：缺键不出现，空串与 __DIRECT__ 原样保留。
    let json = serde_json::to_value(&config).unwrap();
    assert!(json["apps"][0].get("egress").is_none());
    assert_eq!(json["apps"][1]["egress"], "");
    assert_eq!(json["apps"][2]["egress"], "__DIRECT__");

    // 写路径把空串归一化成删键（与 None 等价），__DIRECT__ 原样写入。
    let mut document = parse_document(source).unwrap();
    let mut empty = config.apps[1].clone();
    empty.egress = None;
    assert!(set_app(&mut document, &empty).expect("clearing empty string"));
    let text = document.to_string();
    assert!(!text.contains("egress = \"\""), "{text}");
    assert_eq!(Config::from_toml(&text).unwrap().apps[1].egress, None);
    assert!(
        text.contains("egress = \"__DIRECT__\""),
        "__DIRECT__ 必须原样保留：{text}"
    );

    let mut again = parse_document(&text).unwrap();
    let mut empty_again = config.apps[1].clone();
    empty_again.egress = Some(String::new());
    assert!(
        !set_app(&mut again, &empty_again).expect("empty equals unset"),
        "空串写在缺键的文档上必须是无操作"
    );
}

#[test]
fn set_egress_rejects_non_table_values() {
    let mut document = parse_document("egress = 1\n").unwrap();
    let error = set_egress(&mut document, &EgressSection::default()).unwrap_err();
    assert!(matches!(error, EditError::Structure { .. }), "{error:?}");
    assert_eq!(document.to_string(), "egress = 1\n", "报错时不得改动文档");
}

#[test]
fn schema_bundle_exposes_egress() {
    let bundle = bundle();

    let config = &bundle["definitions"]["config"];
    let config_egress = &config["properties"]["egress"];
    assert!(
        config_egress.is_object(),
        "config schema 缺少 egress：{config_egress}"
    );
    assert!(
        config_egress.to_string().contains("EgressSection"),
        "{config_egress}"
    );

    let section = &config["definitions"]["EgressSection"];
    assert!(section["properties"].get("default").is_some(), "{section}");
    assert_eq!(
        section["additionalProperties"], false,
        "[egress] 只允许 default，schema 必须封闭"
    );
    assert_eq!(
        section["properties"].as_object().map(serde_json::Map::len),
        Some(1),
        "EgressSection 只能有 default 一个属性：{section}"
    );
    let default = &section["properties"]["default"];
    let default_types: Vec<&str> = default["type"]
        .as_array()
        .expect("default.type 必须是数组")
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert!(default_types.contains(&"string"), "{default}");
    assert!(default_types.contains(&"null"), "{default}");
    assert_eq!(default["pattern"], EGRESS_PROXY_NAME_PATTERN);
    assert_eq!(default["not"]["const"], DIRECT_MARKER);

    let app = &bundle["definitions"]["app"];
    let app_egress = &app["properties"]["egress"];
    assert!(
        app_egress.is_object(),
        "app schema 缺少 egress：{app_egress}"
    );
    let app_types: Vec<&str> = app_egress["type"]
        .as_array()
        .expect("app.egress.type 必须是数组")
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert!(app_types.contains(&"string"), "{app_egress}");
    assert!(app_types.contains(&"null"), "{app_egress}");
    let pattern = app_egress["pattern"].as_str().expect("app egress pattern");
    assert!(pattern.contains(DIRECT_MARKER), "{pattern}");
    assert!(pattern.contains("[A-Za-z0-9_.-]{0,63}"), "{pattern}");

    // 模板不支持 egress（契约 §2.4）：template schema 里不得出现该字段。
    let template = &bundle["definitions"]["template"];
    assert!(
        template["properties"].get("egress").is_none(),
        "DeviceTemplate 不支持 egress：{}",
        template["properties"]
    );

    // egress 可省略：不出现在 required 里。
    assert!(
        config["required"]
            .as_array()
            .is_none_or(|required| !required.iter().any(|value| value == "egress")),
        "egress 必须是可选字段"
    );
    assert!(
        app["required"]
            .as_array()
            .is_none_or(|required| !required.iter().any(|value| value == "egress")),
        "AppConfig.egress 必须是可选字段"
    );
}
