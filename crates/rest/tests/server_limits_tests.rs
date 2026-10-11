//! server_limits_tests.rs — The HTTP server must bound what a peer can make it allocate or read.
//! ==============================================================================================
//!
//! Layer:
//!     Test Suite / REST Server Hardening

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use pdfcraft_rest::executor::{BlockingExecutor, ExecutorConfig};
use pdfcraft_rest::routes::RestService;
use pdfcraft_rest::server::{HttpServer, ServerConfig};
use pdfcraft_rest::storage::InMemoryJobRepository;
use pdfcraft_sdk::AutomationEngineAdapter;

fn start() -> HttpServer {
    let engine: Arc<dyn pdfcraft_sdk::OperationExecutor> = Arc::new(AutomationEngineAdapter::default());
    let executor = Arc::new(BlockingExecutor::new(engine, ExecutorConfig::default()));
    let service = Arc::new(RestService::new(executor, Arc::new(InMemoryJobRepository::new()), None));
    HttpServer::start(service, ServerConfig { bind_addr: "127.0.0.1:0".parse().unwrap(), allow_public_bind: false, auth_token: None }).unwrap()
}

fn raw(port: u16, request: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

#[test]
fn oversized_content_length_is_rejected_before_allocation() {
    // Regression: the body buffer was sized straight from the peer's Content-Length header.
    let server = start();
    let resp = raw(server.port(), "POST /v1/merged-pdf HTTP/1.1\r\nContent-Length: 99999999999\r\nConnection: close\r\n\r\n");
    assert!(resp.starts_with("HTTP/1.1 413"), "got: {resp}");
}

#[test]
fn unbounded_header_section_is_rejected() {
    let server = start();
    let mut req = String::from("GET /v1/health HTTP/1.1\r\n");
    for i in 0..500 {
        req.push_str(&format!("X-H{i}: v\r\n"));
    }
    req.push_str("\r\n");
    let resp = raw(server.port(), &req);
    assert!(resp.starts_with("HTTP/1.1 431"), "got: {resp}");
}
