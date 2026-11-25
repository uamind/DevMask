// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! PackageManager 查询：`pm list packages` + `pm list users` + `dumpsys package` 解析。
//!
//! - 一次构建整份索引（包名 / 每用户安装情况 / 是否系统应用 / versionCode），缓存 60s；
//! - `versionName` 只在显式按包名查询（`names=`，≤16 个）时用 `dumpsys package` 补齐，
//!   避免为几百个包各起一个 dumpsys；
//! - `pm` 不可用（host / 受限环境）时索引标记 unavailable，端点返回 `source=unavailable`，
//!   不把它当成 500。

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::commands::{CommandOutcome, CommandRunner};

/// 索引缓存时长（docs/API.md 5.20）。
pub const CACHE_TTL: Duration = Duration::from_secs(60);
/// 单次请求最多用 dumpsys 补 versionName 的包数。
pub const MAX_DUMPSYS: usize = 16;

/// 一个包的索引信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageEntry {
    /// 已安装的用户 id（升序）。
    pub user_ids: Vec<u32>,
    /// 是否系统应用。
    pub system: bool,
    /// versionCode（`pm list packages --show-versioncode`）。
    pub version_code: Option<u64>,
}

/// Android 用户。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UserEntry {
    /// 用户 id。
    pub id: u32,
    /// 用户显示名。
    pub name: String,
    /// 是否 running。
    pub running: bool,
}

/// 全量索引。
#[derive(Debug, Clone, Default)]
pub struct PackageIndex {
    /// `pm` 是否可用。
    pub available: bool,
    /// 包名 → 元数据（按包名排序）。
    pub packages: BTreeMap<String, PackageEntry>,
    /// 用户列表。
    pub users: Vec<UserEntry>,
    /// 当前用户。
    pub current_user: Option<u32>,
}

struct Cached {
    at: Instant,
    index: Arc<PackageIndex>,
}

/// 包查询服务（带缓存）。
pub struct PackageService {
    runner: Arc<dyn CommandRunner>,
    ttl: Duration,
    cache: Mutex<Option<Cached>>,
}

impl std::fmt::Debug for PackageService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PackageService")
            .field("ttl", &self.ttl)
            .finish()
    }
}

impl PackageService {
    /// 用指定执行器构造（默认缓存 60s）。
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            runner,
            ttl: CACHE_TTL,
            cache: Mutex::new(None),
        }
    }

    /// 取索引（命中缓存且未过期时直接返回）。
    pub fn index(&self) -> Arc<PackageIndex> {
        let mut guard = self.cache.lock().expect("package cache poisoned");
        if let Some(cached) = guard.as_ref()
            && cached.at.elapsed() < self.ttl
        {
            return cached.index.clone();
        }
        let index = Arc::new(self.build());
        *guard = Some(Cached {
            at: Instant::now(),
            index: index.clone(),
        });
        index
    }

    /// 清空缓存（测试与显式刷新用）。
    pub fn invalidate(&self) {
        *self.cache.lock().expect("package cache poisoned") = None;
    }

    /// 是否已安装；`pm` 不可用时返回 None。
    pub fn is_installed(&self, package: &str) -> Option<bool> {
        let index = self.index();
        if !index.available {
            return None;
        }
        Some(index.packages.contains_key(base_package(package)))
    }

    /// `dumpsys package <pkg>` 里的 versionName（best effort）。
    pub fn version_name(&self, package: &str) -> Option<String> {
        let outcome = self.runner.run("dumpsys", &["package", package]);
        let stdout = outcome.stdout()?;
        parse_version_name(stdout)
    }

    fn build(&self) -> PackageIndex {
        let users = self.list_users();
        let current_user = self.current_user();
        let mut index = PackageIndex {
            available: false,
            packages: BTreeMap::new(),
            users: users.clone(),
            current_user,
        };

        let Some(listing) = self
            .list_packages(&["list", "packages", "-u", "--show-versioncode"])
            .or_else(|| self.list_packages(&["list", "packages", "-u"]))
        else {
            return index;
        };
        index.available = true;

        let system: HashSet<String> = self
            .list_packages(&["list", "packages", "-s"])
            .map(|entries| entries.into_iter().map(|(package, _)| package).collect())
            .unwrap_or_default();

        let mut per_user: Vec<(u32, HashSet<String>)> = Vec::new();
        for user in &users {
            let id = user.id.to_string();
            if let Some(entries) = self.list_packages(&["list", "packages", "--user", &id]) {
                per_user.push((
                    user.id,
                    entries.into_iter().map(|(package, _)| package).collect(),
                ));
            }
        }

        for (package, version_code) in listing {
            let mut user_ids = Vec::new();
            if per_user.is_empty() {
                user_ids.push(current_user.unwrap_or(0));
            } else {
                for (id, installed) in &per_user {
                    if installed.contains(&package) {
                        user_ids.push(*id);
                    }
                }
            }
            user_ids.sort_unstable();
            index.packages.insert(
                package.clone(),
                PackageEntry {
                    user_ids,
                    system: system.contains(&package),
                    version_code,
                },
            );
        }
        index
    }

    fn list_packages(&self, args: &[&str]) -> Option<Vec<(String, Option<u64>)>> {
        match self.runner.run("pm", args) {
            CommandOutcome::Ok(stdout) => Some(parse_package_list(&stdout)),
            _ => None,
        }
    }

    fn list_users(&self) -> Vec<UserEntry> {
        match self.runner.run("pm", &["list", "users"]) {
            CommandOutcome::Ok(stdout) => parse_users(&stdout),
            _ => Vec::new(),
        }
    }

    fn current_user(&self) -> Option<u32> {
        match self.runner.run("am", &["get-current-user"]) {
            CommandOutcome::Ok(stdout) => stdout.trim().parse::<u32>().ok(),
            _ => None,
        }
    }
}

/// 去掉 `pkg@userId` 的 `@` 后缀。
pub fn base_package(package: &str) -> &str {
    package.split_once('@').map_or(package, |(base, _)| base)
}

/// 解析 `pkg@userId` → (base, Some(user))；非法 user 后缀返回 (package, None)。
pub fn split_user(package: &str) -> (&str, Option<u32>) {
    match package.split_once('@') {
        Some((base, user)) => match user.parse::<u32>() {
            Ok(user) => (base, Some(user)),
            Err(_) => (package, None),
        },
        None => (package, None),
    }
}

fn parse_package_list(stdout: &str) -> Vec<(String, Option<u64>)> {
    let mut packages = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("package:") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let Some(package) = parts.next() else {
            continue;
        };
        let version_code = parts
            .find_map(|part| part.strip_prefix("versionCode:"))
            .and_then(|value| value.parse::<u64>().ok());
        packages.push((package.to_owned(), version_code));
    }
    packages.sort();
    packages
}

fn parse_users(stdout: &str) -> Vec<UserEntry> {
    let mut users = Vec::new();
    for line in stdout.lines() {
        let Some(start) = line.find("UserInfo{") else {
            continue;
        };
        let rest = &line[start + "UserInfo{".len()..];
        let Some(end) = rest.find('}') else {
            continue;
        };
        let body = &rest[..end];
        let mut fields = body.splitn(3, ':');
        let Some(id) = fields
            .next()
            .and_then(|value| value.trim().parse::<u32>().ok())
        else {
            continue;
        };
        let name = fields.next().unwrap_or_default().trim().to_owned();
        users.push(UserEntry {
            id,
            name,
            running: line.contains("running"),
        });
    }
    users.sort_by_key(|user| user.id);
    users
}

fn parse_version_name(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("versionName=") else {
            continue;
        };
        let value = rest.split_whitespace().next().unwrap_or(rest).trim();
        if !value.is_empty() && value != "null" {
            return Some(value.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_list_with_version_code() {
        let stdout = "package:com.android.chrome versionCode:677820000\npackage:com.example.app\n";
        assert_eq!(
            parse_package_list(stdout),
            vec![
                ("com.android.chrome".to_owned(), Some(677_820_000)),
                ("com.example.app".to_owned(), None),
            ]
        );
    }

    #[test]
    fn parses_users_with_running_flag() {
        let stdout =
            "Users:\n\tUserInfo{0:Owner:13} running\n\tUserInfo{999:Work profile:30} running\n";
        let users = parse_users(stdout);
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].id, 0);
        assert_eq!(users[0].name, "Owner");
        assert!(users[0].running);
        assert_eq!(users[1].id, 999);
        assert_eq!(users[1].name, "Work profile");
    }

    #[test]
    fn parses_version_name_from_dumpsys() {
        let stdout = "Packages:\n  Package [com.example.app] (abc):\n    versionCode=420 minSdk=29 targetSdk=34\n    versionName=1.2.3\n";
        assert_eq!(parse_version_name(stdout), Some("1.2.3".to_owned()));
        assert_eq!(parse_version_name("versionName=null\n"), None);
    }

    #[test]
    fn split_user_handles_suffixes() {
        assert_eq!(
            split_user("com.example.app@999"),
            ("com.example.app", Some(999))
        );
        assert_eq!(split_user("com.example.app"), ("com.example.app", None));
        assert_eq!(split_user("com.example.app@x"), ("com.example.app@x", None));
    }

    #[test]
    fn index_reports_unavailable_without_pm() {
        let service = PackageService::new(Arc::new(crate::commands::FixtureRunner::new()));
        let index = service.index();
        assert!(!index.available);
        assert_eq!(service.is_installed("com.example.app"), None);
    }
}
