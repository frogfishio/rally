use crate::config::AppConfig;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, Mutex, RwLock};
use tracing::{error, info, warn};

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
}

impl ManagedProcess {
    pub fn new(config: AppConfig) -> Self {
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

        tokio::spawn(async move {
            loop {
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
                        let mut inner = inner_arc.write().await;
                        inner.state = ProcessState::Failed;
                        inner.push_log("stderr", format!("Failed to spawn: {}", e));
                        let _ = change_tx.send(());
                        break;
                    }
                    Ok(mut child) => {
                        let pid = child.id();
                        info!(name = %config.name, pid = ?pid, "Process started");

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
                                inner.push_log(
                                    "stderr",
                                    format!("Process exited with code {}", code),
                                );
                            }
                            Err(e) => {
                                inner.state = ProcessState::Killed;
                                warn!(name = %config.name, error = %e, "Process wait error");
                                inner.push_log("stderr", format!("Process wait error: {}", e));
                            }
                        }
                        let _ = change_tx.send(());

                        if config.restart_on_exit
                            && !matches!(inner.state, ProcessState::Killed)
                        {
                            inner.restart_count += 1;
                            let count = inner.restart_count;
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

/// Central registry that owns all managed processes.
pub struct ProcessManager {
    pub processes: Vec<Arc<Mutex<ManagedProcess>>>,
}

impl ProcessManager {
    pub fn new(configs: Vec<AppConfig>) -> Self {
        let processes = configs
            .into_iter()
            .map(|c| Arc::new(Mutex::new(ManagedProcess::new(c))))
            .collect();
        Self { processes }
    }

    /// Start all processes concurrently.
    pub async fn start_all(&self) {
        for proc in &self.processes {
            let p = proc.lock().await;
            p.start().await;
        }
    }

    /// Kill all running processes.
    pub async fn kill_all(&self) {
        for proc in &self.processes {
            let p = proc.lock().await;
            p.kill().await;
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
    ) {
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
        });
    }
}

/// Trivially-serialisable wrapper so we can pass `Arc<Mutex<ManagedProcess>>`
/// through axum state without extra boilerplate.
pub type SharedManager = Arc<ProcessManager>;
