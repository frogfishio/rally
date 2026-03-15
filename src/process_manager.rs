use crate::config::{AppConfig, HookConfig};
use crate::sink::TelemetrySink;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet, VecDeque};
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
    logs: VecDeque<LogLine>,
    pub(crate) max_log_lines: usize,
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
            logs: VecDeque::new(),
            max_log_lines: config.log_lines,
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

    /// Spawn (or re-spawn) the process and return immediately.
    /// A background task monitors the child process.
    pub async fn start(&self) {
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
                            let inner_c = inner_arc.clone();
                            let change_c = change_tx.clone();
                            tokio::spawn(async move {
                                let mut reader = BufReader::new(stdout).lines();
                                while let Ok(Some(line)) = reader.next_line().await {
                                    let mut inner = inner_c.write().await;
                                    inner.push_log("stdout", line);
                                    let _ = change_c.send(());
                                    drop(inner);
                                }
                            });
                        }

                        // Capture stderr
                        if let Some(stderr) = child.stderr.take() {
                            let inner_c = inner_arc.clone();
                            let change_c = change_tx.clone();
                            tokio::spawn(async move {
                                let mut reader = BufReader::new(stderr).lines();
                                while let Ok(Some(line)) = reader.next_line().await {
                                    let mut inner = inner_c.write().await;
                                    inner.push_log("stderr", line);
                                    let _ = change_c.send(());
                                    drop(inner);
                                }
                            });
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
        });
    }
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
    health_tasks: std::sync::Mutex<Vec<JoinHandle<()>>>,
    telemetry: Arc<TelemetrySink>,
}

impl ProcessManager {
    pub fn new(configs: Vec<AppConfig>, telemetry: Arc<TelemetrySink>) -> Result<Self> {
        let start_order = resolve_start_order(&configs)?;
        let processes = configs
            .into_iter()
            .map(|c| Arc::new(Mutex::new(ManagedProcess::new(c, telemetry.clone()))))
            .collect();
        Ok(Self {
            processes,
            start_order,
            health_tasks: std::sync::Mutex::new(Vec::new()),
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
    use super::resolve_start_order;
    use crate::config::AppConfig;
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
}

/// Trivially-serialisable wrapper so we can pass `Arc<Mutex<ManagedProcess>>`
/// through axum state without extra boilerplate.
pub type SharedManager = Arc<ProcessManager>;
