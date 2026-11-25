// t11 评审复现：parse_dumpsys_state 对真机原文恒返回 Unknown。
//
// 复用**真实实现** server/src/egress/system.rs（#[path] 直接引用源码，不复制解析逻辑），
// 只给它补上 system.rs 需要的最小 crate 依赖（commands / util）。
//
//   rustc --edition 2021 -o /tmp/repro-dumpsys docs/design/egress/evidence-t11/repro-dumpsys-parse.rs \
//     && /tmp/repro-dumpsys

mod commands {
    //! system.rs 只用到这三个符号；签名逐字对齐 server/src/commands.rs。

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CommandOutcome {
        Ok(String),
        Failed { code: Option<i32>, stderr: String },
        Unavailable { message: String },
    }

    pub trait CommandRunner: Send + Sync + std::fmt::Debug {
        fn run(&self, program: &str, args: &[&str]) -> CommandOutcome;
    }

    pub const SETTINGS: &str = "settings";
    pub const DUMPSYS: &str = "dumpsys";

    pub fn settings_put(r: &dyn CommandRunner, ns: &str, key: &str, v: &str) -> CommandOutcome {
        r.run(SETTINGS, &["put", ns, key, v])
    }
    pub fn settings_get(r: &dyn CommandRunner, ns: &str, key: &str) -> CommandOutcome {
        r.run(SETTINGS, &["get", ns, key])
    }
    pub fn dumpsys(r: &dyn CommandRunner, service: &str) -> CommandOutcome {
        r.run(DUMPSYS, &[service])
    }
}

mod util {
    pub fn now_rfc3339() -> String {
        "2026-11-24T00:00:00Z".to_owned()
    }
}

#[path = "../../../../server/src/egress/system.rs"]
mod system;

fn main() {
    // 三份原文：契约 §3.3 的示例格式、Pixel 9 / Android 17 的真机输出（t9 证据），以及 none。
    let cases = [
        (
            "契约 §3.3 示例（测试里用的假格式）",
            "Global proxy: [203.0.113.10] port: 1080",
        ),
        (
            "Pixel 9 / Android 17 真机（evidence/t1-put-http-proxy.log:75）",
            "Global proxy: [127.0.0.1] 18080 xl=",
        ),
        ("直连（真机）", "Global proxy: none"),
    ];
    for (label, line) in cases {
        let parsed = system::parse_dumpsys_state(Some(line));
        println!("{label}\n  input  = {line}\n  parsed = {parsed:?}");
    }

    // 判定真值表第 4 行的关键场景：主值看起来是直连（:0），但平台仍报有代理。
    // 真机上 parse 得到 Unknown → matches(Expected::Direct) 仍然为 true。
    let readback = system::Readback {
        settings_value: Some(":0".to_owned()),
        dumpsys_global_proxy: Some("Global proxy: [127.0.0.1] 18080 xl=".to_owned()),
        dirty: false,
        at: util::now_rfc3339(),
        available: true,
    };
    println!(
        "\nmatches(Expected::Direct) = {}   // 真代理仍在生效，却判为「已直连」",
        system::matches(&system::Expected::Direct, &readback)
    );
}
