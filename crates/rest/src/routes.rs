//! routes.rs — HTTP route handlers and request dispatching for document processing operations.
//! ==============================================================================================
//!
//! Purpose:
//!     Parses HTTP requests, enforces bearer authentication, routes to sync/async
//!     workflows, and returns RFC 7807 problem details or typed results.
//!
//! Layer:
//!     REST Service / API Routing & Presentation Layer
//!
//! Key Input Dependencies:
//!     - crate::executor::BlockingExecutor
//!     - crate::storage::JobRepository
//!     - crate::models::{JobId, JobRecord, JobResponse, JobState, MergePayload, RenderPayload, SplitPayload}
//!     - crate::problem::ProblemDetails
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_rest::routes::{HttpRequest, RestService};
//!     // service.dispatch(&req) -> HttpResponse
//!     ```
//!
//! Key Types & Functions Index:
//!     - HttpRequest: In-memory HTTP request DTO (with_header, with_body, header)
//!     - HttpResponse: In-memory HTTP response DTO (ok_json, accepted_json, ok_bytes, problem)
//!     - RestService: Dispatcher mapping HTTP routes to engine execution and job management
//!     - RestService::dispatch: Main request router enforcing auth and dispatching paths
//!     - RestService::handle_merge: Handles POST /v1/merged-pdf (sync or async 202)
//!     - RestService::handle_split: Handles POST /v1/split-pdf
//!     - RestService::handle_page_preview: Handles POST /v1/page-preview
//!     - RestService::handle_get_job: Handles GET /v1/jobs/{id}
//!     - RestService::handle_get_job_result: Handles GET /v1/jobs/{id}/result
//!     - RestService::handle_cancel_job: Handles DELETE /v1/jobs/{id}

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

use crate::executor::BlockingExecutor;
use crate::models::{JobId, JobRecord, JobResponse, JobState, MergePayload, RenderPayload, SplitPayload};
use crate::problem::ProblemDetails;
use crate::storage::JobRepository;
use pdfcraft_sdk::DocumentResult;

/// In-memory representation of an HTTP request.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn new(method: impl Into<String>, path: impl Into<String>) -> Self {
        Self { method: method.into(), path: path.into(), headers: HashMap::new(), body: Vec::new() }
    }

    pub fn with_header(mut self, name: impl Into<String>, val: impl Into<String>) -> Self {
        self.headers.insert(name.into().to_lowercase(), val.into());
        self
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_lowercase()).map(|s| s.as_str())
    }
}

/// In-memory representation of an HTTP response.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn ok_json<T: serde::Serialize>(data: &T) -> Self {
        let body = serde_json::to_vec(data).unwrap_or_default();
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "application/json".into());
        headers.insert("content-length".into(), body.len().to_string());
        Self { status: 200, headers, body }
    }

    pub fn accepted_json<T: serde::Serialize>(location: &str, data: &T) -> Self {
        let body = serde_json::to_vec(data).unwrap_or_default();
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "application/json".into());
        headers.insert("location".into(), location.into());
        headers.insert("content-length".into(), body.len().to_string());
        Self { status: 202, headers, body }
    }

    pub fn ok_bytes(mime_type: &str, body: Vec<u8>) -> Self {
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), mime_type.into());
        headers.insert("content-length".into(), body.len().to_string());
        Self { status: 200, headers, body }
    }

    pub fn problem(problem: ProblemDetails) -> Self {
        let body = serde_json::to_vec(&problem).unwrap_or_default();
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "application/problem+json".into());
        headers.insert("content-length".into(), body.len().to_string());
        Self { status: problem.status, headers, body }
    }
}

/// Central document service dispatcher.
pub struct RestService {
    executor: Arc<BlockingExecutor>,
    repository: Arc<dyn JobRepository>,
    auth_token: Option<String>,
}

impl RestService {
    pub fn new(executor: Arc<BlockingExecutor>, repository: Arc<dyn JobRepository>, auth_token: Option<String>) -> Self {
        Self { executor, repository, auth_token }
    }

    /// Dispatches an incoming HTTP request to the matching endpoint.
    pub fn dispatch(&self, req: &HttpRequest) -> HttpResponse {
        // 1. Bearer Token Authentication check
        if let Some(expected_token) = &self.auth_token {
            let auth_header = req.header("authorization").unwrap_or("");
            let token = auth_header.strip_prefix("Bearer ").map(str::trim);
            if token != Some(expected_token.as_str()) {
                return HttpResponse::problem(ProblemDetails::unauthorized("Missing or invalid bearer token"));
            }
        }

        let path = req.path.trim_end_matches('/');
        let method = req.method.to_uppercase();

        match (method.as_str(), path) {
            ("POST", "/v1/merged-pdf") => self.handle_merge(req),
            ("POST", "/v1/split-pdf") => self.handle_split(req),
            ("POST", "/v1/page-preview") => self.handle_page_preview(req),
            ("GET", p) if p.starts_with("/v1/jobs/") => {
                let suffix = &p["/v1/jobs/".len()..];
                if let Some((id_str, "result")) = suffix.split_once('/') {
                    self.handle_get_job_result(&JobId::new(id_str))
                } else {
                    self.handle_get_job(&JobId::new(suffix))
                }
            }
            ("DELETE", p) if p.starts_with("/v1/jobs/") => {
                let id_str = &p["/v1/jobs/".len()..];
                self.handle_cancel_job(&JobId::new(id_str))
            }
            _ => HttpResponse::problem(ProblemDetails::not_found(format!("Route not found: {} {}", req.method, req.path))),
        }
    }

    fn handle_merge(&self, req: &HttpRequest) -> HttpResponse {
        let payload: MergePayload = match serde_json::from_slice(&req.body) {
            Ok(p) => p,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(format!("Invalid JSON payload: {e}"))),
        };

        if payload.files.is_empty() {
            return HttpResponse::problem(ProblemDetails::bad_request("Files array cannot be empty"));
        }

        let mut sources = Vec::new();
        for file in payload.files {
            match file.into_source() {
                Ok(src) => sources.push(src),
                Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(e)),
            }
        }

        if payload.async_job {
            let job_id = JobId::generate();
            let record = JobRecord::new_queued(job_id.clone(), "merge", None);
            if let Err(e) = self.repository.create(record) {
                return HttpResponse::problem(ProblemDetails::internal_error(e.to_string()));
            }

            let executor = Arc::clone(&self.executor);
            let repository = Arc::clone(&self.repository);
            let jid = job_id.clone();
            let opt = payload.options;

            std::thread::Builder::new()
                .name(format!("job-merge-{}", job_id.0))
                .spawn(move || {
                    let _ = repository.transition(&jid, JobState::Queued, JobState::Running, None, None, None);
                    match executor.execute_merge(&sources, &opt) {
                        Ok(res) => {
                            let _ = repository.transition(&jid, JobState::Running, JobState::Succeeded, None, Some(res), None);
                        }
                        Err(e) => {
                            let _ = repository.transition(&jid, JobState::Running, JobState::Failed, Some(e.to_string()), None, None);
                        }
                    }
                })
                .ok();

            let response_body = JobResponse {
                id: job_id.0.clone(),
                operation: "merge".into(),
                status: JobState::Queued,
                progress: 0,
                created_at: 0,
                updated_at: 0,
                error: None,
                result_url: None,
            };
            HttpResponse::accepted_json(&format!("/v1/jobs/{}", job_id.0), &response_body)
        } else {
            match self.executor.execute_merge(&sources, &payload.options) {
                Ok(DocumentResult::Memory { data, mime_type }) => HttpResponse::ok_bytes(&mime_type, data),
                Ok(DocumentResult::File { path, mime_type, .. }) => match fs::read(&path) {
                    Ok(bytes) => HttpResponse::ok_bytes(&mime_type, bytes),
                    Err(e) => HttpResponse::problem(ProblemDetails::internal_error(format!("Failed to read output: {e}"))),
                },
                Err(e) => HttpResponse::problem(ProblemDetails::new(e.http_status(), "Merge Failed", e.to_string(), e.error_code())),
            }
        }
    }

    fn handle_split(&self, req: &HttpRequest) -> HttpResponse {
        let payload: SplitPayload = match serde_json::from_slice(&req.body) {
            Ok(p) => p,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(format!("Invalid JSON payload: {e}"))),
        };

        let source = match payload.file.into_source() {
            Ok(src) => src,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(e)),
        };

        if payload.async_job {
            let job_id = JobId::generate();
            let record = JobRecord::new_queued(job_id.clone(), "split", None);
            if let Err(e) = self.repository.create(record) {
                return HttpResponse::problem(ProblemDetails::internal_error(e.to_string()));
            }

            let executor = Arc::clone(&self.executor);
            let repository = Arc::clone(&self.repository);
            let jid = job_id.clone();
            let mode = payload.mode;

            std::thread::Builder::new()
                .name(format!("job-split-{}", job_id.0))
                .spawn(move || {
                    let _ = repository.transition(&jid, JobState::Queued, JobState::Running, None, None, None);
                    match executor.execute_split(&source, &mode) {
                        Ok(res) => {
                            let _ = repository.transition(&jid, JobState::Running, JobState::Succeeded, None, None, Some(res));
                        }
                        Err(e) => {
                            let _ = repository.transition(&jid, JobState::Running, JobState::Failed, Some(e.to_string()), None, None);
                        }
                    }
                })
                .ok();

            let response_body = JobResponse {
                id: job_id.0.clone(),
                operation: "split".into(),
                status: JobState::Queued,
                progress: 0,
                created_at: 0,
                updated_at: 0,
                error: None,
                result_url: None,
            };
            HttpResponse::accepted_json(&format!("/v1/jobs/{}", job_id.0), &response_body)
        } else {
            match self.executor.execute_split(&source, &payload.mode) {
                Ok(split_res) => HttpResponse::ok_json(&split_res),
                Err(e) => HttpResponse::problem(ProblemDetails::new(e.http_status(), "Split Failed", e.to_string(), e.error_code())),
            }
        }
    }

    fn handle_page_preview(&self, req: &HttpRequest) -> HttpResponse {
        let payload: RenderPayload = match serde_json::from_slice(&req.body) {
            Ok(p) => p,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(format!("Invalid JSON payload: {e}"))),
        };

        if payload.page == 0 {
            return HttpResponse::problem(ProblemDetails::bad_request("Page index must be >= 1 (1-based index)"));
        }

        let source = match payload.file.into_source() {
            Ok(src) => src,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(e)),
        };

        if payload.async_job {
            let job_id = JobId::generate();
            let record = JobRecord::new_queued(job_id.clone(), "render", None);
            if let Err(e) = self.repository.create(record) {
                return HttpResponse::problem(ProblemDetails::internal_error(e.to_string()));
            }

            let executor = Arc::clone(&self.executor);
            let repository = Arc::clone(&self.repository);
            let jid = job_id.clone();
            let page = payload.page;
            let options = payload.options;

            std::thread::Builder::new()
                .name(format!("job-render-{}", job_id.0))
                .spawn(move || {
                    let _ = repository.transition(&jid, JobState::Queued, JobState::Running, None, None, None);
                    match executor.execute_render(&source, page, &options) {
                        Ok(res) => {
                            let _ = repository.transition(&jid, JobState::Running, JobState::Succeeded, None, Some(res), None);
                        }
                        Err(e) => {
                            let _ = repository.transition(&jid, JobState::Running, JobState::Failed, Some(e.to_string()), None, None);
                        }
                    }
                })
                .ok();

            let response_body = JobResponse {
                id: job_id.0.clone(),
                operation: "render".into(),
                status: JobState::Queued,
                progress: 0,
                created_at: 0,
                updated_at: 0,
                error: None,
                result_url: None,
            };
            HttpResponse::accepted_json(&format!("/v1/jobs/{}", job_id.0), &response_body)
        } else {
            match self.executor.execute_render(&source, payload.page, &payload.options) {
                Ok(DocumentResult::Memory { data, mime_type }) => HttpResponse::ok_bytes(&mime_type, data),
                Ok(DocumentResult::File { path, mime_type, .. }) => match fs::read(&path) {
                    Ok(bytes) => HttpResponse::ok_bytes(&mime_type, bytes),
                    Err(e) => HttpResponse::problem(ProblemDetails::internal_error(format!("Failed to read output: {e}"))),
                },
                Err(e) => HttpResponse::problem(ProblemDetails::new(e.http_status(), "Render Failed", e.to_string(), e.error_code())),
            }
        }
    }

    fn handle_get_job(&self, id: &JobId) -> HttpResponse {
        match self.repository.get(id) {
            Ok(Some(record)) => HttpResponse::ok_json(&JobResponse::from(&record)),
            Ok(None) => HttpResponse::problem(ProblemDetails::not_found(format!("Job not found: {id}"))),
            Err(e) => HttpResponse::problem(ProblemDetails::internal_error(e.to_string())),
        }
    }

    fn handle_get_job_result(&self, id: &JobId) -> HttpResponse {
        match self.repository.get(id) {
            Ok(Some(record)) => {
                if record.state != JobState::Succeeded {
                    return HttpResponse::problem(ProblemDetails::conflict(format!(
                        "Job {} has not succeeded yet (current state: {:?})",
                        id, record.state
                    )));
                }

                if let Some(res) = record.result {
                    match res {
                        DocumentResult::Memory { data, mime_type } => HttpResponse::ok_bytes(&mime_type, data),
                        DocumentResult::File { path, mime_type, .. } => match fs::read(&path) {
                            Ok(bytes) => HttpResponse::ok_bytes(&mime_type, bytes),
                            Err(e) => HttpResponse::problem(ProblemDetails::internal_error(format!("Failed to read file: {e}"))),
                        },
                    }
                } else if let Some(split) = record.split_result {
                    HttpResponse::ok_json(&split)
                } else {
                    HttpResponse::problem(ProblemDetails::internal_error("Job succeeded but has no output result"))
                }
            }
            Ok(None) => HttpResponse::problem(ProblemDetails::not_found(format!("Job not found: {id}"))),
            Err(e) => HttpResponse::problem(ProblemDetails::internal_error(e.to_string())),
        }
    }

    fn handle_cancel_job(&self, id: &JobId) -> HttpResponse {
        match self.repository.get(id) {
            Ok(Some(record)) => {
                if record.state.is_terminal() {
                    return HttpResponse::problem(ProblemDetails::conflict(format!("Cannot cancel terminal job {} in state {:?}", id, record.state)));
                }

                match self.repository.transition(id, record.state, JobState::Canceled, Some("Canceled by user".into()), None, None) {
                    Ok(()) => {
                        let updated = self.repository.get(id).ok().flatten();
                        match updated {
                            Some(r) => HttpResponse::ok_json(&JobResponse::from(&r)),
                            None => HttpResponse::ok_json(&JobResponse {
                                id: id.0.clone(),
                                operation: record.operation,
                                status: JobState::Canceled,
                                progress: record.progress_pct,
                                created_at: record.created_at,
                                updated_at: record.updated_at,
                                error: Some("Canceled by user".into()),
                                result_url: None,
                            }),
                        }
                    }
                    Err(e) => HttpResponse::problem(ProblemDetails::conflict(e.to_string())),
                }
            }
            Ok(None) => HttpResponse::problem(ProblemDetails::not_found(format!("Job not found: {id}"))),
            Err(e) => HttpResponse::problem(ProblemDetails::internal_error(e.to_string())),
        }
    }
}
