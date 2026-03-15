// SPDX-FileCopyrightText: 2026 Alexander R. Croft
// SPDX-License-Identifier: GPL-3.0-or-later

mod config;
mod process_manager;
mod sink;
mod ui;
mod web;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use process_manager::ProcessManager;
use sink::TelemetrySink;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Debug, Clone, Parser, PartialEq, Eq)]
#[command(
    name = "rally",
    version,
    about = "Rally your services with a local process dashboard",
    long_about = "Rally launches and supervises multiple local development processes, serves an embedded dashboard, and can optionally forward lifecycle and process output events to a ratatouille sink.",
    after_help = "Examples:\n  rally\n  rally --config ./rally.toml\n  rally --sink http://127.0.0.1:9100/ingest\n  rally ./custom-rally.toml"
)]
struct CliArgs {
    #[arg(
        short = 'c',
        long = "config",
        value_name = "FILE",
        help = "Path to the Rally config file",
        conflicts_with = "config_path_positional"
    )]
    config_path: Option<PathBuf>,

    #[arg(
        value_name = "FILE",
        help = "Legacy positional config path",
        hide = true
    )]
    config_path_positional: Option<PathBuf>,

    #[arg(long = "sink", value_name = "URL", help = "Optional ratatouille HTTP sink URL")]
    sink_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliOptions {
    config_path: PathBuf,
    sink_url: Option<String>,
}

fn parse_cli_args() -> Result<CliOptions> {
    parse_cli_args_from(std::env::args_os())
}

fn parse_cli_args_from<I, T>(args: I) -> Result<CliOptions>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args = CliArgs::try_parse_from(args).map_err(|error| anyhow!(error.to_string()))?;

    Ok(CliOptions {
        config_path: args
            .config_path
            .or(args.config_path_positional)
            .unwrap_or_else(|| PathBuf::from("rally.toml")),
        sink_url: args.sink_url,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialise logging (respects RUST_LOG env var; defaults to info)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new("warn")
                        .add_directive("rally=info".parse().unwrap())
                }),
        )
        .compact()
        .init();

    let cli = parse_cli_args()?;
    let config_path = cli.config_path.clone();

    let cfg = config::load(&config_path)
        .with_context(|| format!("Could not load {}", config_path.display()))?;

    let telemetry = Arc::new(TelemetrySink::new(cli.sink_url.clone()));
    telemetry.emit(
        "rally:lifecycle",
        format!("loaded config from {}", config_path.display()),
    );

    if cfg.app.is_empty() {
        warn!("No [[app]] entries found in {}; nothing to start.", config_path.display());
        telemetry.emit(
            "rally:lifecycle",
            format!("no apps configured in {}", config_path.display()),
        );
    }

    let ui_addr: SocketAddr = format!("{}:{}", cfg.ui.host, cfg.ui.port)
        .parse()
        .context("Invalid UI host/port")?;

    // Build process manager
    let manager = Arc::new(ProcessManager::new(cfg.app.clone(), telemetry.clone())?);

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    // Start all processes
    manager.start_all().await;
    web::spawn_health_checkers(&manager, &http_client).await;
    web::spawn_watch_tasks(&manager).await;

    info!("Rally dashboard available at http://{}", ui_addr);
    info!("Press Ctrl-C to stop all processes and exit.");
    telemetry.emit(
        "rally:lifecycle",
        format!("dashboard available at http://{}", ui_addr),
    );

    // Build and run the axum web server
    let state = Arc::new(web::AppState::new(
        config_path.clone(),
        http_client,
        manager.clone(),
        telemetry.clone(),
    ));
    let app = web::router(state.clone());
    let listener = tokio::net::TcpListener::bind(ui_addr)
        .await
        .with_context(|| format!("Failed to bind to {}", ui_addr))?;

    // Graceful shutdown: stop all children on Ctrl-C
    let shutdown = async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl-C");
        info!("Shutting down…");
        telemetry.emit("rally:lifecycle", "shutdown requested".to_owned());
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .context("Web server error")?;

    // Kill remaining child processes
    state.shutdown().await;
    // Brief pause so children can flush
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CliArgs, parse_cli_args_from};
    use clap::CommandFactory;
    use std::path::PathBuf;

    fn parse_from(args: &[&str]) -> anyhow::Result<super::CliOptions> {
        let mut argv = vec!["rally"];
        argv.extend_from_slice(args);
        parse_cli_args_from(argv)
    }

    #[test]
    fn parses_default_cli_args() {
        let cli = parse_from(&[]).unwrap();
        assert_eq!(cli.config_path, PathBuf::from("rally.toml"));
        assert_eq!(cli.sink_url, None);
    }

    #[test]
    fn parses_sink_and_config_path() {
        let cli = parse_from(&["--sink", "http://127.0.0.1:9000", "--config", "custom.toml"]).unwrap();
        assert_eq!(cli.config_path, PathBuf::from("custom.toml"));
        assert_eq!(cli.sink_url.as_deref(), Some("http://127.0.0.1:9000"));
    }

    #[test]
    fn parses_legacy_positional_config_path() {
        let cli = parse_from(&["custom.toml"]).unwrap();
        assert_eq!(cli.config_path, PathBuf::from("custom.toml"));
    }

    #[test]
    fn rejects_multiple_config_sources() {
        let error = parse_from(&["--config", "one.toml", "two.toml"]).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("cannot be used with"));
    }

    #[test]
    fn rejects_unknown_flags() {
        let error = parse_from(&["--wat"]).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("unexpected argument") || message.contains("unknown argument"));
    }

    #[test]
    fn help_mentions_config_and_sink() {
        let mut command = CliArgs::command();
        let help = command.render_long_help().to_string();
        assert!(help.contains("--config"));
        assert!(help.contains("--sink"));
        assert!(help.contains("Examples:"));
    }
}

