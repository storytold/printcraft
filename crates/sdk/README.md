# pdfcraft-sdk

Rust in-process client library for PDF document processing operations, providing an open-source, `#![forbid(unsafe_code)]` alternative to Adobe PDF Library (PDFL).

- **Layer:** Headless Client / SDK (Layer 6).
- **Core Abstractions:** `PdfCraftClient` trait, `LocalClient` facade, `DocumentSource`, `DocumentResult`.
- **Safety & Containment:** `#![forbid(unsafe_code)]`, strict resource limits, automatic disk spillover for large files (> 10 MB), and `std::panic::catch_unwind` isolation against hostile inputs.

## Quickstart

Add `pdfcraft-sdk` to your `Cargo.toml`:

```toml
[dependencies]
pdfcraft-sdk = { path = "../sdk" } # or version = "0.5.0"
```

```rust
use pdfcraft_sdk::prelude::*;

fn main() -> Result<(), SdkError> {
    let client = LocalClient::builder()
        .max_memory_bytes(64 * 1024 * 1024)
        .build()?;

    // Merge documents
    let merged = client.merge_documents(&[
        DocumentSource::from_path("cover.pdf"),
        DocumentSource::from_path("body.pdf"),
    ])?;
    merged.save_to_path("combined.pdf")?;

    // Render high-DPI page preview
    let preview = client.render_page_preview(
        DocumentSource::from_path("combined.pdf"),
        1,   // Page 1
        1024 // Max side length (px)
    )?;
    std::fs::write("preview.png", preview.png_bytes())?;

    Ok(())
}
```

## Documentation

For full architecture details, enterprise deployment guidelines, and multi-language guides, refer to the canonical [SDK Guide](../../docs/sdk/README.md).
