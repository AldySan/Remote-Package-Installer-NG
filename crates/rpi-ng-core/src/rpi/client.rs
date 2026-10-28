//! HTTP client for the RPI dialect (JSON POST API on the console).

use std::{net::SocketAddr, time::Duration};

use serde::{Deserialize, de::DeserializeOwned};

use super::{json_fix, types::*};
use crate::model::{BackendCaps, BackendKind, ProgressView};

#[derive(Debug, thiserror::Error)]
pub enum RpiError {
    #[error("network error: {0}")]
    Network(String),
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("connection refused (is the installer app running on the console?)")]
    ConnectionRefused,
    #[error("backend answered HTTP {code}")]
    Http { code: u16 },
    #[error("invalid response (not valid JSON): {0}")]
    InvalidJson(String),
    #[error("backend refused: {code} ({gloss})")]
    BackendError { code: i64, gloss: String },
    #[error("task id missing in response")]
    MissingTaskId,
}

impl RpiError {
    /// Transient errors are worth retrying; permanent ones are not.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::Timeout(_) | Self::ConnectionRefused
        )
    }
}

/// The console's answer to a backend probe.
#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub kind: BackendKind,
    pub caps: BackendCaps,
    /// Present when the backend speaks the v2 API (name/version/storage).
    pub identity: Option<V2SystemData>,
}

/// HTTP client for the RPI dialect. Cheap to clone (the underlying
/// connection pool is shared).
#[derive(Clone)]
pub struct RpiClient {
    http: reqwest::Client,
    base: String,
    timeout: Duration,
}

impl RpiClient {
    pub fn new(addr: SocketAddr, timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            // Never route console traffic through system proxies — the console
            // is on the LAN.
            .no_proxy()
            .build()
            .expect("reqwest client");
        Self {
            http,
            base: format!("http://{addr}"),
            timeout,
        }
    }

    async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<T, RpiError> {
        let url = format!("{}{path}", self.base);
        let resp = self.http.post(&url).json(body).send().await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => return Err(map_transport_err(e, self.timeout)),
        };
        let text = resp.text().await.unwrap_or_default();
        self.parse_response(&text)
    }

    /// GET a JSON document, treating a non-success HTTP status as an error.
    /// Legacy RPI backends answer unknown GET paths with 404 (+ HTML), so this
    /// doubles as the v2-availability probe.
    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, RpiError> {
        let url = format!("{}{path}", self.base);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| map_transport_err(e, self.timeout))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(RpiError::Http {
                code: status.as_u16(),
            });
        }
        self.parse_response(&text)
    }

    /// Parse an RPI JSON body uniformly: repair hex literals, reject
    /// `"status":"fail"` bodies as backend errors, then deserialize into `T`.
    fn parse_response<T: DeserializeOwned>(&self, text: &str) -> Result<T, RpiError> {
        let repaired = json_fix::repair(text);
        let value: serde_json::Value = serde_json::from_str(repaired.trim())
            .or_else(|_| serde_json::from_str(text.trim()))
            .map_err(|e| RpiError::InvalidJson(format!("{e} | body: {}", truncate(text, 300))))?;

        if value.get("status").and_then(|s| s.as_str()) == Some("fail") {
            let code = lenient_i64_from_value(value.get("error_code"));
            // Some dialects (v2 install failures, parameter errors) carry a
            // human-readable `error` string instead of / next to `error_code`.
            let gloss = value
                .get("error")
                .and_then(|e| e.as_str())
                .map(str::to_string)
                .or_else(|| explain_error_code(code).map(str::to_string))
                .unwrap_or_else(|| "no known translation".into());
            return Err(RpiError::BackendError { code, gloss });
        }
        serde_json::from_value(value)
            .map_err(|e| RpiError::InvalidJson(format!("{e} | body: {}", truncate(text, 300))))
    }

    /// Probe whether the endpoint speaks the RPI dialect, and what it supports.
    ///
    /// Evidence used (never a bare port check):
    /// 1. `GET /api` must answer the payload's "Unsupported method" body —
    ///    this is how the ecosystem distinguishes RPI servers from random HTTP.
    /// 2. `GET /api/status` answering JSON marks newer receivers (caps).
    /// 3. `POST /api/is_exists` validates the full request/response path.
    pub async fn probe(&self) -> Result<ProbeOutcome, RpiError> {
        // (1) GET /api — RPI answers 400 "Unsupported method" mentioning "fail".
        let get = self.http.get(format!("{}/api", self.base)).send().await;
        let speaks_rpi = match get {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                (status == 400 || status == 405)
                    && (body.contains("Unsupported method") || body.contains("\"fail\""))
            }
            Err(_) => false,
        };

        // (2) /api/status — newer receivers answer {"busy":...}; old ones HTML.
        let mut caps = BackendCaps::default();
        if let Ok(resp) = self
            .http
            .get(format!("{}/api/status", self.base))
            .send()
            .await
            && let Ok(text) = resp.text().await
            && text.contains("busy")
            && serde_json::from_str::<StatusResponse>(json_fix::repair(&text).as_str()).is_ok()
        {
            caps.status_endpoint = true;
        }

        // (3) Full request/response path validation (read-only API call).
        // This is also the fallback dialect proof when the /api fingerprint
        // is missing, so it is issued exactly once.
        match self.is_exists("PROBE00000").await {
            Ok(_) => {
                caps.install_direct = true;
            }
            Err(RpiError::BackendError { .. }) if speaks_rpi => {
                // The endpoint understood the dialect but rejected the probe —
                // still an RPI server.
                caps.install_direct = true;
            }
            Err(e) => {
                if speaks_rpi {
                    return Err(e);
                }
                return Err(RpiError::Network(
                    "endpoint did not answer the RPI probes".into(),
                ));
            }
        }

        // (4) v2 API — NextGen-class backends answer /api/v2/system with
        // their identity. One extra GET against an already-verified backend.
        let mut identity = None;
        if caps.install_direct
            && let Ok(sys) = self.v2_system().await
            && !sys.name.is_empty()
        {
            caps.api_v2 = true;
            identity = Some(sys);
        }

        Ok(ProbeOutcome {
            kind: BackendKind::Rpi,
            caps,
            identity,
        })
    }

    pub async fn install_direct(
        &self,
        package_urls: &[String],
    ) -> Result<InstallResponse, RpiError> {
        let req = InstallDirectRequest {
            kind: "direct",
            packages: package_urls.to_vec(),
        };
        let resp: InstallResponse = self.post_json("/api/install", &req).await?;
        if resp.status != "success" {
            return Err(RpiError::BackendError {
                code: 0,
                gloss: "install did not report success".into(),
            });
        }
        check_task_id(&resp)?;
        Ok(resp)
    }

    pub async fn install_ref(&self, url: &str) -> Result<InstallResponse, RpiError> {
        let req = InstallRefRequest {
            kind: "ref_pkg_url",
            url: url.to_string(),
        };
        let resp: InstallResponse = self.post_json("/api/install", &req).await?;
        if resp.status != "success" {
            return Err(RpiError::BackendError {
                code: 0,
                gloss: "install did not report success".into(),
            });
        }
        check_task_id(&resp)?;
        Ok(resp)
    }

    pub async fn task_progress_raw(&self, task_id: i64) -> Result<TaskProgressDto, RpiError> {
        let req = TaskIdRequest { task_id };
        self.post_json("/api/get_task_progress", &req).await
    }

    /// Poll progress and fold it into a UI-ready view.
    pub async fn task_progress(&self, task_id: i64) -> Result<ProgressView, RpiError> {
        let dto = self.task_progress_raw(task_id).await?;
        Ok(progress_dto_to_view(&dto))
    }

    pub async fn pause_task(&self, task_id: i64) -> Result<(), RpiError> {
        let req = TaskIdRequest { task_id };
        let _: FailResponse = self.post_json("/api/pause_task", &req).await?;
        Ok(())
    }

    pub async fn resume_task(&self, task_id: i64) -> Result<(), RpiError> {
        let req = TaskIdRequest { task_id };
        let _: FailResponse = self.post_json("/api/resume_task", &req).await?;
        Ok(())
    }

    pub async fn stop_task(&self, task_id: i64) -> Result<(), RpiError> {
        let req = TaskIdRequest { task_id };
        let _: FailResponse = self.post_json("/api/stop_task", &req).await?;
        Ok(())
    }

    pub async fn start_task(&self, task_id: i64) -> Result<(), RpiError> {
        let req = TaskIdRequest { task_id };
        let _: FailResponse = self.post_json("/api/start_task", &req).await?;
        Ok(())
    }

    pub async fn unregister_task(&self, task_id: i64) -> Result<(), RpiError> {
        let req = TaskIdRequest { task_id };
        let _: FailResponse = self.post_json("/api/unregister_task", &req).await?;
        Ok(())
    }

    pub async fn is_exists(&self, title_id: &str) -> Result<IsExistsResponse, RpiError> {
        let req = TitleIdRequest {
            title_id: title_id.to_string(),
        };
        self.post_json("/api/is_exists", &req).await
    }

    pub async fn find_task(
        &self,
        content_id: &str,
        sub_type: i32,
    ) -> Result<FindTaskResponse, RpiError> {
        let req = FindTaskRequest {
            content_id: content_id.to_string(),
            sub_type,
        };
        self.post_json("/api/find_task", &req).await
    }

    // -- v2 API ------------------------------------------------------------

    /// `GET /api/v2/system` — identity and storage of a v2 backend.
    pub async fn v2_system(&self) -> Result<V2SystemData, RpiError> {
        let resp: V2SystemResponse = self.get_json("/api/v2/system").await?;
        Ok(resp.data)
    }

    /// `POST /api/v2/install` — `url` for single files, `urls` for multipart.
    /// Maps the v2 response into the shared [`InstallResponse`] shape
    /// (`job_id` filled, `task_id: -1` = already installed).
    pub async fn v2_install(&self, package_urls: &[String]) -> Result<InstallResponse, RpiError> {
        if package_urls.is_empty() {
            return Err(RpiError::BackendError {
                code: 0,
                gloss: "no package urls to install".into(),
            });
        }
        let req = if package_urls.len() == 1 {
            V2InstallRequest {
                url: Some(package_urls[0].clone()),
                urls: None,
            }
        } else {
            V2InstallRequest {
                url: None,
                urls: Some(package_urls.to_vec()),
            }
        };
        let resp: V2InstallResponse = self.post_json("/api/v2/install", &req).await?;
        if resp.status != "success" {
            return Err(RpiError::BackendError {
                code: 0,
                gloss: "install did not report success".into(),
            });
        }
        let d = resp.data;
        let out = InstallResponse {
            status: "success".into(),
            task_id: d.task_id,
            title: d.title,
            job_id: Some(d.job_id),
        };
        check_task_id(&out)?;
        Ok(out)
    }

    /// `GET /api/v2/jobs` — the console's whole job registry in one request.
    pub async fn v2_jobs(&self) -> Result<Vec<V2Job>, RpiError> {
        let resp: V2JobsResponse = self.get_json("/api/v2/jobs").await?;
        Ok(resp.data.jobs)
    }

    /// `POST /api/v2/jobs/<id>/{pause|resume|stop}`.
    ///
    /// The API documents this as POST, but NextGen 0.0.1/0.0.2 only route the
    /// operation under GET/HEAD — a POST answers 404. Try POST first (correct
    /// for fixed builds and future versions), then fall back to GET once.
    pub async fn v2_job_op(&self, job_id: i64, op: &str) -> Result<(), RpiError> {
        let url = format!("{}/api/v2/jobs/{job_id}/{op}", self.base);
        for attempt in 0..2 {
            let req = if attempt == 0 {
                self.http.post(&url)
            } else {
                self.http.get(&url)
            };
            let resp = req
                .send()
                .await
                .map_err(|e| map_transport_err(e, self.timeout))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.is_success() {
                let _: StatusAck = self.parse_response(&text)?;
                return Ok(());
            }
            if attempt == 0 && status.as_u16() == 404 {
                continue; // documented-POST / routed-GET mismatch: retry as GET
            }
            return Err(RpiError::Http {
                code: status.as_u16(),
            });
        }
        Err(RpiError::Http { code: 404 })
    }
}

/// Acknowledgment body of lifecycle endpoints: `{"status":"success"}`.
#[derive(Debug, Deserialize)]
struct StatusAck {
    #[allow(dead_code)]
    status: String,
}

/// Install responses must carry a usable task id: `> 0` is a real BGFT task,
/// `-1` means "already installed" (both dialects). Anything else (0, absent,
/// or other negative sentinels from fork dialects) is untrackable.
fn check_task_id(resp: &InstallResponse) -> Result<(), RpiError> {
    if resp.task_id > 0 || resp.task_id == -1 {
        Ok(())
    } else {
        Err(RpiError::MissingTaskId)
    }
}

fn map_transport_err(e: reqwest::Error, timeout: Duration) -> RpiError {
    if e.is_timeout() {
        RpiError::Timeout(timeout)
    } else if e.is_connect() {
        RpiError::ConnectionRefused
    } else {
        RpiError::Network(e.to_string())
    }
}

fn lenient_i64_from_value(v: Option<&serde_json::Value>) -> i64 {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                i64::from_str_radix(hex, 16).unwrap_or(0)
            } else {
                t.parse::<i64>().unwrap_or(0)
            }
        }
        _ => 0,
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let mut end = n;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Fold a progress DTO into the UI view. Only observed evidence is used.
pub fn progress_dto_to_view(dto: &TaskProgressDto) -> ProgressView {
    let total = dto.length_total;
    let transferred = dto.transferred_total;
    let percent = if total > 0 {
        (transferred as f64 / total as f64).clamp(0.0, 1.0) * 100.0
    } else {
        0.0
    };
    ProgressView {
        percent: percent as f32,
        transferred,
        total,
        speed_bps: 0, // computed by the engine from poll deltas
        eta_sec: if dto.rest_sec_total > 0 && dto.rest_sec_total < u32::MAX as u64 {
            Some(dto.rest_sec_total)
        } else {
            None
        },
        piece_index: dto.num_index.min(u32::MAX as u64) as u32,
        piece_total: dto.num_total.min(u32::MAX as u64) as u32,
        preparing_percent: dto.preparing_percent.clamp(0, 100) as i32,
        local_copy_percent: dto.local_copy_percent.clamp(0, 100) as i32,
        raw_error: dto.error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dto_accepts_all_three_dialects() {
        // strict decimal
        let a: TaskProgressDto =
            serde_json::from_str(r#"{"status":"success","bits":32,"error":0,"length":4096,"transferred":0,"length_total":4096,"transferred_total":0,"num_index":0,"num_total":1,"rest_sec":0,"rest_sec_total":0,"preparing_percent":0,"local_copy_percent":0}"#).unwrap();
        assert_eq!(a.bits, 32);
        // quoted hex
        let b: TaskProgressDto =
            serde_json::from_str(r#"{"status":"success","bits":"0x20","error":"0","length":"0x1000","transferred":"0","length_total":"0x1000","transferred_total":"0","num_index":"0","num_total":"1"}"#).unwrap();
        assert_eq!(b.bits, 32);
        assert_eq!(b.length_total, 4096);
        // unquoted hex after repair
        let raw = r#"{"status":"success","bits": 0x20, "error":0, "length": 0x1000, "transferred":0x0, "length_total": 0x1000, "transferred_total":0x0, "num_index":0, "num_total":1, "rest_sec":0, "rest_sec_total":0, "preparing_percent":0, "local_copy_percent":0}"#;
        let c: TaskProgressDto = serde_json::from_str(json_fix::repair(raw).trim()).unwrap();
        assert_eq!(c.bits, 32);
    }

    #[test]
    fn error_glossary_known_codes() {
        assert!(explain_error_code(0x8099_0006).is_some());
        assert!(explain_error_code(1).is_none());
    }
}
