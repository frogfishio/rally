use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Root configuration loaded from `start.toml`.
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub app: Vec<AppConfig>,
    #[serde(default)]
    pub ui: UiConfig,
}

/// Per-application configuration entry.
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    /// Unique display name for the application.
    pub name: String,
    /// Path (or binary name if on $PATH) of the executable.
    pub command: String,
    /// Optional working directory; defaults to the directory containing `start.toml`.
    pub workdir: Option<String>,
    /// Command-line arguments passed to the process.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables injected into the process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional HTTP health-check URL polled periodically.
    pub health_url: Option<String>,
    /// Health-check interval in seconds (default: 10).
    #[serde(default = "default_health_interval")]
    pub health_interval_secs: u64,
    /// Whether to automatically restart the process when it exits (default: false).
    #[serde(default)]
    pub restart_on_exit: bool,
    /// Number of lines of stdout/stderr to keep in memory per process (default: 500).
    #[serde(default = "default_log_lines")]
    pub log_lines: usize,
}

/// Embedded web-UI configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct UiConfig {
    /// Address to listen on (default: "127.0.0.1").
    #[serde(default = "default_host")]
    pub host: String,
    /// Port to listen on (default: 7700).
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".to_owned()
}

fn default_port() -> u16 {
    7700
}

fn default_health_interval() -> u64 {
    10
}

fn default_log_lines() -> usize {
    500
}

/// Load and parse `start.toml` from the given path.
pub fn load(path: &Path) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let config: Config = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
    Ok(config)
}

/// Parse a config from a TOML string (used in tests).
pub fn parse(toml: &str) -> Result<Config> {
    toml::from_str(toml).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_valid() {
        let cfg = parse("").unwrap();
        assert!(cfg.app.is_empty());
        assert_eq!(cfg.ui.port, 7700);
        assert_eq!(cfg.ui.host, "127.0.0.1");
    }

    #[test]
    fn single_app_minimal() {
        let toml = r#"
[[app]]
name    = "myapp"
command = "/usr/bin/myapp"
"#;
        let cfg = parse(toml).unwrap();
        assert_eq!(cfg.app.len(), 1);
        let app = &cfg.app[0];
        assert_eq!(app.name, "myapp");
        assert_eq!(app.command, "/usr/bin/myapp");
        assert!(app.args.is_empty());
        assert!(app.env.is_empty());
        assert!(!app.restart_on_exit);
        assert_eq!(app.health_interval_secs, 10);
        assert_eq!(app.log_lines, 500);
    }

    #[test]
    fn app_with_env_and_args() {
        let toml = r#"
[[app]]
name    = "server"
command = "./server"
args    = ["--port", "8080"]
restart_on_exit = true
log_lines = 200

[app.env]
PORT = "8080"
LOG  = "debug"
"#;
        let cfg = parse(toml).unwrap();
        let app = &cfg.app[0];
        assert_eq!(app.args, vec!["--port", "8080"]);
        assert!(app.restart_on_exit);
        assert_eq!(app.log_lines, 200);
        assert_eq!(app.env["PORT"], "8080");
        assert_eq!(app.env["LOG"], "debug");
    }

    #[test]
    fn custom_ui_settings() {
        let toml = r#"
[ui]
host = "0.0.0.0"
port = 9000
"#;
        let cfg = parse(toml).unwrap();
        assert_eq!(cfg.ui.host, "0.0.0.0");
        assert_eq!(cfg.ui.port, 9000);
    }

    #[test]
    fn multiple_apps() {
        let toml = r#"
[[app]]
name    = "a"
command = "a"

[[app]]
name    = "b"
command = "b"
"#;
        let cfg = parse(toml).unwrap();
        assert_eq!(cfg.app.len(), 2);
        assert_eq!(cfg.app[0].name, "a");
        assert_eq!(cfg.app[1].name, "b");
    }

    #[test]
    fn health_url_and_interval() {
        let toml = r#"
[[app]]
name    = "svc"
command = "./svc"
health_url           = "http://localhost:8080/health"
health_interval_secs = 30
"#;
        let cfg = parse(toml).unwrap();
        let app = &cfg.app[0];
        assert_eq!(app.health_url.as_deref(), Some("http://localhost:8080/health"));
        assert_eq!(app.health_interval_secs, 30);
    }
}
