mod config;
mod process_manager;
mod sink;
mod ui;
mod web;

use anyhow::{Context, Result, anyhow};
use process_manager::ProcessManager;
use sink::TelemetrySink;
use std::net::SocketAddr;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliOptions {
    config_path: PathBuf,
    sink_url: Option<String>,
}

fn parse_cli_args() -> Result<CliOptions> {
    parse_cli_args_from(std::env::args_os().skip(1))
}

fn parse_cli_args_from<I>(args: I) -> Result<CliOptions>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let mut config_path: Option<PathBuf> = None;
    let mut sink_url: Option<String> = None;

    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy().into_owned();

        if arg == "--sink" {
            let value = args
                .next()
                .ok_or_else(|| anyhow!("--sink requires a URL"))?;
            sink_url = Some(value.to_string_lossy().into_owned());
            continue;
        }

        if let Some(value) = arg.strip_prefix("--sink=") {
            sink_url = Some(value.to_owned());
            continue;
        }

        if arg.starts_with('-') {
            return Err(anyhow!("Unknown argument: {}", arg));
        }

        if config_path.is_some() {
            return Err(anyhow!("Multiple config paths provided"));
        }

        config_path = Some(PathBuf::from(arg));
    }

    Ok(CliOptions {
        config_path: config_path.unwrap_or_else(|| PathBuf::from("rally.toml")),
        sink_url,
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

    web::spawn_health_checkers(&manager, &http_client).await;

    // Start all processes
    manager.start_all().await;

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
    use super::parse_cli_args_from;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn parse_from(args: &[&str]) -> anyhow::Result<super::CliOptions> {
        let args = args.iter().map(|value| OsString::from(*value)).collect::<Vec<_>>();
        parse_cli_args_from(args)
    }

    #[test]
    fn parses_default_cli_args() {
        let cli = parse_from(&[]).unwrap();
        assert_eq!(cli.config_path, PathBuf::from("rally.toml"));
        assert_eq!(cli.sink_url, None);
    }

    #[test]
    fn parses_sink_and_config_path() {
        let cli = parse_from(&["--sink", "http://127.0.0.1:9000", "custom.toml"]).unwrap();
        assert_eq!(cli.config_path, PathBuf::from("custom.toml"));
        assert_eq!(cli.sink_url.as_deref(), Some("http://127.0.0.1:9000"));
    }

    #[test]
    fn rejects_unknown_flags() {
        let error = parse_from(&["--wat"]).unwrap_err();
        assert!(format!("{error:#}").contains("Unknown argument"));
    }
}

