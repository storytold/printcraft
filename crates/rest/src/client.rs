//! client.rs — Remote REST client implementing the unified PdfCraftClient trait (Issue #872).
//! =========================================================================================
//!
//! Purpose:
//!     Connects to an on-premise PdfCraft REST daemon, providing 100% semantic parity
//!     with LocalClient through identical input/output types and error mappings.
//!
//! Layer:
//!     REST Service / Client Transport Layer
//!
//! Key Input Dependencies:
//!     - pdfcraft_sdk::PdfCraftClient
//!     - pdfcraft_sdk::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, SdkError, SplitMode, SplitResult}
//!     - crate::routes::{HttpRequest, HttpResponse, RestService}
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_rest::client::RestClient;
//!     use pdfcraft_sdk::PdfCraftClient;
//!     let client = RestClient::remote("http://127.0.0.1:8080", None);
//!     ```
//!
//! Key Types & Functions Index:
//!     - RestClient: Client enum supporting Direct (in-process) and Remote (HTTP network) modes
//!     - RestClient::direct: In-process service invocation bypassing network stack
//!     - RestClient::remote: Remote HTTP/1.1 client connecting to base URL
//!     - RestClient::merge: Merges documents with synchronous or polling resolution
//!     - RestClient::split: Splits document
//!     - RestClient::render_page: Renders page to image format
//!     - RestClient::poll_job_result: Polls background job until completion or error

use std::sync::Arc;
use std::time::Duration;

use crate::models::{JobResponse, JobState, MergePayload, RenderPayload, SourcePayload, SplitPayload};
use crate::problem::ProblemDetails;
use crate::routes::{HttpRequest, HttpResponse, RestService};
use pdfcraft_sdk::{DocumentResult, DocumentSource, MergeOptions, PdfCraftClient, RenderOptions, SdkError, SplitMode, SplitResult};

/// Unified REST client communicating with a PdfCraft REST service.
pub enum RestClient {
    /// In-process service invocation bypassing network stack (for high performance and zero-cost testing).
    Direct(Arc<RestService>),
    /// Remote network connection using HTTP/1.1 endpoint.
    Remote { base_url: String, auth_token: Option<String> },
}

impl RestClient {
    /// Creates a client connecting to an in-process `RestService`.
    pub fn direct(service: Arc<RestService>) -> Self {
        Self::Direct(service)
    }

    /// Creates a remote HTTP client targeting a base URL (e.g. `http://127.0.0.1:8080`).
    pub fn remote(base_url: impl Into<String>, auth_token: Option<String>) -> Self {
        Self::Remote { base_url: base_url.into().trim_end_matches('/').to_string(), auth_token }
    }

    fn execute_request(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        match self {
            Self::Direct(service) => Ok(service.dispatch(&req)),
            Self::Remote { base_url, auth_token } => send_http_request(base_url, auth_token.as_deref(), req),
        }
    }
}

impl PdfCraftClient for RestClient {
    fn merge(&self, files: &[DocumentSource], options: MergeOptions) -> Result<DocumentResult, SdkError> {
        if files.is_empty() {
            return Err(SdkError::InvalidArgument("Cannot merge zero files".into()));
        }

        let mut sources = Vec::new();
        for file in files {
            sources.push(source_to_payload(file));
        }

        let payload = MergePayload { files: sources, options, async_job: false };

        let body = serde_json::to_vec(&payload).map_err(|e| SdkError::InvalidArgument(e.to_string()))?;

        let req = HttpRequest::new("POST", "/v1/merged-pdf").with_header("content-type", "application/json").with_body(body);

        let resp = self.execute_request(req)?;

        if resp.status == 200 {
            let mime = resp.headers.get("content-type").cloned().unwrap_or_else(|| "application/pdf".into());
            Ok(DocumentResult::memory(resp.body, mime))
        } else if resp.status == 202 {
            // Asynchronous job returned: poll until completion
            let job: JobResponse = serde_json::from_slice(&resp.body).map_err(|e| SdkError::EngineError(e.to_string()))?;
            self.poll_job_result(&job.id)
        } else {
            Err(parse_problem_error(&resp.body, resp.status))
        }
    }

    fn split(&self, file: &DocumentSource, mode: SplitMode) -> Result<SplitResult, SdkError> {
        let payload = SplitPayload { file: source_to_payload(file), mode, async_job: false };

        let body = serde_json::to_vec(&payload).map_err(|e| SdkError::InvalidArgument(e.to_string()))?;

        let req = HttpRequest::new("POST", "/v1/split-pdf").with_header("content-type", "application/json").with_body(body);

        let resp = self.execute_request(req)?;

        if resp.status == 200 {
            let res: SplitResult = serde_json::from_slice(&resp.body).map_err(|e| SdkError::EngineError(e.to_string()))?;
            Ok(res)
        } else {
            Err(parse_problem_error(&resp.body, resp.status))
        }
    }

    fn render_page(&self, file: &DocumentSource, page: usize, options: RenderOptions) -> Result<DocumentResult, SdkError> {
        if page == 0 {
            return Err(SdkError::InvalidArgument("Page index must be >= 1".into()));
        }

        let payload = RenderPayload { file: source_to_payload(file), page, options, async_job: false };

        let body = serde_json::to_vec(&payload).map_err(|e| SdkError::InvalidArgument(e.to_string()))?;

        let req = HttpRequest::new("POST", "/v1/page-preview").with_header("content-type", "application/json").with_body(body);

        let resp = self.execute_request(req)?;

        if resp.status == 200 {
            let mime = resp.headers.get("content-type").cloned().unwrap_or_else(|| "image/png".into());
            Ok(DocumentResult::memory(resp.body, mime))
        } else {
            Err(parse_problem_error(&resp.body, resp.status))
        }
    }
}

impl RestClient {
    fn poll_job_result(&self, job_id: &str) -> Result<DocumentResult, SdkError> {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);

        while std::time::Instant::now() < deadline {
            let req = HttpRequest::new("GET", format!("/v1/jobs/{job_id}"));
            let resp = self.execute_request(req)?;
            if resp.status != 200 {
                return Err(parse_problem_error(&resp.body, resp.status));
            }

            let job: JobResponse = serde_json::from_slice(&resp.body).map_err(|e| SdkError::EngineError(e.to_string()))?;

            match job.status {
                JobState::Succeeded => {
                    let res_req = HttpRequest::new("GET", format!("/v1/jobs/{job_id}/result"));
                    let res_resp = self.execute_request(res_req)?;
                    if res_resp.status == 200 {
                        let mime = res_resp.headers.get("content-type").cloned().unwrap_or_else(|| "application/pdf".into());
                        return Ok(DocumentResult::memory(res_resp.body, mime));
                    } else {
                        return Err(parse_problem_error(&res_resp.body, res_resp.status));
                    }
                }
                JobState::Failed => {
                    return Err(SdkError::EngineError(job.error.unwrap_or_else(|| "Job failed".into())));
                }
                JobState::Canceled => {
                    return Err(SdkError::EngineError("Job was canceled".into()));
                }
                JobState::Queued | JobState::Running => {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }

        Err(SdkError::EngineError("Job polling timeout exceeded".into()))
    }
}

fn source_to_payload(source: &DocumentSource) -> SourcePayload {
    match source {
        DocumentSource::Path(p) => SourcePayload::Path { path: p.clone() },
        DocumentSource::Bytes { data, name } => SourcePayload::Bytes { data_base64: encode_base64(data), name: name.clone() },
    }
}

fn parse_problem_error(body: &[u8], status: u16) -> SdkError {
    if let Ok(problem) = serde_json::from_slice::<ProblemDetails>(body) {
        match problem.status {
            400 => SdkError::InvalidArgument(problem.detail),
            401 => SdkError::Unauthorized(problem.detail),
            404 => SdkError::FileNotFound { path: problem.detail },
            413 => SdkError::QuotaExceeded(problem.detail),
            429 => SdkError::QuotaExceeded(problem.detail),
            501 => SdkError::UnsupportedOperation(problem.detail),
            _ => SdkError::EngineError(problem.detail),
        }
    } else {
        SdkError::EngineError(format!("HTTP error {status}"))
    }
}

fn encode_base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn send_http_request(base_url: &str, auth_token: Option<&str>, req: HttpRequest) -> Result<HttpResponse, SdkError> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;

    let url_without_proto = base_url.strip_prefix("http://").unwrap_or(base_url);
    let (host, port_str) = url_without_proto.split_once(':').unwrap_or((url_without_proto, "80"));
    let port: u16 = port_str.parse().unwrap_or(80);

    let mut stream = TcpStream::connect((host, port)).map_err(SdkError::Io)?;

    let mut request_bytes = Vec::new();
    let full_path = req.path;
    request_bytes.extend_from_slice(format!("{} {} HTTP/1.1\r\n", req.method, full_path).as_bytes());
    request_bytes.extend_from_slice(format!("Host: {host}:{port}\r\n").as_bytes());

    if let Some(token) = auth_token {
        request_bytes.extend_from_slice(format!("Authorization: Bearer {token}\r\n").as_bytes());
    }

    for (k, v) in &req.headers {
        request_bytes.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    // The server reads the body only when told how long it is.
    if !req.headers.keys().any(|k| k.eq_ignore_ascii_case("content-length")) {
        request_bytes.extend_from_slice(format!("Content-Length: {}\r\n", req.body.len()).as_bytes());
    }
    request_bytes.extend_from_slice(b"Connection: close\r\n\r\n");
    request_bytes.extend_from_slice(&req.body);

    stream.write_all(&request_bytes).map_err(SdkError::Io)?;
    stream.flush().map_err(SdkError::Io)?;

    let Ok(stream_clone) = stream.try_clone() else {
        return Err(SdkError::EngineError("Failed to clone TCP stream".into()));
    };
    let mut reader = BufReader::new(stream_clone);

    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(SdkError::Io)?;
    let parts: Vec<&str> = status_line.split_whitespace().collect();
    let status = parts.get(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(500);

    let mut headers = std::collections::HashMap::new();
    let mut content_length = 0usize;

    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(SdkError::Io)?;
        if line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_lowercase();
            let val = v.trim().to_string();
            if key == "content-length"
                && let Ok(len) = val.parse::<usize>()
            {
                content_length = len;
            }
            headers.insert(key, val);
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).map_err(SdkError::Io)?;
    }

    Ok(HttpResponse { status, headers, body })
}
