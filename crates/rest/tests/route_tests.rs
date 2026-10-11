//! route_tests.rs — HTTP Route and RFC 7807 Problem Details tests.
//! ====================================================================
//!
//! Purpose:
//!     Verifies HTTP routing, synchronous vs asynchronous job lifecycles,
//!     bearer authentication gates, and RFC 7807 problem details serialization.
//!
//! Layer:
//!     Test Suite / REST Route Tests

use std::sync::Arc;
use std::time::Duration;

use pdfcraft_rest::executor::{BlockingExecutor, ExecutorConfig};
use pdfcraft_rest::models::{JobResponse, JobState};
use pdfcraft_rest::problem::ProblemDetails;
use pdfcraft_rest::routes::{HttpRequest, RestService};
use pdfcraft_rest::storage::InMemoryJobRepository;
use pdfcraft_sdk::{DocumentResult, DocumentSource, MergeOptions, OperationExecutor, RenderOptions, SdkError, SplitMode, SplitResult};

/// Simple mock executor for fast route testing.
struct FastMockExecutor;

impl OperationExecutor for FastMockExecutor {
    fn execute_merge(&self, _sources: &[DocumentSource], _options: &MergeOptions) -> Result<DocumentResult, SdkError> {
        Ok(DocumentResult::memory(b"%PDF-1.7 merged mock".to_vec(), "application/pdf"))
    }

    fn execute_split(&self, _source: &DocumentSource, _mode: &SplitMode) -> Result<SplitResult, SdkError> {
        Ok(SplitResult { files: vec![] })
    }

    fn execute_render(&self, _source: &DocumentSource, page: usize, _options: &RenderOptions) -> Result<DocumentResult, SdkError> {
        if page == 0 {
            return Err(SdkError::InvalidArgument("Page 0 is invalid".into()));
        }
        Ok(DocumentResult::memory(b"mock-png-bytes".to_vec(), "image/png"))
    }
}

fn create_test_service(token: Option<&str>) -> Arc<RestService> {
    let mock = Arc::new(FastMockExecutor);
    let config = ExecutorConfig::default();
    let executor = Arc::new(BlockingExecutor::new(mock, config));
    let repository = Arc::new(InMemoryJobRepository::new());
    Arc::new(RestService::new(executor, repository, token.map(str::to_string)))
}

#[test]
fn test_sync_merge_route_success() {
    let service = create_test_service(None);

    let body = serde_json::json!({
        "files": [
            { "data_base64": "JVBERi0xLjQK", "name": "doc1.pdf" }
        ],
        "async_job": false
    });

    let req =
        HttpRequest::new("POST", "/v1/merged-pdf").with_header("content-type", "application/json").with_body(serde_json::to_vec(&body).unwrap());

    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.headers.get("content-type"), Some(&"application/pdf".to_string()));
    assert_eq!(resp.body, b"%PDF-1.7 merged mock");
}

#[test]
fn test_async_merge_job_lifecycle() {
    let service = create_test_service(None);

    let body = serde_json::json!({
        "files": [
            { "data_base64": "JVBERi0xLjQK", "name": "doc1.pdf" }
        ],
        "async_job": true
    });

    let req =
        HttpRequest::new("POST", "/v1/merged-pdf").with_header("content-type", "application/json").with_body(serde_json::to_vec(&body).unwrap());

    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 202);
    let job: JobResponse = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(job.status, JobState::Queued);

    // Give background worker a brief moment to finish
    std::thread::sleep(Duration::from_millis(50));

    // Poll GET /v1/jobs/{id}
    let get_req = HttpRequest::new("GET", format!("/v1/jobs/{}", job.id));
    let get_resp = service.dispatch(&get_req);
    assert_eq!(get_resp.status, 200);
    let finished_job: JobResponse = serde_json::from_slice(&get_resp.body).unwrap();
    assert_eq!(finished_job.status, JobState::Succeeded);
    assert_eq!(finished_job.progress, 100);

    // Fetch result: GET /v1/jobs/{id}/result
    let res_req = HttpRequest::new("GET", format!("/v1/jobs/{}/result", job.id));
    let res_resp = service.dispatch(&res_req);
    assert_eq!(res_resp.status, 200);
    assert_eq!(res_resp.headers.get("content-type"), Some(&"application/pdf".to_string()));
    assert_eq!(res_resp.body, b"%PDF-1.7 merged mock");
}

#[test]
fn test_page_0_rejection_rfc7807() {
    let service = create_test_service(None);

    let body = serde_json::json!({
        "file": { "data_base64": "JVBERi0xLjQK", "name": "doc.pdf" },
        "page": 0,
        "async_job": false
    });

    let req =
        HttpRequest::new("POST", "/v1/page-preview").with_header("content-type", "application/json").with_body(serde_json::to_vec(&body).unwrap());

    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 400);
    assert_eq!(resp.headers.get("content-type"), Some(&"application/problem+json".to_string()));

    let problem: ProblemDetails = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(problem.status, 400);
    assert_eq!(problem.code, "BAD_REQUEST");
    assert!(problem.detail.contains("Page index must be >= 1"));
}

#[test]
fn test_bearer_authentication_gate() {
    let service = create_test_service(Some("secret-token-123"));

    let body = serde_json::json!({
        "files": [{ "data_base64": "JVBERi0xLjQK" }],
        "async_job": false
    });

    // 1. Without Authorization header -> 401 Unauthorized
    let unauth_req = HttpRequest::new("POST", "/v1/merged-pdf").with_body(serde_json::to_vec(&body).unwrap());
    let resp1 = service.dispatch(&unauth_req);
    assert_eq!(resp1.status, 401);
    let prob1: ProblemDetails = serde_json::from_slice(&resp1.body).unwrap();
    assert_eq!(prob1.code, "UNAUTHORIZED");

    // 2. With valid Authorization header -> 200 OK
    let auth_req = HttpRequest::new("POST", "/v1/merged-pdf")
        .with_header("authorization", "Bearer secret-token-123")
        .with_body(serde_json::to_vec(&body).unwrap());
    let resp2 = service.dispatch(&auth_req);
    assert_eq!(resp2.status, 200);
}
