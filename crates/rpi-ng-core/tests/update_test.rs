//! Update-checker tests against a mock of the GitHub releases API.
//!
//! The mock reproduces the response shapes GitHub actually sends (including
//! the 404 "Not Found" body when a repo has no releases and the 403 rate-limit
//! body), so the client's classification logic is exercised against realistic
//! inputs without touching the network.

use std::{net::SocketAddr, time::Duration};

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use serde_json::json;

use rpi_ng_core::update::{self, UpdateError};

async fn mock_api() -> SocketAddr {
    let app = Router::new()
        .route(
            "/newer",
            get(|| async {
                Json(json!({
                    "tag_name": "v9.9.9",
                    "html_url": "https://example.com/rpi-ng/tag/v9.9.9"
                }))
            }),
        )
        .route(
            "/same",
            get(|| async {
                Json(json!({
                    "tag_name": "v1.2.3",
                    "html_url": "https://example.com/rpi-ng/tag/v1.2.3"
                }))
            }),
        )
        .route(
            "/older",
            get(|| async {
                Json(json!({
                    "tag_name": "v0.0.1",
                    "html_url": "https://example.com/rpi-ng/tag/v0.0.1"
                }))
            }),
        )
        .route("/no-release", get(|| async { not_found() }))
        .route(
            "/rate-limit",
            get(|| async {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({"message": "API rate limit exceeded for 1.2.3.4."})),
                )
            }),
        )
        .route("/garbage", get(|| async { "this is not json" }))
        .route(
            "/empty-tag",
            get(|| async { Json(json!({"tag_name": "", "html_url": "https://x"})) }),
        )
        .route(
            "/no-url",
            get(|| async { Json(json!({"tag_name": "v1.2.4"})) }),
        );

    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"message": "Not Found", "documentation_url": "https://docs.github.com"})),
    )
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("rpi-ng-test")
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

async fn check(path: &str, current: &str) -> Result<Option<update::UpdateInfo>, UpdateError> {
    let addr = mock_api().await;
    let url = format!("http://{addr}{path}");
    update::check_latest(&client(), &url, current).await
}

#[tokio::test]
async fn newer_release_is_reported() {
    let found = check("/newer", "1.2.3")
        .await
        .unwrap()
        .expect("update expected");
    assert_eq!(found.version, "9.9.9");
    assert_eq!(found.tag, "v9.9.9");
    assert_eq!(found.url, "https://example.com/rpi-ng/tag/v9.9.9");
}

#[tokio::test]
async fn same_release_reports_nothing() {
    assert!(check("/same", "1.2.3").await.unwrap().is_none());
}

#[tokio::test]
async fn older_release_reports_nothing() {
    assert!(check("/older", "1.2.3").await.unwrap().is_none());
}

#[tokio::test]
async fn repo_without_releases_reports_nothing() {
    // GitHub answers 404 here; that is "no news", not an error.
    assert!(check("/no-release", "1.2.3").await.unwrap().is_none());
}

#[tokio::test]
async fn rate_limit_is_classified() {
    let err = check("/rate-limit", "1.2.3").await.unwrap_err();
    assert!(matches!(err, UpdateError::RateLimited), "got {err:?}");
}

#[tokio::test]
async fn malformed_body_is_an_error() {
    let err = check("/garbage", "1.2.3").await.unwrap_err();
    assert!(matches!(err, UpdateError::Malformed(_)), "got {err:?}");
}

#[tokio::test]
async fn empty_tag_is_malformed() {
    let err = check("/empty-tag", "1.2.3").await.unwrap_err();
    assert!(matches!(err, UpdateError::Malformed(_)), "got {err:?}");
}

#[tokio::test]
async fn missing_html_url_falls_back_to_the_releases_page() {
    let found = check("/no-url", "1.2.3")
        .await
        .unwrap()
        .expect("update expected");
    assert_eq!(found.tag, "v1.2.4");
    assert_eq!(found.url, format!("{}/tag/v1.2.4", update::RELEASES_PAGE));
}

#[tokio::test]
async fn unreachable_api_is_a_network_error() {
    // Bind then drop a listener to get a refused port.
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let err = update::check_latest(&client(), &format!("http://{addr}/x"), "1.2.3")
        .await
        .unwrap_err();
    assert!(matches!(err, UpdateError::Network(_)), "got {err:?}");
}
