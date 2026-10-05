use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use demine::{
    collector::ScanReport,
    server::{AppState, router},
    store::Store,
};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

fn app() -> axum::Router {
    router(Arc::new(AppState {
        store: Mutex::new(Store::open(std::path::Path::new(":memory:"), "test").unwrap()),
        report: Mutex::new(ScanReport::default()),
        project: "test".into(),
        sessions_root: "fixtures".into(),
        poll_seconds: 2,
        port: 4317,
    }))
}

#[tokio::test]
async fn serves_bundled_ui_with_security_headers() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "127.0.0.1:4317")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("看见结果背后的过程"));
}

#[tokio::test]
async fn rejects_foreign_host_and_origin() {
    for (host, origin) in [
        ("attacker.example:4317", None),
        ("127.0.0.1:4317", Some("https://attacker.example")),
    ] {
        let mut request = Request::builder().uri("/api/sessions").header("host", host);
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = app()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn unknown_evidence_is_404_and_window_is_validated() {
    for (path, status) in [
        ("/api/events/999/evidence", StatusCode::NOT_FOUND),
        (
            "/api/sessions/unknown/events?before=5&after=3",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "localhost:4317")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
}

#[tokio::test]
async fn exposes_read_only_api() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions")
                .header("host", "127.0.0.1:4317")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
