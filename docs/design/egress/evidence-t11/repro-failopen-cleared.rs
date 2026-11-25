// t11 评审复现：fail-open 的「清直连」用 CommandOutcome::Ok 当成功信号，读回不一致也算成功。
//
// 复用真实实现 server/src/egress/system.rs（#[path]），用一个「put 返回 0 但不生效」的
// CommandRunner（等价于 SELinux/权限层面写不动、或代理被 VPN/其它组件重新写回）驱动
// SystemProxy::apply / clear，检查它们返回的 Ok 与 matches 是否会互相矛盾。
//
//   rustc --edition 2021 -o /tmp/repro-failopen docs/design/egress/evidence-t11/repro-failopen-cleared.rs && /tmp/repro-failopen

mod commands {
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

/// settings put 恒返回 0，但状态不变（proxy 永远是 127.0.0.1:18080）。
#[derive(Debug)]
struct StuckRunner;

impl commands::CommandRunner for StuckRunner {
    fn run(&self, program: &str, args: &[&str]) -> commands::CommandOutcome {
        match (program, args) {
            ("settings", ["get", "global", "http_proxy"]) => {
                commands::CommandOutcome::Ok("127.0.0.1:18080".to_owned())
            }
            ("settings", ["get", "global", "global_http_proxy_host"]) => {
                commands::CommandOutcome::Ok("127.0.0.1".to_owned())
            }
            ("settings", ["get", "global", "global_http_proxy_exclusion_list"]) => {
                commands::CommandOutcome::Ok("null".to_owned())
            }
            ("dumpsys", ["connectivity"]) => commands::CommandOutcome::Ok(
                "ConnectivityService:\n  Global proxy: [127.0.0.1] 18080 xl=\n".to_owned(),
            ),
            ("settings", ["put", ..]) => commands::CommandOutcome::Ok(String::new()),
            _ => commands::CommandOutcome::Unavailable {
                message: "unsimulated".to_owned(),
            },
        }
    }
}

fn main() {
    let proxy = system::SystemProxy::new(&StuckRunner);
    let outcome = proxy.apply(&system::Expected::Direct, Some(&[])).unwrap();
    println!(
        "clear() 的返回值: Ok(ApplyOutcome {{ write_count: {}, matches: {} }})",
        outcome.write_count, outcome.matches
    );
    println!(
        "degrade_to_direct 只看 is_ok() -> cleared = {}",
        proxy.apply(&system::Expected::Direct, Some(&[])).is_ok()
    );
    println!("而读回仍然是: {:?}", outcome.readback.settings_value);
    println!("=> 服务会把 health.degraded 置 true、effectiveMode 报 direct；设备实际仍走 127.0.0.1:18080");
}
