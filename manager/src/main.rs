// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 可执行入口：参数、日志、初始化、监听与优雅退出。

use std::{env, process::ExitCode};

use anyhow::Context as _;
use axum::serve;
use device_faker_manager::{Config, VERSION, args, build_router, initialize};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let parsed = args::parse(env::args().skip(1)).map_err(|error| anyhow::anyhow!(error))?;
    if parsed.help {
        print!("{}", args::help_text());
        return Ok(());
    }
    if parsed.version {
        println!("device_faker_manager {VERSION}");
        return Ok(());
    }

    init_tracing(parsed.log_level.as_deref());

    let config = Config::load(parsed).context("加载配置失败")?;
    if config.token.source == device_faker_manager::state::TokenSource::Default {
        tracing::warn!(
            "正在使用默认 Manager Token admin123；建议用 --token / DFM_TOKEN / manager.token 轮换"
        );
    }
    tracing::info!(
        version = VERSION,
        bind = %config.bind,
        data_dir = %config.data_dir.display(),
        token_source = config.token.source.as_str(),
        "Device Faker Manager 启动中"
    );

    let state = initialize(config).await?;
    let router = build_router(state.clone());

    let listener = TcpListener::bind(state.cfg.bind)
        .await
        .with_context(|| format!("监听 {} 失败", state.cfg.bind))?;
    let address = listener.local_addr().context("读取监听地址失败")?;
    tracing::info!(address = %address, "Manager 已就绪");

    serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP 服务异常退出")?;

    tracing::info!("Manager 已停止");
    Ok(())
}

fn init_tracing(level: Option<&str>) {
    let filter = level
        .map(str::to_owned)
        .or_else(|| env::var("DFM_LOG").ok())
        .or_else(|| env::var("RUST_LOG").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "info".to_string());

    let env_filter = EnvFilter::try_new(&filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .try_init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %error, "监听 Ctrl-C 失败");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::warn!(error = %error, "监听 SIGTERM 失败");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
