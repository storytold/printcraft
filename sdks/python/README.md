# pdfcraft (Python SDK)

Official Python bindings for PdfCraft document processing operations, powered by high-performance Rust native extensions (PyO3).

## Installation

```bash
pip install pdfcraft
```

## Quickstart

### In-Process Local Processing

```python
from pdfcraft.local import LocalClient
from pdfcraft.types import DocumentSource

client = LocalClient(max_memory_bytes=64 * 1024 * 1024)

# Merge PDFs
merged = client.merge_documents([
    DocumentSource.from_path("section1.pdf"),
    DocumentSource.from_path("section2.pdf"),
])
merged.save_to_path("final.pdf")

# High-DPI Page Preview
preview = client.render_page_preview(
    DocumentSource.from_path("final.pdf"),
    page_number=1,
    max_side=1024,
)
with open("preview.png", "wb") as f:
    f.write(preview.png_bytes)
```

### Remote REST Service

```python
from pdfcraft.rest import RestClient
from pdfcraft.types import DocumentSource

client = RestClient(base_url="http://127.0.0.1:8080", auth_token="secret-token")

job = client.merge_documents_async([
    DocumentSource.from_path("docA.pdf"),
    DocumentSource.from_path("docB.pdf"),
])
result = job.wait_for_completion(timeout_secs=60)
result.save_to_path("merged.pdf")
```

## Documentation

For full documentation and API references, see the canonical [SDK Guide](../../docs/sdk/README.md).
