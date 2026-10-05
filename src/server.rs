use crate::{
    collector::{Collector, ScanReport},
    store::Store,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

pub struct AppState {
    pub store: Mutex<Store>,
    pub report: Mutex<ScanReport>,
    pub project: String,
    pub sessions_root: String,
    pub poll_seconds: u64,
    pub port: u16,
}

#[derive(Serialize)]
struct Status<'a> {
    project: &'a str,
    sessions_root: &'a str,
    poll_seconds: u64,
    scan: ScanReport,
    version: &'a str,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/public/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/public/app.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/public/style.css"),
                )
            }),
        )
        .route("/api/status", get(status))
        .route("/api/sessions", get(sessions))
        .route("/api/sessions/{id}/events", get(events))
        .route("/api/events/{id}/evidence", get(evidence))
        .layer(middleware::from_fn_with_state(state.clone(), local_only))
        .with_state(state)
}

async fn local_only(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let allowed = [
        format!("127.0.0.1:{}", state.port),
        format!("localhost:{}", state.port),
    ];
    let valid_host = host.is_some_and(|h| allowed.iter().any(|a| a == h));
    let valid_origin = headers.get(header::ORIGIN).is_none_or(|origin| {
        origin
            .to_str()
            .is_ok_and(|o| allowed.iter().any(|a| o == format!("http://{a}")))
    });
    // Loopback binding alone doesn't prevent DNS rebinding to local transcript APIs.
    if !valid_host || !valid_origin {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    h.insert(header::CONTENT_SECURITY_POLICY,header::HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    Ok::<_, StatusCode>(response).into_response()
}

fn failure(_: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    // Never include transcript contents or SQL arguments in browser errors.
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error":"本地数据暂时无法读取，请查看终端状态。"})),
    )
}

async fn status(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let report = state.report.lock().map_err(failure)?.clone();
    Ok(Json(Status {
        project: &state.project,
        sessions_root: &state.sessions_root,
        poll_seconds: state.poll_seconds,
        scan: report,
        version: env!("CARGO_PKG_VERSION"),
    })
    .into_response())
}

async fn sessions(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    Ok(Json(
        state
            .store
            .lock()
            .map_err(failure)?
            .sessions()
            .map_err(failure)?,
    ))
}

#[derive(Default, Deserialize)]
struct Window {
    after: Option<i64>,
    before: Option<i64>,
    limit: Option<usize>,
}

async fn events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(window): Query<Window>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if window.after.is_some() && window.before.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"不能同时请求新增和更早记录"})),
        ));
    }
    Ok(Json(
        state
            .store
            .lock()
            .map_err(failure)?
            .events(
                &id,
                window.after,
                window.before,
                window.limit.unwrap_or(100),
            )
            .map_err(failure)?,
    ))
}

async fn evidence(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let rows = state
        .store
        .lock()
        .map_err(failure)?
        .evidence(id)
        .map_err(failure)?;
    if rows.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error":"未找到对应证据"})),
        ));
    }
    Ok(Json(rows))
}

pub async fn collect_loop(
    state: Arc<AppState>,
    collector: Collector,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        let state_for_scan = state.clone();
        let worker = Collector {
            project: collector.project.clone(),
            sessions_root: collector.sessions_root.clone(),
        };
        let result = tokio::task::spawn_blocking(move || {
            let mut store = state_for_scan.store.lock().map_err(|_| "数据库锁不可用")?;
            Ok::<_, &str>(worker.scan(&mut store))
        })
        .await;
        if let Ok(mut report) = state.report.lock() {
            match result {
                Ok(Ok(scan)) => *report = scan,
                _ => report.errors = vec!["采集任务失败，将重试；已有数据仍然保留。".into()],
            }
        }
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(state.poll_seconds)) => {},
            _ = stop.changed() => { break; }
        }
    }
}
