# @pdfcraft/sdk (TypeScript / Node.js SDK)

Official TypeScript & Node.js SDK for PdfCraft document processing operations, offering both native in-process execution via N-API and typed REST client access.

## Installation

```bash
npm install @pdfcraft/sdk
```

## Quickstart

### In-Process Local Execution

```typescript
import { PdfCraftLocalClient, DocumentSource } from "@pdfcraft/sdk";

const client = new PdfCraftLocalClient({ maxMemoryBytes: 64 * 1024 * 1024 });

// Merge PDF files
const merged = await client.mergeDocuments([
  DocumentSource.fromPath("report_part1.pdf"),
  DocumentSource.fromPath("report_part2.pdf"),
]);
await merged.saveToPath("combined_report.pdf");

// Render Page Preview
const preview = await client.renderPagePreview(
  DocumentSource.fromPath("combined_report.pdf"),
  1,
  1024
);
await preview.saveToPath("preview.png");
```

### Remote REST Execution

```typescript
import { PdfCraftRestClient, DocumentSource } from "@pdfcraft/sdk";

const client = new PdfCraftRestClient({
  baseUrl: "http://127.0.0.1:8080",
  authToken: "secret-token",
});

const merged = await client.mergeDocuments([
  DocumentSource.fromPath("file1.pdf"),
  DocumentSource.fromPath("file2.pdf"),
]);
await merged.saveToPath("output.pdf");
```

## Documentation

For full documentation and API references, see the canonical [SDK Guide](../../docs/sdk/README.md).
