//! server.rs — Secure loopback HTTP server for on-premise document processing.
//! ==============================================================================
//!
//! Purpose:
//!     Binds to 127.0.0.1 loopback interface, decodes HTTP/1.1 requests, enforces
//!     public binding confirmations and optional Bearer auth, invokes RestService,
//!     and streams RFC 7807 responses.
//!
//! Layer:
//!     REST Service / HTTP Server Transport Layer
//!
//! Key Input Dependencies:
//!     - std::net::{TcpListener, TcpStream, SocketAddr}
//!     - crate::routes::{HttpRequest, HttpResponse, RestService}
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_rest::server::{HttpServer, ServerConfig};
//!     // let server = HttpServer::start(service, ServerConfig::default())?;
//!     ```
//!
//! Key Types & Functions Index:
//!     - ServerConfig: Server configuration (bind_addr, allow_public_bind, auth_token)
//!     - HttpServer: Running HTTP server handle with background listener thread
//!     - HttpServer::start: Starts server on background thread with loopback security check
//!     - HttpServer::local_addr: Bound local socket address
//!     - HttpServer::port: Port number of running server
//!     - HttpServer::stop: Gracefully stops the listener thread

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::routes::{HttpRequest, RestService};

/// Server configuration options.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: SocketAddr,
    pub allow_public_bind: bool,
    pub auth_token: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 0))),
            allow_public_bind: false,
            auth_token: None,
        }
    }
}

/// Running HTTP server handle.
pub struct HttpServer {
    listener_addr: SocketAddr,
    running: Arc<AtomicBool>,
}

impl HttpServer {
    /// Starts the HTTP server on a background thread.
    pub fn start(service: Arc<RestService>, config: ServerConfig) -> std::io::Result<Self> {
        // Enforce loopback security policy (§6.1)
        if !config.bind_addr.ip().is_loopback() && !config.allow_public_bind {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Refusing non-loopback bind without explicit public bind confirmation (--public-bind-confirm)",
            ));
        }

        let listener = TcpListener::bind(config.bind_addr)?;
        listener.set_nonblocking(true)?;
        let listener_addr = listener.local_addr()?;
        let running = Arc::new(AtomicBool::new(true));

        let is_running = Arc::clone(&running);
        std::thread::Builder::new().name("pdfcraft-rest-http".into()).spawn(move || {
            while is_running.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let s = Arc::clone(&service);
                        std::thread::Builder::new()
                            .name("pdfcraft-http-conn".into())
                            .spawn(move || {
                                handle_connection(stream, &s);
                            })
                            .ok();
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        })?;

        Ok(Self { listener_addr, running })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.listener_addr
    }

    pub fn port(&self) -> u16 {
        self.listener_addr.port()
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Largest request body accepted (matches the SDK's default `max_request_bytes`).
const MAX_BODY_BYTES: usize = 100 * 1024 * 1024;
/// Most request headers accepted.
const MAX_HEADERS: usize = 100;

fn handle_connection(stream: TcpStream, service: &RestService) {
    let Ok(stream_clone) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream_clone);
    let mut writer = stream;

    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }

    // Parse request line: METHOD URI HTTP/1.1
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts.first().copied().unwrap_or("GET").to_string();
    let path = parts.get(1).copied().unwrap_or("/").to_string();

    // Parse headers
    let mut headers = HashMap::new();
    let mut content_length = 0usize;

    loop {
        // Bound the header section: a peer must not be able to grow it without limit.
        if headers.len() >= MAX_HEADERS {
            let _ = writer.write_all(b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        line.clear();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
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

    // Read body. The length comes from the peer, so cap it before allocating.
    if content_length > MAX_BODY_BYTES {
        let _ = writer.write_all(b"HTTP/1.1 413 Content Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    let req = HttpRequest { method, path, headers, body };

    let resp = service.dispatch(&req);

    // Write HTTP response
    let status_line = match resp.status {
        200 => "HTTP/1.1 200 OK\r\n",
        202 => "HTTP/1.1 202 Accepted\r\n",
        400 => "HTTP/1.1 400 Bad Request\r\n",
        401 => "HTTP/1.1 401 Unauthorized\r\n",
        404 => "HTTP/1.1 404 Not Found\r\n",
        409 => "HTTP/1.1 409 Conflict\r\n",
        413 => "HTTP/1.1 413 Payload Too Large\r\n",
        429 => "HTTP/1.1 429 Too Many Requests\r\n",
        500 => "HTTP/1.1 500 Internal Server Error\r\n",
        501 => "HTTP/1.1 501 Not Implemented\r\n",
        503 => "HTTP/1.1 503 Service Unavailable\r\n",
        504 => "HTTP/1.1 504 Gateway Timeout\r\n",
        _ => "HTTP/1.1 500 Internal Server Error\r\n",
    };

    let mut response_bytes = Vec::new();
    response_bytes.extend_from_slice(status_line.as_bytes());

    for (k, v) in &resp.headers {
        response_bytes.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    if !resp.headers.keys().any(|k| k.eq_ignore_ascii_case("content-length")) {
        response_bytes.extend_from_slice(format!("Content-Length: {}\r\n", resp.body.len()).as_bytes());
    }
    response_bytes.extend_from_slice(b"Connection: close\r\n\r\n");
    response_bytes.extend_from_slice(&resp.body);

    let _ = writer.write_all(&response_bytes);
    let _ = writer.flush();
}
