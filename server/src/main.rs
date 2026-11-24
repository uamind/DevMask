// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! device_faker_server 二进制入口。
//!
//! 启动顺序（任何一步失败都不进入监听）：
//!
//! 1. 解析命令行（--config-path / --static-dir / --bind / --server-config / --module-prop /
//!    --device-fixture）；
//! 2. 合并 server.toml 并做启动前校验（loopback / allow_lan / Token 是否存在）；
//! 3. 绑定端口（先绑定，避免端口占用时白白生成 Token）；
//! 4. 无 Token 且 loopback 时生成 Token，只写 sha256，明文打一行横幅；
//! 5. 读设备信息，起服务（SIGTERM / Ctrl-C 优雅退出）。

use std::process::ExitCode;

use anyhow::{Context, Result};
use device_faker_server::{
    AppState, args, args::CliArgs, device::DeviceSource, serve, settings, token,
};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[device-faker-server] 启动失败: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let cli = CliArgs::from_env()?;
    if cli.help {
        print!("{}", args::USAGE);
        return Ok(());
    }
    if cli.version {
        println!("device_faker_server {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    init_tracing();

    let mut settings = settings::resolve(&cli)?;

    // 配置文件目录不存在时先建好：ConfigStore 的 flock 与备份都落在这个目录。
    if let Some(parent) = settings.config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建配置目录 {} 失败", parent.display()))?;
    }

    let listener = tokio::net::TcpListener::bind(&settings.bind)
        .await
        .with_context(|| {
            format!(
                "绑定 {} 失败（端口被占用或无权限；server.toml 的 bind 可改）",
                settings.bind
            )
        })?;
    let bind = listener
        .local_addr()
        .context("读取监听地址失败")?
        .to_string();

    let bootstrap = token::bootstrap(&mut settings)?;
    if let Some(banner) = bootstrap.banner() {
        // 唯一一次输出 Token 明文：service.sh 会把 stdout 重定向到 logs/server.log。
        println!("{banner}");
    }

    let device = DeviceSource::from_env(cli.device_fixture.clone()).load();
    let state = AppState::new(settings, bind.clone(), device);

    tracing::info!(
        bind = %state.bind(),
        static_dir = %state.site().root().display(),
        config = %state.store().path().display(),
        server_config = %state.settings().server_config_path.display(),
        tokens = state.settings().tokens.len(),
        "device_faker_server 已启动"
    );
    if !state.site().exists() {
        tracing::warn!(
            static_dir = %state.site().root().display(),
            "静态目录不存在或不可读：WebUI 会返回 404，但 /api/v1 仍可用"
        );
    }
    if bootstrap.created_file {
        tracing::info!(
            path = %bootstrap.server_config_path.display(),
            "已生成 server.toml（0600，只存 Token 的 sha256）"
        );
    }

    serve(state, listener).await.context("服务异常退出")?;
    tracing::info!("device_faker_server 已退出");
    Ok(())
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("device_faker_server=info,tower_http=info")
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
