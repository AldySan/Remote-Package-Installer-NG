//! Integration tests against a faithful mock of the RPI (flatz) backend.
//!
//! The mock reproduces the documented behavior of the original payload
//! (`docs/BASELINE.md`), including its **invalid JSON with unquoted hex
//! literals** — so these tests lock in the quirks real consoles exhibit.
//! Mocks are protocol-conformance evidence only, **not** evidence of
//! real-console compatibility.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

use rpi_ng_core::{
    engine::Command,
    model::{Event, ItemState, Snapshot},
    rpi::{RpiClient, RpiError},
    settings::Settings,
};

// ---------------------------------------------------------------------------
// Mock console
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Dialect {
    /// The original payload: unquoted hex literals (invalid JSON).
    BrokenHex,
    /// Forks that emit strict JSON.
    Strict,
}

#[derive(Debug, Clone, Default)]
struct MockTask {
    total: u64,
    transferred: u64,
    local_copy: i32,
    error: i64,
}

#[derive(Debug)]
struct MockState {
    next_task_id: i64,
    tasks: std::collections::HashMap<i64, MockTask>,
    dialect: Dialect,
    /// Advance transferred by this much per progress poll.
    step: u64,
    /// On install, actually fetch the first package URL's header (Range),
    /// like the real prerequisite fetcher does.
    fetch_urls: bool,
    /// Captured install requests for assertions.
    installs: Vec<Value>,
    /// Content id → task id map for find_task.
    by_content: std::collections::HashMap<String, i64>,
    /// When set, install answers with this failure code.
    fail_installs: Option<i64>,
}

struct MockConsole {
    state: Arc<Mutex<MockState>>,
    addr: SocketAddr,
}

impl MockConsole {
    async fn start(dialect: Dialect) -> Self {
        let state = Arc::new(Mutex::new(MockState {
            next_task_id: 1,
            tasks: Default::default(),
            dialect,
            step: 1024 * 1024,
            fetch_urls: true,
            installs: Vec::new(),
            by_content: Default::default(),
            fail_installs: None,
        }));
        let app = Router::new()
            .route("/api", get(unsupported))
            .route("/api/status", get(status))
            .route("/api/is_exists", post(is_exists))
            .route("/api/install", post(install))
            .route("/api/get_task_progress", post(progress))
            .route("/api/find_task", post(find_task))
            .route("/api/pause_task", post(ok_task))
            .route("/api/resume_task", post(ok_task))
            .route("/api/stop_task", post(ok_task))
            .route("/api/start_task", post(ok_task))
            .route("/api/unregister_task", post(ok_task))
            .with_state(Arc::clone(&state));

        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { state, addr }
    }

    fn installs(&self) -> Vec<Value> {
        self.state.lock().unwrap().installs.clone()
    }

    fn tasks(&self) -> std::collections::HashMap<i64, MockTask> {
        self.state.lock().unwrap().tasks.clone()
    }
}

async fn unsupported() -> Response {
    // The original answers GET /api with 400 + "Unsupported method" (this is
    // what ecosystem senders probe for).
    (
        StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "text/plain")],
        "Unsupported method",
    )
        .into_response()
}

async fn status() -> Response {
    // Newer receivers answer JSON; the original answers HTML → unsupported.
    Json(json!({"busy": false, "active": 0})).into_response()
}

async fn is_exists(Json(_body): Json<Value>) -> Response {
    Json(json!({"status": "success", "exists": "false"})).into_response()
}

async fn find_task(
    State(state): State<Arc<Mutex<MockState>>>,
    Json(body): Json<Value>,
) -> Response {
    let cid = body["content_id"].as_str().unwrap_or_default().to_string();
    let st = state.lock().unwrap();
    match st.by_content.get(&cid) {
        Some(id) => Json(json!({"status": "success", "task_id": id})).into_response(),
        None => fail_body(&st.dialect, 0x8099_0019),
    }
}

async fn ok_task(Json(_body): Json<Value>) -> Response {
    Json(json!({"status": "success"})).into_response()
}

async fn install(State(state): State<Arc<Mutex<MockState>>>, Json(body): Json<Value>) -> Response {
    {
        let mut st = state.lock().unwrap();
        st.installs.push(body.clone());
        if let Some(code) = st.fail_installs {
            return fail_body(&st.dialect, code);
        }
    }
    let urls: Vec<String> = body["packages"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    if urls.is_empty() || body["type"].as_str() != Some("direct") {
        return fail_body(&state.lock().unwrap().dialect, 0x8099_0007);
    }

    // Prerequisite fetch: range-request the first 0x2000 bytes of piece 0,
    // exactly like the real payload (docs/BASELINE.md §2).
    let total = if state.lock().unwrap().fetch_urls {
        match fetch_range_total(&urls[0]).await {
            Some(t) => t,
            None => return fail_body(&state.lock().unwrap().dialect, 0x8099_000A),
        }
    } else {
        10 * 1024 * 1024
    };

    let mut st = state.lock().unwrap();
    let task_id = st.next_task_id;
    st.next_task_id += 1;
    st.tasks.insert(
        task_id,
        MockTask {
            total,
            transferred: 0,
            local_copy: 0,
            error: 0,
        },
    );
    st.by_content
        .insert(format!("CONTENT-ID-{task_id}"), task_id);
    Json(json!({"status": "success", "task_id": task_id, "title": "Mock Game"})).into_response()
}

async fn progress(State(state): State<Arc<Mutex<MockState>>>, Json(body): Json<Value>) -> Response {
    let task_id = body["task_id"].as_i64().unwrap_or(-1);
    let dialect = state.lock().unwrap().dialect;
    let mut st = state.lock().unwrap();
    // Advance the simulated download on each poll.
    let step = st.step;
    let (total, transferred, copy, error) = {
        let Some(task) = st.tasks.get_mut(&task_id) else {
            return fail_body(&dialect, 0x8099_0019);
        };
        if task.error == 0 && task.transferred < task.total {
            task.transferred = (task.transferred + step).min(task.total);
            if task.transferred >= task.total {
                task.local_copy = 50; // copy phase begins
            }
        } else if task.local_copy > 0 && task.local_copy < 100 {
            task.local_copy = 100;
        }
        (task.total, task.transferred, task.local_copy, task.error)
    };
    let rest = if total > transferred {
        (total - transferred) / step.max(1)
    } else {
        0
    };

    match dialect {
        Dialect::Strict => Json(json!({
            "status": "success",
            "bits": 32,
            "error": error,
            "length": total,
            "transferred": transferred,
            "length_total": total,
            "transferred_total": transferred,
            "num_index": 0,
            "num_total": 1,
            "rest_sec": rest,
            "rest_sec_total": rest,
            "preparing_percent": 0,
            "local_copy_percent": copy,
        }))
        .into_response(),
        Dialect::BrokenHex => {
            // Byte-for-byte the original payload's format string (invalid JSON).
            let body = format!(
                "{{ \"status\": \"success\", \"bits\": 0x{bits:X}, \"error\": {error}, \"length\": 0x{len:X}, \"transferred\": 0x{tr:X}, \"length_total\": 0x{lt:X}, \"transferred_total\": 0x{tt:X}, \"num_index\": 0, \"num_total\": 1, \"rest_sec\": {rest}, \"rest_sec_total\": {rest}, \"preparing_percent\": 0, \"local_copy_percent\": {copy} }}\n",
                bits = 32u32,
                error = error,
                len = total,
                tr = transferred,
                lt = total,
                tt = transferred,
                rest = rest,
                copy = copy,
            );
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap()
        }
    }
}

fn fail_body(dialect: &Dialect, code: i64) -> Response {
    match dialect {
        Dialect::Strict => Json(json!({"status": "fail", "error_code": code})).into_response(),
        Dialect::BrokenHex => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(
                "{{ \"status\": \"fail\", \"error_code\": 0x{code:08X} }}\n"
            )))
            .unwrap(),
    }
}

/// Fetch `bytes=0-8191` of a URL and return the total size from Content-Range.
async fn fetch_range_total(url: &str) -> Option<u64> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client
        .get(url)
        .header("Range", "bytes=0-8191")
        .send()
        .await
        .ok()?;
    if resp.status().as_u16() != 206 {
        return None;
    }
    let cr = resp
        .headers()
        .get("content-range")?
        .to_str()
        .ok()?
        .to_string();
    resp.bytes().await.ok()?;
    cr.split('/').nth(1)?.parse().ok()
}

// ---------------------------------------------------------------------------
// Client-level protocol tests
// ---------------------------------------------------------------------------

fn client_for(addr: SocketAddr) -> RpiClient {
    RpiClient::new(addr, Duration::from_secs(5))
}

#[tokio::test]
async fn probe_recognizes_rpi_dialect() {
    let mock = MockConsole::start(Dialect::BrokenHex).await;
    let probe = client_for(mock.addr).probe().await.unwrap();
    assert_eq!(probe.kind, rpi_ng_core::model::BackendKind::Rpi);
    assert!(probe.caps.install_direct);
    assert!(probe.caps.status_endpoint);
}

#[tokio::test]
async fn install_and_progress_with_broken_hex_dialect() {
    let mock = MockConsole::start(Dialect::BrokenHex).await;
    let client = client_for(mock.addr);

    // Serve a small file so the prerequisite fetch succeeds.
    let tmp = tempfile::tempdir().unwrap();
    let pkg_path = tmp.path().join("game.pkg");
    std::fs::write(&pkg_path, vec![0u8; 256 * 1024]).unwrap();
    let server = rpi_ng_core::fileserver::FileServer::start(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let id = server.register(pkg_path, Some("game.pkg".into()));
    let url = server.url_for(id).unwrap();

    let resp = client.install_direct(&[url]).await.unwrap();
    assert_eq!(resp.status, "success");
    assert!(resp.task_id > 0);
    assert_eq!(resp.title.as_deref(), Some("Mock Game"));

    // Poll until completion; the progress bodies are invalid JSON with hex.
    let mut completed = false;
    for _ in 0..20 {
        let pv = client.task_progress(resp.task_id).await.unwrap();
        if pv.local_copy_percent >= 100 && pv.transferred >= pv.total && pv.total > 0 {
            completed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(completed, "mock task should complete through polls");
    let last = client.task_progress(resp.task_id).await.unwrap();
    assert_eq!(last.total, 256 * 1024);
    assert_eq!(last.percent, 100.0);
}

#[tokio::test]
async fn install_with_strict_dialect() {
    let mock = MockConsole::start(Dialect::Strict).await;
    let client = client_for(mock.addr);
    // Unreachable package URL → the mock's prerequisite fetch fails →
    // the backend surfaces the console-side error code.
    let err = client
        .install_direct(&["http://127.0.0.1:1/pkg".into()])
        .await
        .unwrap_err();
    match err {
        RpiError::BackendError { code, .. } => assert_eq!(code, 0x8099_000A),
        other => panic!("expected BackendError, got {other:?}"),
    }
}

#[tokio::test]
async fn backend_failures_surface_error_codes() {
    let mock = MockConsole::start(Dialect::BrokenHex).await;
    mock.state.lock().unwrap().fail_installs = Some(0x8099_0007);
    let client = client_for(mock.addr);
    let err = client
        .install_direct(&["http://127.0.0.1:1/x.pkg".into()])
        .await
        .unwrap_err();
    match err {
        RpiError::BackendError { code, .. } => assert_eq!(code, 0x8099_0007),
        other => panic!("expected BackendError, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_task_maps_to_task_gone() {
    let mock = MockConsole::start(Dialect::BrokenHex).await;
    let client = client_for(mock.addr);
    let err = client.task_progress(999).await.unwrap_err();
    match err {
        RpiError::BackendError { code, .. } => assert_eq!(code, 0x8099_0019),
        other => panic!("expected BackendError, got {other:?}"),
    }
}

#[tokio::test]
async fn connection_refused_is_transparent() {
    // Bind then drop a listener to get a refused port.
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let client = RpiClient::new(addr, Duration::from_secs(2));
    let err = client.is_exists("CUSA00000").await.unwrap_err();
    // The concrete transport error differs by platform (RST vs silently
    // dropped SYN → timeout on some Windows setups); what must hold is that
    // the failure is classified as transient and retryable.
    assert!(
        err.is_transient(),
        "error should be transient, was: {err:?}"
    );
}

#[tokio::test]
async fn pause_resume_roundtrip() {
    let mock = MockConsole::start(Dialect::Strict).await;
    let client = client_for(mock.addr);
    // No task registered; the mock answers success for lifecycle commands.
    client.pause_task(1).await.unwrap();
    client.resume_task(1).await.unwrap();
    client.stop_task(1).await.unwrap();
    client.unregister_task(1).await.unwrap();
}

// ---------------------------------------------------------------------------
// Engine end-to-end flow
// ---------------------------------------------------------------------------

fn build_fake_pkg(dir: &std::path::Path, name: &str, content_id: &str) -> std::path::PathBuf {
    use std::io::Write;
    // Minimal SFO.
    let entries: Vec<(&str, &str)> = vec![
        ("TITLE", "Mock Game"),
        ("CONTENT_ID", content_id),
        ("CATEGORY", "gd"),
    ];
    let mut keys = Vec::new();
    let mut values = Vec::new();
    let mut index = Vec::new();
    let key_base = 0x14 + entries.len() * 0x10;
    for (k, v) in &entries {
        let key_off = keys.len();
        keys.extend_from_slice(k.as_bytes());
        keys.push(0);
        let val_off = values.len();
        values.extend_from_slice(v.as_bytes());
        values.push(0);
        index.push((
            key_off as u16,
            0x0204u16,
            (v.len() + 1) as u32,
            (v.len() + 1) as u32,
            val_off as u32,
        ));
    }
    let mut sfo = Vec::new();
    sfo.extend_from_slice(b"\0PSF");
    sfo.extend_from_slice(&1u32.to_be_bytes());
    sfo.extend_from_slice(&(key_base as u32).to_be_bytes());
    sfo.extend_from_slice(&((key_base + keys.len()) as u32).to_be_bytes());
    sfo.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (k, f, u, m, v) in index {
        sfo.extend_from_slice(&k.to_be_bytes());
        sfo.extend_from_slice(&f.to_be_bytes());
        sfo.extend_from_slice(&u.to_be_bytes());
        sfo.extend_from_slice(&m.to_be_bytes());
        sfo.extend_from_slice(&v.to_be_bytes());
    }
    sfo.extend_from_slice(&keys);
    sfo.extend_from_slice(&values);

    let entry_table_offset = 0x2000 + sfo.len() as u32;
    let mut entry_table = Vec::new();
    entry_table.extend_from_slice(&0x1000u32.to_be_bytes());
    entry_table.extend_from_slice(&[0u8; 0x0C]);
    entry_table.extend_from_slice(&0x2000u32.to_be_bytes());
    entry_table.extend_from_slice(&(sfo.len() as u32).to_be_bytes());
    entry_table.extend_from_slice(&[0u8; 0x0C]);

    let mut header = vec![0u8; 0x2000];
    header[0..4].copy_from_slice(&[0x7F, b'P', b'K', b'G']);
    header[0x10..0x14].copy_from_slice(&1u32.to_be_bytes());
    header[0x18..0x1C].copy_from_slice(&entry_table_offset.to_be_bytes());
    header[0x40..0x40 + content_id.len()].copy_from_slice(content_id.as_bytes());
    header[0x74..0x78].copy_from_slice(&0x1Au32.to_be_bytes());

    let data_len = 512 * 1024usize;
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(&header).unwrap();
    f.write_all(&sfo).unwrap();
    f.write_all(&entry_table).unwrap();
    let cur = 0x2000 + sfo.len() + entry_table.len();
    f.write_all(&vec![0u8; data_len.saturating_sub(cur)])
        .unwrap();
    f.sync_all().unwrap();
    path
}

/// Drain engine events until a condition on the latest snapshot holds.
async fn wait_for_snapshot(
    handle: &rpi_ng_core::engine::EngineHandle,
    mut cond: impl FnMut(&Snapshot) -> bool,
    timeout: Duration,
) -> Snapshot {
    let mut last: Option<Snapshot> = None;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        for ev in handle.poll_events() {
            if let Event::Snapshot(s) = ev {
                last = Some(s);
            }
        }
        if let Some(s) = &last
            && cond(s)
        {
            return s.clone();
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timeout waiting for snapshot condition; last={last:?}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn engine_end_to_end_flow_completes_with_evidence() {
    let data_dir = tempfile::tempdir().unwrap();
    let pkg_dir = tempfile::tempdir().unwrap();
    let pkg_path = build_fake_pkg(
        pkg_dir.path(),
        "UP0001-CUSA07777_00-MOCKGAME00000000.pkg",
        "UP0001-CUSA07777_00-MOCKGAME00000000",
    );

    let mock = MockConsole::start(Dialect::BrokenHex).await;
    let settings = Settings {
        poll_interval: Duration::from_millis(30),
        discovery_enabled: false,
        ..Settings::default()
    };

    let handle = rpi_ng_core::engine::spawn_with(settings, Some(data_dir.path().to_path_buf()));

    // 1. Connect to the mock console.
    handle.send(Command::Connect(mock.addr));
    let snap = wait_for_snapshot(
        &handle,
        |s| {
            matches!(
                s.console.status,
                rpi_ng_core::model::ConsoleStatus::Connected
            )
        },
        Duration::from_secs(10),
    )
    .await;
    assert!(
        snap.server.is_some(),
        "file server must be running after connect"
    );

    // 2. Add the PKG.
    handle.send(Command::AddPaths {
        paths: vec![pkg_path],
        recursive: false,
    });
    let snap = wait_for_snapshot(
        &handle,
        |s| {
            s.items
                .iter()
                .any(|i| i.parts.iter().any(|p| p.pkg.is_some()))
        },
        Duration::from_secs(10),
    )
    .await;
    let item = &snap.items[0];
    assert_eq!(item.state, ItemState::Queued);
    assert_eq!(
        item.parts[0].pkg.as_ref().unwrap().title.as_deref(),
        Some("Mock Game")
    );
    assert_eq!(
        item.parts[0].pkg.as_ref().unwrap().title_id.as_deref(),
        Some("CUSA07777")
    );

    // 3. Start the queue.
    handle.send(Command::Start);

    // 4. Submission -> transferring -> installing -> completed.
    let snap = wait_for_snapshot(
        &handle,
        |s| s.items.iter().any(|i| i.state == ItemState::Completed),
        Duration::from_secs(30),
    )
    .await;
    let item = snap
        .items
        .iter()
        .find(|i| i.state == ItemState::Completed)
        .unwrap();
    let progress = item.progress.unwrap();
    assert_eq!(progress.total, 512 * 1024);
    assert_eq!(progress.percent, 100.0);
    assert_eq!(progress.raw_error, 0);
    assert!(item.task_id.unwrap() > 0);
    assert_eq!(item.console_title.as_deref(), Some("Mock Game"));

    // 5. The mock saw a well-formed install request and fetched the real URL.
    let installs = mock.installs();
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0]["type"], "direct");
    let url = installs[0]["packages"][0].as_str().unwrap();
    assert!(url.starts_with("http://"), "URL was {url}");
    let tasks = mock.tasks();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[&item.task_id.unwrap()].total, 512 * 1024);

    // 6. Clean shutdown.
    handle.shutdown();

    // 7. Persistence: reloading the queue keeps the completed item.
    let text = std::fs::read_to_string(data_dir.path().join("queue.json")).unwrap();
    let q = rpi_ng_core::queue::Queue::from_json(&text).unwrap();
    assert!(q.items().iter().any(|i| i.state == ItemState::Completed));
}

#[tokio::test]
async fn engine_marks_failed_on_console_error() {
    let data_dir = tempfile::tempdir().unwrap();
    let pkg_dir = tempfile::tempdir().unwrap();
    let pkg_path = build_fake_pkg(
        pkg_dir.path(),
        "fail.pkg",
        "UP0001-CUSA08888_00-FAILPKG0000000",
    );

    let mock = MockConsole::start(Dialect::BrokenHex).await;
    // Tiny step so the transfer takes many polls — time to poison the task
    // with a console-side error before it completes.
    mock.state.lock().unwrap().step = 4096;

    let settings = Settings {
        poll_interval: Duration::from_millis(30),
        discovery_enabled: false,
        ..Settings::default()
    };
    let handle = rpi_ng_core::engine::spawn_with(settings, Some(data_dir.path().to_path_buf()));

    handle.send(Command::Connect(mock.addr));
    wait_for_snapshot(
        &handle,
        |s| {
            matches!(
                s.console.status,
                rpi_ng_core::model::ConsoleStatus::Connected
            )
        },
        Duration::from_secs(10),
    )
    .await;

    handle.send(Command::AddPaths {
        paths: vec![pkg_path],
        recursive: false,
    });
    wait_for_snapshot(&handle, |s| !s.items.is_empty(), Duration::from_secs(10)).await;

    handle.send(Command::Start);
    // Wait for the task to exist on the mock, then poison it.
    let snap = wait_for_snapshot(
        &handle,
        |s| {
            s.items
                .iter()
                .any(|i| i.state == ItemState::Transferring || i.state == ItemState::Installing)
        },
        Duration::from_secs(20),
    )
    .await;
    let item_id = snap.items[0].id;
    let task_id = snap.items[0].task_id.unwrap();
    {
        let mut st = mock.state.lock().unwrap();
        if let Some(t) = st.tasks.get_mut(&task_id) {
            t.error = 0x8099_0005;
        }
    }

    let snap = wait_for_snapshot(
        &handle,
        |s| {
            s.items
                .iter()
                .any(|i| i.id == item_id && i.state == ItemState::Failed)
        },
        Duration::from_secs(20),
    )
    .await;
    let item = snap.items.iter().find(|i| i.id == item_id).unwrap();
    assert!(
        item.error.as_deref().unwrap().contains("0x80990005"),
        "error was {:?}",
        item.error
    );
    handle.shutdown();
}
