// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

mod zip_ext;

use std::{
    fs::{self},
    path::{Path, PathBuf},
    process::{self, Command},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use fs_extra::{dir, file};
use time::{OffsetDateTime, UtcOffset};
use zip::{CompressionMethod, DateTime, write::FileOptions};
use zip_ext::zip_create_from_directory_with_options;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Check the build of devmask
    Check {
        /// Build in release mode (default: false)
        #[clap(short, long, default_value = "false")]
        release: bool,

        /// Print detailed output (default: false)
        #[clap(short, long, default_value = "false")]
        verbose: bool,
    },

    /// Build devmask and devmask_cli
    Build {
        /// Build in release mode (default: false)
        #[clap(short, long, default_value = "false")]
        release: bool,

        /// Print detailed output (default: false)
        #[clap(short, long, default_value = "false")]
        verbose: bool,
    },

    /// Clean build artifacts
    Clean,

    /// Format source code
    Format {
        /// Print detailed output (default: false)
        #[clap(short, long, default_value = "false")]
        verbose: bool,
    },

    /// Run the Clippy linter
    Lint {
        /// Automatically fix lint issues (default: false)
        #[clap(short, long, default_value = "false")]
        fix: bool,
    },

    /// Update project dependencies
    Update,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let Some(command) = cli.command else {
        eprintln!("No command specified. Use --help to see available commands.");
        process::exit(1);
    };

    match command {
        Commands::Check { release, verbose } => {
            check(release, verbose)?;
        }
        Commands::Build { release, verbose } => {
            build(release, verbose)?;
        }
        Commands::Clean => {
            clean()?;
        }
        Commands::Format { verbose } => {
            format(verbose)?;
        }
        Commands::Lint { fix } => {
            lint(fix)?;
        }
        Commands::Update => {
            update()?;
        }
    }

    Ok(())
}

/// 跑外部命令并在**非零退出码**时直接失败。
///
/// `Command::spawn()?.wait()?` 只能发现「进程起不来」；命令自己跑挂了（退出码非 0）
/// 会被静默忽略。2026-10-30 的 CI（run 41123564715）就是这样：`cargo ndk build` 因
/// `error[E0425]: cannot find function build_ident` 失败，退出码被丢掉，23 秒后才在
/// `file::copy(...).unwrap()` 处 panic——日志里只剩那句 unwrap，真正的 rustc 报错被
/// 埋在几十行 vite 输出之前，很容易误判成环境问题。
fn run_command(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to spawn {what}"))?;
    if !status.success() {
        bail!("{what} failed ({status})");
    }
    Ok(())
}

fn build(release: bool, verbose: bool) -> Result<()> {
    let temp_dir = temp_dir(release);

    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir)?;

    let mut cargo = cargo_ndk();
    let args = vec!["build", "--target", "aarch64-linux-android"];
    cargo.args(args);
    if release {
        cargo.arg("--release");
    }

    if verbose {
        cargo.arg("--verbose");
    }

    run_command(&mut cargo, "cargo ndk build")?;
    // TODO(remove-cli): devmask_cli 已不随模块打包（WebUI 的「ZIP/属性 → 模板」转换
    // 已在 utils/propsToTemplate.ts 纯前端实现，模块运行期也不需要它）。仍保留 crate 作为
    // 开发/排障用的命令行工具（`cargo run -p devmask_cli -- ...`）。若确认无人再用，
    // 可整 crate 删除并一并清掉本文件的 cli_bin_path()、AGENTS.md 的 CLI 说明。

    // 原生 Web 服务：与 zygisk 打进同一个模块包（bin/devmask_server），
    // 由 module/service.sh 在开机后拉起。
    build_server(release, verbose)?;

    // Build WebUI first so that module/webroot/ exists before copying
    build_webui()?;

    let module_dir = module_dir();
    check_module_scripts_lf(&module_dir)?;
    dir::copy(
        &module_dir,
        &temp_dir,
        &dir::CopyOptions::new().overwrite(true).content_only(true),
    )
    .with_context(|| format!("failed to copy {} into build dir", module_dir.display()))?;
    if temp_dir.join(".gitignore").exists() {
        fs::remove_file(temp_dir.join(".gitignore")).context("failed to drop .gitignore")?;
    }
    let so_source = bin_path(release);
    if !so_source.exists() {
        bail!(
            "{} was not produced by the build (see the build output above)",
            so_source.display()
        );
    }
    file::copy(
        &so_source,
        temp_dir.join("zygisk/arm64-v8a.so"),
        &file::CopyOptions::new().overwrite(true),
    )
    .context("failed to stage zygisk/arm64-v8a.so")?;
    stage_server_binary(release, &temp_dir)?;
    let build_type = if release { "release" } else { "debug" };
    let package_path = Path::new("output").join(format!("devmask-({build_type}).zip"));

    let local_offset = UtcOffset::from_hms(8, 0, 0).expect("UTC+8 offset should always be valid");

    zip_create_from_directory_with_options(&package_path, &temp_dir, |entry_path| {
        let mut options: FileOptions<'_, '_, ()> = FileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(9))
            .unix_permissions(zip_entry_mode(entry_path));

        if let Some(modified_time) = zip_entry_modified_time(entry_path, local_offset) {
            options = options.last_modified_time(modified_time);
        }

        options
    })
    .unwrap();

    println!("devmask built successfully: {:?}", package_path);

    Ok(())
}

/// zip 条目的 unix 权限位。
///
/// Magisk / KernelSU 安装时按 zip 里的 mode 落盘：`*.sh` 与 `bin/` 下的
/// `devmask_server` 必须是 0755，否则装机后 service.sh 与二进制都没有
/// 执行权限（只有 customize.sh 里的 set_perm 兜底，一旦脚本本身跑不起来就彻底失效）。
fn zip_entry_mode(entry_path: &Path) -> u32 {
    if entry_path.is_dir() {
        return 0o755;
    }

    let in_bin_dir = entry_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "bin");
    let is_shell_script = entry_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sh"));

    if in_bin_dir || is_shell_script {
        0o755
    } else {
        0o644
    }
}

fn zip_entry_modified_time(entry_path: &Path, local_offset: UtcOffset) -> Option<DateTime> {
    let modified = fs::metadata(entry_path).ok()?.modified().ok()?;
    let modified = OffsetDateTime::from(modified).to_offset(local_offset);
    // zip 9 移除了 `TryFrom<OffsetDateTime> for DateTime`，只剩 PrimitiveDateTime 版本。
    // ZIP 条目只存墙上时间（无时区），因此取 date+time 与原语义完全一致。
    let primitive = time::PrimitiveDateTime::new(modified.date(), modified.time());

    DateTime::try_from(primitive).ok()
}

/// devmask_server 的 ABI：与 zygisk 一致，只出 arm64-v8a。
const SERVER_ABI: &str = "arm64-v8a";
/// devmask_server 的最低 API level（计划书 8.6：-p 29）。
const SERVER_NDK_PLATFORM: &str = "29";
/// 模块 bin/ 里的文件名。
const SERVER_BIN_NAME: &str = "devmask_server";

/// 构建 devmask_server（arm64）。
///
/// 服务是纯 Rust + tokio，不需要任何 `-Z` 特性，所以走默认 toolchain
/// （不跟随 zygisk 的 `+nightly`，少一个 nightly 依赖）。
fn build_server(release: bool, verbose: bool) -> Result<()> {
    let mut cargo = Command::new("cargo");
    // cargo-ndk 4.x 把短参数从 `-p` 改成了 `-P`；长参数 `--platform` 两代语义一致，
    // 因此这里只用长写法，装哪个版本都能跑。
    cargo.args(["ndk", "--platform", SERVER_NDK_PLATFORM, "-t", SERVER_ABI]);
    cargo.args(["build", "-p", "devmask_server"]);
    if release {
        cargo.arg("--release");
    }
    if verbose {
        cargo.arg("--verbose");
    }
    run_command(&mut cargo, "cargo ndk build -p devmask_server")?;

    Ok(())
}

/// 把 server 二进制复制到 staging 的 bin/ 下并置 0755。
fn stage_server_binary(release: bool, temp_dir: &Path) -> Result<()> {
    let source = server_bin_path(release);
    if !source.exists() {
        bail!(
            "{} was not produced by the build (see the cargo ndk output above)",
            source.display()
        );
    }

    let bin_dir = temp_dir.join("bin");
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create {}", bin_dir.display()))?;
    let staged = bin_dir.join(SERVER_BIN_NAME);
    file::copy(&source, &staged, &file::CopyOptions::new().overwrite(true))
        .with_context(|| format!("failed to stage {}", staged.display()))?;
    set_executable(&staged)?;

    let size = fs::metadata(&staged).map(|meta| meta.len()).unwrap_or(0);
    println!(
        "devmask_server staged: {} ({:.2} MiB)",
        staged.display(),
        size as f64 / (1024.0 * 1024.0)
    );

    Ok(())
}

fn server_bin_path(release: bool) -> PathBuf {
    Path::new("target")
        .join("aarch64-linux-android")
        .join(if release { "release" } else { "debug" })
        .join(SERVER_BIN_NAME)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to chmod 755 {}", path.display()))?;

    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    // Windows 上没有 unix mode：zip 条目由 zip_entry_mode() 写成 0755，装机后即可执行。
    Ok(())
}

fn check(release: bool, verbose: bool) -> Result<()> {
    // check 不需要链接，因此不需要 cargo-ndk 设置的 Android linker。
    // 同时避免与 -Z build-std 冲突导致 duplicate core lang item。
    let mut cargo = Command::new("cargo");
    cargo.args([
        "+nightly",
        "check",
        "--target",
        "aarch64-linux-android",
        "-Z",
        "trim-paths",
    ]);

    if release {
        cargo.arg("--release");
    }

    if verbose {
        cargo.arg("--verbose");
    }

    run_command(&mut cargo, "cargo ndk check")?;

    Ok(())
}

fn clean() -> Result<()> {
    let temp_dir = temp_dir(false);
    let _ = fs::remove_dir_all(&temp_dir);

    run_command(Command::new("cargo").arg("clean"), "cargo clean")?;

    Ok(())
}

fn format(verbose: bool) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args(["fmt", "--all"]);
    if verbose {
        command.arg("--verbose");
    }
    run_command(&mut command, "cargo fmt")?;

    Ok(())
}

fn lint(fix: bool) -> Result<()> {
    let command_builder = |fix: bool| {
        let mut command = cargo_ndk();
        command.arg("clippy");
        if fix {
            command.args(["--fix", "--allow-dirty", "--allow-staged", "--all"]);
        }
        command.args(["--target", "aarch64-linux-android"]);
        command
    };

    run_command(&mut command_builder(fix), "cargo clippy")?;
    run_command(
        command_builder(fix).arg("--release"),
        "cargo clippy --release",
    )?;

    Ok(())
}

fn update() -> Result<()> {
    run_command(
        Command::new("cargo").args(["update", "--recursive"]),
        "cargo update",
    )?;
    run_command(
        Command::new("cargo")
            .current_dir("xtask")
            .args(["update", "--recursive"]),
        "cargo update (xtask)",
    )?;

    Ok(())
}

/// `.gitattributes` 的 `text eol=lf` 只在 git checkout / add 时生效；`xtask` 直接读工作区
/// 打包，被编辑器或脚本写成 CRLF 的 `*.sh` 会原样进 zip，导致安装器 ash 解析失败
fn check_module_scripts_lf(module_dir: &Path) -> Result<()> {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                collect(&path, out)?;
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("sh") {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut scripts = Vec::new();
    collect(module_dir, &mut scripts)?;
    for script in scripts {
        let bytes = fs::read(&script)?;
        if bytes.windows(2).any(|window| window == b"\x0d\x0a") {
            bail!(
                "{} has CRLF line endings; module scripts must use LF (see .gitattributes)",
                script.display()
            );
        }
    }
    Ok(())
}

fn module_dir() -> PathBuf {
    Path::new("module").to_path_buf()
}

fn temp_dir(release: bool) -> PathBuf {
    Path::new("output")
        .join(".temp")
        .join(if release { "release" } else { "debug" })
}

fn bin_path(release: bool) -> PathBuf {
    Path::new("target")
        .join("aarch64-linux-android")
        .join(if release { "release" } else { "debug" })
        .join("libzygisk.so")
}

fn cargo_ndk() -> Command {
    let mut command = Command::new("cargo");
    command
        .args(["+nightly", "ndk", "--platform", "31", "-t", "arm64-v8a"])
        .env("RUSTFLAGS", "-C default-linker-libraries");
    command
}

fn build_webui() -> Result<()> {
    let npm_cmd = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let npm = || {
        let mut command = Command::new(npm_cmd);
        command.current_dir("webui");
        command
    };

    run_command(npm().arg("install"), "npm install")?;
    run_command(npm().args(["run", "build"]), "npm run build")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_entry_mode_marks_scripts_and_bin_executable() {
        assert_eq!(zip_entry_mode(Path::new("service.sh")), 0o755);
        assert_eq!(zip_entry_mode(Path::new("uninstall.sh")), 0o755);
        assert_eq!(zip_entry_mode(Path::new("bin/devmask_server")), 0o755);
        assert_eq!(zip_entry_mode(Path::new("config.toml")), 0o644);
        assert_eq!(zip_entry_mode(Path::new("server.default.toml")), 0o644);
        assert_eq!(zip_entry_mode(Path::new("webroot/assets/index.js")), 0o644);
    }

    #[test]
    fn zip_entry_mode_marks_directories_executable() {
        // cargo test 的工作目录是 package root（xtask/），src/ 一定存在。
        assert_eq!(zip_entry_mode(Path::new("src")), 0o755);
    }

    #[test]
    fn server_bin_path_points_at_ndk_output() {
        let release = server_bin_path(true);
        assert!(
            release.ends_with("aarch64-linux-android/release/devmask_server"),
            "{}",
            release.display()
        );
        let debug = server_bin_path(false);
        assert!(
            debug.ends_with("aarch64-linux-android/debug/devmask_server"),
            "{}",
            debug.display()
        );
    }
}
