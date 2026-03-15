// SPDX-FileCopyrightText: 2026 Alexander R. Croft
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::config::{AppConfig, HookConfig};
use crate::sink::TelemetrySink;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use notify::{recommended_watcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use serde::Serialize;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, Mutex, RwLock};
use tracing::{error, info, warn};

#[derive(Debug)]
enum HookError {
    Spawn(std::io::Error),
    Exit(i32),
    Wait(std::io::Error),
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::Spawn(error) => write!(f, "failed to spawn: {}", error),
            HookError::Exit(code) => write!(f, "exited with code {}", code),
            HookError::Wait(error) => write!(f, "wait failed: {}", error),
        }
    }
}

/// Current lifecycle state of a managed process.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    /// Process has not been started yet.
    Pending,
    /// Process is running.
    Running,
    /// Process exited with the given code.
    Exited(i32),
    /// Process was killed by a signal.
    Killed,
    /// Process failed to start.
    Failed,
}

impl std::fmt::Display for ProcessState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessState::Pending => write!(f, "pending"),
            ProcessState::Running => write!(f, "running"),
            ProcessState::Exited(code) => write!(f, "exited({})", code),
            ProcessState::Killed => write!(f, "killed"),
            ProcessState::Failed => write!(f, "failed"),
        }
    }
}

/// Health-check outcome.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// No health URL configured.
    NotConfigured,
    /// Health check has not run yet.
    Unknown,
    Healthy,
    Unhealthy,
}

/// A single log line captured from stdout or stderr.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub timestamp: DateTime<Utc>,
    pub stream: String, // "stdout" | "stderr"
    pub text: String,
}

/// Snapshot of a process's runtime state (serialisable for the API).
#[derive(Debug, Clone, Serialize)]
pub struct ProcessStatus {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub state: ProcessState,
    pub health: HealthStatus,
    pub pid: Option<u32>,
    pub started_at: Option<DateTime<Utc>>,
    pub exit_time: Option<DateTime<Utc>>,
    pub restart_count: u32,
    pub last_restart_reason: Option<String>,
    pub watch_enabled: bool,
    pub watch_paths: Vec<String>,
    pub watch_debounce_millis: Option<u64>,
    pub logs: Vec<LogLine>,
}

/// Internal mutable state kept behind a lock.
pub(crate) struct Inner {
    state: ProcessState,
    health: HealthStatus,
    pid: Option<u32>,
    started_at: Option<DateTime<Utc>>,
    exit_time: Option<DateTime<Utc>>,
    restart_count: u32,
    last_restart_reason: Option<String>,
    logs: VecDeque<LogLine>,
    pub(crate) max_log_lines: usize,
    supervisor_active: bool,
    /// Sender used to kill the running process on demand.
    kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Inner {
    fn push_log(&mut self, stream: &str, text: String) {
        if self.logs.len() == self.max_log_lines {
            self.logs.pop_front();
        }
        self.logs.push_back(LogLine {
            timestamp: Utc::now(),
            stream: stream.to_owned(),
            text,
        });
    }
}

/// A single managed process.
pub struct ManagedProcess {
    pub config: AppConfig,
    pub(crate) inner: Arc<RwLock<Inner>>,
    /// Broadcast channel used to notify UI websocket clients of state changes.
    pub change_tx: broadcast::Sender<()>,
    telemetry: Arc<TelemetrySink>,
}

impl ManagedProcess {
    pub fn new(config: AppConfig, telemetry: Arc<TelemetrySink>) -> Self {
        let (change_tx, _) = broadcast::channel(32);
        let inner = Arc::new(RwLock::new(Inner {
            state: ProcessState::Pending,
            health: if config.health_url.is_some() {
                HealthStatus::Unknown
            } else {
                HealthStatus::NotConfigured
            },
            pid: None,
            started_at: None,
            exit_time: None,
            restart_count: 0,
            last_restart_reason: None,
            logs: VecDeque::new(),
            max_log_lines: config.log_lines,
            supervisor_active: false,
            kill_tx: None,
        }));
        Self {
            config,
            inner,
            change_tx,
            telemetry,
        }
    }

    /// Return a serialisable snapshot.
    pub async fn status(&self) -> ProcessStatus {
        let inner = self.inner.read().await;
        let watch_targets = collect_watch_targets(&self.config)
            .into_iter()
            .map(|target| target.path.display().to_string())
            .collect::<Vec<_>>();
        ProcessStatus {
            name: self.config.name.clone(),
            command: self.config.command.clone(),
            args: self.config.args.clone(),
            env: self.config.env.clone(),
            state: inner.state.clone(),
            health: inner.health.clone(),
            pid: inner.pid,
            started_at: inner.started_at,
            exit_time: inner.exit_time,
            restart_count: inner.restart_count,
            last_restart_reason: inner.last_restart_reason.clone(),
            watch_enabled: !watch_targets.is_empty(),
            watch_paths: watch_targets,
            watch_debounce_millis: self.config.watch.as_ref().map(|watch| watch.debounce_millis),
            logs: inner.logs.iter().cloned().collect(),
        }
    }

    /// Send SIGKILL / terminate the running process.
    pub async fn kill(&self) {
        let mut inner = self.inner.write().await;
        if let Some(tx) = inner.kill_tx.take() {
            self.telemetry.emit(
                "rally:process",
                format!("kill requested for {}", self.config.name),
            );
            let _ = tx.send(());
        }
    }

    /// Clear the in-memory log buffer for this process.
    pub async fn clear_logs(&self) {
        let mut inner = self.inner.write().await;
        inner.logs.clear();
    }

    pub async fn is_supervisor_active(&self) -> bool {
        let inner = self.inner.read().await;
        inner.supervisor_active
    }

    /// Spawn (or re-spawn) the process and return immediately.
    /// A background task monitors the child process.
    pub async fn start(&self) {
        {
            let mut inner = self.inner.write().await;
            if inner.supervisor_active {
                return;
            }
            inner.supervisor_active = true;
        }

        let inner_arc = self.inner.clone();
        let config = self.config.clone();
        let change_tx = self.change_tx.clone();
        let telemetry = self.telemetry.clone();

        tokio::spawn(async move {
            loop {
                if let Err(error) = run_hooks(
                    &config.name,
                    &config.before,
                    &config,
                    &inner_arc,
                    &change_tx,
                    &telemetry,
                    "before",
                )
                .await
                {
                    error!(name = %config.name, error = %error, "Before hook failed");
                    telemetry.emit(
                        "rally:process",
                        format!("before hook failed for {}: {}", config.name, error),
                    );
                    let mut inner = inner_arc.write().await;
                    inner.state = ProcessState::Failed;
                    inner.exit_time = Some(Utc::now());
                    inner.push_log("stderr", format!("Before hook failed: {}", error));
                    let _ = change_tx.send(());
                    break;
                }

                let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();

                // Build the command
                let mut cmd = Command::new(&config.command);
                cmd.args(&config.args);
                cmd.envs(&config.env);
                if let Some(ref wd) = config.workdir {
                    cmd.current_dir(wd);
                }
                cmd.stdout(std::process::Stdio::piped());
                cmd.stderr(std::process::Stdio::piped());
                // Create a new process group so we can terminate the whole tree (Unix only)
                #[cfg(unix)]
                cmd.process_group(0);

                match cmd.spawn() {
                    Err(e) => {
                        error!(name = %config.name, error = %e, "Failed to spawn process");
                        telemetry.emit(
                            "rally:process",
                            format!("failed to spawn {}: {}", config.name, e),
                        );
                        let mut inner = inner_arc.write().await;
                        inner.state = ProcessState::Failed;
                        inner.push_log("stderr", format!("Failed to spawn: {}", e));
                        let _ = change_tx.send(());
                        break;
                    }
                    Ok(mut child) => {
                        let pid = child.id();
                        info!(name = %config.name, pid = ?pid, "Process started");
                        telemetry.emit(
                            "rally:process",
                            format!("started {} pid={:?}", config.name, pid),
                        );

                        {
                            let mut inner = inner_arc.write().await;
                            inner.state = ProcessState::Running;
                            inner.pid = pid;
                            inner.started_at = Some(Utc::now());
                            inner.exit_time = None;
                            inner.kill_tx = Some(kill_tx);
                            let _ = change_tx.send(());
                        }

                        // Capture stdout
                        if let Some(stdout) = child.stdout.take() {
                            capture_process_output(
                                config.name.clone(),
                                "stdout",
                                stdout,
                                inner_arc.clone(),
                                change_tx.clone(),
                                telemetry.clone(),
                            );
                        }

                        // Capture stderr
                        if let Some(stderr) = child.stderr.take() {
                            capture_process_output(
                                config.name.clone(),
                                "stderr",
                                stderr,
                                inner_arc.clone(),
                                change_tx.clone(),
                                telemetry.clone(),
                            );
                        }

                        // Wait for exit or kill signal
                        let exit_status = tokio::select! {
                            status = child.wait() => status,
                            _ = kill_rx => {
                                let _ = child.kill().await;
                                child.wait().await
                            }
                        };

                        let mut inner = inner_arc.write().await;
                        inner.pid = None;
                        inner.exit_time = Some(Utc::now());
                        inner.kill_tx = None;

                        match exit_status {
                            Ok(status) => {
                                let code = status.code().unwrap_or(-1);
                                inner.state = ProcessState::Exited(code);
                                info!(name = %config.name, code, "Process exited");
                                telemetry.emit(
                                    "rally:process",
                                    format!("{} exited code={}", config.name, code),
                                );
                                inner.push_log(
                                    "stderr",
                                    format!("Process exited with code {}", code),
                                );
                            }
                            Err(e) => {
                                inner.state = ProcessState::Killed;
                                warn!(name = %config.name, error = %e, "Process wait error");
                                telemetry.emit(
                                    "rally:process",
                                    format!("{} wait error: {}", config.name, e),
                                );
                                inner.push_log("stderr", format!("Process wait error: {}", e));
                            }
                        }
                        let _ = change_tx.send(());

                        if let Err(error) = run_hooks(
                            &config.name,
                            &config.after,
                            &config,
                            &inner_arc,
                            &change_tx,
                            &telemetry,
                            "after",
                        )
                        .await
                        {
                            warn!(name = %config.name, error = %error, "After hook failed");
                            telemetry.emit(
                                "rally:process",
                                format!("after hook failed for {}: {}", config.name, error),
                            );
                            let mut inner = inner_arc.write().await;
                            inner.push_log("stderr", format!("After hook failed: {}", error));
                            let _ = change_tx.send(());
                        }

                        if config.restart_on_exit
                            && !matches!(inner.state, ProcessState::Killed)
                        {
                            inner.restart_count += 1;
                            let count = inner.restart_count;
                            inner.last_restart_reason = Some(format!("auto-restart attempt {}", count));
                            telemetry.emit(
                                "rally:process",
                                format!("restarting {} attempt={}", config.name, count),
                            );
                            inner.push_log(
                                "stderr",
                                format!("Restarting (attempt {})…", count),
                            );
                            drop(inner);
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            continue;
                        }
                        break;
                    }
                }
            }

            let mut inner = inner_arc.write().await;
            inner.supervisor_active = false;
            inner.kill_tx = None;
            let _ = change_tx.send(());
        });
    }
}

#[derive(Clone)]
struct WatchTarget {
    path: PathBuf,
    recursive: bool,
}

async fn run_hooks(
    app_name: &str,
    hooks: &[HookConfig],
    app_config: &AppConfig,
    inner_arc: &Arc<RwLock<Inner>>,
    change_tx: &broadcast::Sender<()>,
    telemetry: &Arc<TelemetrySink>,
    phase: &str,
) -> Result<(), HookError> {
    for hook in hooks {
        run_single_hook(
            app_name,
            hook,
            app_config,
            inner_arc,
            change_tx,
            telemetry,
            phase,
        )
        .await?;
    }

    Ok(())
}

async fn run_single_hook(
    app_name: &str,
    hook: &HookConfig,
    app_config: &AppConfig,
    inner_arc: &Arc<RwLock<Inner>>,
    change_tx: &broadcast::Sender<()>,
    telemetry: &Arc<TelemetrySink>,
    phase: &str,
) -> Result<(), HookError> {
    telemetry.emit(
        "rally:process",
        format!("running {} hook for {}: {}", phase, app_name, hook.command),
    );
    push_process_log(
        inner_arc,
        change_tx,
        "stderr",
        format!(
            "Running {} hook: {}{}",
            phase,
            hook.command,
            if hook.args.is_empty() {
                String::new()
            } else {
                format!(" {}", hook.args.join(" "))
            }
        ),
    )
    .await;

    let mut cmd = Command::new(&hook.command);
    cmd.args(&hook.args);
    cmd.envs(&app_config.env);
    cmd.envs(&hook.env);
    if let Some(workdir) = hook.workdir.as_ref().or(app_config.workdir.as_ref()) {
        cmd.current_dir(workdir);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(HookError::Spawn)?;

    if let Some(stdout) = child.stdout.take() {
        capture_hook_output(
            inner_arc.clone(),
            change_tx.clone(),
            format!("{}:stdout", phase),
            stdout,
        );
    }

    if let Some(stderr) = child.stderr.take() {
        capture_hook_output(
            inner_arc.clone(),
            change_tx.clone(),
            format!("{}:stderr", phase),
            stderr,
        );
    }

    let status = child.wait().await.map_err(HookError::Wait)?;
    if status.success() {
        info!(name = %app_name, hook = %hook.command, phase, "Hook completed successfully");
        return Ok(());
    }

    Err(HookError::Exit(status.code().unwrap_or(-1)))
}

fn capture_hook_output(
    inner_arc: Arc<RwLock<Inner>>,
    change_tx: broadcast::Sender<()>,
    stream: String,
    pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            push_process_log(&inner_arc, &change_tx, &stream, line).await;
        }
    });
}

fn capture_process_output(
    app_name: String,
    stream: &'static str,
    pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    inner_arc: Arc<RwLock<Inner>>,
    change_tx: broadcast::Sender<()>,
    telemetry: Arc<TelemetrySink>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            telemetry.emit_process_output(&app_name, stream, &line);
            push_process_log(&inner_arc, &change_tx, stream, line).await;
        }
    });
}

async fn push_process_log(
    inner_arc: &Arc<RwLock<Inner>>,
    change_tx: &broadcast::Sender<()>,
    stream: &str,
    message: String,
) {
    let mut inner = inner_arc.write().await;
    inner.push_log(stream, message);
    let _ = change_tx.send(());
}

/// Central registry that owns all managed processes.
pub struct ProcessManager {
    pub processes: Vec<Arc<Mutex<ManagedProcess>>>,
    start_order: Vec<usize>,
    index_by_name: HashMap<String, usize>,
    health_tasks: std::sync::Mutex<Vec<JoinHandle<()>>>,
    watch_tasks: std::sync::Mutex<Vec<JoinHandle<()>>>,
    telemetry: Arc<TelemetrySink>,
}

impl ProcessManager {
    pub fn new(configs: Vec<AppConfig>, telemetry: Arc<TelemetrySink>) -> Result<Self> {
        let start_order = resolve_start_order(&configs)?;
        let index_by_name = configs
            .iter()
            .enumerate()
            .map(|(index, config)| (config.name.clone(), index))
            .collect();
        let processes = configs
            .into_iter()
            .map(|c| Arc::new(Mutex::new(ManagedProcess::new(c, telemetry.clone()))))
            .collect();
        Ok(Self {
            processes,
            start_order,
            index_by_name,
            health_tasks: std::sync::Mutex::new(Vec::new()),
            watch_tasks: std::sync::Mutex::new(Vec::new()),
            telemetry,
        })
    }

    /// Start all processes concurrently.
    pub async fn start_all(&self) {
        self.telemetry.emit("rally:lifecycle", "starting all configured apps".to_owned());
        for index in &self.start_order {
            let p = self.processes[*index].lock().await;
            p.start().await;
        }
    }

    /// Kill all running processes.
    pub async fn kill_all(&self) {
        self.telemetry.emit("rally:lifecycle", "stopping all configured apps".to_owned());
        for index in self.start_order.iter().rev() {
            let p = self.processes[*index].lock().await;
            p.kill().await;
        }
    }

    pub async fn restart_by_name(&self, name: &str, reason: &str) -> bool {
        let Some(index) = self.index_by_name.get(name).copied() else {
            return false;
        };

        self.restart_by_index(index, reason).await;
        true
    }

    pub async fn restart_by_index(&self, index: usize, reason: &str) {
        let proc = self.processes[index].clone();

        {
            let p = proc.lock().await;
            {
                let mut inner = p.inner.write().await;
                inner.last_restart_reason = Some(reason.to_owned());
                let _ = p.change_tx.send(());
            }
            p.telemetry.emit(
                "rally:process",
                format!("restart requested for {} reason={}", p.config.name, reason),
            );
            p.kill().await;
        }

        self.wait_for_shutdown(&proc).await;

        let p = proc.lock().await;
        p.start().await;
    }

    pub fn register_health_task(&self, task: JoinHandle<()>) {
        let mut tasks = self.health_tasks.lock().unwrap();
        tasks.push(task);
    }

    pub fn abort_health_tasks(&self) {
        let mut tasks = self.health_tasks.lock().unwrap();
        for task in tasks.drain(..) {
            task.abort();
        }
    }

    pub fn register_watch_task(&self, task: JoinHandle<()>) {
        let mut tasks = self.watch_tasks.lock().unwrap();
        tasks.push(task);
    }

    pub fn abort_watch_tasks(&self) {
        let mut tasks = self.watch_tasks.lock().unwrap();
        for task in tasks.drain(..) {
            task.abort();
        }
    }

    /// Snapshot of all process statuses.
    pub async fn all_statuses(&self) -> Vec<ProcessStatus> {
        let mut out = Vec::with_capacity(self.processes.len());
        for proc in &self.processes {
            let p = proc.lock().await;
            out.push(p.status().await);
        }
        out
    }

    /// Start a health-check polling task for a single process.
    pub fn spawn_health_checker(
        proc: Arc<Mutex<ManagedProcess>>,
        http_client: reqwest::Client,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let (url, interval_secs, inner_arc, change_tx) = {
                let p = proc.lock().await;
                let url = match p.config.health_url.clone() {
                    Some(u) => u,
                    None => return,
                };
                (
                    url,
                    p.config.health_interval_secs,
                    p.inner.clone(),
                    p.change_tx.clone(),
                )
            };

            let interval = std::time::Duration::from_secs(interval_secs);
            loop {
                tokio::time::sleep(interval).await;

                // Only check if the process is running
                let is_running = {
                    let inner = inner_arc.read().await;
                    inner.state == ProcessState::Running
                };
                if !is_running {
                    continue;
                }

                let new_health = match http_client.get(&url).send().await {
                    Ok(resp) if resp.status().is_success() => HealthStatus::Healthy,
                    _ => HealthStatus::Unhealthy,
                };

                {
                    let mut inner = inner_arc.write().await;
                    inner.health = new_health;
                    let _ = change_tx.send(());
                }
            }
        })
    }

    pub fn spawn_watch_task(manager: SharedManager, index: usize) -> JoinHandle<()> {
        tokio::spawn(async move {
            let (config, telemetry) = {
                let p = manager.processes[index].lock().await;
                (p.config.clone(), p.telemetry.clone())
            };

            let watch_targets = collect_watch_targets(&config);
            if watch_targets.is_empty() {
                return;
            }

            let debounce_millis = config
                .watch
                .as_ref()
                .map(|watch| watch.debounce_millis.max(50))
                .unwrap_or(500);
            let app_name = config.name.clone();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
            let callback_name = app_name.clone();
            let callback_telemetry = telemetry.clone();
            let mut watcher = match recommended_watcher(move |result: notify::Result<notify::Event>| {
                match result {
                    Ok(event) => {
                        if event.paths.is_empty() {
                            let _ = tx.send(PathBuf::new());
                        } else {
                            for path in event.paths {
                                let _ = tx.send(path);
                            }
                        }
                    }
                    Err(error) => {
                        warn!(name = %callback_name, error = %error, "Watch error");
                        callback_telemetry.emit(
                            "rally:watch",
                            format!("watch error for {}: {}", callback_name, error),
                        );
                    }
                }
            }) {
                Ok(watcher) => watcher,
                Err(error) => {
                    warn!(name = %app_name, error = %error, "Failed to create watcher");
                    telemetry.emit(
                        "rally:watch",
                        format!("failed to create watcher for {}: {}", app_name, error),
                    );
                    return;
                }
            };

            let mut watched_count = 0usize;
            for target in watch_targets {
                let mode = if target.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };

                match watcher.watch(&target.path, mode) {
                    Ok(()) => watched_count += 1,
                    Err(error) => {
                        warn!(name = %app_name, path = %target.path.display(), error = %error, "Failed to watch path");
                        telemetry.emit(
                            "rally:watch",
                            format!(
                                "failed to watch {} for {}: {}",
                                target.path.display(),
                                app_name,
                                error
                            ),
                        );
                    }
                }
            }

            if watched_count == 0 {
                return;
            }

            telemetry.emit(
                "rally:watch",
                format!("watching {} target(s) for {}", watched_count, app_name),
            );

            while let Some(mut changed_path) = rx.recv().await {
                let debounce = tokio::time::sleep(std::time::Duration::from_millis(debounce_millis));
                tokio::pin!(debounce);

                loop {
                    tokio::select! {
                        _ = &mut debounce => break,
                        next = rx.recv() => match next {
                            Some(path) => changed_path = path,
                            None => return,
                        }
                    }
                }

                let reason = if changed_path.as_os_str().is_empty() {
                    "watch change".to_owned()
                } else {
                    format!("watch change: {}", changed_path.display())
                };

                telemetry.emit(
                    "rally:watch",
                    format!("detected {} for {}", reason, app_name),
                );
                manager.restart_by_index(index, &reason).await;
            }
        })
    }

    async fn wait_for_shutdown(&self, proc: &Arc<Mutex<ManagedProcess>>) {
        for _ in 0..50 {
            let p = proc.lock().await;
            let active = p.is_supervisor_active().await;
            drop(p);
            if !active {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

fn collect_watch_targets(config: &AppConfig) -> Vec<WatchTarget> {
    let mut targets = HashMap::<PathBuf, bool>::new();

    if let Some(watch) = &config.watch {
        for path in &watch.paths {
            insert_watch_target(&mut targets, resolve_watch_path(config, path), watch.recursive);
        }
    }

    if looks_like_local_path(&config.command) {
        insert_watch_target(&mut targets, resolve_watch_path(config, &config.command), false);
    }

    targets
        .into_iter()
        .map(|(path, recursive)| WatchTarget { path, recursive })
        .collect()
}

fn insert_watch_target(targets: &mut HashMap<PathBuf, bool>, path: PathBuf, recursive: bool) {
    let (watch_path, watch_recursive) = normalize_watch_target(path, recursive);
    targets
        .entry(watch_path)
        .and_modify(|existing| *existing = *existing || watch_recursive)
        .or_insert(watch_recursive);
}

fn normalize_watch_target(path: PathBuf, recursive: bool) -> (PathBuf, bool) {
    if path.exists() {
        return (path.clone(), recursive && path.is_dir());
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && parent.exists() {
            return (parent.to_path_buf(), false);
        }
    }

    (path, false)
}

fn resolve_watch_path(config: &AppConfig, raw_path: &str) -> PathBuf {
    let path = PathBuf::from(raw_path);
    if path.is_relative() {
        if let Some(workdir) = &config.workdir {
            return Path::new(workdir).join(path);
        }
    }
    path
}

fn looks_like_local_path(command: &str) -> bool {
    command.starts_with('.') || Path::new(command).components().count() > 1
}

fn resolve_start_order(configs: &[AppConfig]) -> Result<Vec<usize>> {
    let mut index_by_name = HashMap::with_capacity(configs.len());
    for (index, config) in configs.iter().enumerate() {
        if index_by_name.insert(config.name.clone(), index).is_some() {
            return Err(anyhow!("Duplicate app name: {}", config.name));
        }
    }

    let mut incoming_edges = vec![0usize; configs.len()];
    let mut dependents = vec![Vec::new(); configs.len()];

    for (index, config) in configs.iter().enumerate() {
        let mut seen = HashSet::new();
        for dependency in &config.depends_on {
            if !seen.insert(dependency) {
                return Err(anyhow!(
                    "App {} lists dependency {} more than once",
                    config.name,
                    dependency
                ));
            }

            let dependency_index = *index_by_name
                .get(dependency)
                .ok_or_else(|| anyhow!("App {} depends on unknown app {}", config.name, dependency))?;
            if dependency_index == index {
                return Err(anyhow!("App {} cannot depend on itself", config.name));
            }

            incoming_edges[index] += 1;
            dependents[dependency_index].push(index);
        }
    }

    let mut queue = VecDeque::new();
    for (index, count) in incoming_edges.iter().enumerate() {
        if *count == 0 {
            queue.push_back(index);
        }
    }

    let mut ordered = Vec::with_capacity(configs.len());
    while let Some(index) = queue.pop_front() {
        ordered.push(index);
        for dependent in &dependents[index] {
            incoming_edges[*dependent] -= 1;
            if incoming_edges[*dependent] == 0 {
                queue.push_back(*dependent);
            }
        }
    }

    if ordered.len() != configs.len() {
        let blocked: Vec<_> = configs
            .iter()
            .enumerate()
            .filter_map(|(index, config)| (incoming_edges[index] > 0).then_some(config.name.clone()))
            .collect();
        return Err(anyhow!(
            "Dependency cycle detected involving: {}",
            blocked.join(", ")
        ));
    }

    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::{collect_watch_targets, resolve_start_order};
    use crate::config::{AppConfig, WatchConfig};
    use crate::sink::TelemetrySink;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn app(name: &str, depends_on: &[&str]) -> AppConfig {
        AppConfig {
            name: name.to_owned(),
            command: name.to_owned(),
            workdir: None,
            args: Vec::new(),
            env: HashMap::new(),
            depends_on: depends_on.iter().map(|value| (*value).to_owned()).collect(),
            before: Vec::new(),
            after: Vec::new(),
            watch: None,
            health_url: None,
            health_interval_secs: 10,
            restart_on_exit: false,
            log_lines: 500,
        }
    }

    #[test]
    fn resolves_dependency_order() {
        let order = resolve_start_order(&[
            app("api", &["db"]),
            app("frontend", &["api"]),
            app("db", &[]),
        ])
        .unwrap();

        assert_eq!(order, vec![2, 0, 1]);
    }

    #[test]
    fn rejects_unknown_dependency() {
        let error = resolve_start_order(&[app("api", &["db"])]).unwrap_err();
        assert!(error.to_string().contains("unknown app db"));
    }

    #[test]
    fn rejects_dependency_cycle() {
        let error = resolve_start_order(&[app("api", &["worker"]), app("worker", &["api"])]).unwrap_err();
        assert!(error.to_string().contains("Dependency cycle detected"));
    }

    #[test]
    fn process_manager_accepts_disabled_sink() {
        let manager = super::ProcessManager::new(
            vec![app("api", &[])],
            Arc::new(TelemetrySink::new(None)),
        )
        .unwrap();

        assert_eq!(manager.processes.len(), 1);
    }

    #[test]
    fn collects_watch_targets_from_watch_paths_and_command() {
        let temp_root = std::env::temp_dir().join(format!(
            "rally-watch-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(temp_root.join("config")).unwrap();
        std::fs::create_dir_all(temp_root.join("target/debug")).unwrap();

        let mut app = app("api", &[]);
        app.workdir = Some(temp_root.display().to_string());
        app.command = "./target/debug/api".to_owned();
        app.watch = Some(WatchConfig {
            paths: vec!["config/dev.toml".to_owned()],
            recursive: true,
            debounce_millis: 500,
        });

        let targets = collect_watch_targets(&app)
            .into_iter()
            .map(|target| target.path)
            .collect::<Vec<_>>();

        assert!(targets.contains(&temp_root.join("config")));
        assert!(targets.contains(&temp_root.join("target/debug")));

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}

/// Trivially-serialisable wrapper so we can pass `Arc<Mutex<ManagedProcess>>`
/// through axum state without extra boilerplate.
pub type SharedManager = Arc<ProcessManager>;
