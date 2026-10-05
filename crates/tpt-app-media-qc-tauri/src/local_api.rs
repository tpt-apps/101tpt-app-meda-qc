//! Optional localhost-only automation API (spec §17).
//!
//! The service is intentionally implemented with the standard library only.
//! It accepts small JSON requests, has no upload/remote-media endpoint, and
//! refuses to bind to a non-loopback address. Jobs run the same local engine as
//! the desktop command; cancellation is cooperative at job boundaries.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tpt_app_media_qc_cli::app::run_qc;
use tpt_app_media_qc_model::severity::VerdictDecision;

use super::{desktop_run, list_profiles, resolve_profile, DesktopRun};

const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_BODY_BYTES: usize = 64 * 1024;
const JOB_ID_START: u64 = 1;

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(JOB_ID_START);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ApiJobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ApiJob {
    id: String,
    asset: String,
    profile: String,
    quick: bool,
    status: ApiJobStatus,
    cancel_requested: bool,
    verdict: Option<VerdictDecision>,
    error: Option<String>,
}

#[derive(Default)]
struct ApiState {
    jobs: Mutex<BTreeMap<String, ApiJob>>,
    results: Mutex<BTreeMap<String, DesktopRun>>,
    cancelled: Mutex<BTreeSet<String>>,
}

#[derive(Debug, Deserialize)]
struct CreateJob {
    asset: String,
    #[serde(default)]
    profile: String,
    #[serde(default)]
    quick: bool,
}

#[derive(Debug)]
struct HttpError {
    status: u16,
    code: &'static str,
    message: String,
}

type ApiResult<T> = Result<T, HttpError>;

impl HttpError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HttpError {}

/// A bound, not-yet-serving localhost API.
pub struct LocalApi {
    listener: TcpListener,
}

impl LocalApi {
    /// Read the opt-in environment switch. All unset/false values leave the API
    /// disabled; only an explicit true value binds a socket.
    pub fn from_env() -> Result<Option<Self>, String> {
        match std::env::var("TPT_MEDIA_QC_API") {
            Ok(value) => Self::from_switch(Some(value)),
            Err(std::env::VarError::NotPresent) => Self::from_switch(None),
            Err(error) => Err(format!("TPT_MEDIA_QC_API is not valid Unicode: {error}")),
        }
    }

    fn from_switch(value: Option<String>) -> Result<Option<Self>, String> {
        let Some(value) = value else {
            return Ok(None);
        };
        if matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ) {
            return Self::bind(SocketAddr::new(
                IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                0,
            ))
            .map(Some);
        }
        if matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ) {
            return Ok(None);
        }
        Err(format!(
            "TPT_MEDIA_QC_API must be 1/true or 0/false, got '{value}'"
        ))
    }

    /// Bind an ephemeral loopback port. This is public for tests and embedders;
    /// external interface addresses are rejected before any socket is opened.
    pub fn bind(address: SocketAddr) -> Result<Self, String> {
        if !address.ip().is_loopback() {
            return Err(format!(
                "local API may bind only to loopback addresses, got {}",
                address.ip()
            ));
        }
        let listener = TcpListener::bind(address)
            .map_err(|error| format!("could not bind local API to {address}: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("could not configure local API listener: {error}"))?;
        Ok(Self { listener })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Start serving in a background thread. Dropping the returned handle stops
    /// accepting new connections and waits for the listener thread to exit.
    pub fn serve(self) -> ApiServer {
        let address = self.local_addr().expect("bound listener has an address");
        let state = Arc::new(ApiState::default());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_state = Arc::clone(&state);
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("tpt-media-qc-local-api".into())
            .spawn(move || serve_listener(self.listener, worker_state, worker_stop))
            .expect("local API listener thread must start");
        ApiServer {
            address,
            stop,
            worker: Some(worker),
        }
    }
}

/// Active local API server. Kept alive for the desktop process lifetime.
pub struct ApiServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ApiServer {
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn serve_listener(listener: TcpListener, state: Arc<ApiState>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                // Accepted sockets can inherit the listener's nonblocking mode
                // on Windows. Keep request parsing blocking so a client that
                // writes headers/body in separate packets is not reset while
                // the connection thread is still reading.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let connection_state = Arc::clone(&state);
                let _ = thread::Builder::new()
                    .name("tpt-media-qc-local-api-connection".into())
                    .spawn(move || serve_connection(stream, connection_state));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
}

struct IncomingRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

fn serve_connection(mut stream: TcpStream, state: Arc<ApiState>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    let response = read_request(&mut stream)
        .and_then(|request| route_request(&state, request))
        .unwrap_or_else(|error| {
            let body = serde_json::json!({
                "error": error.code,
                "message": error.message,
            });
            json_response(error.status, &body).unwrap_or_else(|_| HttpResponse {
                status: 500,
                body: Vec::new(),
            })
        });
    let _ = write_response(&mut stream, response);
}

fn read_request(stream: &mut TcpStream) -> ApiResult<IncomingRequest> {
    let mut reader = BufReader::new(stream);
    let request_line = read_line_limited(&mut reader)?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| HttpError::new(400, "bad_request", "missing HTTP method"))?
        .to_ascii_uppercase();
    let target = parts
        .next()
        .ok_or_else(|| HttpError::new(400, "bad_request", "missing request target"))?;
    let version = parts
        .next()
        .ok_or_else(|| HttpError::new(400, "bad_request", "missing HTTP version"))?;
    if parts.next().is_some() || (version != "HTTP/1.0" && version != "HTTP/1.1") {
        return Err(HttpError::new(
            400,
            "bad_request",
            "unsupported HTTP request line",
        ));
    }
    let path = target
        .split('?')
        .next()
        .filter(|path| path.starts_with('/'))
        .ok_or_else(|| HttpError::new(400, "bad_request", "unsupported request target"))?
        .to_string();

    let mut content_length = None;
    let mut headers_ended = false;
    for _ in 0..MAX_HEADERS {
        let header = read_line_limited(&mut reader)?;
        if header.is_empty() {
            headers_ended = true;
            break;
        }
        let (name, value) = header
            .split_once(':')
            .ok_or_else(|| HttpError::new(400, "bad_request", "malformed header"))?;
        if name.trim().eq_ignore_ascii_case("transfer-encoding") {
            return Err(HttpError::new(
                400,
                "bad_request",
                "transfer encoding is not supported",
            ));
        }
        if name.trim().eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(HttpError::new(
                    400,
                    "bad_request",
                    "duplicate Content-Length header",
                ));
            }
            let parsed = value
                .trim()
                .parse::<usize>()
                .map_err(|_| HttpError::new(400, "bad_request", "invalid Content-Length"))?;
            if parsed > MAX_BODY_BYTES {
                return Err(HttpError::new(
                    413,
                    "payload_too_large",
                    "request body is too large",
                ));
            }
            content_length = Some(parsed);
        }
    }
    if !headers_ended {
        return Err(HttpError::new(
            431,
            "header_too_large",
            "request has too many headers",
        ));
    }
    let content_length = content_length.unwrap_or(0);
    let mut body = vec![0; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).map_err(|error| {
            HttpError::new(
                400,
                "bad_request",
                format!("incomplete request body: {error}"),
            )
        })?;
    }
    Ok(IncomingRequest { method, path, body })
}

fn read_line_limited(reader: &mut impl BufRead) -> ApiResult<String> {
    let mut bytes = Vec::new();
    let count = reader.read_until(b'\n', &mut bytes).map_err(|error| {
        HttpError::new(
            400,
            "bad_request",
            format!("could not read request: {error}"),
        )
    })?;
    if count == 0 {
        return Err(HttpError::new(400, "bad_request", "empty request"));
    }
    if count > MAX_HEADER_BYTES {
        return Err(HttpError::new(
            431,
            "header_too_large",
            "request header is too large",
        ));
    }
    let line = String::from_utf8(bytes)
        .map_err(|_| HttpError::new(400, "bad_request", "request is not UTF-8"))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> std::io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        response.status,
        reason,
        response.body.len()
    )?;
    stream.write_all(&response.body)
}

fn route_request(state: &Arc<ApiState>, request: IncomingRequest) -> ApiResult<HttpResponse> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => json_response(
            200,
            &serde_json::json!({
                "status": "ok",
                "service": "tpt-media-qc-local-api",
                "version": tpt_app_media_qc_core::config::APP_VERSION,
            }),
        ),
        ("GET", "/profiles") => json_response(200, &list_profiles()),
        ("POST", "/jobs") => create_job(state, request.body),
        ("GET", path) if path.starts_with("/jobs/") && path.ends_with("/results") => {
            job_results(state, job_id_from_path(path, Some("/results"))?)
        }
        ("POST", path) if path.starts_with("/jobs/") && path.ends_with("/cancel") => {
            cancel_job(state, job_id_from_path(path, Some("/cancel"))?)
        }
        ("GET", path) if path.starts_with("/jobs/") => {
            let id = job_id_from_path(path, None)?;
            let jobs = lock(&state.jobs)?;
            let job = jobs
                .get(&id)
                .cloned()
                .ok_or_else(|| HttpError::new(404, "not_found", format!("job '{id}' not found")))?;
            json_response(200, &job)
        }
        (_, "/health" | "/profiles" | "/jobs") => Err(HttpError::new(
            405,
            "method_not_allowed",
            "HTTP method is not allowed for this endpoint",
        )),
        _ => Err(HttpError::new(404, "not_found", "endpoint not found")),
    }
}

fn job_id_from_path(path: &str, suffix: Option<&str>) -> ApiResult<String> {
    let id = path
        .strip_prefix("/jobs/")
        .and_then(|value| {
            suffix
                .map(|suffix| value.strip_suffix(suffix))
                .unwrap_or(Some(value))
        })
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or_else(|| HttpError::new(404, "not_found", "job id is missing"))?;
    Ok(id.to_string())
}

fn create_job(state: &Arc<ApiState>, body: Vec<u8>) -> ApiResult<HttpResponse> {
    let request: CreateJob = serde_json::from_slice(&body).map_err(|error| {
        HttpError::new(400, "invalid_json", format!("invalid job request: {error}"))
    })?;
    if request.asset.trim().is_empty() {
        return Err(HttpError::new(
            400,
            "bad_request",
            "asset must not be empty",
        ));
    }
    let profile = resolve_profile(&request.profile).map_err(|error| {
        HttpError::new(
            400,
            "invalid_profile",
            format!("could not resolve profile: {error}"),
        )
    })?;
    let id = format!("job-{:08}", NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed));
    let job = ApiJob {
        id: id.clone(),
        asset: request.asset.clone(),
        profile: request.profile,
        quick: request.quick,
        status: ApiJobStatus::Queued,
        cancel_requested: false,
        verdict: None,
        error: None,
    };
    lock(&state.jobs)?.insert(id.clone(), job.clone());
    spawn_job(
        Arc::clone(state),
        id,
        PathBuf::from(request.asset),
        profile,
        request.quick,
    );
    json_response(202, &job)
}

fn job_results(state: &Arc<ApiState>, id: String) -> ApiResult<HttpResponse> {
    let job = lock(&state.jobs)?
        .get(&id)
        .cloned()
        .ok_or_else(|| HttpError::new(404, "not_found", format!("job '{id}' not found")))?;
    if job.status != ApiJobStatus::Succeeded {
        return json_response(202, &serde_json::json!({"id": id, "status": job.status}));
    }
    let results = lock(&state.results)?;
    let result = results
        .get(&id)
        .ok_or_else(|| HttpError::new(500, "internal_error", "completed job has no result"))?;
    json_response(200, result)
}

fn cancel_job(state: &Arc<ApiState>, id: String) -> ApiResult<HttpResponse> {
    let mut jobs = lock(&state.jobs)?;
    let job = jobs
        .get_mut(&id)
        .ok_or_else(|| HttpError::new(404, "not_found", format!("job '{id}' not found")))?;
    match job.status {
        ApiJobStatus::Queued => job.status = ApiJobStatus::Cancelled,
        ApiJobStatus::Running => job.cancel_requested = true,
        ApiJobStatus::Succeeded | ApiJobStatus::Failed | ApiJobStatus::Cancelled => {
            return Err(HttpError::new(
                409,
                "terminal_job",
                "job is already terminal",
            ));
        }
    }
    let job = job.clone();
    drop(jobs);
    if job.status == ApiJobStatus::Cancelled {
        lock(&state.cancelled)?.insert(id);
    }
    json_response(202, &job)
}

fn spawn_job(
    state: Arc<ApiState>,
    id: String,
    asset: PathBuf,
    profile: tpt_app_media_qc_profile::model::Profile,
    quick: bool,
) {
    let _ = thread::Builder::new()
        .name("tpt-media-qc-local-api-job".into())
        .spawn(move || {
            if !mark_running(&state, &id) {
                return;
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_qc(&asset, profile.clone(), quick)
            }));
            if is_cancelled(&state, &id) {
                finish_job(&state, &id, ApiJobStatus::Cancelled, None, None);
                return;
            }
            match outcome {
                Ok(Ok(run)) => {
                    let result = desktop_run(run, &profile);
                    let verdict = Some(result.verdict);
                    if let Ok(mut results) = state.results.lock() {
                        results.insert(id.clone(), result);
                    }
                    finish_job(&state, &id, ApiJobStatus::Succeeded, verdict, None);
                }
                Ok(Err(error)) => finish_job(&state, &id, ApiJobStatus::Failed, None, Some(error)),
                Err(_) => finish_job(
                    &state,
                    &id,
                    ApiJobStatus::Failed,
                    None,
                    Some("QC worker panicked".into()),
                ),
            }
        });
}

fn mark_running(state: &Arc<ApiState>, id: &str) -> bool {
    let Ok(mut jobs) = state.jobs.lock() else {
        return false;
    };
    let Some(job) = jobs.get_mut(id) else {
        return false;
    };
    if job.status != ApiJobStatus::Queued {
        return false;
    }
    job.status = ApiJobStatus::Running;
    true
}

fn is_cancelled(state: &Arc<ApiState>, id: &str) -> bool {
    state
        .cancelled
        .lock()
        .map(|ids| ids.contains(id))
        .unwrap_or(false)
        || state
            .jobs
            .lock()
            .map(|jobs| {
                jobs.get(id).is_some_and(|job| {
                    job.status == ApiJobStatus::Cancelled || job.cancel_requested
                })
            })
            .unwrap_or(false)
}

fn finish_job(
    state: &Arc<ApiState>,
    id: &str,
    status: ApiJobStatus,
    verdict: Option<VerdictDecision>,
    error: Option<String>,
) {
    let Ok(mut jobs) = state.jobs.lock() else {
        return;
    };
    let Some(job) = jobs.get_mut(id) else {
        return;
    };
    if job.status == ApiJobStatus::Cancelled && status != ApiJobStatus::Cancelled {
        return;
    }
    job.status = status;
    job.verdict = verdict;
    job.error = error;
    if status != ApiJobStatus::Running {
        job.cancel_requested = false;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> ApiResult<std::sync::MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| HttpError::new(500, "internal_error", "local API state lock was poisoned"))
}

fn json_response<T: Serialize>(status: u16, value: &T) -> ApiResult<HttpResponse> {
    let body = serde_json::to_vec(value).map_err(|error| {
        HttpError::new(
            500,
            "internal_error",
            format!("could not encode response: {error}"),
        )
    })?;
    Ok(HttpResponse { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn request(address: SocketAddr, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let mut stream = TcpStream::connect(address).expect("connect to local API");
        let headers = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let marker = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("HTTP response headers");
        let status = std::str::from_utf8(&response)
            .unwrap()
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse().ok())
            .expect("HTTP response status");
        (status, response[marker + 4..].to_vec())
    }

    #[test]
    fn api_switch_is_disabled_unless_explicitly_enabled() {
        assert!(LocalApi::from_switch(None).unwrap().is_none());
        assert!(LocalApi::from_switch(Some("0".into())).unwrap().is_none());
        let api = LocalApi::from_switch(Some("1".into())).unwrap().unwrap();
        assert!(api.local_addr().unwrap().ip().is_loopback());
    }

    #[test]
    fn rejects_non_loopback_binds_and_serves_health_and_profiles() {
        assert!(LocalApi::bind("0.0.0.0:0".parse().unwrap()).is_err());
        let api = LocalApi::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = api.local_addr().unwrap();
        let server = api.serve();

        let (status, body) = request(address, "GET", "/health", b"");
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["status"],
            "ok"
        );
        let (status, body) = request(address, "GET", "/profiles", b"");
        assert_eq!(status, 200);
        assert!(!serde_json::from_slice::<Vec<serde_json::Value>>(&body)
            .unwrap()
            .is_empty());
        drop(server);
    }

    /// A minimal but *valid* 16-bit mono PCM WAV: 44-byte canonical header plus
    /// eight samples of silence. The local API job runs the real engine, whose
    /// metadata pass parses the file, so the fixture must be valid media — a
    /// bare `RIFF` magic prefix is reported as a corrupt container.
    fn minimal_wav() -> Vec<u8> {
        const SAMPLES: u32 = 8;
        let data_len = SAMPLES * 2;
        let mut wav = Vec::with_capacity(44 + data_len as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // format = PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // channels
        wav.extend_from_slice(&8_000u32.to_le_bytes()); // sample rate
        wav.extend_from_slice(&16_000u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.resize(44 + data_len as usize, 0);
        wav
    }

    #[test]
    fn api_runs_a_local_job_and_exposes_its_result() {
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("api-sample.wav");
        std::fs::write(&media, minimal_wav()).unwrap();
        let api = LocalApi::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = api.local_addr().unwrap();
        let server = api.serve();
        let body = serde_json::to_vec(&serde_json::json!({
            "asset": media,
            "profile": "generic",
            "quick": true,
        }))
        .unwrap();

        let (status, body) = request(address, "POST", "/jobs", &body);
        assert_eq!(status, 202);
        let job: ApiJob = serde_json::from_slice(&body).unwrap();
        let mut terminal = None;
        for _ in 0..1500 {
            let (status, body) = request(address, "GET", &format!("/jobs/{}", job.id), b"");
            assert_eq!(status, 200);
            let current: ApiJob = serde_json::from_slice(&body).unwrap();
            if matches!(
                current.status,
                ApiJobStatus::Succeeded | ApiJobStatus::Failed | ApiJobStatus::Cancelled
            ) {
                terminal = Some(current);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let terminal = terminal.expect("API job must reach a terminal state");
        assert_eq!(
            terminal.status,
            ApiJobStatus::Succeeded,
            "{:?}",
            terminal.error
        );
        let (status, body) = request(address, "GET", &format!("/jobs/{}/results", job.id), b"");
        assert_eq!(status, 200);
        let result: DesktopRun = serde_json::from_slice(&body).unwrap();
        assert_eq!(result.asset.path, media.canonicalize().unwrap());
        drop(server);
    }
}
