//! Allowlist-scoped local HTTP file server that streams PKG files to the
//! console with Range support.
//!
//! Design points (see `docs/ARCHITECTURE.md` §4.5):
//! - Only files explicitly registered in the queue are served; URLs use opaque
//!   ids, so path traversal is structurally impossible.
//! - Files are streamed in bounded chunks — never loaded whole.
//! - Single-range and suffix-range requests are supported (required by the
//!   console payload's prerequisite fetcher); multi-range requests fall back
//!   to a full response, which RFC 7233 permits.
//! - Binds the interface reachable from the console (never `0.0.0.0`
//!   silently).

use std::{
    collections::HashMap,
    io::SeekFrom,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::{Path as AxumPath, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    net::TcpListener,
    sync::watch,
};
use tokio_util::io::ReaderStream;

use crate::model::PartId;

/// Percent-encode set for URL path segments: keep RFC 3986 `pchar` unreserved
/// and sub-delims literal (letters, digits, `-._~!$&'()*+,;=:@`), encode the
/// rest. Spaces and specials must be encoded — console installs break on them
/// (pkg-sender issue #6, `docs/RESEARCH.md` §2.3).
const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'\\')
    .add(b'^')
    .add(b'|');

/// Concurrency note: the console opens only a handful of connections
/// (prerequisite fetches + the BGFT download), so no artificial open-file gate
/// is imposed; the allowlist bounds exposure to registered files.

#[derive(Default)]
struct Registry {
    /// Opaque id → (canonical path, display file name).
    entries: HashMap<PartId, (PathBuf, String)>,
    next: PartId,
}

impl Registry {
    fn register(&mut self, path: PathBuf, name: String) -> PartId {
        let id = self.next;
        self.next += 1;
        self.entries.insert(id, (path, name));
        id
    }
}

// ---------------------------------------------------------------------------
// Throughput accounting (live in/out Mbps for the UI)
// ---------------------------------------------------------------------------

/// Live byte counters for the file server, plus a smoothed rate sampler.
///
/// - **tx** = response body bytes (what the console downloads — the number
///   users think of as "transfer speed").
/// - **rx** = request bytes (request line + headers; tiny — control traffic).
///
/// Both are best-effort approximations, documented on purpose: tx counts the
/// promised `Content-Length` (a mid-body disconnect over-counts by the
/// unsent tail), rx estimates the parsed request's wire size.
#[derive(Debug, Default)]
pub struct ServerStats {
    tx_bytes: std::sync::atomic::AtomicU64,
    rx_bytes: std::sync::atomic::AtomicU64,
    requests: std::sync::atomic::AtomicU64,
    rate: Mutex<RateState>,
}

#[derive(Debug, Clone, Copy, Default)]
struct RateState {
    /// (t, tx, rx) of the previous sample.
    last: Option<(std::time::Instant, u64, u64)>,
    /// Smoothed bytes/sec (EMA over per-sample deltas).
    tx_ema: f64,
    rx_ema: f64,
}

impl ServerStats {
    fn add_tx(&self, bytes: u64) {
        self.tx_bytes
            .fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
    }

    fn add_rx(&self, bytes: u64) {
        self.rx_bytes
            .fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
    }

    fn add_request(&self) {
        self.requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Totals since the server started: (tx, rx, requests).
    pub fn totals(&self) -> (u64, u64, u64) {
        (
            self.tx_bytes.load(std::sync::atomic::Ordering::Relaxed),
            self.rx_bytes.load(std::sync::atomic::Ordering::Relaxed),
            self.requests.load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Smoothed transfer rates in **bytes per second**, sampled on demand
    /// (the engine snapshots on each poll tick and on a 250 ms ticker while
    /// the server runs, so the UI stays live without the server itself
    /// keeping any timer).
    pub fn rates(&self) -> (f64, f64) {
        let now = std::time::Instant::now();
        let (tx, rx, _) = self.totals();
        let mut state = self.rate.lock().unwrap();
        update_rates(&mut state, now, tx, rx)
    }
}

/// EMA update, split out so the math is directly testable. α = 0.5 keeps it
/// responsive on speed-up and calm on jitter; samples closer than 100 ms
/// return the previous EMA unchanged (no divide-by-tiny spikes).
fn update_rates(state: &mut RateState, now: std::time::Instant, tx: u64, rx: u64) -> (f64, f64) {
    const MIN_DELTA: Duration = Duration::from_millis(100);
    let Some((t, prev_tx, prev_rx)) = state.last else {
        state.last = Some((now, tx, rx));
        return (state.tx_ema, state.rx_ema);
    };
    let dt = now.saturating_duration_since(t);
    if dt < MIN_DELTA {
        return (state.tx_ema, state.rx_ema);
    }
    let secs = dt.as_secs_f64();
    let tx_rate = tx.saturating_sub(prev_tx) as f64 / secs;
    let rx_rate = rx.saturating_sub(prev_rx) as f64 / secs;
    state.tx_ema = state.tx_ema * 0.5 + tx_rate * 0.5;
    state.rx_ema = state.rx_ema * 0.5 + rx_rate * 0.5;
    state.last = Some((now, tx, rx));
    (state.tx_ema, state.rx_ema)
}

/// Per-request state handed to the handlers.
#[derive(Clone)]
struct ServerState {
    registry: Arc<Mutex<Registry>>,
    stats: Arc<ServerStats>,
}

/// Running file server handle.
pub struct FileServer {
    registry: Arc<Mutex<Registry>>,
    stats: Arc<ServerStats>,
    pub bind: SocketAddr,
    /// Base URL the console should use, e.g. `http://192.168.1.20:53742`.
    pub base_url: String,
    shutdown: watch::Sender<bool>,
}

impl FileServer {
    /// Bind `bind_addr` and serve the registry until [`FileServer::shutdown`].
    pub async fn start(bind_addr: SocketAddr) -> std::io::Result<Self> {
        let registry = Arc::new(Mutex::new(Registry::default()));
        let stats = Arc::new(ServerStats::default());
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let app = Router::new()
            .route(
                "/pkg/{id}/{name}",
                get(serve_with_name).head(serve_with_name),
            )
            .route("/pkg/{id}", get(serve).head(serve))
            // Count incoming request bytes before routing.
            .layer(middleware::from_fn_with_state(Arc::clone(&stats), track_rx))
            .with_state(ServerState {
                registry: Arc::clone(&registry),
                stats: Arc::clone(&stats),
            });

        let listener = TcpListener::bind(bind_addr).await?;
        let bind = listener.local_addr()?;
        let host = match bind.ip() {
            std::net::IpAddr::V4(ip) => ip.to_string(),
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        };
        let base_url = format!("http://{host}:{}", bind.port());

        tokio::spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                let _ = shutdown_rx.wait_for(|&stop| stop).await;
            });
            let _ = server.await;
        });

        Ok(Self {
            registry,
            stats,
            bind,
            base_url,
            shutdown: shutdown_tx,
        })
    }

    /// Live counters and smoothed rates for the UI.
    pub fn stats(&self) -> &Arc<ServerStats> {
        &self.stats
    }

    /// Register a file; returns its id.
    pub fn register(&self, path: PathBuf, name: Option<String>) -> PartId {
        let display = name.unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "package.pkg".into())
        });
        let mut reg = self.registry.lock().unwrap();
        reg.register(path, display)
    }

    /// URL for a registered part, percent-encoded for safe transport
    /// (spaces/special characters break console installs — see
    /// `docs/RESEARCH.md` §2.3).
    pub fn url_for(&self, id: PartId) -> Option<String> {
        let reg = self.registry.lock().unwrap();
        let (_, name) = reg.entries.get(&id)?.clone();
        let encoded = percent_encoding::utf8_percent_encode(&name, SEGMENT).to_string();
        Some(format!("{}/pkg/{id}/{encoded}", self.base_url))
    }

    /// Remove a part from the allowlist. In-flight downloads finish; new ones 404.
    pub fn unregister(&self, id: PartId) {
        let mut reg = self.registry.lock().unwrap();
        reg.entries.remove(&id);
    }

    /// Signal graceful shutdown; in-flight requests get a grace period.
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Middleware: approximate the request's wire size (request line + headers)
/// and count it as incoming traffic.
async fn track_rx(State(stats): State<Arc<ServerStats>>, req: Request, next: Next) -> Response {
    stats.add_request();
    // "METHOD /uri HTTP/1.1\r\n" + "Name: value\r\n" per header + final CRLF.
    let mut size = req.method().as_str().len() + req.uri().to_string().len() + 13;
    for (name, value) in req.headers() {
        size += name.as_str().len() + value.as_bytes().len() + 4;
    }
    stats.add_rx(size as u64);
    next.run(req).await
}

async fn serve_with_name(
    state: State<ServerState>,
    AxumPath((id, _name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    method: Method,
) -> Response {
    serve_inner(state, id, headers, method).await
}

async fn serve(
    state: State<ServerState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    method: Method,
) -> Response {
    serve_inner(state, id, headers, method).await
}

async fn serve_inner(
    State(ServerState { registry, stats }): State<ServerState>,
    id_str: String,
    headers: HeaderMap,
    method: Method,
) -> Response {
    // The file is resolved exclusively through the opaque id — the name
    // segment is decorative and never interpreted as a path.
    let Ok(id) = id_str.parse::<PartId>() else {
        return not_found();
    };
    let path = {
        let reg = registry.lock().unwrap();
        match reg.entries.get(&id) {
            Some((path, _)) => path.clone(),
            None => return not_found(),
        }
    };

    // Never trust the client's path: we only serve the registered file.
    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return not_found();
    };
    let size = match file.metadata().await {
        Ok(m) => m.len(),
        Err(_) => return not_found(),
    };

    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let (status, offset, length, content_range) = match range_header
        .as_deref()
        .and_then(|r| r.strip_prefix("bytes="))
    {
        Some(spec) if !spec.contains(',') => match parse_range(spec, size) {
            Some((start, len)) => {
                let cr = if len > 0 {
                    format!("bytes {start}-{}/{}", start + len - 1, size)
                } else {
                    format!("bytes {start}-{start}/{size}")
                };
                (StatusCode::PARTIAL_CONTENT, start, len, Some(cr))
            }
            None => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                    .body(Body::empty())
                    .unwrap();
            }
        },
        // Multi-range or no range → full body (200), permitted by RFC 7233.
        _ => (StatusCode::OK, 0u64, size, None),
    };

    if method == Method::HEAD {
        let mut builder = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, length)
            .header(header::ACCEPT_RANGES, "bytes");
        if let Some(cr) = content_range {
            builder = builder.header(header::CONTENT_RANGE, cr);
        }
        return builder.body(Body::empty()).unwrap();
    }

    if offset > 0 && file.seek(SeekFrom::Start(offset)).await.is_err() {
        return not_found();
    }
    // Count the promised body bytes as outgoing traffic (see ServerStats doc
    // for why approximation-by-Content-Length is deliberate).
    stats.add_tx(length);
    // Stream exactly `length` bytes in bounded chunks. 256 KiB keeps
    // per-chunk syscall/wakeup overhead low on gigabit LANs while bounding
    // memory per connection (the console opens only a handful).
    let stream = ReaderStream::with_capacity(file.take(length), 256 * 1024);
    let body = Body::from_stream(stream);

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, length)
        .header(header::ACCEPT_RANGES, "bytes");
    if let Some(cr) = content_range {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }
    builder.body(body).unwrap()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not registered").into_response()
}

/// Parse a single-range spec (`start-end`, `start-`, `-suffix`).
fn parse_range(spec: &str, size: u64) -> Option<(u64, u64)> {
    let spec = spec.trim();
    let (start_s, end_s) = spec.split_once('-')?;
    if start_s.is_empty() {
        // suffix: last N bytes
        let n: u64 = end_s.trim().parse().ok()?;
        if n == 0 || size == 0 {
            return None;
        }
        let n = n.min(size);
        return Some((size - n, n));
    }
    let start: u64 = start_s.trim().parse().ok()?;
    if start >= size {
        return None;
    }
    if end_s.trim().is_empty() {
        return Some((start, size - start));
    }
    let end: u64 = end_s.trim().parse().ok()?;
    if end < start {
        return None;
    }
    let end = end.min(size - 1);
    Some((start, end - start + 1))
}

// ---------------------------------------------------------------------------
// Self-check ("Check File Server" button)
// ---------------------------------------------------------------------------

/// Outcome of a loopback probe against the local file server.
///
/// The probe deliberately talks to `127.0.0.1` — it answers "is *our* server
/// up and speaking HTTP?", which is exactly the split users need: if this
/// succeeds but the console still cannot download, the problem is between
/// the two machines (firewall, subnet, AP isolation), not the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerCheck {
    /// The server answered; the status code it returned (404 for the probe
    /// URL, since it deliberately asks for an unregistered id).
    Responded(u16),
    /// Nothing is listening on the port — the server is not running (it
    /// starts together with a console connection).
    NotRunning,
    /// The port accepted the connection but never answered within the
    /// timeout — typically a firewall/antivirus intercepting loopback.
    NoAnswer,
    /// Anything else (unexpected I/O trouble).
    Error(String),
}

/// Probe the local file server on `port` over loopback, `HEAD`-ing an
/// unregistered URL. Sync + std-only on purpose: the UI calls it from a
/// plain background thread (same pattern as the update checker).
pub fn self_check(port: u16, timeout: Duration) -> ServerCheck {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let addr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port);
    let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(s) => s,
        // Refused/reset = nothing (usable) is listening there.
        Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused) => {
            return ServerCheck::NotRunning;
        }
        // connect_timeout signals expiry as WouldBlock/TimedOut depending on
        // the platform.
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            return ServerCheck::NoAnswer;
        }
        Err(e) => return ServerCheck::Error(e.to_string()),
    };
    let req =
        format!("HEAD /pkg/0/server-check HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if let Err(e) = stream.write_all(req.as_bytes()) {
        return ServerCheck::Error(e.to_string());
    }
    let _ = stream.set_read_timeout(Some(timeout));
    let mut buf = [0u8; 128];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            return ServerCheck::NoAnswer;
        }
        Err(e) => return ServerCheck::Error(e.to_string()),
    };
    if n == 0 {
        // Peer closed without answering.
        return ServerCheck::NoAnswer;
    }
    let status = std::str::from_utf8(&buf[..n])
        .ok()
        .and_then(|head| head.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    ServerCheck::Responded(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_track_deltas_and_decay() {
        let t0 = std::time::Instant::now();
        let mut state = RateState::default();
        // First sample just seeds the baseline (rates stay 0).
        assert_eq!(update_rates(&mut state, t0, 0, 0), (0.0, 0.0));
        // 1 s later, 1 MiB sent → EMA = 0.5 * 1_048_576 B/s.
        let (tx, _) = update_rates(&mut state, t0 + Duration::from_secs(1), 1_048_576, 0);
        assert!((tx - 524_288.0).abs() < 1.0, "tx = {tx}");
        // Another second at the same speed → EMA converges upward.
        let (tx, _) = update_rates(&mut state, t0 + Duration::from_secs(2), 2_097_152, 0);
        assert!(tx > 700_000.0 && tx < 1_100_000.0, "tx = {tx}");
        // Traffic stops → EMA halves each sample, decaying to ~0.
        update_rates(&mut state, t0 + Duration::from_secs(3), 2_097_152, 0);
        update_rates(&mut state, t0 + Duration::from_secs(4), 2_097_152, 0);
        let (tx, _) = update_rates(&mut state, t0 + Duration::from_secs(5), 2_097_152, 0);
        assert!(tx < 200_000.0, "tx should decay, got {tx}");
        // Sub-100 ms bursts never divide by a tiny dt.
        let (tx2, _) = update_rates(&mut state, t0 + Duration::from_millis(5040), 2_097_152, 0);
        assert_eq!(tx2, tx);
    }

    #[test]
    fn self_check_reports_a_dead_port_as_not_running() {
        // Grab a definitely-free port by binding and dropping a listener.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        // Loopback refusals are immediate and reliable on all three OSes.
        match self_check(port, Duration::from_millis(750)) {
            ServerCheck::NotRunning => {}
            other => panic!("expected NotRunning, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn self_check_against_the_real_server_and_stats_count_traffic() {
        let dir = tempfile::tempdir().unwrap();
        let payload = vec![0xA5u8; 64 * 1024];
        let file = dir.path().join("probe.pkg");
        std::fs::write(&file, &payload).unwrap();

        let server = FileServer::start(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();

        // The probe URL is unregistered → 404, but that still proves the
        // server is alive and speaking HTTP. (Probed before the register
        // below so the id stays deterministic; in-app, any status answer —
        // even 200 for a real id — means "alive".)
        assert_eq!(
            self_check(server.bind.port(), Duration::from_millis(750)),
            ServerCheck::Responded(404)
        );

        let id = server.register(file, Some("probe.pkg".into()));

        // A real ranged download must land in the stats (tx exact, rx > 0).
        let url = server.url_for(id).unwrap();
        let client = reqwest::Client::new();
        let res = client
            .get(&url)
            .header("Range", "bytes=1024-2047")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status().as_u16(), 206);
        let bytes = res.bytes().await.unwrap();
        assert_eq!(bytes.len(), 1024);
        // The server keeps running until polled; give the counters a beat.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (tx, rx, requests) = server.stats().totals();
        assert_eq!(tx, 1024, "tx must count exactly the range length");
        assert!(rx > 0, "request bytes must be counted");
        assert!(requests >= 2, "probe + download = at least 2 requests");

        server.shutdown();
    }
}
