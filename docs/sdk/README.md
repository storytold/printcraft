# PdfCraft Document Processing SDK & REST Service Guide

The **PdfCraft Document Processing SDK** provides a high-performance, `#![forbid(unsafe_code)]`, memory-safe document processing engine. It serves as an open-source alternative to the proprietary Adobe PDF Library (PDFL) for in-process execution, and pdfRest / Adobe PDF Services for containerized on-premise REST deployments.

> [!TIP]
> For the complete function-by-function reference organized by domain with exhaustive parameters, return types, and code snippets, see the [**SDK API Reference Manual**](sdk-api-reference.md).

---

## 1. Architectural Model & Layer Boundaries

### 1.1 PdfCraft Layers and Supported Boundaries

PdfCraft enforces a strict 7-layer architecture. The SDK and REST service operate entirely at the headless application and network layers, decoupled from the desktop/web graphical interface:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│  Layer 7: Presentations & Remote Services                                   │
│  • pdfcraft-ui-egui (Desktop/Web UI — NOT exposed to SDK consumers)         │
│  • pdfcraft-rest (HTTP REST Daemon, Job Repository, Problem Details)        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
┌──────────────────────────────────────▼──────────────────────────────────────┐
│  Layer 6: Application Façades & Orchestration                               │
│  • pdfcraft-sdk (Unified Client Contracts: LocalClient, RestClient)         │
│  • pdfcraft-automation (Headless Tool Registry, CLI, MCP Server)           │
│  • pdfcraft-engine (Session Orchestration, Undo Stack, CoW Revisions)       │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
┌──────────────────────────────────────▼──────────────────────────────────────┐
│  Layers 1–5: Domain Core & Low-Level Codecs                                 │
│  • L4–L5: pdfcraft-organize, pdfcraft-render, pdfcraft-annot, forms, sign   │
│  • L1–L3: pdfcraft-cos (Object Graph), crypt (AES/RC4), filters, fonts      │
└─────────────────────────────────────────────────────────────────────────────┘
```

| Layer | Component | Responsibility | Public to SDK Consumers? |
|---|---|---|---|
| **L7 (UI)** | `pdfcraft-ui-egui` | egui desktop/web presentation | **No** (Strictly isolated) |
| **L7 (Net)** | `pdfcraft-rest` | HTTP server, Bearer auth, job journal | REST clients only |
| **L6 (SDK)** | `pdfcraft-sdk` | Transport drivers (`LocalClient`, `RestClient`) | **Yes** (Primary API) |
| **L6 (Auto)**| `pdfcraft-automation`| Canonical headless tool catalog | Via CLI & MCP protocol |
| **L6 (Core)**| `pdfcraft-engine` | Document session & undo management | Via application adapters |
| **L1–L5** | `cos`, `render`, etc. | Pure PDF parsing, rendering, manipulation | Internal engine dependencies |

### 1.2 Dual-Transport Execution Model

PdfCraft exposes the **same operation vocabulary** across two transports. Operation semantics are shared, while result delivery, lifecycle, errors, authentication, and cancellation are transport-specific:

```text
                        ┌──────────────────────────────┐
                        │ Unified Operation Vocabulary │
                        │ merge, split, render_page    │
                        └──────────────┬───────────────┘
                                       │
                ┌──────────────────────┴──────────────────────┐
                │                                             │
 ┌──────────────▼──────────────┐               ┌──────────────▼──────────────┐
 │     In-Process Driver       │               │     Remote REST Driver      │
 │       (LocalClient)         │               │        (RestClient)         │
 ├─────────────────────────────┤               ├─────────────────────────────┤
 │ • Zero network latency      │               │ • HTTP/1.1 Loopback or LAN  │
 │ • In-memory byte buffers    │               │ • Async Job Lifecycle       │
 │ • Native Rust / PyO3 / N-API│               │ • Bearer Token Auth         │
 │ • Ephemeral sandbox drop    │               │ • Bounded Worker Semaphore  │
 └──────────────┬──────────────┘               └──────────────┬──────────────┘
                │                                             │
                ▼                                             ▼
 ┌─────────────────────────────┐               ┌─────────────────────────────┐
 │  AutomationEngineAdapter    │               │     pdfcraft-rest Server    │
 └──────────────┬──────────────┘               ├─────────────────────────────┤
                │                              │ • Route Dispatcher          │
                │                              │ • BlockingExecutor (Pool)   │
                │                              │ • JobRepository (Journal)   │
                │                              └──────────────┬──────────────┘
                │                                             │
                └──────────────────────►◄─────────────────────┘
                                        │
                                        ▼
                        ┌──────────────────────────────┐
                        │ pdfcraft-engine / automation │
                        └──────────────────────────────┘
```

1. **In-Process Mode (`LocalClient`)**:
   - Executes directly within the host process via Rust, Python (PyO3 native extension), or TypeScript (Node.js N-API native addon).
   - Zero daemon lifecycle, zero port binding, zero network serialization.
   - Ideal for CLI tools, desktop plugins, and high-throughput serverless compute pipelines.

2. **On-Premise REST Mode (`RestClient`)**:
   - Connects over HTTP/HTTPS to an on-premise `pdfcraft-rest` daemon.
   - Bounded concurrency with worker semaphores, crash-resilient disk journaling, and RFC 7807 problem details.
   - Ideal for microservice fleets, web applications, and multi-service environments.

---

## 2. Divisions of Functions & Resource Limits

### 2.1 Document Merge (`merge`)
- **Capability**: Combines multiple independent PDF documents into a single document.
- **Input Sources**: Path references (`PathBuf`, file path strings) or in-memory byte buffers (`Vec<u8>`, `bytes`, `Buffer`).
- **Options (`MergeOptions`)**:
  - `output_filename`: Optional virtual filename for output metadata.
  - `pages`: Specific page range selectors (e.g., `["1-3", "5"]`).

### 2.2 Document Split & Extraction (`split`)
- **Capability**: Partitions a source document into multiple PDF documents.
- **Split Modes (`SplitMode`)**:
  - `EveryNPages(n)`: Splits into consecutive chunks of $N$ pages each.
  - `BeforePages(vec![p1, p2])`: Inserts split boundaries before designated page numbers.
  - `AtBookmarks(level)`: Splits at outline bookmarks of the specified level.

### 2.3 High-DPI Page Rendering (`render_page`)
- **Capability**: Renders a designated page to a raster image at an exact physical target resolution.
- **DPI Calculation**: Decoupled from monitor or OS PPI (#739); dimensions are computed strictly via:
  $$\text{pixels} = \frac{\text{points} \times \text{dpi}}{72.0}$$
- **Options (`RenderOptions`)**:
  - `dpi`: Target resolution (default `150.0`, screen `72.0`, print `300.0`, archival `600.0`).
  - `image_format`: Output format (`png` or `jpeg`).

### 2.4 Resource Limits & Safe Spilling Contract

PdfCraft guarantees resource containment through explicit quotas:

| Dimension | Default Value | Configurable? | Enforcement Behavior |
|---|---|---|---|
| **Memory Spill Threshold** | 10 MB | Yes (`max_memory_bytes`) | Outputs $\le$ 10 MB stay in RAM; outputs $>$ 10 MB automatically spill to disk. |
| **Max Input Payload** | 128 MB | Yes (`max_input_bytes`) | Fast rejection with `QUOTA_EXCEEDED` before parsing. |
| **Max Render Resolution** | 600.0 DPI | Yes (`max_render_dpi`) | Rejection with `INVALID_ARGUMENT` if exceeded. |
| **Max Worker Concurrency** | CPU core count | Yes (`max_workers`) | Bounded semaphore queue; rejects with `WORKER_POOL_SATURATED` (429) if full. |
| **Panic Containment** | Enabled | Permanent | `catch_unwind` turns hostile engine panics into actionable `ENGINE_PANIC_CONTAINED` errors. |

### 2.5 Unified Error Taxonomy & Transport Mapping

The application defines stable, transport-neutral error categories. The REST transport serializes them as RFC 7807 Problem Details; local clients expose them as language-native typed exceptions carrying the same error code:

| Error Code | HTTP Status | Rust SDK | Python SDK | TypeScript SDK |
|---|---|---|---|---|
| `INVALID_ARGUMENT` | 400 Bad Request | `SdkError::InvalidArgument` | `InvalidArgumentError` | `InvalidArgumentError` |
| `UNAUTHORIZED` | 401 Unauthorized | `SdkError::Unauthorized` | `UnauthorizedError` | `UnauthorizedError` |
| `NOT_FOUND` | 404 Not Found | `SdkError::NotFound` | `NotFoundError` | `NotFoundError` |
| `CONFLICT` | 409 Conflict | `SdkError::Conflict` | `ConflictError` | `ConflictError` |
| `QUOTA_EXCEEDED` | 413 Payload Too Large | `SdkError::QuotaExceeded` | `QuotaExceededError` | `QuotaExceededError` |
| `WORKER_POOL_SATURATED`| 429 Too Many Requests | `SdkError::PoolSaturated` | `PoolSaturatedError` | `PoolSaturatedError` |
| `ENGINE_ERROR` | 500 Internal Error | `SdkError::EngineError` | `PdfCraftError` | `PdfCraftError` |
| `ENGINE_PANIC_CONTAINED`| 500 Internal Error | `SdkError::EnginePanic` | `PdfCraftError` | `PdfCraftError` |

---

## 3. Language Guide: Rust Developers

### Crate Dependencies (`Cargo.toml`)
```toml
[dependencies]
pdfcraft-sdk = { version = "0.5.0", path = "crates/sdk" }
# Optional: for self-hosting the REST daemon or connecting remotely
pdfcraft-rest = { version = "0.5.0", path = "crates/rest" }
```

### 3.1 In-Process Execution (`LocalClient`)
```rust
use pdfcraft_sdk::prelude::*;

fn main() -> Result<(), SdkError> {
    let client = LocalClient::builder()
        .max_memory_bytes(64 * 1024 * 1024) // 64 MB before disk spill
        .build()?;

    // 1. Merge documents from disk or memory buffers
    let doc1 = DocumentSource::from_path("cover.pdf");
    let doc2 = DocumentSource::from_bytes(b"%PDF-1.4 sample content".to_vec(), None);
    let merged = client.merge(&[doc1, doc2], MergeOptions::default())?;
    
    println!("Merged: {} bytes (MIME: {})", merged.byte_len(), merged.mime_type());
    merged.save_to_path("final.pdf")?;

    // 2. High-DPI page rendering
    let render = client.render_page(
        &DocumentSource::from_path("final.pdf"),
        1, // Page 1 (1-based)
        RenderOptions { dpi: 300.0, ..Default::default() },
    )?;
    println!("Rendered 300 DPI page image: {} bytes", render.byte_len());

    Ok(())
}
```

### 3.2 Production REST Daemon Deployment
```rust
use std::sync::Arc;
use pdfcraft_rest::server::{RestServer, ServerConfig};
use pdfcraft_rest::storage::FileJobRepository;
use pdfcraft_sdk::local::LocalClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Arc::new(LocalClient::default());
    
    // Durable disk journal with crash recovery
    let repo = Arc::new(FileJobRepository::new("./data/job-journal")?);

    let config = ServerConfig {
        bind_addr: "127.0.0.1:8080".parse()?,
        bearer_token: Some("production-secret-token".into()),
        max_concurrent_workers: 8,
        storage_dir: "./data/job-storage".into(),
        job_retention_secs: 86400, // Retain completed jobs for 24 hours
    };

    let server = RestServer::with_repository(config, engine, repo)?;
    println!("PdfCraft REST Daemon listening on http://127.0.0.1:8080");
    server.run().await?;
    Ok(())
}
```

---

## 4. Language Guide: Python Developers

### Package Installation
```bash
# Production install
pip install pdfcraft

# Or local development checkout
pip install ./sdks/python
```

### 4.1 In-Process Execution (`LocalClient`)
```python
from pathlib import Path
from pdfcraft.local import LocalClient
from pdfcraft.types import DocumentSource, MergeOptions, RenderOptions

client = LocalClient(max_memory_bytes=64 * 1024 * 1024)

# 1. Merge documents
merged = client.merge([
    DocumentSource.from_path("section1.pdf"),
    DocumentSource.from_path("section2.pdf"),
], options=MergeOptions(output_filename="complete.pdf"))
merged.save_to_path("complete.pdf")

# 2. Split document
split = client.split("complete.pdf", every_n_pages=2)
print(f"Split generated {len(split.files)} files")

# 3. High-DPI Page Rendering
preview = client.render_page("complete.pdf", page_number=1, options=RenderOptions(dpi=300.0))
with open("page1_300dpi.png", "wb") as f:
    f.write(preview.png_bytes)
```

### 4.2 Remote REST Client (`RestClient`)
```python
from pdfcraft.rest import RestClient
from pdfcraft.errors import InvalidArgumentError

client = RestClient(base_url="http://127.0.0.1:8080", auth_token="production-secret-token")

# 1. Synchronous Merge
try:
    result = client.merge(["docA.pdf", "docB.pdf"])
    result.save_to_path("merged.pdf")
except InvalidArgumentError as e:
    print(f"Validation failed: {e}")

# 2. Asynchronous Job with Polling
job = client.merge_async(["large_doc1.pdf", "large_doc2.pdf"])
result = job.wait_for_completion(timeout_secs=60)
result.save_to_path("async_merged.pdf")
```

---

## 5. Language Guide: TypeScript / Node.js Developers

### Package Installation
```bash
# Production install
npm install @pdfcraft/sdk

# Or local development checkout
npm install ./sdks/typescript
```

### 5.1 In-Process Execution (`PdfCraftLocalClient`)
```typescript
import { PdfCraftLocalClient, DocumentSource } from "@pdfcraft/sdk";

const client = new PdfCraftLocalClient({ maxMemoryBytes: 64 * 1024 * 1024 });

async function run() {
  // 1. Merge files
  const merged = await client.mergeDocuments([
    DocumentSource.fromPath("chapter1.pdf"),
    DocumentSource.fromPath("chapter2.pdf"),
  ]);
  await merged.saveToPath("book.pdf");

  // 2. High-DPI page preview
  const preview = await client.renderPagePreview(
    DocumentSource.fromPath("book.pdf"),
    1,   // Page 1
    1024 // Max side length (px)
  );
  await preview.saveToPath("preview.png");
}

run().catch(console.error);
```

### 5.2 Remote REST Client (`PdfCraftRestClient`)
```typescript
import { PdfCraftRestClient, DocumentSource, InvalidArgumentError } from "@pdfcraft/sdk";

const client = new PdfCraftRestClient({
  baseUrl: "http://127.0.0.1:8080",
  authToken: "production-secret-token",
});

async function run() {
  try {
    // 1. Synchronous execution
    const merged = await client.mergeDocuments([
      DocumentSource.fromPath("doc1.pdf"),
      DocumentSource.fromPath("doc2.pdf"),
    ]);
    await merged.saveToPath("out.pdf");

    // 2. Asynchronous background execution
    const job = await client.submitMergeJob([
      DocumentSource.fromPath("large1.pdf"),
      DocumentSource.fromPath("large2.pdf"),
    ]);
    const result = await job.waitForCompletion(60000);
    await result.saveToPath("async_out.pdf");
  } catch (err) {
    if (err instanceof InvalidArgumentError) {
      console.error("Validation rejected:", err.message);
    }
  }
}

run().catch(console.error);
```

---

## 6. Where Documents Belong in the Repository

| Document Type | Location | Tracked in Git? | Description |
|---|---|---|---|
| **Public User Guide** | [`docs/sdk/README.md`](README.md) | **Yes** | Canonical developer documentation across Rust, Python, and TypeScript. |
| **API Reference Manual**| [`docs/sdk/api-reference.md`](api-reference.md) | **Yes** | Exhaustive domain-by-domain function reference with full signatures and examples. |
| **Runnable Examples** | [`docs/sdk/examples/`](examples/) | **Yes** | Standalone executable CLI examples for all 5 lifecycle stages. |
| **Package READMEs** | `crates/sdk/README.md`<br>`crates/rest/README.md`<br>`sdks/python/README.md`<br>`sdks/typescript/README.md` | **Yes** | Quickstarts for crates.io, PyPI, and npm registry packages. |
| **API Specification** | [`sdks/openapi.json`](../sdks/openapi.json) | **Yes** | Canonical OpenAPI 3.1.0 schema for the REST service. |
| **Architecture Specification** | `docs/plans/pdfcraft-rest-sdk-plan.md` | **No** (Gitignored) | Comprehensive technical architecture and design document. |
| **Implementation Tasks** | `docs/plans/tasks.md` | **No** (Gitignored) | Engineering task breakdown across Phases 1–5. |
| **Local Test Fixtures** | `/sample-docs/` | **No** (Gitignored) | Local test files (never committed or published per `AGENTS.md` §1 asset policy). |
| **Local Test Scripts** | `/scripts/` | **No** (Gitignored) | Local test harnesses for offline fixture verification. |
