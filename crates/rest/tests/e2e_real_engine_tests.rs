//! e2e_real_engine_tests.rs — The REST service against the real PdfCraft engine, over real HTTP.
//! ===============================================================================================
//!
//! Purpose:
//!     The other REST tests use a mock engine. These start the loopback `HttpServer` on top of the
//!     production `AutomationEngineAdapter` and drive it with `RestClient::remote`, so request
//!     framing, authentication, the engine and the response path are all exercised together, and
//!     results are compared with the in-process `LocalClient`.
//!
//! Layer:
//!     Test Suite / REST End-to-End

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use pdfcraft_rest::client::RestClient;
use pdfcraft_rest::executor::{BlockingExecutor, ExecutorConfig};
use pdfcraft_rest::routes::RestService;
use pdfcraft_rest::server::{HttpServer, ServerConfig};
use pdfcraft_rest::storage::InMemoryJobRepository;
use pdfcraft_sdk::{
    AutomationEngineAdapter, DocumentResult, DocumentSource, LocalClient, MergeOptions, OperationExecutor, PdfCraftClient, RenderOptions, SdkError,
    SplitMode,
};

const TOKEN: &str = "e2e-token";

/// A small, valid PDF (correct xref) with `pages` Letter pages, each with a line of text.
fn make_pdf(pages: usize, label: &str) -> Vec<u8> {
    let mut objs: Vec<String> = Vec::new();
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + i * 2)).collect();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objs.push(format!("<< /Type /Pages /Count {pages} /Kids [{}] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..pages {
        let content = format!("BT /F1 24 Tf 72 700 Td ({label} page {}) Tj ET", i + 1);
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
            5 + i * 2
        ));
        objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn start_server() -> HttpServer {
    let engine: Arc<dyn OperationExecutor> = Arc::new(AutomationEngineAdapter::default());
    let executor = Arc::new(BlockingExecutor::new(engine, ExecutorConfig::default()));
    let service = Arc::new(RestService::new(executor, Arc::new(InMemoryJobRepository::new()), Some(TOKEN.into())));
    HttpServer::start(service, ServerConfig { bind_addr: "127.0.0.1:0".parse().unwrap(), allow_public_bind: false, auth_token: Some(TOKEN.into()) })
        .unwrap()
}

fn client_for(server: &HttpServer, token: Option<&str>) -> RestClient {
    RestClient::remote(format!("http://127.0.0.1:{}", server.port()), token.map(str::to_string))
}

fn png_dims(png: &[u8]) -> (u32, u32) {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    (u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap()))
}

fn bytes_of(r: DocumentResult) -> Vec<u8> {
    match r {
        DocumentResult::Memory { data, .. } => data,
        DocumentResult::File { path, .. } => std::fs::read(path).unwrap(),
    }
}

#[test]
fn merge_over_http_produces_a_pdf_with_all_pages() {
    let server = start_server();
    let rest = client_for(&server, Some(TOKEN));
    let a = DocumentSource::from_bytes(make_pdf(1, "A"), Some("a.pdf".into()));
    let b = DocumentSource::from_bytes(make_pdf(2, "B"), Some("b.pdf".into()));

    let merged = rest.merge(&[a, b], MergeOptions::default()).expect("merge over HTTP");
    assert_eq!(merged.mime_type(), "application/pdf");
    let bytes = bytes_of(merged);
    assert!(bytes.starts_with(b"%PDF-"));

    // The merged file must really contain 1 + 2 pages: split it into single pages through REST.
    let parts = rest
        .split(&DocumentSource::from_bytes(bytes, Some("merged.pdf".into())), SplitMode::EveryNPages(1))
        .expect("split merged document over HTTP");
    assert_eq!(parts.files.len(), 3);
}

#[test]
fn render_over_http_returns_a_png_identical_to_the_local_client() {
    let server = start_server();
    let rest = client_for(&server, Some(TOKEN));
    let local = LocalClient::default();
    let src = DocumentSource::from_bytes(make_pdf(2, "R"), Some("r.pdf".into()));
    let opts = || RenderOptions { dpi: 72.0, ..Default::default() };

    let via_rest = rest.render_page(&src, 2, opts()).expect("render over HTTP");
    let via_local = local.render_page(&src, 2, opts()).expect("render locally");
    assert_eq!(via_rest.mime_type(), "image/png");

    let (rest_png, local_png) = (bytes_of(via_rest), bytes_of(via_local));
    assert_eq!(png_dims(&rest_png), (612, 792), "72 dpi Letter page");
    assert_eq!(rest_png, local_png, "REST and local rendering must be byte-identical");
}

#[test]
fn errors_cross_the_http_boundary_with_the_same_codes() {
    let server = start_server();
    let rest = client_for(&server, Some(TOKEN));
    let src = DocumentSource::from_bytes(make_pdf(1, "E"), Some("e.pdf".into()));

    let err = rest.render_page(&src, 0, RenderOptions::default()).unwrap_err();
    assert_eq!(err.error_code(), "INVALID_ARGUMENT");
    assert_eq!(err.http_status(), 400);

    let err = rest.render_page(&src, 99, RenderOptions::default()).unwrap_err();
    assert!(matches!(err, SdkError::EngineError(_) | SdkError::PageOutOfBounds { .. }), "got {err:?}");
}

#[test]
fn wrong_or_missing_token_is_rejected_with_401() {
    let server = start_server();
    let src = DocumentSource::from_bytes(make_pdf(1, "U"), Some("u.pdf".into()));
    for token in [None, Some("wrong")] {
        let rest = client_for(&server, token);
        let err = rest.render_page(&src, 1, RenderOptions::default()).unwrap_err();
        assert_eq!(err.http_status(), 401, "token {token:?} should be unauthorized, got {err:?}");
    }
}

#[test]
fn unknown_route_returns_problem_json_404() {
    let server = start_server();
    let mut s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
    write!(s, "GET /v1/nope HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n").unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 404"), "got: {resp}");
    assert!(resp.contains("application/problem+json"), "RFC 7807 content type expected: {resp}");
}
