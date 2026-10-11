mod projection;

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
};

use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode, header, uri::Authority},
    middleware::{self, Next},
    response::{
        Html, IntoResponse, Response, Sse,
        sse::{Event as SseEvent, KeepAlive},
    },
    routing::get,
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use network_monitor_web_types::{ApiError, LiveSnapshot, RecordingDetail, RecordingSummary};
use thiserror::Error;
use tokio::{
    net::TcpListener,
    sync::watch,
    task::{JoinHandle, JoinSet},
};
use tower_http::services::ServeDir;

use crate::{storage::StorageError, ui::View};

#[derive(Debug, Error)]
pub enum WebError {
    #[error("web server failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("web server task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("Recording not found")]
    NotFound,
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Io(_) | Self::Task(_) | Self::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ApiError {
                message: self.to_string(),
            }),
        )
            .into_response()
    }
}

#[derive(Clone)]
struct AppState {
    database: PathBuf,
    views: watch::Receiver<Option<View>>,
    stopping: watch::Receiver<bool>,
}

pub struct WebServer {
    pub address: SocketAddr,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<Result<(), std::io::Error>>>,
}

impl WebServer {
    pub async fn shutdown(mut self) -> Result<(), WebError> {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub async fn start(
    port: u16,
    database: PathBuf,
    views: watch::Receiver<Option<View>>,
    assets: PathBuf,
) -> Result<WebServer, WebError> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let address = listener.local_addr()?;
    let (shutdown, mut stopping) = watch::channel(false);
    let app = Router::new()
        .route("/api/live", get(live))
        .route("/api/live/stream", get(live_stream))
        .route("/api/recordings", get(recordings))
        .route("/api/recordings/{id}", get(recording))
        .route("/api/{*path}", get(api_not_found))
        .route("/api", get(api_not_found));
    let app = if assets.join("index.html").is_file() {
        app.fallback_service(ServeDir::new(assets))
    } else {
        app.fallback(missing_assets)
    }
    .layer(middleware::from_fn_with_state(address, local_requests))
    .with_state(AppState {
        database,
        views,
        stopping: stopping.clone(),
    });
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _changed = stopping.changed() => break,
                _finished = connections.join_next(), if !connections.is_empty() => {}
                accepted = listener.accept() => {
                    let (stream, _address) = accepted?;
                    let service = TowerToHyperService::new(app.clone());
                    connections.spawn(async move {
                        let _ = Builder::new(TokioExecutor::new())
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            }
        }
        drop(listener);
        // A stalled reader must not keep the monitor alive after it stops recording.
        connections.shutdown().await;
        Ok(())
    });
    Ok(WebServer {
        address,
        shutdown,
        task: Some(task),
    })
}

async fn local_requests(
    State(address): State<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .and_then(|host| host.parse::<Authority>().ok());
    let allowed = host.is_some_and(|host| {
        (host.host() == "127.0.0.1" || host.host().eq_ignore_ascii_case("localhost"))
            && host.port_u16().unwrap_or(80) == address.port()
    });
    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiError {
                message: "Use the local network monitor address".into(),
            }),
        )
            .into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn api_not_found() -> WebError {
    WebError::NotFound
}

async fn missing_assets() -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Html(
            "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Network monitor</title><body><h1>Website assets are missing</h1><p>Build the website with <code>./scripts/build-web.sh</code> and restart the monitor.</p></body></html>",
        ),
    )
}

async fn live(State(state): State<AppState>) -> Json<Option<LiveSnapshot>> {
    Json(state.views.borrow().as_ref().map(projection::live))
}

async fn live_stream(State(mut state): State<AppState>) -> impl IntoResponse {
    let stream = async_stream::stream! {
        let initial = { state.views.borrow_and_update().as_ref().map(projection::live) };
        yield SseEvent::default().event("snapshot").json_data(initial);
        loop {
            tokio::select! {
                biased;
                _changed = state.stopping.changed() => break,
                changed = state.views.changed() => {
                    if changed.is_err() { break; }
                    let snapshot = { state.views.borrow_and_update().as_ref().map(projection::live) };
                    yield SseEvent::default().event("snapshot").json_data(snapshot);
                }
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn recordings(
    State(state): State<AppState>,
) -> Result<Json<Vec<RecordingSummary>>, WebError> {
    let current = state
        .views
        .borrow()
        .as_ref()
        .map(|view| view.session_id.clone());
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            projection::recordings(&state.database, current.as_deref())
        })
        .await??,
    ))
}

async fn recording(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RecordingDetail>, WebError> {
    let current = state
        .views
        .borrow()
        .as_ref()
        .map(|view| view.session_id.clone());
    let detail = tokio::task::spawn_blocking(move || {
        projection::recording(&state.database, &id, current.as_deref())
    })
    .await??;
    detail.map(Json).ok_or(WebError::NotFound)
}
