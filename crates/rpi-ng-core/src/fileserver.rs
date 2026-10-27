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
};

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
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

/// Running file server handle.
pub struct FileServer {
    registry: Arc<Mutex<Registry>>,
    pub bind: SocketAddr,
    /// Base URL the console should use, e.g. `http://192.168.1.20:53742`.
    pub base_url: String,
    shutdown: watch::Sender<bool>,
}

impl FileServer {
    /// Bind `bind_addr` and serve the registry until [`FileServer::shutdown`].
    pub async fn start(bind_addr: SocketAddr) -> std::io::Result<Self> {
        let registry = Arc::new(Mutex::new(Registry::default()));
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let app = Router::new()
            .route("/pkg/{id}/{name}", get(serve_with_name).head(serve_with_name))
            .route("/pkg/{id}", get(serve).head(serve))
            .with_state(Arc::clone(&registry));

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
            bind,
            base_url,
            shutdown: shutdown_tx,
        })
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

type Shared = Arc<Mutex<Registry>>;

async fn serve_with_name(
    state: State<Shared>,
    AxumPath((id, _name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    method: Method,
) -> Response {
    serve_inner(state, id, headers, method).await
}

async fn serve(
    state: State<Shared>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    method: Method,
) -> Response {
    serve_inner(state, id, headers, method).await
}

async fn serve_inner(
    State(registry): State<Shared>,
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

    if offset > 0
        && file.seek(SeekFrom::Start(offset)).await.is_err() {
            return not_found();
        }
    // Stream exactly `length` bytes in bounded chunks.
    let stream = ReaderStream::with_capacity(file.take(length), 64 * 1024);
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
