// SPDX-FileCopyrightText: 2026 Alexander R. Croft
// SPDX-License-Identifier: GPL-3.0-or-later

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Root configuration loaded from `rally.toml`.
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
    /// Optional dashboard-friendly access string shown instead of the launch command.
    pub access: Option<String>,
    /// Path (or binary name if on $PATH) of the executable.
    pub command: String,
    /// Optional working directory; defaults to the directory containing `rally.toml`.
    pub workdir: Option<String>,
    /// Command-line arguments passed to the process.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables injected into the process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Other apps that must be started before this app.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Commands to run before the process starts; any failure blocks startup.
    #[serde(default)]
    pub before: Vec<HookConfig>,
    /// Commands to run after the process exits or is stopped.
    #[serde(default)]
    pub after: Vec<HookConfig>,
    /// Optional watch settings used to restart this app when tracked files change.
    pub watch: Option<WatchConfig>,
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

/// A one-shot command hook that runs before or after an app lifecycle event.
#[derive(Debug, Deserialize, Clone)]
pub struct HookConfig {
    /// Path (or binary name if on $PATH) of the executable.
    pub command: String,
    /// Command-line arguments passed to the hook.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional working directory; defaults to the app working directory.
    pub workdir: Option<String>,
    /// Environment variables injected into the hook.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// File watching rules for automatic restarts.
#[derive(Debug, Deserialize, Clone)]
pub struct WatchConfig {
    /// Extra files or directories to watch.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Whether watched directories should be observed recursively.
    #[serde(default = "default_watch_recursive")]
    pub recursive: bool,
    /// Debounce window before a restart is triggered.
    #[serde(default = "default_watch_debounce_millis")]
    pub debounce_millis: u64,
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

fn default_watch_recursive() -> bool {
    true
}

fn default_watch_debounce_millis() -> u64 {
    500
}

/// Load and parse `rally.toml` from the given path.
pub fn load(path: &Path) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let config: Config = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
    interpolate_config(config)
}

/// Parse a config from a TOML string (used in tests).
#[cfg(test)]
pub fn parse(toml: &str) -> Result<Config> {
    let config = toml::from_str(toml)?;
    interpolate_config(config)
}

fn interpolate_config(mut config: Config) -> Result<Config> {
    let process_env: HashMap<String, String> = std::env::vars().collect();

    for app in &mut config.app {
        let resolved_env = resolve_env_map(&app.env, &process_env)
            .with_context(|| format!("Failed to resolve env for app {}", app.name))?;
        app.env = resolved_env.clone();

        let mut app_scope = process_env.clone();
        app_scope.extend(resolved_env.clone());

        app.name = interpolate_string(&app.name, &app_scope)
            .with_context(|| "Failed to resolve app name")?;
        app.access = app
            .access
            .as_ref()
            .map(|value| {
                interpolate_string(value, &app_scope)
                    .with_context(|| format!("Failed to resolve access for app {}", app.name))
            })
            .transpose()?;
        app.command = interpolate_string(&app.command, &app_scope)
            .with_context(|| format!("Failed to resolve command for app {}", app.name))?;
        app.args = app
            .args
            .iter()
            .map(|arg| {
                interpolate_string(arg, &app_scope)
                    .with_context(|| format!("Failed to resolve argument for app {}", app.name))
            })
            .collect::<Result<Vec<_>>>()?;
        app.workdir = app
            .workdir
            .as_ref()
            .map(|value| {
                interpolate_string(value, &app_scope)
                    .with_context(|| format!("Failed to resolve workdir for app {}", app.name))
            })
            .transpose()?;
        app.health_url = app
            .health_url
            .as_ref()
            .map(|value| {
                interpolate_string(value, &app_scope)
                    .with_context(|| format!("Failed to resolve health_url for app {}", app.name))
            })
            .transpose()?;

        for hook in &mut app.before {
            interpolate_hook(hook, &app_scope, &app.name, "before")?;
        }

        for hook in &mut app.after {
            interpolate_hook(hook, &app_scope, &app.name, "after")?;
        }

        if let Some(watch) = &mut app.watch {
            watch.paths = watch
                .paths
                .iter()
                .map(|path| {
                    interpolate_string(path, &app_scope)
                        .with_context(|| format!("Failed to resolve watch path for app {}", app.name))
                })
                .collect::<Result<Vec<_>>>()?;
        }
    }

    Ok(config)
}

fn interpolate_hook(
    hook: &mut HookConfig,
    app_scope: &HashMap<String, String>,
    app_name: &str,
    phase: &str,
) -> Result<()> {
    let resolved_env = resolve_env_map(&hook.env, app_scope).with_context(|| {
        format!("Failed to resolve {} hook env for app {}", phase, app_name)
    })?;
    hook.env = resolved_env.clone();

    let mut scope = app_scope.clone();
    scope.extend(resolved_env);

    hook.command = interpolate_string(&hook.command, &scope)
        .with_context(|| format!("Failed to resolve {} hook command for app {}", phase, app_name))?;
    hook.args = hook
        .args
        .iter()
        .map(|arg| {
            interpolate_string(arg, &scope).with_context(|| {
                format!("Failed to resolve {} hook arg for app {}", phase, app_name)
            })
        })
        .collect::<Result<Vec<_>>>()?;
    hook.workdir = hook
        .workdir
        .as_ref()
        .map(|value| {
            interpolate_string(value, &scope).with_context(|| {
                format!("Failed to resolve {} hook workdir for app {}", phase, app_name)
            })
        })
        .transpose()?;

    Ok(())
}

fn resolve_env_map(
    values: &HashMap<String, String>,
    base_scope: &HashMap<String, String>,
) -> Result<HashMap<String, String>> {
    let mut resolved = HashMap::with_capacity(values.len());
    let mut visiting = Vec::new();

    for key in values.keys() {
        resolve_env_key(key, values, base_scope, &mut resolved, &mut visiting)?;
    }

    Ok(resolved)
}

fn resolve_env_key(
    key: &str,
    values: &HashMap<String, String>,
    base_scope: &HashMap<String, String>,
    resolved: &mut HashMap<String, String>,
    visiting: &mut Vec<String>,
) -> Result<String> {
    if let Some(value) = resolved.get(key) {
        return Ok(value.clone());
    }

    if visiting.iter().any(|entry| entry == key) {
        visiting.push(key.to_owned());
        anyhow::bail!("ENV interpolation cycle detected: {}", visiting.join(" -> "));
    }

    let template = values
        .get(key)
        .with_context(|| format!("Missing env key {} during interpolation", key))?;
    visiting.push(key.to_owned());
    let value = interpolate_with_lookup(template, &mut |name| {
        if values.contains_key(name) {
            return resolve_env_key(name, values, base_scope, resolved, visiting).map(Some);
        }
        Ok(base_scope.get(name).cloned())
    })?;
    visiting.pop();
    resolved.insert(key.to_owned(), value.clone());
    Ok(value)
}

fn interpolate_string(template: &str, scope: &HashMap<String, String>) -> Result<String> {
    interpolate_with_lookup(template, &mut |name| Ok(scope.get(name).cloned()))
}

fn interpolate_with_lookup<F>(template: &str, lookup: &mut F) -> Result<String>
where
    F: FnMut(&str) -> Result<Option<String>>,
{
    let mut output = String::with_capacity(template.len());
    let mut cursor = 0;

    while let Some(start) = template[cursor..].find("${") {
        let start_index = cursor + start;
        output.push_str(&template[cursor..start_index]);
        let name_start = start_index + 2;
        let Some(end_rel) = template[name_start..].find('}') else {
            anyhow::bail!("Unclosed ENV interpolation in {}", template);
        };
        let end_index = name_start + end_rel;
        let name = &template[name_start..end_index];
        if name.is_empty() {
            anyhow::bail!("Empty ENV interpolation in {}", template);
        }
        let value = lookup(name)?
            .ok_or_else(|| anyhow::anyhow!("Unknown ENV variable {} in {}", name, template))?;
        output.push_str(&value);
        cursor = end_index + 1;
    }

    output.push_str(&template[cursor..]);
    Ok(output)
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
        assert_eq!(app.access, None);
        assert_eq!(app.command, "/usr/bin/myapp");
        assert!(app.args.is_empty());
        assert!(app.env.is_empty());
        assert!(app.depends_on.is_empty());
        assert!(app.before.is_empty());
        assert!(app.after.is_empty());
        assert!(app.watch.is_none());
        assert!(!app.restart_on_exit);
        assert_eq!(app.health_interval_secs, 10);
        assert_eq!(app.log_lines, 500);
    }

    #[test]
    fn app_with_env_and_args() {
        let toml = r#"
[[app]]
name    = "server"
access  = "http://localhost:8080"
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
        assert_eq!(app.access.as_deref(), Some("http://localhost:8080"));
        assert_eq!(app.args, vec!["--port", "8080"]);
        assert!(app.restart_on_exit);
        assert_eq!(app.log_lines, 200);
        assert_eq!(app.env["PORT"], "8080");
        assert_eq!(app.env["LOG"], "debug");
    }

    #[test]
    fn app_with_before_and_after_hooks() {
        let toml = r#"
[[app]]
name    = "server"
command = "./server"
depends_on = ["database"]

[[app.before]]
command = "./prep"
args = ["--ensure-db"]

[app.before.env]
MODE = "prepare"

[[app.after]]
command = "./cleanup"
args = ["--remove-lock"]
workdir = "./tmp"

[app.watch]
paths = ["./config/dev.toml"]
debounce_millis = 250
"#;
        let cfg = parse(toml).unwrap();
        let app = &cfg.app[0];
        assert_eq!(app.depends_on, vec!["database"]);
        assert_eq!(app.before.len(), 1);
        assert_eq!(app.after.len(), 1);
        assert_eq!(app.before[0].command, "./prep");
        assert_eq!(app.before[0].args, vec!["--ensure-db"]);
        assert_eq!(app.before[0].env["MODE"], "prepare");
        assert_eq!(app.after[0].command, "./cleanup");
        assert_eq!(app.after[0].workdir.as_deref(), Some("./tmp"));
        let watch = app.watch.as_ref().unwrap();
        assert_eq!(watch.paths, vec!["./config/dev.toml"]);
        assert_eq!(watch.debounce_millis, 250);
        assert!(watch.recursive);
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

    #[test]
    fn interpolates_app_and_hook_fields() {
        let (home_var, home) = std::env::var("HOME")
            .map(|value| ("HOME", value))
            .or_else(|_| std::env::var("USERPROFILE").map(|value| ("USERPROFILE", value)))
            .unwrap();
        let toml = format!(
            r#"
[[app]]
name = "svc"
access = "http://${{HOST}}:8080"
command = "${{{home_var}}}/bin/svc"
    args = ["--data=${{DATA_DIR}}"]
workdir = "${{{home_var}}}/workspace"
    health_url = "http://${{HOST}}:8080/health"

[app.env]
HOST = "127.0.0.1"
DATA_DIR = "${{{home_var}}}/data"

[[app.before]]
command = "echo"
    args = ["${{DATA_DIR}}", "${{HOOK_DIR}}"]

[app.before.env]
    HOOK_DIR = "${{DATA_DIR}}/hooks"

[app.watch]
    paths = ["${{DATA_DIR}}/config.toml"]
"#
    );
        let cfg = parse(&toml).unwrap();
        let app = &cfg.app[0];
        assert_eq!(app.access.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(app.command, format!("{}/bin/svc", home));
        assert_eq!(app.args, vec![format!("--data={}/data", home)]);
        let expected_workdir = format!("{}/workspace", home);
        assert_eq!(app.workdir.as_deref(), Some(expected_workdir.as_str()));
        assert_eq!(app.health_url.as_deref(), Some("http://127.0.0.1:8080/health"));
        assert_eq!(app.env["DATA_DIR"], format!("{}/data", home));
        assert_eq!(app.before[0].args[0], format!("{}/data", home));
        assert_eq!(app.before[0].args[1], format!("{}/data/hooks", home));
        assert_eq!(
            app.watch.as_ref().unwrap().paths,
            vec![format!("{}/data/config.toml", home)]
        );
    }

    #[test]
    fn rejects_unknown_env_variable() {
        let toml = r#"
[[app]]
name    = "svc"
command = "${MISSING}/svc"
"#;
        let error = parse(toml).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("Unknown ENV variable MISSING"));
    }

    #[test]
    fn rejects_env_cycles() {
        let toml = r#"
[[app]]
name    = "svc"
command = "svc"

[app.env]
A = "${B}"
B = "${A}"
"#;
        let error = parse(toml).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("ENV interpolation cycle detected"));
    }
}
