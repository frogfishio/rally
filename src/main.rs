// SPDX-FileCopyrightText: 2026 Alexander R. Croft
// SPDX-License-Identifier: GPL-3.0-or-later

mod config;
mod process_manager;
mod sink;
mod ui;
mod web;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, error::ErrorKind};
use process_manager::ProcessManager;
use sink::TelemetrySink;
use std::ffi::{OsStr, OsString};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

const DISPLAY_VERSION: &str = env!("RALLY_DISPLAY_VERSION");
const LICENSE_SUMMARY: &str = "Copyright (C) 2026 Alexander R. Croft\nLicense: GPL-3.0-or-later\n";
const DEFAULT_CONFIG_PATH: &str = "rally.toml";
const CONFIG_ENV_VAR: &str = "RALLY_CONFIG";

#[derive(Debug, Clone, Parser, PartialEq, Eq)]
#[command(
    name = "rally",
    version = DISPLAY_VERSION,
    about = "Rally your services with a local process dashboard",
    long_about = "Rally launches and supervises multiple local development processes, serves an embedded dashboard, and can optionally forward lifecycle and process output events to a ratatouille sink. Config path precedence is: --config, legacy positional path, RALLY_CONFIG, then ./rally.toml.",
    after_help = "Examples:\n  rally\n  RALLY_CONFIG=./dev.rally.toml rally\n  rally --config ./rally.toml\n  rally --sink http://127.0.0.1:9100/ingest\n  rally ./custom-rally.toml\n  rally start api-server\n  rally stop api-server\n  rally enable worker"
)]
struct CliArgs {
    #[arg(
        short = 'c',
        long = "config",
        value_name = "FILE",
        help = "Path to the Rally config file (overrides RALLY_CONFIG)",
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

    #[arg(long = "license", help = "Print copyright and license summary")]
    license: bool,

    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum CliCommand {
    /// Start an app through an existing Rally instance.
    Start { name: String },
    /// Stop an app through an existing Rally instance.
    Stop { name: String },
    /// Restart an app through an existing Rally instance.
    Restart { name: String },
    /// Enable an app at runtime through an existing Rally instance.
    Enable { name: String },
    /// Disable an app at runtime through an existing Rally instance.
    Disable { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliOptions {
    config_path: PathBuf,
    sink_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlOptions {
    config_path: PathBuf,
    command: ControlCommand,
    app_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlCommand {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliAction {
    Run(CliOptions),
    Control(ControlOptions),
    Print(String),
}

fn parse_cli_args() -> Result<CliAction> {
    parse_cli_args_from(std::env::args_os())
}

fn parse_cli_args_from<I, T>(args: I) -> Result<CliAction>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    parse_cli_args_from_with_env(args, |key| std::env::var_os(key))
}

fn parse_cli_args_from_with_env<I, T, F>(args: I, env_lookup: F) -> Result<CliAction>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    F: Fn(&OsStr) -> Option<OsString>,
{
    let args = match CliArgs::try_parse_from(args) {
        Ok(args) => args,
        Err(error) => {
            return match error.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                    Ok(CliAction::Print(error.to_string()))
                }
                _ => Err(anyhow!(error.to_string())),
            };
        }
    };

    if args.license {
        return Ok(CliAction::Print(format_license_text()));
    }

    let config_path = resolve_config_path(&args, env_lookup);

    if let Some(command) = args.command {
        let (command, app_name) = match command {
            CliCommand::Start { name } => (ControlCommand::Start, name),
            CliCommand::Stop { name } => (ControlCommand::Stop, name),
            CliCommand::Restart { name } => (ControlCommand::Restart, name),
            CliCommand::Enable { name } => (ControlCommand::Enable, name),
            CliCommand::Disable { name } => (ControlCommand::Disable, name),
        };

        return Ok(CliAction::Control(ControlOptions {
            config_path,
            command,
            app_name,
        }));
    }

    Ok(CliAction::Run(CliOptions {
        config_path,
        sink_url: args.sink_url,
    }))
}

fn resolve_config_path<F>(args: &CliArgs, env_lookup: F) -> PathBuf
where
    F: Fn(&OsStr) -> Option<OsString>,
{
    args.config_path
        .clone()
        .or(args.config_path_positional.clone())
        .or_else(|| env_lookup(OsStr::new(CONFIG_ENV_VAR)).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH))
}

fn format_license_text() -> String {
    LICENSE_SUMMARY.to_owned()
}

fn control_command_name(command: ControlCommand) -> &'static str {
    match command {
        ControlCommand::Start => "start",
        ControlCommand::Stop => "stop",
        ControlCommand::Restart => "restart",
        ControlCommand::Enable => "enable",
        ControlCommand::Disable => "disable",
    }
}

fn control_connect_host(host: &str) -> String {
    match host {
        "0.0.0.0" => "127.0.0.1".to_owned(),
        "::" => "::1".to_owned(),
        _ => host.to_owned(),
    }
}

fn control_base_url(host: &str, port: u16) -> String {
    let host = control_connect_host(host);
    if host.contains(':') && !host.starts_with('[') {
        format!("http://[{host}]:{port}")
    } else {
        format!("http://{host}:{port}")
    }
}

async fn run_control_command(control: ControlOptions) -> Result<()> {
    let cfg = config::load(&control.config_path)
        .with_context(|| format!("Could not load {}", control.config_path.display()))?;
    let base_url = control_base_url(&cfg.ui.host, cfg.ui.port);
    let url = format!(
        "{}/api/{}/{}",
        base_url,
        control_command_name(control.command),
        urlencoding::encode(&control.app_name)
    );

    let response = reqwest::Client::new()
        .post(&url)
        .send()
        .await
        .with_context(|| format!("Failed to connect to Rally at {}", base_url))?;

    match response.status() {
        reqwest::StatusCode::OK => Ok(()),
        reqwest::StatusCode::NOT_FOUND => {
            Err(anyhow!("App {} was not found in the running Rally instance", control.app_name))
        }
        reqwest::StatusCode::CONFLICT => Err(anyhow!(
            "App {} is disabled; enable it before using {}",
            control.app_name,
            control_command_name(control.command)
        )),
        status => Err(anyhow!(
            "Rally rejected {} for {} with status {}",
            control_command_name(control.command),
            control.app_name,
            status
        )),
    }
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

    let cli = match parse_cli_args()? {
        CliAction::Run(cli) => cli,
        CliAction::Control(control) => {
            run_control_command(control).await?;
            return Ok(());
        }
        CliAction::Print(output) => {
            print!("{output}");
            return Ok(());
        }
    };
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
    use super::{
        CliAction, CliArgs, CONFIG_ENV_VAR, ControlCommand, DEFAULT_CONFIG_PATH,
        DISPLAY_VERSION, format_license_text, parse_cli_args_from,
        parse_cli_args_from_with_env,
    };
    use anyhow::anyhow;
    use clap::CommandFactory;
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;

    fn parse_from(args: &[&str]) -> anyhow::Result<super::CliOptions> {
        parse_from_with_env(args, None)
    }

    fn parse_from_with_env(
        args: &[&str],
        env_config: Option<&str>,
    ) -> anyhow::Result<super::CliOptions> {
        let mut argv = vec!["rally"];
        argv.extend_from_slice(args);
        match parse_cli_args_from_with_env(argv, |key| {
            if key == OsStr::new(CONFIG_ENV_VAR) {
                env_config.map(OsString::from)
            } else {
                None
            }
        })? {
            CliAction::Run(cli) => Ok(cli),
            CliAction::Print(output) => Err(anyhow!("unexpected print action: {output}")),
            CliAction::Control(_) => Err(anyhow!("unexpected control action")),
        }
    }

    fn parse_action(args: &[&str]) -> anyhow::Result<CliAction> {
        let mut argv = vec!["rally"];
        argv.extend_from_slice(args);
        parse_cli_args_from(argv)
    }

    #[test]
    fn parses_default_cli_args() {
        let cli = parse_from(&[]).unwrap();
        assert_eq!(cli.config_path, PathBuf::from(DEFAULT_CONFIG_PATH));
        assert_eq!(cli.sink_url, None);
    }

    #[test]
    fn uses_rally_config_env_when_no_cli_path_is_provided() {
        let cli = parse_from_with_env(&[], Some("env-rally.toml")).unwrap();
        assert_eq!(cli.config_path, PathBuf::from("env-rally.toml"));
    }

    #[test]
    fn explicit_config_flag_overrides_rally_config_env() {
        let cli = parse_from_with_env(&["--config", "custom.toml"], Some("env-rally.toml")).unwrap();
        assert_eq!(cli.config_path, PathBuf::from("custom.toml"));
    }

    #[test]
    fn positional_config_overrides_rally_config_env() {
        let cli = parse_from_with_env(&["custom.toml"], Some("env-rally.toml")).unwrap();
        assert_eq!(cli.config_path, PathBuf::from("custom.toml"));
    }

    #[test]
    fn parses_start_subcommand() {
        let action = parse_action(&["start", "api-server"]).unwrap();
        assert_eq!(
            action,
            CliAction::Control(super::ControlOptions {
                config_path: PathBuf::from(DEFAULT_CONFIG_PATH),
                command: ControlCommand::Start,
                app_name: "api-server".to_owned(),
            })
        );
    }

    #[test]
    fn parses_control_command_with_env_config() {
        let mut argv = vec!["rally"];
        argv.extend_from_slice(&["enable", "worker"]);
        let action = parse_cli_args_from_with_env(argv, |key| {
            if key == OsStr::new(CONFIG_ENV_VAR) {
                Some(OsString::from("env-rally.toml"))
            } else {
                None
            }
        })
        .unwrap();

        assert_eq!(
            action,
            CliAction::Control(super::ControlOptions {
                config_path: PathBuf::from("env-rally.toml"),
                command: ControlCommand::Enable,
                app_name: "worker".to_owned(),
            })
        );
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
        assert!(help.contains("--license"));
        assert!(help.contains("--sink"));
        assert!(help.contains(CONFIG_ENV_VAR));
        assert!(help.contains("start"));
        assert!(help.contains("enable"));
        assert!(help.contains("Examples:"));
    }

    #[test]
    fn version_flag_prints_display_version() {
        let action = parse_action(&["--version"]).unwrap();
        let CliAction::Print(output) = action else {
            panic!("expected print action");
        };

        assert!(output.contains(DISPLAY_VERSION));
    }

    #[test]
    fn license_flag_prints_license_summary() {
        let action = parse_action(&["--license"]).unwrap();
        let CliAction::Print(output) = action else {
            panic!("expected print action");
        };

        assert_eq!(output, format_license_text());
    }
}

