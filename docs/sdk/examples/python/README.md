# Python SDK examples

Run everything **from the repository root**.

## Prerequisites

| Need | How |
|---|---|
| Python 3.9+ | `python3 --version` |
| `pillow`, `pypdf` | `python3 -m pip install pillow pypdf` |
| `pdftoppm` (only for `--render-previews`) | `brew install poppler` (macOS) or `sudo apt install poppler-utils` |

The scripts add `sdks/python` to `sys.path` themselves, so you do **not** need to install the
`pdfcraft` package or build the native (PyO3) extension. The `DocumentSession` used here is pure
Python. The native extension is only needed for `LocalClient.merge/split/render_page`.

## Examples

| Script | What it makes |
|---|---|
| `sdk_capability_tour.py` | 5-page showcase: cover art, typography, charts, find/highlight/stamps/forms, landscape timeline |
| `e2e_showcase_example.py` | 5-page README specimen (cover, multilingual type, columns, forms, review markup) |
| `e2e_document_lifecycle.py` | Create, edit, search, image add/remove, save and render a document (`--output my.pdf --preview my.png`) |
| `ocr_document_to_markdown.py` | OCR a document to Markdown. Also needs `tesseract` and `pdftotext` (`brew install tesseract poppler`) |

```bash
python3 docs/sdk/examples/python/sdk_capability_tour.py
python3 docs/sdk/examples/python/sdk_capability_tour.py --render-previews
open sample-docs/outputs/sdk_capability_tour.pdf          # macOS
```

Options: `--output PATH` to choose the PDF path; `--render-previews` writes PNGs to
`previews/` next to the PDF.

## Notes

- Text uses the built-in PDF fonts, so keep strings **ASCII**; other characters render as garbage.
- `ModuleNotFoundError: PIL` or `pypdf`: run the `pip install` above with the same `python3`
  that runs the script.
- Output goes to `sample-docs/outputs/` (gitignored).
