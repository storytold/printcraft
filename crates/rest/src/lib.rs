//! lib.rs — On-Premise REST Service for PdfCraft Document Processing (Issue #872).
//! ==============================================================================
//!
//! Purpose:
//!     Main entry point for pdfcraft-rest daemon crate. Exports durable storage,
//!     bounded blocking executor, HTTP server, RFC 7807 problem details, and remote RestClient.
//!
//! Layer:
//!     REST Service Root
//!
//! Key Input Dependencies:
//!     - pdfcraft-sdk
//!     - serde
//!     - serde_json
//!     - thiserror
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_rest::{HttpServer, RestService, ServerConfig};
//!     // Launches daemon on 127.0.0.1 loopback
//!     ```
//!
//! Key Types & Functions Index:
//!     - RestClient: Unified remote client
//!     - HttpServer: Loopback HTTP/1.1 server
//!     - RestService: Core route dispatcher
//!     - BlockingExecutor: Worker pool governor
//!     - JobRepository: Durable state machine interface
//!     - ProblemDetails: RFC 7807 error model

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod client;
pub mod executor;
pub mod models;
pub mod problem;
pub mod routes;
pub mod server;
pub mod storage;

pub use client::RestClient;
pub use executor::{BlockingExecutor, ExecutionError, ExecutorConfig};
pub use models::{JobId, JobRecord, JobResponse, JobState, MergePayload, RenderPayload, SourcePayload, SplitPayload};
pub use problem::{InvalidParam, ProblemDetails};
pub use routes::{HttpRequest, HttpResponse, RestService};
pub use server::{HttpServer, ServerConfig};
pub use storage::{DurableFileJobRepository, InMemoryJobRepository, JobError, JobRepository};
