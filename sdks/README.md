# PdfCraft Multi-Language SDKs

This directory contains client libraries and API specifications for integrating PdfCraft into enterprise services, automated workflows, and application code.

## Language Packages

| Package | Language | Runtime / Bindings | Directory |
|---|---|---|---|
| `pdfcraft-sdk` | Rust | In-process native crate | [`crates/sdk`](../crates/sdk) |
| `pdfcraft-rest` | Rust | HTTP REST daemon & client | [`crates/rest`](../crates/rest) |
| `pdfcraft` | Python | PyO3 native extension + HTTP REST | [`sdks/python`](python) |
| `@pdfcraft/sdk` | TypeScript / Node.js | N-API native addon + typed HTTP REST | [`sdks/typescript`](typescript) |

## API Specifications & CI Verification

- **OpenAPI 3.1 Specification:** [`sdks/openapi.json`](openapi.json) documents all HTTP REST routes (`/v1/merged-pdf`, `/v1/split-pdf`, `/v1/page-preview`, `/v1/jobs/{id}`, `/v1/jobs/{id}/result`).
- **Contract Drift Checker:** Run `python3 sdks/check_contract_drift.py` in CI to ensure 100% alignment between OpenAPI schemas, Rust models, Python dataclasses, and TypeScript types.

## Comprehensive Guides

- [**PdfCraft SDK & REST Service Guide**](../docs/sdk/README.md): Full architecture details, enterprise deployment guidelines, and multi-language tutorials.
- [**SDK API Reference Manual**](../docs/sdk/api-reference.md): Complete domain-by-domain function reference with parameter tables, return types, and code examples for Rust, Python, and TypeScript.
- [**Runnable CLI Code Examples**](../docs/sdk/examples/): Standalone executable examples for the full document lifecycle.
