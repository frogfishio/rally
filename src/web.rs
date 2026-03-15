use crate::process_manager::SharedManager;
use crate::ui::dashboard_html;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Sse},
    routing::{get, post},
    Json, Router,
};
use axum::response::sse::{Event, KeepAlive};
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::StreamExt;

pub fn router(manager: SharedManager) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/api/status", get(status_handler))
        .route("/api/kill/{name}", post(kill_handler))
        .route("/api/restart/{name}", post(restart_handler))
        .route("/api/clear-logs/{name}", post(clear_logs_handler))
        .route("/api/events", get(sse_handler))
        .with_state(manager)
}

async fn index_handler() -> Html<&'static str> {
    Html(dashboard_html())
}

async fn status_handler(State(mgr): State<SharedManager>) -> impl IntoResponse {
    let statuses = mgr.all_statuses().await;
    Json(statuses)
}

async fn kill_handler(
    Path(name): Path<String>,
    State(mgr): State<SharedManager>,
) -> impl IntoResponse {
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
    State(mgr): State<SharedManager>,
) -> impl IntoResponse {
    for proc in &mgr.processes {
        let p = proc.lock().await;
        if p.config.name == name {
            p.kill().await;
            // Small delay so kill propagates before restart
            drop(p);
            tokio::time::sleep(Duration::from_millis(300)).await;
            let p = proc.lock().await;
            p.start().await;
            return StatusCode::OK;
        }
    }
    StatusCode::NOT_FOUND
}

async fn clear_logs_handler(
    Path(name): Path<String>,
    State(mgr): State<SharedManager>,
) -> impl IntoResponse {
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
    State(mgr): State<SharedManager>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
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
