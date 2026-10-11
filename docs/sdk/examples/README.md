# PdfCraft SDK examples

Runnable examples for each SDK. Every folder has its own README with prerequisites and
exact commands. All commands are run **from the repository root** unless stated otherwise.

| Folder | Language | Headline example | Needs |
|---|---|---|---|
| [`python/`](python/README.md) | Python 3.9+ | `sdk_capability_tour.py` | `pip install pillow pypdf`; `pdftoppm` for previews |
| [`typescript/`](typescript/README.md) | TypeScript (Node 22.18+) | `sdk_capability_tour.ts` | `npm install` + `npm run build` in `sdks/typescript`; `pdftoppm` for previews |
| [`rust/`](rust/README.md) | Rust 1.90+ | `sdk_capability_tour.rs` | `rustup` toolchain; nothing else |

Each `sdk_capability_tour` builds a 5-page showcase PDF into `sample-docs/outputs/`
(gitignored: generated PDFs and PNG previews are never committed).

Install the one external tool the Python and TypeScript previews use:

```bash
brew install poppler        # macOS; provides pdftoppm
# Debian/Ubuntu: sudo apt install poppler-utils
```
