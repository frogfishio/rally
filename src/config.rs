// SPDX-FileCopyrightText: 2026 Alexander R. Croft
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::sink::TelemetrySink;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Root configuration loaded from `rally.toml`.
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub app: Vec<AppConfig>,
    #[serde(default)]
    pub ui: UiConfig,
    pub env_command: Option<EnvCommandConfig>,
    #[serde(skip)]
    pub env_provider_info: Option<EnvProviderInfo>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EnvCommandConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub format: EnvCommandFormat,
    #[serde(default = "default_env_command_timeout_ms")]
    pub timeout_ms: u64,
    pub workdir: Option<String>,
    #[serde(default)]
    pub override_existing: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnvCommandFormat {
    Json,
    Shell,
}

#[derive(Debug, Serialize, Clone)]
pub struct EnvProviderInfo {
    pub status: String,
    pub command: String,
    pub format: EnvCommandFormat,
    pub override_existing: bool,
    pub key_count: usize,
    pub duration_ms: u64,
    pub loaded_at: DateTime<Utc>,
}

/// Per-application configuration entry.
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    /// Unique display name for the application.
    pub name: String,
    /// Optional dashboard-friendly access string shown instead of the launch command.
    pub access: Option<String>,
    /// Optional cargo install target used when the command is not reachable.
    pub cargo: Option<String>,
    /// Whether the app is enabled at startup and after config reload (default: true).
    #[serde(default = "default_enabled")]
    pub enabled: bool,
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
    /// Managed subset of the effective environment contributed by Rally config or env_command.
    #[serde(skip)]
    pub managed_env: HashMap<String, String>,
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

fn default_enabled() -> bool {
    true
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

fn default_env_command_timeout_ms() -> u64 {
    5_000
}

/// Load and parse `rally.toml` from the given path.
pub fn load(path: &Path) -> Result<Config> {
    load_with_telemetry(path, None)
}

/// Load and parse `rally.toml` from the given path and emit env provider lifecycle telemetry.
pub fn load_with_telemetry(path: &Path, telemetry: Option<&TelemetrySink>) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let config: Config = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
    let config_dir = resolve_config_dir(path)?;
    resolve_config(
        config,
        Some(config_dir.as_path()),
        telemetry,
        &std::env::vars().collect::<HashMap<_, _>>(),
    )
}

fn resolve_config_dir(path: &Path) -> Result<PathBuf> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => Ok(parent.to_path_buf()),
        _ => std::env::current_dir().context("Failed to resolve current directory"),
    }
}

/// Parse a config from a TOML string (used in tests).
#[cfg(test)]
pub fn parse(toml: &str) -> Result<Config> {
    let config = toml::from_str(toml)?;
    resolve_config(config, None, None, &std::env::vars().collect::<HashMap<_, _>>())
}

fn resolve_config(
    mut config: Config,
    config_dir: Option<&Path>,
    telemetry: Option<&TelemetrySink>,
    process_env: &HashMap<String, String>,
) -> Result<Config> {
    let (base_env, provider_env, env_provider_info) = resolve_base_env(
        config.env_command.as_ref(),
        config_dir,
        telemetry,
        process_env,
    )?;
    config.env_provider_info = env_provider_info;

    let shared_env = resolve_env_map(&config.env, &base_env)
        .with_context(|| "Failed to resolve shared env")?;
    config.env = shared_env.clone();
    let mut shared_scope = base_env.clone();
    shared_scope.extend(shared_env.clone());

    for app in &mut config.app {
        let resolved_env = resolve_env_map(&app.env, &shared_scope)
            .with_context(|| format!("Failed to resolve env for app {}", app.name))?;
        let mut effective_env = base_env.clone();
        effective_env.extend(shared_env.clone());
        effective_env.extend(resolved_env.clone());
        app.managed_env = build_managed_env(&effective_env, &provider_env, &shared_env, &resolved_env);
        app.env = effective_env.clone();

        let app_scope = effective_env.clone();

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
        app.cargo = app
            .cargo
            .as_ref()
            .map(|value| {
                interpolate_string(value, &app_scope)
                    .with_context(|| format!("Failed to resolve cargo target for app {}", app.name))
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

fn resolve_base_env(
    env_command: Option<&EnvCommandConfig>,
    config_dir: Option<&Path>,
    telemetry: Option<&TelemetrySink>,
    process_env: &HashMap<String, String>,
) -> Result<(HashMap<String, String>, HashMap<String, String>, Option<EnvProviderInfo>)> {
    let Some(env_command) = env_command else {
        return Ok((process_env.clone(), HashMap::new(), None));
    };

    let (provider_env, info) = execute_env_command(env_command, config_dir, telemetry, process_env)?;

    let mut base_env = if env_command.override_existing {
        process_env.clone()
    } else {
        provider_env.clone()
    };
    if env_command.override_existing {
        base_env.extend(provider_env.clone());
    } else {
        base_env.extend(process_env.clone());
    }

    Ok((base_env, provider_env, Some(info)))
}

fn build_managed_env(
    effective_env: &HashMap<String, String>,
    provider_env: &HashMap<String, String>,
    shared_env: &HashMap<String, String>,
    app_env: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut managed_env = HashMap::new();

    for key in provider_env
        .keys()
        .chain(shared_env.keys())
        .chain(app_env.keys())
    {
        if let Some(value) = effective_env.get(key) {
            managed_env.insert(key.clone(), value.clone());
        }
    }

    managed_env
}

fn execute_env_command(
    env_command: &EnvCommandConfig,
    config_dir: Option<&Path>,
    telemetry: Option<&TelemetrySink>,
    process_env: &HashMap<String, String>,
) -> Result<(HashMap<String, String>, EnvProviderInfo)> {
    let workdir = resolve_env_command_workdir(env_command, config_dir)?;
    let command_for_spawn = resolve_env_command_path(&env_command.command, &workdir);
    let command_display = format_command_display(&env_command.command, &env_command.args);
    let started_at = Instant::now();

    validate_env_command_workdir(&command_display, &workdir)?;

    let mut child = Command::new(&command_for_spawn);
    child
        .args(&env_command.args)
        .current_dir(&workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match child.spawn() {
        Ok(child) => child,
        Err(error) => {
            let error_message = format_env_command_spawn_error(
                &command_display,
                env_command,
                &workdir,
                process_env,
                &error,
            );
            emit_env_command_failed(
                telemetry,
                &command_display,
                started_at.elapsed(),
                true,
                None,
                &error_message,
            );
            return Err(anyhow::anyhow!(error_message));
        }
    };

    let stdout_reader = child
        .stdout
        .take()
        .context("env_command stdout pipe was not available")?;
    let stderr_reader = child
        .stderr
        .take()
        .context("env_command stderr pipe was not available")?;

    let stdout_handle = thread::spawn(move || read_pipe(stdout_reader));
    let stderr_handle = thread::spawn(move || read_pipe(stderr_reader));

    let timeout = Duration::from_millis(env_command.timeout_ms);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }

        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let stderr = join_pipe(stderr_handle)?;
            let _ = join_pipe(stdout_handle)?;
            let stderr_summary = summarize_stderr(&stderr);
            emit_env_command_failed(
                telemetry,
                &command_display,
                started_at.elapsed(),
                true,
                None,
                &stderr_summary,
            );
            return Err(anyhow::anyhow!(
                "env_command {} timed out after {}ms{}",
                command_display,
                env_command.timeout_ms,
                format_stderr_suffix(&stderr_summary),
            ));
        }

        thread::sleep(Duration::from_millis(10));
    };

    let stdout = join_pipe(stdout_handle)?;
    let stderr = join_pipe(stderr_handle)?;
    let stderr_summary = summarize_stderr(&stderr);

    if !status.success() {
        emit_env_command_failed(
            telemetry,
            &command_display,
            started_at.elapsed(),
            false,
            Some(status.to_string()),
            &stderr_summary,
        );
        return Err(anyhow::anyhow!(
            "env_command {} exited with status {}{}",
            command_display,
            status,
            format_stderr_suffix(&stderr_summary),
        ));
    }

    let stdout = String::from_utf8(stdout).map_err(|error| {
        emit_env_command_failed(
            telemetry,
            &command_display,
            started_at.elapsed(),
            false,
            Some(status.to_string()),
            &stderr_summary,
        );
        anyhow::anyhow!(
            "env_command {} produced invalid UTF-8 on stdout: {}{}",
            command_display,
            error,
            format_stderr_suffix(&stderr_summary),
        )
    })?;

    let values = parse_env_command_output(env_command.format, &stdout)
        .with_context(|| format!("Failed to parse env_command output from {}", command_display))?;
    let duration_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;

    emit_env_command_loaded(telemetry, &command_display, duration_ms, values.len());

    Ok((
        values.clone(),
        EnvProviderInfo {
            status: "loaded".to_owned(),
            command: command_display,
            format: env_command.format,
            override_existing: env_command.override_existing,
            key_count: values.len(),
            duration_ms,
            loaded_at: Utc::now(),
        },
    ))
}

fn resolve_env_command_workdir(
    env_command: &EnvCommandConfig,
    config_dir: Option<&Path>,
) -> Result<PathBuf> {
    let base_dir = match config_dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().context("Failed to resolve current directory")?,
    };

    match env_command.workdir.as_deref() {
        Some(workdir) => {
            let workdir = expand_home_path(workdir).unwrap_or_else(|| PathBuf::from(workdir));
            if workdir.is_absolute() {
                Ok(workdir)
            } else {
                Ok(base_dir.join(workdir))
            }
        }
        None => Ok(base_dir),
    }
}

fn resolve_env_command_path(command: &str, workdir: &Path) -> PathBuf {
    let path = expand_home_path(command).unwrap_or_else(|| PathBuf::from(command));
    if path.is_relative() && path.components().count() > 1 {
        workdir.join(path)
    } else {
        path
    }
}

fn validate_env_command_workdir(command_display: &str, workdir: &Path) -> Result<()> {
    let metadata = std::fs::metadata(workdir).with_context(|| {
        format!(
            "env_command {} could not use workdir {}",
            command_display,
            workdir.display()
        )
    })?;

    if !metadata.is_dir() {
        anyhow::bail!(
            "env_command {} workdir {} is not a directory",
            command_display,
            workdir.display()
        );
    }

    Ok(())
}

fn expand_home_path(path: &str) -> Option<PathBuf> {
    let suffix = if path == "~" {
        Some("")
    } else {
        path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
    }?;

    let home_dir = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;

    if suffix.is_empty() {
        Some(home_dir)
    } else {
        Some(home_dir.join(suffix))
    }
}

fn format_env_command_spawn_error(
    command_display: &str,
    env_command: &EnvCommandConfig,
    workdir: &Path,
    process_env: &HashMap<String, String>,
    error: &std::io::Error,
) -> String {
    let mut message = format!("env_command {} failed to start: {}", command_display, error);

    if error.kind() != ErrorKind::NotFound {
        return message;
    }

    match std::fs::metadata(workdir) {
        Ok(metadata) if !metadata.is_dir() => {
            message.push_str(&format!(
                ". Working directory {} is not a directory",
                workdir.display()
            ));
            return message;
        }
        Err(metadata_error) if metadata_error.kind() == ErrorKind::NotFound => {
            message.push_str(&format!(
                ". Working directory {} does not exist",
                workdir.display()
            ));
            return message;
        }
        Err(_) | Ok(_) => {}
    }

    let command_path = resolve_env_command_path(&env_command.command, workdir);
    if command_path.is_absolute() || Path::new(&env_command.command).components().count() > 1 {
        match std::fs::metadata(&command_path) {
            Ok(_) => message.push_str(&format!(
                ". Resolved command path {} exists, so the OS may be unable to locate its interpreter or executable loader; verify the file is executable and that any shebang/runtime dependency is installed",
                command_path.display()
            )),
            Err(metadata_error) if metadata_error.kind() == ErrorKind::NotFound => {
                message.push_str(&format!(
                    ". Resolved command path {} does not exist",
                    command_path.display()
                ));
            }
            Err(_) => {}
        }

        return message;
    }

    message.push_str(
        ". env_command is executed directly without a shell, so shell-only PATH setup does not apply",
    );

    if let Some(path) = process_env.get("PATH") {
        message.push_str(&format!(". PATH={}", path));
    } else {
        message.push_str(". PATH is unset");
    }

    if Path::new(&env_command.command).components().count() == 1 {
        message.push_str(
            ". If the binary lives outside Rally's PATH, set env_command.command to an absolute path",
        );
    }

    message
}

fn read_pipe<R>(mut reader: R) -> Result<Vec<u8>>
where
    R: Read,
{
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}

fn join_pipe(handle: thread::JoinHandle<Result<Vec<u8>>>) -> Result<Vec<u8>> {
    match handle.join() {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("env_command output reader thread panicked")),
    }
}

fn parse_env_command_output(
    format: EnvCommandFormat,
    output: &str,
) -> Result<HashMap<String, String>> {
    match format {
        EnvCommandFormat::Json => parse_json_env_output(output),
        EnvCommandFormat::Shell => parse_shell_env_output(output),
    }
}

fn parse_json_env_output(output: &str) -> Result<HashMap<String, String>> {
    struct StrictStringMap(HashMap<String, String>);

    impl<'de> Deserialize<'de> for StrictStringMap {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_map(StrictStringMapVisitor)
        }
    }

    struct StrictStringMapVisitor;

    impl<'de> Visitor<'de> for StrictStringMapVisitor {
        type Value = StrictStringMap;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a JSON object containing only string values")
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut values = HashMap::new();
            while let Some((key, value)) = access.next_entry::<String, serde_json::Value>()? {
                if !is_valid_env_name(&key) {
                    return Err(de::Error::custom(format!("Invalid env key {}", key)));
                }
                if values.contains_key(&key) {
                    return Err(de::Error::custom(format!("Duplicate env key {}", key)));
                }

                let value = value
                    .as_str()
                    .ok_or_else(|| de::Error::custom(format!("Value for {} must be a string", key)))?;
                values.insert(key, value.to_owned());
            }
            Ok(StrictStringMap(values))
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(output);
    let StrictStringMap(values) = StrictStringMap::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(values)
}

fn parse_shell_env_output(output: &str) -> Result<HashMap<String, String>> {
    let mut values = HashMap::new();

    for (line_index, line) in output.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let rest = trimmed.strip_prefix("export ").ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid shell output line {}: expected export NAME=VALUE",
                line_index + 1
            )
        })?;
        let (name, raw_value) = rest.split_once('=').ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid shell output line {}: expected export NAME=VALUE",
                line_index + 1
            )
        })?;
        if !is_valid_env_name(name) {
            anyhow::bail!("Invalid env key {} on line {}", name, line_index + 1);
        }
        if values.contains_key(name) {
            anyhow::bail!("Duplicate env key {}", name);
        }

        values.insert(name.to_owned(), parse_shell_value(raw_value.trim(), line_index + 1)?);
    }

    Ok(values)
}

fn parse_shell_value(raw: &str, line_number: usize) -> Result<String> {
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        return Ok(raw[1..raw.len() - 1].to_owned());
    }

    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        let mut output = String::with_capacity(raw.len() - 2);
        let mut chars = raw[1..raw.len() - 1].chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                let escaped = chars.next().ok_or_else(|| {
                    anyhow::anyhow!("Invalid shell escape on line {}", line_number)
                })?;
                output.push(match escaped {
                    '\\' => '\\',
                    '"' => '"',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    '$' => '$',
                    other => other,
                });
            } else {
                output.push(ch);
            }
        }
        return Ok(output);
    }

    if raw.chars().any(char::is_whitespace) {
        anyhow::bail!("Unquoted shell value on line {} cannot contain whitespace", line_number);
    }

    Ok(raw.to_owned())
}

fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(ch) if ch == '_' || ch.is_ascii_alphabetic() => {}
        _ => return false,
    }

    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn format_command_display(command: &str, args: &[String]) -> String {
    std::iter::once(command.to_owned())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn summarize_stderr(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let mut summary = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    if summary.len() > 240 {
        summary.truncate(240);
        summary.push_str("...");
    }
    summary
}

fn format_stderr_suffix(stderr_summary: &str) -> String {
    if stderr_summary.is_empty() {
        String::new()
    } else {
        format!("; stderr: {}", stderr_summary)
    }
}

fn emit_env_command_loaded(
    telemetry: Option<&TelemetrySink>,
    command: &str,
    duration_ms: u64,
    key_count: usize,
) {
    if let Some(telemetry) = telemetry {
        telemetry.emit(
            "rally:lifecycle",
            format!(
                "env_command_loaded command={} duration_ms={} key_count={}",
                command, duration_ms, key_count
            ),
        );
    }
}

fn emit_env_command_failed(
    telemetry: Option<&TelemetrySink>,
    command: &str,
    duration: Duration,
    timed_out: bool,
    exit_status: Option<String>,
    stderr_summary: &str,
) {
    if let Some(telemetry) = telemetry {
        let duration_ms = duration.as_millis().min(u64::MAX as u128) as u64;
        let exit_status = exit_status.unwrap_or_else(|| "n/a".to_owned());
        telemetry.emit(
            "rally:lifecycle",
            format!(
                "env_command_failed command={} duration_ms={} timeout={} exit_status={} stderr={}",
                command,
                duration_ms,
                timed_out,
                exit_status,
                if stderr_summary.is_empty() {
                    "none"
                } else {
                    stderr_summary
                }
            ),
        );
    }
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
    use std::fs;

    fn parse_with_process_env(toml: &str, process_env: HashMap<String, String>) -> Result<Config> {
        let config = toml::from_str(toml)?;
        resolve_config(config, None, None, &process_env)
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn empty_config_is_valid() {
        let cfg = parse("").unwrap();
        assert!(cfg.env.is_empty());
        assert!(cfg.app.is_empty());
        assert_eq!(cfg.ui.port, 7700);
        assert_eq!(cfg.ui.host, "127.0.0.1");
        assert!(cfg.env_command.is_none());
        assert!(cfg.env_provider_info.is_none());
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
        assert_eq!(app.cargo, None);
        assert!(app.enabled);
        assert_eq!(app.command, "/usr/bin/myapp");
        assert!(app.args.is_empty());
        assert_eq!(app.env, std::env::vars().collect::<HashMap<_, _>>());
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
cargo   = "frogfish-server"
enabled = false
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
        assert_eq!(app.cargo.as_deref(), Some("frogfish-server"));
        assert!(!app.enabled);
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
[env]
SHARED = "1"

[[app]]
name    = "a"
command = "a"

[[app]]
name    = "b"
command = "b"
"#;
        let cfg = parse(toml).unwrap();
        assert_eq!(cfg.app.len(), 2);
        assert_eq!(cfg.env["SHARED"], "1");
        assert_eq!(cfg.app[0].name, "a");
        assert_eq!(cfg.app[1].name, "b");
        assert_eq!(cfg.app[0].env["SHARED"], "1");
        assert_eq!(cfg.app[1].env["SHARED"], "1");
    }

    #[test]
    fn merges_shared_env_into_each_app_with_app_override() {
        let toml = r#"
[env]
HOST = "127.0.0.1"
LOG_LEVEL = "info"

[[app]]
name = "api"
command = "./api"

[app.env]
LOG_LEVEL = "debug"
PORT = "8080"
API_URL = "http://${HOST}:${PORT}"
"#;

        let cfg = parse(toml).unwrap();
        let app = &cfg.app[0];

        assert_eq!(cfg.env["HOST"], "127.0.0.1");
        assert_eq!(app.env["HOST"], "127.0.0.1");
        assert_eq!(app.env["LOG_LEVEL"], "debug");
        assert_eq!(app.env["PORT"], "8080");
        assert_eq!(app.env["API_URL"], "http://127.0.0.1:8080");
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
cargo = "svc-installer-${{HOST}}"
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
        assert_eq!(app.cargo.as_deref(), Some("svc-installer-127.0.0.1"));
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

    #[test]
    fn env_command_json_parser_rejects_duplicate_keys() {
        let error = parse_json_env_output(r#"{"A":"1","A":"2"}"#).unwrap_err();
        assert!(error.to_string().contains("Duplicate env key A"));
    }

    #[test]
    fn env_command_json_parser_rejects_non_string_values() {
        let error = parse_json_env_output(r#"{"A":1}"#).unwrap_err();
        assert!(error.to_string().contains("must be a string"));
    }

    #[test]
    fn env_command_shell_parser_accepts_exports_and_comments() {
        let env = parse_shell_env_output(
            "# comment\nexport API_URL='http://127.0.0.1:8080'\nexport LOG_LEVEL=debug\n",
        )
        .unwrap();

        assert_eq!(env["API_URL"], "http://127.0.0.1:8080");
        assert_eq!(env["LOG_LEVEL"], "debug");
    }

    #[test]
    fn env_command_provider_merges_before_shared_env_and_app_env() {
        let toml = r#"
[env]
BASE_URL = "http://${HOST}:${PORT}"

[[app]]
name = "api"
command = "api"

[app.env]
PORT = "8080"
API_URL = "${BASE_URL}/v1"
"#;

        let mut process_env = HashMap::new();
        process_env.insert("HOST".to_owned(), "127.0.0.1".to_owned());
        process_env.insert("PORT".to_owned(), "9000".to_owned());

        let cfg = parse_with_process_env(toml, process_env).unwrap();
        let app = &cfg.app[0];

        assert_eq!(cfg.env["BASE_URL"], "http://127.0.0.1:9000");
        assert_eq!(app.env["PORT"], "8080");
        assert_eq!(app.env["API_URL"], "http://127.0.0.1:9000/v1");
    }

    #[test]
    fn env_command_override_existing_controls_base_precedence() {
        let process_env = HashMap::from([
            ("FROM_PROCESS".to_owned(), "process".to_owned()),
            ("SHARED".to_owned(), "process".to_owned()),
        ]);

        let mut provider_env = HashMap::new();
        provider_env.insert("SHARED".to_owned(), "provider".to_owned());
        provider_env.insert("FROM_PROVIDER".to_owned(), "provider".to_owned());

        let mut without_override = provider_env.clone();
        without_override.extend(process_env.clone());
        assert_eq!(without_override["SHARED"], "process");

        let mut with_override = process_env;
        with_override.extend(provider_env);
        assert_eq!(with_override["SHARED"], "provider");
        assert_eq!(with_override["FROM_PROCESS"], "process");
        assert_eq!(with_override["FROM_PROVIDER"], "provider");
    }

    #[test]
    fn env_command_not_found_error_mentions_path_and_direct_execution() {
        let env_command = EnvCommandConfig {
            command: "macrun".to_owned(),
            args: vec!["env".to_owned()],
            format: EnvCommandFormat::Shell,
            timeout_ms: 5_000,
            workdir: None,
            override_existing: false,
        };
        let process_env = HashMap::from([("PATH".to_owned(), "/usr/bin".to_owned())]);
        let error = std::io::Error::from(ErrorKind::NotFound);

        let message = format_env_command_spawn_error(
            "macrun env",
            &env_command,
            Path::new("/tmp"),
            &process_env,
            &error,
        );

        assert!(message.contains("without a shell"));
        assert!(message.contains("PATH=/usr/bin"));
        assert!(message.contains("absolute path"));
    }

    #[test]
    fn resolve_env_command_path_expands_home_directory() {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap();

        let resolved = resolve_env_command_path("~/.cargo/bin/macrun", Path::new("/tmp"));

        assert_eq!(resolved, PathBuf::from(home).join(".cargo/bin/macrun"));
    }

    #[test]
    fn env_command_not_found_error_for_missing_absolute_path_reports_resolved_location() {
        let missing = std::env::temp_dir().join(format!(
            "rally-missing-env-command-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let env_command = EnvCommandConfig {
            command: missing.display().to_string(),
            args: vec!["env".to_owned()],
            format: EnvCommandFormat::Shell,
            timeout_ms: 5_000,
            workdir: None,
            override_existing: false,
        };
        let error = std::io::Error::from(ErrorKind::NotFound);

        let message = format_env_command_spawn_error(
            "missing env",
            &env_command,
            Path::new("/tmp"),
            &HashMap::new(),
            &error,
        );

        assert!(message.contains("does not exist"));
        assert!(message.contains(&missing.display().to_string()));
    }

    #[test]
    fn env_command_not_found_error_for_missing_workdir_reports_workdir() {
        let env_command = EnvCommandConfig {
            command: "/bin/echo".to_owned(),
            args: vec!["env".to_owned()],
            format: EnvCommandFormat::Shell,
            timeout_ms: 5_000,
            workdir: None,
            override_existing: false,
        };
        let missing_workdir = std::env::temp_dir().join(format!(
            "rally-missing-workdir-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let error = std::io::Error::from(ErrorKind::NotFound);

        let message = format_env_command_spawn_error(
            "echo env",
            &env_command,
            &missing_workdir,
            &HashMap::new(),
            &error,
        );

        assert!(message.contains("Working directory"));
        assert!(message.contains("does not exist"));
        assert!(message.contains(&missing_workdir.display().to_string()));
    }

    #[test]
    fn validate_env_command_workdir_rejects_missing_directory() {
        let missing_workdir = std::env::temp_dir().join(format!(
            "rally-invalid-workdir-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));

        let error = validate_env_command_workdir("macrun env", &missing_workdir).unwrap_err();

        assert!(error.to_string().contains("could not use workdir"));
        assert!(error.to_string().contains(&missing_workdir.display().to_string()));
    }

    #[test]
    fn resolve_config_dir_uses_current_directory_for_bare_relative_path() {
        let resolved = resolve_config_dir(Path::new("rally.toml")).unwrap();

        assert_eq!(resolved, std::env::current_dir().unwrap());
    }

    #[test]
    fn resolve_config_dir_preserves_non_empty_parent() {
        let resolved = resolve_config_dir(Path::new("configs/rally.toml")).unwrap();

        assert_eq!(resolved, PathBuf::from("configs"));
    }

    #[cfg(unix)]
    #[test]
    fn env_command_not_found_error_for_existing_absolute_path_mentions_interpreter_or_loader() {
        let temp_dir = std::env::temp_dir().join(format!(
            "rally-env-command-existing-path-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&temp_dir).unwrap();
        let script_path = temp_dir.join("emit-env.sh");
        fs::write(&script_path, "#!/bin/sh\nprintf '{}'\n").unwrap();
        make_executable(&script_path);

        let env_command = EnvCommandConfig {
            command: script_path.display().to_string(),
            args: vec!["env".to_owned()],
            format: EnvCommandFormat::Shell,
            timeout_ms: 5_000,
            workdir: None,
            override_existing: false,
        };
        let error = std::io::Error::from(ErrorKind::NotFound);

        let message = format_env_command_spawn_error(
            "emit-env.sh env",
            &env_command,
            Path::new("/tmp"),
            &HashMap::new(),
            &error,
        );

        assert!(message.contains("interpreter or executable loader"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[test]
    fn load_executes_relative_env_command_from_config_directory() {
        let temp_dir = std::env::temp_dir().join(format!(
            "rally-env-command-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&temp_dir).unwrap();

        let script_path = temp_dir.join("emit-env.sh");
        fs::write(
            &script_path,
            "#!/bin/sh\nprintf '{\"RALLY_SAMPLE_HOST\":\"127.0.0.1\",\"RALLY_SAMPLE_PORT\":\"7811\"}'\n",
        )
        .unwrap();
        make_executable(&script_path);

        let config_path = temp_dir.join("rally.toml");
        fs::write(
            &config_path,
            r#"
[env_command]
command = "./emit-env.sh"
format = "json"
timeout_ms = 1000

[env]
RALLY_SAMPLE_ORIGIN = "http://${RALLY_SAMPLE_HOST}:${RALLY_SAMPLE_PORT}"

[[app]]
name = "sample"
command = "sleep"
args = ["1"]
access = "${RALLY_SAMPLE_ORIGIN}/"

[app.env]
RALLY_SAMPLE_URL = "${RALLY_SAMPLE_ORIGIN}/ready"
"#,
        )
        .unwrap();

        let cfg = load(&config_path).unwrap();
        let app = &cfg.app[0];

        assert_eq!(cfg.env_provider_info.as_ref().map(|info| info.key_count), Some(2));
        assert_eq!(cfg.env["RALLY_SAMPLE_ORIGIN"], "http://127.0.0.1:7811");
        assert_eq!(app.managed_env["RALLY_SAMPLE_HOST"], "127.0.0.1");
        assert_eq!(app.managed_env["RALLY_SAMPLE_PORT"], "7811");
        assert_eq!(app.managed_env["RALLY_SAMPLE_ORIGIN"], "http://127.0.0.1:7811");
        assert_eq!(app.managed_env["RALLY_SAMPLE_URL"], "http://127.0.0.1:7811/ready");
        assert_eq!(app.access.as_deref(), Some("http://127.0.0.1:7811/"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[test]
    fn load_fails_when_env_command_times_out() {
        let temp_dir = std::env::temp_dir().join(format!(
            "rally-env-command-timeout-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&temp_dir).unwrap();

        let script_path = temp_dir.join("sleepy.sh");
        fs::write(&script_path, "#!/bin/sh\nsleep 1\nprintf '{}'\n").unwrap();
        make_executable(&script_path);

        let config_path = temp_dir.join("rally.toml");
        fs::write(
            &config_path,
            r#"
[env_command]
command = "./sleepy.sh"
format = "json"
timeout_ms = 10
"#,
        )
        .unwrap();

        let error = load(&config_path).unwrap_err();
        assert!(error.to_string().contains("timed out"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
