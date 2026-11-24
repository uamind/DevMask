// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! unified diff（写端点的 `diff=true` / `dry_run=true`）。
//!
//! 只做「公共前缀 + 公共后缀 + 一个 hunk（3 行上下文）」的简化实现：
//! 对 config.toml 这种「少量定点修改」的场景输出与 `diff -u` 等价的可读结果，
//! 又不需要引入 diff 依赖。结果上限 64 KiB（docs/API.md 2.6）。

/// diff 上限（字节）。
pub const DIFF_LIMIT: usize = 64 * 1024;
/// 上下文行数。
const CONTEXT: usize = 3;

/// 生成 `--- a/<name>` / `+++ b/<name>` 形式的 unified diff；内容相同返回空串。
pub fn unified_diff(name: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let old_lines: Vec<&str> = split_lines(old);
    let new_lines: Vec<&str> = split_lines(new);

    let prefix = old_lines
        .iter()
        .zip(new_lines.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let max_suffix = old_lines.len().min(new_lines.len()).saturating_sub(prefix);
    let suffix = old_lines
        .iter()
        .rev()
        .zip(new_lines.iter().rev())
        .take(max_suffix)
        .take_while(|(left, right)| left == right)
        .count();

    let old_change_start = prefix;
    let old_change_end = old_lines.len() - suffix;
    let new_change_end = new_lines.len() - suffix;

    let context_start = old_change_start.saturating_sub(CONTEXT);
    let old_end = (old_change_end + CONTEXT).min(old_lines.len());
    let new_end = (new_change_end + CONTEXT).min(new_lines.len());

    let mut out = String::new();
    out.push_str(&format!("--- a/{name}\n+++ b/{name}\n"));
    out.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        context_start + 1,
        old_end - context_start,
        context_start + 1,
        new_end - context_start,
    ));
    for line in &old_lines[context_start..old_change_start] {
        out.push(' ');
        out.push_str(line);
        out.push('\n');
    }
    for line in &old_lines[old_change_start..old_change_end] {
        out.push('-');
        out.push_str(line);
        out.push('\n');
    }
    for line in &new_lines[prefix..new_change_end] {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    for line in &old_lines[old_change_end..old_end] {
        out.push(' ');
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// 截断到 [`DIFF_LIMIT`]，返回 (文本, 是否截断)。
pub fn cap(mut diff: String) -> (String, bool) {
    if diff.len() <= DIFF_LIMIT {
        return (diff, false);
    }
    let mut end = DIFF_LIMIT;
    while end > 0 && !diff.is_char_boundary(end) {
        end -= 1;
    }
    diff.truncate(end);
    diff.push_str("\n… diff truncated …\n");
    (diff, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_has_no_diff() {
        assert_eq!(unified_diff("config.toml", "a = 1\n", "a = 1\n"), "");
    }

    #[test]
    fn single_line_change_has_hunk_header() {
        let old = "debug = false\n\n[templates.a]\nmodel = \"A\"\n";
        let new = "debug = false\n\n[templates.a]\nmodel = \"B\"\n";
        let diff = unified_diff("config.toml", old, new);
        assert!(
            diff.starts_with("--- a/config.toml\n+++ b/config.toml\n@@ "),
            "{diff}"
        );
        assert!(diff.contains("-model = \"A\""), "{diff}");
        assert!(diff.contains("+model = \"B\""), "{diff}");
        assert!(diff.contains(" [templates.a]"), "{diff}");
    }

    #[test]
    fn cap_marks_truncation() {
        let big = "x".repeat(DIFF_LIMIT + 10);
        let (capped, truncated) = cap(big);
        assert!(truncated);
        assert!(capped.len() <= DIFF_LIMIT + 32);
        assert!(capped.ends_with("… diff truncated …\n"));
    }
}
