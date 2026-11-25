// t11 评审复现：PUT /egress?dry_run=true|diff=true 的 egress.toml diff 会带出密码。
//
// 复现方式：直接复用被评审的真实实现 server/src/diff.rs（不复制、不改写），
// 只把 api/egress.rs::combined_diff 的两段拼装照抄一遍（config 段 + egress 段）。
//
//   rustc --edition 2021 -o /tmp/repro docs/design/egress/evidence-t11/repro-diff-credential.rs && /tmp/repro
//
// 输入文本就是 server/src/egress/file.rs::render() 会写进 egress.toml 的形态
// （同一个仓库里的 file.rs 测试 store_writes_0600_and_backs_up_with_same_mode /
// render_round_trips_and_preserves_unknown_keys 断言了 password = "..." 会原样出现）。

#[path = "../../../../server/src/diff.rs"]
mod diff;

fn main() {
    // 旧 egress.toml（含已存密码）
    let old = "version = 1\n\
\n\
[proxies.us_la_01]\n\
type = \"http\"\n\
host = \"203.0.113.10\"\n\
port = 1080\n\
username = \"alice\"\n\
password = \"s3cret\"\n\
\n\
[probe]\n\
url = \"http://checkip.amazonaws.com/\"\n\
timeout_seconds = 5\n\
fail_open = true\n";

    // 场景 A：只改 probeUrl（其余照旧）——最普通的 dry_run 预览。
    let new_a = old.replace(
        "http://checkip.amazonaws.com/",
        "http://checkip.example.com/",
    );

    // 场景 B：轮换密码——diff 的语义本来就是这个。
    let new_b = old.replace("password = \"s3cret\"", "password = \"n3wpass\"");

    for (label, new) in [("A: 只改 probeUrl（dry_run 预览）", &new_a), ("B: 轮换密码", &new_b)] {
        // api/egress.rs::combined_diff 会对 egress.toml 也做一次 unified_diff 并拼接
        let egress_diff = diff::unified_diff("egress.toml", &old, new);
        println!("=== 场景 {label} ===");
        if egress_diff.is_empty() {
            println!("(egress.toml 无变化，diff 为空)");
        } else {
            print!("{egress_diff}");
        }
        println!(
            "-> 泄漏 s3cret: {} / 泄漏 n3wpass: {} / 泄漏 alice: {}",
            egress_diff.contains("s3cret"),
            egress_diff.contains("n3wpass"),
            egress_diff.contains("alice"),
        );
        println!();
    }
}
