//! File server tests: allowlist enforcement, Range requests, HEAD, encoding.

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use rpi_ng_core::fileserver::FileServer;

async fn start_server() -> (FileServer, SocketAddr) {
    // Bind loopback with an ephemeral port.
    let server = FileServer::start(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let bind = server.bind;
    (server, bind)
}

async fn get(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> (u16, Vec<u8>, Option<String>, Option<String>) {
    let mut req = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
        .get(format!("http://{addr}{path}"));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let content_range = resp.headers().get("content-range").map(|v| v.to_str().unwrap().into());
    let accept_ranges = resp.headers().get("accept-ranges").map(|v| v.to_str().unwrap().into());
    let body = resp.bytes().await.unwrap().to_vec();
    (status, body, content_range, accept_ranges)
}

#[tokio::test]
async fn serves_registered_file_completely() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("game.pkg");
    let data: Vec<u8> = (0u8..255u8).cycle().take(100_000).collect();
    std::fs::write(&path, &data).unwrap();

    let (server, addr) = start_server().await;
    let id = server.register(path, Some("game.pkg".into()));
    let url = server.url_for(id).unwrap();
    assert!(url.ends_with("/pkg/0/game.pkg"));

    let (status, body, _, accept) = get(addr, &format!("/pkg/{id}/game.pkg"), &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body.len(), 100_000);
    assert_eq!(accept.as_deref(), Some("bytes"));
    assert_eq!(body[..16], data[..16]);
}

#[tokio::test]
async fn unregistered_ids_404() {
    let (server, addr) = start_server().await;
    let _ = server;
    let (status, _, _, _) = get(addr, "/pkg/999/nope.pkg", &[]).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn no_traversal_even_with_dots() {
    let tmp = tempfile::tempdir().unwrap();
    let secret = tmp.path().join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();
    let pkg = tmp.path().join("a.pkg");
    std::fs::write(&pkg, "pkg-bytes").unwrap();

    let (server, addr) = start_server().await;
    let id = server.register(pkg, None);

    // Unregistered numeric id with traversal-shaped name → 404.
    let (status, _, _, _) = get(addr, "/pkg/999/..%2Fsecret.txt", &[]).await;
    assert_eq!(status, 404);

    // Path not matching any route → 404 (secret never reachable).
    let (status, body, _, _) = get(addr, "/pkg/../secret.txt", &[]).await;
    assert_eq!(status, 404);
    assert!(body != b"secret");

    // A traversal-shaped *name* on a registered id still serves only the
    // registered file — never anything from disk outside the allowlist.
    let (status, body, _, _) = get(addr, &format!("/pkg/{id}/%2e%2e%2fsecret.txt"), &[]).await;
    assert!(status == 200 || status == 404, "status was {status}");
    assert!(body != b"secret", "traversal must never leak other files");
    if status == 200 {
        assert_eq!(body, b"pkg-bytes");
    }
}

#[tokio::test]
async fn range_requests_single_and_suffix() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("game.pkg");
    let data: Vec<u8> = (0u8..=255u8).cycle().take(10_000).collect();
    std::fs::write(&path, &data).unwrap();

    let (server, addr) = start_server().await;
    let id = server.register(path, Some("game.pkg".into()));

    // Middle range.
    let (status, body, cr, _) = get(addr, &format!("/pkg/{id}/game.pkg"), &[("Range", "bytes=100-199")]).await;
    assert_eq!(status, 206);
    assert_eq!(body.len(), 100);
    assert_eq!(body[..10], data[100..110]);
    assert_eq!(cr.as_deref(), Some("bytes 100-199/10000"));

    // Open-ended range.
    let (status, body, _, _) = get(addr, &format!("/pkg/{id}/game.pkg"), &[("Range", "bytes=9900-")]).await;
    assert_eq!(status, 206);
    assert_eq!(body.len(), 100);

    // Suffix range: last 50 bytes.
    let (status, body, cr, _) = get(addr, &format!("/pkg/{id}/game.pkg"), &[("Range", "bytes=-50")]).await;
    assert_eq!(status, 206);
    assert_eq!(body.len(), 50);
    assert_eq!(body, data[9950..]);
    assert_eq!(cr.as_deref(), Some("bytes 9950-9999/10000"));

    // Unsatisfiable range → 416.
    let (status, _, _, _) = get(addr, &format!("/pkg/{id}/game.pkg"), &[("Range", "bytes=99999-")]).await;
    assert_eq!(status, 416);

    // Multi-range falls back to a full 200 response (RFC-permitted).
    let (status, body, _, _) = get(addr, &format!("/pkg/{id}/game.pkg"), &[("Range", "bytes=0-1,5-6")]).await;
    assert_eq!(status, 200);
    assert_eq!(body.len(), 10_000);
}

#[tokio::test]
async fn head_requests_have_headers_no_body() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("game.pkg");
    std::fs::write(&path, vec![7u8; 4096]).unwrap();

    let (server, addr) = start_server().await;
    let id = server.register(path, Some("game.pkg".into()));

    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .head(format!("http://{addr}/pkg/{id}/game.pkg"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("content-length").unwrap(), "4096");
    assert_eq!(resp.headers().get("accept-ranges").unwrap(), "bytes");
    let body = resp.bytes().await.unwrap();
    assert!(body.is_empty());
}

#[tokio::test]
async fn url_encoding_escapes_special_characters() {
    let (server, _addr) = start_server().await;
    let id = server.register(PathBuf::from("/tmp/my game+.pkg"), Some("my game+.pkg".into()));
    let url = server.url_for(id).unwrap();
    assert!(url.contains("my%20game+.pkg"), "url was: {url}");
}

#[tokio::test]
async fn unregister_removes_access() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("game.pkg");
    std::fs::write(&path, vec![1u8; 16]).unwrap();

    let (server, addr) = start_server().await;
    let id = server.register(path, Some("game.pkg".into()));
    let (status, _, _, _) = get(addr, &format!("/pkg/{id}/game.pkg"), &[]).await;
    assert_eq!(status, 200);

    server.unregister(id);
    let (status, _, _, _) = get(addr, &format!("/pkg/{id}/game.pkg"), &[]).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn concurrent_requests_stream_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("game.pkg");
    let data: Vec<u8> = (0u8..=255u8).cycle().take(300_000).collect();
    std::fs::write(&path, &data).unwrap();

    let (server, addr) = start_server().await;
    let id = server.register(path, Some("game.pkg".into()));

    let mut handles = Vec::new();
    for i in 0..4 {
        let url = format!("http://{addr}/pkg/{id}/game.pkg");
        let offset = i * 1000;
        let range = format!("bytes={offset}-{}", offset + 999);
        handles.push(tokio::spawn(async move {
            let resp = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap()
                .get(url)
                .header("Range", range)
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), 206);
            resp.bytes().await.unwrap().to_vec()
        }));
    }
    // All four requests ran concurrently; collect sequentially (join).
    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.unwrap());
    }
    for (i, body) in results.iter().enumerate() {
        let offset = i * 1000;
        assert_eq!(&body[..10], &data[offset..offset + 10]);
    }
}
