mod config;
mod process_manager;
mod ui;
mod web;

use anyhow::{Context, Result};
use process_manager::{ProcessManager, SharedManager};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialise logging (respects RUST_LOG env var; defaults to info)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new("warn")
                        .add_directive("start=info".parse().unwrap())
                }),
        )
        .compact()
        .init();

    // Resolve the config file path (first CLI arg or default `start.toml`)
    let config_path: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("start.toml"));

    let cfg = config::load(&config_path)
        .with_context(|| format!("Could not load {}", config_path.display()))?;

    if cfg.app.is_empty() {
        warn!("No [[app]] entries found in {}; nothing to start.", config_path.display());
    }

    let ui_addr: SocketAddr = format!("{}:{}", cfg.ui.host, cfg.ui.port)
        .parse()
        .context("Invalid UI host/port")?;

    // Build process manager
    let manager: SharedManager = Arc::new(ProcessManager::new(cfg.app.clone()));

    // Spawn health checkers for apps that have a health_url
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    for proc in &manager.processes {
        let p = proc.lock().await;
        if p.config.health_url.is_some() {
            drop(p);
            ProcessManager::spawn_health_checker(proc.clone(), http_client.clone());
        }
    }

    // Start all processes
    manager.start_all().await;

    info!("Dashboard available at http://{}", ui_addr);
    info!("Press Ctrl-C to stop all processes and exit.");

    // Build and run the axum web server
    let app = web::router(manager.clone());
    let listener = tokio::net::TcpListener::bind(ui_addr)
        .await
        .with_context(|| format!("Failed to bind to {}", ui_addr))?;

    // Graceful shutdown: stop all children on Ctrl-C
    let shutdown = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl-C");
        info!("Shutting down…");
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .context("Web server error")?;

    // Kill remaining child processes
    manager.kill_all().await;
    // Brief pause so children can flush
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    Ok(())
}

