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
use futures_util::stream;
use std::path::PathBuf;
use std::sync::Arc;
use std::convert::Infallible;
use tokio::sync::{RwLock, watch};

#[cfg(test)]
use axum::{body::{to_bytes, Body}, http::Request};

#[cfg(test)]
use tower::util::ServiceExt;

pub struct AppState {
    config_path: PathBuf,
    http_client: reqwest::Client,
    manager: RwLock<SharedManager>,
    telemetry: Arc<TelemetrySink>,
    shutdown_tx: watch::Sender<bool>,
}

pub type SharedAppState = Arc<AppState>;

impl AppState {
    pub fn new(
        config_path: PathBuf,
        http_client: reqwest::Client,
        manager: SharedManager,
        telemetry: Arc<TelemetrySink>,
    ) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            config_path,
            http_client,
            manager: RwLock::new(manager),
            telemetry,
            shutdown_tx,
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
        let cfg = config::load_with_telemetry(&self.config_path, Some(self.telemetry.as_ref()))?;
        let new_manager = Arc::new(crate::process_manager::ProcessManager::new(
            cfg.app.clone(),
            self.telemetry.clone(),
            cfg.env_provider_info.clone(),
        )?);

        let mut manager = self.manager.write().await;
        let old_manager = std::mem::replace(&mut *manager, new_manager.clone());
        drop(manager);

        old_manager.shutdown_all().await;
        old_manager.abort_health_tasks();
        old_manager.abort_watch_tasks();
        new_manager.start_all().await;
        spawn_health_checkers(&new_manager, &self.http_client).await;
        spawn_watch_tasks(&new_manager).await;
        self.telemetry.emit("rally:lifecycle", "reload complete".to_owned());

        Ok(())
    }

    pub async fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        let manager = self.current_manager().await;
        manager.shutdown_all().await;
        manager.abort_health_tasks();
        manager.abort_watch_tasks();
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
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
        .route("/api/recheck/{name}", post(recheck_handler))
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

async fn recheck_handler(
    Path(name): Path<String>,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    let mgr = state.current_manager().await;
    control_status(mgr.restart_by_name(&name, "manual recheck").await)
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
    let shutdown_rx = state.subscribe_shutdown();

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

    let stream = stream::unfold((rx, shutdown_rx), |(mut rx, mut shutdown_rx)| async move {
        tokio::select! {
            message = rx.recv() => match message {
                Some(_) => Some((Ok::<Event, Infallible>(Event::default().data("update")), (rx, shutdown_rx))),
                None => None,
            },
            changed = shutdown_rx.changed() => match changed {
                Ok(()) if *shutdown_rx.borrow() => None,
                Ok(()) => Some((Ok::<Event, Infallible>(Event::default().data("update")), (rx, shutdown_rx))),
                Err(_) => None,
            }
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_manager::ProcessManager;
    use crate::sink::TelemetrySink;
    use serde_json::Value;
    use std::fs;

    fn temp_config_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rally-web-test-{}-{}-{}",
            name,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn reload_endpoint_rebuilds_manager_from_updated_config() {
        let temp_dir = temp_config_dir("reload");
        let config_path = temp_dir.join("rally.toml");
        fs::write(
            &config_path,
            r#"
[[app]]
name = "first"
command = "sleep"
args = ["1"]
"#,
        )
        .unwrap();

        let cfg = config::load(&config_path).unwrap();
        let manager = Arc::new(
            ProcessManager::new(cfg.app.clone(), Arc::new(TelemetrySink::new(None)), cfg.env_provider_info.clone())
                .unwrap(),
        );
        let http_client = reqwest::Client::builder().build().unwrap();
        let state = Arc::new(AppState::new(
            config_path.clone(),
            http_client,
            manager,
            Arc::new(TelemetrySink::new(None)),
        ));
        let app = router(state);

        fs::write(
            &config_path,
            r#"
[[app]]
name = "second"
command = "sleep"
args = ["1"]
"#,
        )
        .unwrap();

        let reload_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload_response.status(), StatusCode::OK);

        let status_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status_response.status(), StatusCode::OK);

        let body = to_bytes(status_response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload.as_array().unwrap().len(), 1);
        assert_eq!(payload[0]["name"], "second");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
