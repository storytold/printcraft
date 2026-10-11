# pdfcraft-rest

On-premise HTTP REST service and client library for PdfCraft document processing operations, offering an open-source alternative to pdfRest containers and Adobe PDF Services.

- **Layer:** Network Service & Remote Client (Layer 7).
- **Core Abstractions:** `RestServer`, `RestClient`, `BlockingExecutor`, `JobRepository`.
- **API Spec:** OpenAPI 3.1.0 compliant (`sdks/openapi.json`), RFC 7807 Problem Details for errors, async job lifecycle (`/v1/jobs/{id}`) with Bearer auth token support.

## Running the Server

```rust
use pdfcraft_rest::server::{RestServer, ServerConfig};
use pdfcraft_sdk::local::LocalClient;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(LocalClient::default());
    let config = ServerConfig {
        bind_addr: "127.0.0.1:8080".parse()?,
        bearer_token: Some("secret-token".to_string()),
        max_concurrent_workers: 8,
        storage_dir: "./job-storage".into(),
        job_retention_secs: 3600,
    };

    let server = RestServer::new(config, client)?;
    server.run().await?;
    Ok(())
}
```

## Using the Rust REST Client

```rust
use pdfcraft_rest::client::RestClient;
use pdfcraft_sdk::ports::PdfCraftClient;
use pdfcraft_sdk::types::DocumentSource;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = RestClient::new("http://127.0.0.1:8080", Some("secret-token"));

    let merged = client.merge_documents(&[
        DocumentSource::from_path("doc1.pdf"),
        DocumentSource::from_path("doc2.pdf"),
    ])?;
    merged.save_to_path("out.pdf")?;

    Ok(())
}
```

## Documentation

For full REST route specifications, schema payloads, and multi-language guides, refer to the canonical [SDK Guide](../../docs/sdk/README.md).
