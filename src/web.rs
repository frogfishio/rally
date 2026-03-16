// SPDX-FileCopyrightText: 2026 Alexander R. Croft
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::config;
use crate::process_manager::{ControlResult, SharedManager};
use crate::sink::TelemetrySink;
use crate::ui::dashboard_html;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Sse},
    routing::{get, post},
    Json, Router,
};
use axum::response::sse::{Event, KeepAlive};
use std::path::PathBuf;
use std::sync::Arc;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_stream::StreamExt;

pub struct AppState {
    config_path: PathBuf,
    http_client: reqwest::Client,
    manager: RwLock<SharedManager>,
    telemetry: Arc<TelemetrySink>,
}

pub type SharedAppState = Arc<AppState>;

impl AppState {
    pub fn new(
        config_path: PathBuf,
        http_client: reqwest::Client,
        manager: SharedManager,
        telemetry: Arc<TelemetrySink>,
    ) -> Self {
        Self {
            config_path,
            http_client,
            manager: RwLock::new(manager),
            telemetry,
        }
    }

    pub async fn current_manager(&self) -> SharedManager {
        self.manager.read().await.clone()
    }

    pub async fn reload(&self) -> anyhow::Result<()> {
        self.telemetry.emit(
            "rally:lifecycle",
            format!("reloading config from {}", self.config_path.display()),
        );
        let cfg = config::load(&self.config_path)?;
        let new_manager = Arc::new(crate::process_manager::ProcessManager::new(
            cfg.app.clone(),
            self.telemetry.clone(),
        )?);

        let mut manager = self.manager.write().await;
        let old_manager = std::mem::replace(&mut *manager, new_manager.clone());
        drop(manager);

        old_manager.kill_all().await;
        old_manager.abort_health_tasks();
        old_manager.abort_watch_tasks();
        tokio::time::sleep(Duration::from_millis(300)).await;
        new_manager.start_all().await;
        spawn_health_checkers(&new_manager, &self.http_client).await;
        spawn_watch_tasks(&new_manager).await;
        self.telemetry.emit("rally:lifecycle", "reload complete".to_owned());

        Ok(())
    }

    pub async fn shutdown(&self) {
        let manager = self.current_manager().await;
        manager.kill_all().await;
        manager.abort_health_tasks();
        manager.abort_watch_tasks();
    }
}

pub async fn spawn_health_checkers(manager: &SharedManager, http_client: &reqwest::Client) {
    for proc in &manager.processes {
        let p = proc.lock().await;
        if p.config.health_url.is_some() {
            drop(p);
            let task = crate::process_manager::ProcessManager::spawn_health_checker(
                proc.clone(),
                http_client.clone(),
            );
            manager.register_health_task(task);
        }
    }
}

pub async fn spawn_watch_tasks(manager: &SharedManager) {
    for index in 0..manager.processes.len() {
        let task = crate::process_manager::ProcessManager::spawn_watch_task(manager.clone(), index);
        manager.register_watch_task(task);
    }
}

pub fn router(state: SharedAppState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/api/status", get(status_handler))
        .route("/api/start/{name}", post(start_handler))
        .route("/api/stop/{name}", post(stop_handler))
        .route("/api/enable/{name}", post(enable_handler))
        .route("/api/disable/{name}", post(disable_handler))
        .route("/api/kill/{name}", post(kill_handler))
        .route("/api/restart/{name}", post(restart_handler))
        .route("/api/reload", post(reload_handler))
        .route("/api/clear-logs/{name}", post(clear_logs_handler))
        .route("/api/events", get(sse_handler))
        .with_state(state)
}

async fn index_handler() -> Html<&'static str> {
    Html(dashboard_html())
}

async fn status_handler(State(state): State<SharedAppState>) -> impl IntoResponse {
    let mgr = state.current_manager().await;
    let statuses = mgr.all_statuses().await;
    Json(statuses)
}

fn control_status(result: ControlResult) -> StatusCode {
    match result {
        ControlResult::Ok => StatusCode::OK,
        ControlResult::NotFound => StatusCode::NOT_FOUND,
        ControlResult::Disabled => StatusCode::CONFLICT,
    }
}

async fn start_handler(
    Path(name): Path<String>,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    let mgr = state.current_manager().await;
    control_status(mgr.start_by_name(&name, "manual start").await)
}

async fn stop_handler(
    Path(name): Path<String>,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    let mgr = state.current_manager().await;
    control_status(mgr.stop_by_name(&name, "manual stop").await)
}

async fn enable_handler(
    Path(name): Path<String>,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    let mgr = state.current_manager().await;
    if mgr.set_enabled_by_name(&name, true).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn disable_handler(
    Path(name): Path<String>,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    let mgr = state.current_manager().await;
    if mgr.set_enabled_by_name(&name, false).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn kill_handler(
    Path(name): Path<String>,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    let mgr = state.current_manager().await;
    for proc in &mgr.processes {
        let p = proc.lock().await;
        if p.config.name == name {
            p.kill().await;
            return StatusCode::OK;
        }
    }
    StatusCode::NOT_FOUND
}

async fn restart_handler(
    Path(name): Path<String>,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    let mgr = state.current_manager().await;
    control_status(mgr.restart_by_name(&name, "manual restart").await)
}

async fn reload_handler(State(state): State<SharedAppState>) -> impl IntoResponse {
    match state.reload().await {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn clear_logs_handler(
    Path(name): Path<String>,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    let mgr = state.current_manager().await;
    for proc in &mgr.processes {
        let p = proc.lock().await;
        if p.config.name == name {
            p.clear_logs().await;
            return StatusCode::OK;
        }
    }
    StatusCode::NOT_FOUND
}

/// Server-sent events: merges change broadcasts from all processes into one stream.
async fn sse_handler(
    State(state): State<SharedAppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let mgr = state.current_manager().await;
    let (tx, rx) = tokio::sync::mpsc::channel::<()>(64);

    for proc in &mgr.processes {
        let p = proc.lock().await;
        let mut sub = p.change_tx.subscribe();
        let tx2 = tx.clone();
        tokio::spawn(async move {
            while sub.recv().await.is_ok() {
                if tx2.send(()).await.is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx)
        .map(|_| Ok::<Event, Infallible>(Event::default().data("update")));

    Sse::new(stream).keep_alive(KeepAlive::default())
}
