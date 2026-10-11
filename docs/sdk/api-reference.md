# PdfCraft SDK API Reference Manual

This manual provides the canonical, domain-by-domain API reference for the **PdfCraft Document Processing SDK** and **REST Service**. It implements the standard industry domain architecture (matching Datalogics Adobe PDF Library APDFL 21) as an open-source, `#![forbid(unsafe_code)]` pure Rust foundation with native bindings for **Rust**, **Python**, and **TypeScript**.

---

## Table of Contents

- [Architectural Conventions & Cross-Language Casing](#architectural-conventions--cross-language-casing)
- [Domain 1: Core Library & Client Facade](#domain-1-core-library--client-facade)
- [Domain 2: Document Layer](#domain-2-document-layer)
- [Domain 3: Content Elements & Vector Geometry](#domain-3-content-elements--vector-geometry)
- [Domain 4: Graphics State & Color Management](#domain-4-graphics-state--color-management)
- [Domain 5: Text & Fonts](#domain-5-text--fonts)
- [Domain 6: Image Processing & Rasterization](#domain-6-image-processing--rasterization)
- [Domain 7: Annotations & Actions](#domain-7-annotations--actions)
- [Domain 8: Low-Level PDF Objects (COS Layer)](#domain-8-low-level-pdf-objects-cos-layer)
- [Domain 9: Optional Content (Layers / OCG)](#domain-9-optional-content-layers--ocg)
- [Domain 10: Forms & Data Exchange (AcroForms)](#domain-10-forms--data-exchange-acroforms)
- [Domain 11: PDF Optimization & Compression](#domain-11-pdf-optimization--compression)
- [Domain 12: Digital Signatures & Security](#domain-12-digital-signatures--security)
- [Domain 13: Metadata & File Attachments](#domain-13-metadata--file-attachments)
- [Domain 14: High-Level Pipeline Operations (`merge`, `split`, `render_page`)](#domain-14-high-level-pipeline-operations)
- [Domain 15: Error Taxonomy & RFC 7807 Mapping](#domain-15-error-taxonomy--rfc-7807-mapping)
- [Runnable Code Examples & CLI Demonstration](#runnable-code-examples--cli-demonstration)
- [Documentation Architecture: Domain Hub vs Monolith](#documentation-architecture-domain-hub-vs-monolith)

---

## Architectural Conventions & Cross-Language Casing

PdfCraft maintains a unified operation vocabulary across transports and languages. The table below defines the mechanical mapping conventions applied across SDK bindings:

| Logical Concept | Rust (`pdfcraft-sdk`) | Python (`pdfcraft`) | TypeScript (`@pdfcraft/sdk`) | REST OpenAPI Route / Field |
|---|---|---|---|---|
| Merge Operation | `client.merge(...)` | `client.merge(...)` | `client.mergeDocuments(...)` | `POST /v1/merged-pdf` |
| Split Operation | `client.split(...)` | `client.split(...)` | `client.splitDocument(...)` | `POST /v1/split-pdf` |
| Render Operation| `client.render_page(...)` | `client.render_page(...)` | `client.renderPagePreview(...)` | `POST /v1/page-preview` |
| GoTo Action | `Action::GoTo(GoToAction)` | `GoToAction(...)` | `new GoToAction(...)` | `{"type": "GoTo", ...}` |
| Highlight Annot | `HighlightAnnotation` | `HighlightAnnotation` | `HighlightAnnotation` | `{"subtype": "Highlight", ...}` |
| Output Filename | `options.output_filename` | `options.output_filename` | `options.outputFilename` | `output_filename` |
| Target DPI | `options.dpi` | `options.dpi` | `options.dpi` | `dpi` |
| Image Format | `options.image_format` | `options.image_format` | `options.imageFormat` | `image_format` |
| Page Number | `page: usize` (1-based) | `page_number: int` (1-based) | `page: number` (1-based) | `page` |

All paths and endpoints are strictly environment-independent. Code examples use environment variables or explicit relative paths (`$PDFCRAFT_BASE_URL`, `$PDFCRAFT_AUTH_TOKEN`) rather than hardcoded credentials or machine paths.

---

## Domain 1: Core Library & Client Facade

### `LocalClient`
* **Category:** Core Client Facade
* **Detailed Description:** In-process client for running document transformations without an HTTP server. Backed by `AutomationEngineAdapter`, it provides thread-safe (`Send + Sync`) execution, automatic disk spillover for outputs exceeding memory limits, and complete panic isolation.
* **Uses types:** `DocumentSource`, `DocumentResult`, `ResourceLimits`, `MergeOptions`, `SplitMode`, `RenderOptions`, `SdkError`

#### Methods
- `new(executor: Arc<dyn OperationExecutor>) -> LocalClient`: Creates a client with an injected executor.
- `default() -> LocalClient`: Creates a client backed by the production automation engine.
- `with_limits(limits: ResourceLimits) -> LocalClient`: Configures request and memory thresholds.
- `merge(sources: &[DocumentSource], options: MergeOptions) -> Result<DocumentResult, SdkError>`
- `split(source: &DocumentSource, mode: SplitMode) -> Result<SplitResult, SdkError>`
- `render_page(source: &DocumentSource, page: usize, options: RenderOptions) -> Result<DocumentResult, SdkError>`

---

## Domain 2: Document Layer

### `Document`
* **Category:** Document Layer
* **Detailed Description:** Represents a PDF document, providing access to its page tree, outline bookmarks, encryption state, and global catalog attributes.
* **Uses types:** `Page`, `Bookmark`, `PageRange`, `PageLabel`

#### Properties
- `num_pages: usize`: Total number of pages in the document.
- `version: String`: PDF header specification version (e.g. `"1.7"`, `"2.0"`).
- `is_encrypted: bool`: Indicates whether standard security encryption is active.
- `linearized: bool`: Indicates whether the file is organized for Fast Web View.
- `bookmarks: Vec<Bookmark>`: Hierarchical document outline bookmarks.

### `Page`
* **Category:** Document Layer
* **Detailed Description:** Represents an individual page within a document, exposing media and crop box dimensions and orientation.
* **Uses types:** `Rect`, `Point`

#### Properties
- `page_number: usize`: 1-based page index.
- `media_box: Rect`: Physical medium boundaries.
- `crop_box: Rect`: Visible page region displayed or printed.
- `rotation_degrees: u16`: Visual rotation (0, 90, 180, 270 degrees).

### `Bookmark`
* **Category:** Document Layer
* **Detailed Description:** An outline item in the document's navigational hierarchy. A bookmark contains a display title and may be linked to a `ViewDestination` or an `Action`.
* **Uses types:** `ViewDestination`, `Action`

#### Properties
- `title: String`: Display text of the bookmark.
- `destination: Option<ViewDestination>`: Target view destination jumped to upon click.
- `children: Vec<Bookmark>`: Nested sub-bookmarks.
- `is_open: bool`: Initial expansion state in document viewer outlines.

### `ViewDestination`
* **Category:** Document Layer
* **Detailed Description:** Defines a specific view location, target page, and zoom factor within a document.
* **Uses types:** `FitMode`, `Rect`

#### Properties
- `page_number: usize`: Target page index (1-based).
- `fit_mode: FitMode`: Target zoom fit rule (`XYZ`, `Fit`, `FitH`, `FitV`, `FitR`, `FitB`, `FitBH`, `FitBV`).
- `zoom: Option<f64>`: Magnification factor (e.g. `1.0` for 100%, `1.5` for 150%).
- `rect: Option<Rect>`: Optional target rectangle for box-fitting modes.

---

## Domain 3: Content Elements & Vector Geometry

### `Path`
* **Category:** Content Elements
* **Detailed Description:** Vector graphics path composed of straight lines, cubic Bézier curves, and rectangles. Supports both stroke and fill painting operators.
* **Uses types:** `Segment`, `Point`, `Rect`

#### Methods
- `move_to(p: Point)`: Begins a new subpath at coordinate `p`.
- `line_to(p: Point)`: Appends a straight line segment from the current point to `p`.
- `curve_to(cp1: Point, cp2: Point, endpoint: Point)`: Appends a cubic Bézier curve.
- `close()`: Closes the current subpath with a straight segment to the start point.

### `Segment`
* **Category:** Content Elements
* **Detailed Description:** Individual operator token in a vector path:
  - `MoveTo(Point)`: Move to operator (`m`).
  - `LineTo(Point)`: Straight line operator (`l`).
  - `CurveTo { cp1, cp2, endpoint }`: Cubic Bézier curve (`c`).
  - `CurveToV { cp2, endpoint }`: Bézier curve with first control point coincident with start (`v`).
  - `CurveToY { cp1, endpoint }`: Bézier curve with second control point coincident with endpoint (`y`).
  - `ClosePath`: Close path operator (`h`).
  - `RectSegment(Rect)`: Rectangle operator (`re`).

### `Element`
* **Category:** Content Elements
* **Detailed Description:** Polymorphic base element in a content stream:
  - `Path(Path)`: Vector geometry.
  - `Clip(Clip)`: Clipping path boundary.
  - `Container(Container)`: Logical layer or element grouping.
  - `Group(Group)`: Named element group.
  - `Form(Form)`: Reusable Form XObject.

---

## Domain 4: Graphics State & Color Management

### `Point`, `Rect`, `Quad`
* **Category:** Graphics State & Geometry
* **Detailed Description:**
  - `Point`: 2D coordinate `(x, y)` in PDF user points (1/72 inch).
  - `Rect`: Bounding rectangle `[x, y, width, height]`.
  - `Quad`: Four vertices `(top_left, top_right, bottom_left, bottom_right)` for non-axis-aligned text highlighting.

### `Matrix`
* **Category:** Graphics State
* **Detailed Description:** 3x2 affine transformation matrix `[a, b, c, d, tx, ty]` for translation, scaling, skewing, and rotation.

### `Color` & `ColorSpace`
* **Category:** Graphics State & Color Management
* **Detailed Description:**
  - `Color`: `RGB(r, g, b)`, `CMYK(c, m, y, k)`, `Gray(g)`, or `Named { name, components }`.
  - `ColorSpace`: `DeviceRGB`, `DeviceCMYK`, `DeviceGray`, `CalGray`, `CalRGB`, `Lab`, `ICCBased`, `Separation`, `Indexed`.
  - `BlendMode`: Standard transparency blend modes (`Normal`, `Multiply`, `Screen`, `Overlay`, `Darken`, `Lighten`, `ColorDodge`, `ColorBurn`, `HardLight`, `SoftLight`, `Difference`, `Exclusion`).

---

## Domain 5: Text & Fonts

### `Font`
* **Category:** Text & Fonts
* **Detailed Description:** Font resource metadata including PostScript name, base font family, embedded status, and subset flags.

### `TextRun`
* **Category:** Text & Fonts
* **Detailed Description:** A contiguous slice of text characters sharing uniform font, font size, transformation matrix, and fill color.

### `Word` & `DocTextFinderMatch`
* **Category:** Text & Fonts
* **Detailed Description:**
  - `Word`: Extracted word token with spatial bounding box (`Rect`) and polygon vertices (`Vec<Quad>`).
  - `DocTextFinderMatch`: Hit result produced by querying text patterns across document pages.

---

## Domain 6: Image Processing & Rasterization

### `ImageFormat`
* **Category:** Image Processing
* **Detailed Description:** Supported raster output image formats:
  - `Png`: Lossless Portable Network Graphics (`image/png`).
  - `Jpeg`: Lossy JPEG image (`image/jpeg`).
  - `Webp`: Modern high-efficiency WebP image (`image/webp`).
  - `Tiff`: Tagged Image File Format (`image/tiff`).

### `DrawParams` & `PageImageParams`
* **Category:** Image Processing
* **Detailed Description:**
  - `dpi: f64`: Physical resolution for rendering (e.g. `150.0`, `300.0`).
  - `color_space: ColorSpace`: Target rendering color space.
  - `clip_box: Option<Rect>`: Sub-region crop rectangle in PDF points.
  - `smooth_text: bool` / `smooth_images: bool`: Antialiasing flags.

---

## Domain 7: Annotations & Actions

### `Action`
* **Category:** Annotations & Actions
* **Detailed Description:** Base polymorphic action triggered by document links, bookmarks, or page events.
  - `GoTo(GoToAction)`
  - `URI(URIAction)`
  - `Launch(LaunchAction)`
  - `RemoteGoTo(RemoteGoToAction)`

### `GoToAction`
* **Category:** Annotations & Actions
* **Inherits from:** `Action`
* **Detailed Description:** Jumps to a page and view inside the same document (PDF `/S /GoTo`). Typically attached to a bookmark or link annotation.
* **Uses types:** `Document`, `ViewDestination`
* **Properties:**
  - `destination: ViewDestination`: The target page and view zoom to display.

### `URIAction`
* **Category:** Annotations & Actions
* **Inherits from:** `Action`
* **Detailed Description:** An action causing a uniform resource identifier (web link) to be opened in the user's default browser.
* **Properties:**
  - `uri: String`: Web URL (must adhere to `http`, `https`, or `mailto` schemes).
  - `is_map: bool`: Server-side image map flag.

### `LaunchAction`
* **Category:** Annotations & Actions
* **Inherits from:** `Action`
* **Detailed Description:** Opens an external file or application (PDF `/S /Launch`).
* **Properties:**
  - `file_path: String`: Target executable or document path.
  - `new_window: bool`: Flag indicating whether to launch in a new window.

### `RemoteGoToAction`
* **Category:** Annotations & Actions
* **Inherits from:** `Action`
* **Detailed Description:** Jumps to a destination in a different PDF file (PDF `/S /GoToR`).
* **Uses types:** `ViewDestination`
* **Properties:**
  - `file_path: String`: Relative or absolute path to the destination PDF document.
  - `destination: ViewDestination`: Target view coordinates in the remote document.
  - `new_window: bool`: Open destination in a new window.

### `Annotation`
* **Category:** Annotations & Actions
* **Detailed Description:** Base annotation object appearing on a PDF page. Contains location geometry, author metadata, flags, and a subtype-specific payload.
* **Uses types:** `Rect`, `Color`, `AnnotationFlags`, `AnnotationSubtype`

#### Subtypes
- `HighlightAnnotation`: Quadrilateral quads covering highlighted text spans.
- `UnderlineAnnotation`: Quadrilateral quads for underlined text.
- `FreeTextAnnotation`: Embedded text box with custom font, size, and appearance stream.
- `LineAnnotation`: Straight line connecting two points, supporting arrow and diamond line endings (`LineEnding`).
- `CircleAnnotation` / `SquareAnnotation`: Geometric shapes with optional stroke and fill.
- `PolygonAnnotation` / `PolyLineAnnotation`: Multi-segment vector paths.
- `InkAnnotation`: Freehand pen drawing strokes (`Vec<Vec<Point>>`).
- `LinkAnnotation`: Clickable region associated with an `Action` or `ViewDestination`.
- `Redaction`: Legal redaction box with quads, fill color, and overlay text.

---

## Domain 8: Low-Level PDF Objects (COS Layer)

### `PDFObject`
* **Category:** Low-level PDF Objects
* **Detailed Description:** Direct strongly-typed AST representing the underlying COS syntax tree:
  - `Null`: Null object (`null`).
  - `Boolean(bool)`: Boolean literal (`true` / `false`).
  - `Integer(i64)`: 64-bit signed integer.
  - `Real(f64)`: IEEE-754 floating point number.
  - `String(String)`: Literal or hexadecimal string.
  - `Name(String)`: PDF name token (e.g. `/Type`, `/MediaBox`).
  - `Array(Vec<PDFObject>)`: Ordered heterogeneous list.
  - `Dict(BTreeMap<String, PDFObject>)`: Key-value dictionary.
  - `Stream { dict, data }`: Byte stream with metadata dictionary.
  - `Reference(PDFReference)`: Indirect object reference `obj_number gen_number R`.

### `NameTree` & `NumberTree`
* **Category:** Low-level PDF Objects
* **Detailed Description:** Hierarchical balanced B-tree structures used in PDF catalogs to map string keys (destinations, embedded files) or integer keys (page labels, structure elements) to indirect objects.

---

## Domain 9: Optional Content (Layers / OCG)

### `OptionalContentGroup` (OCG)
* **Category:** Optional Content (Layers)
* **Detailed Description:** Represents a visual layer within a document that can be dynamically toggled visible or invisible during viewing or printing passes.
* **Properties:**
  - `name: String`: Display label shown in the layers sidebar.
  - `is_visible: bool`: Initial visibility state.
  - `intent: Vec<String>`: Intended usage intents (`"View"`, `"Design"`).

### `OptionalContentContext`
* **Category:** Optional Content (Layers)
* **Detailed Description:** Runtime evaluation context holding active layer states. Used during rendering passes to evaluate visibility membership dictionaries (`OptionalContentMembershipDict`).

---

## Domain 10: Forms & Data Exchange (AcroForms)

### `Field`
* **Category:** Forms & Data Exchange
* **Detailed Description:** Interactive form field descriptor supporting reading, value mutation, validation, and visual flattening.
* **Uses types:** `FieldType`, `Rect`

#### Subtypes (`FieldType`)
- `TextField`: Single-line, multi-line, or password input field.
- `ButtonField`: Checkbox, radio button group, or push button.
- `ChoiceField`: Dropdown combo box or multi-select list box.
- `SignatureField`: Digital signature form field placeholder.

### `AcroFormExportType` & `AcroFormImportType`
* **Category:** Forms & Data Exchange
* **Detailed Description:** Data exchange format enumerations:
  - `FDF = 1`: Forms Data Format.
  - `XFDF = 2`: XML-based Forms Data Format.
  - `XML = 3`: Arbitrary XML form dataset.
  - `JSON = 4`: Modern JSON form data payload.

---

## Domain 11: PDF Optimization & Compression

### `PDFOptimizer`
* **Category:** PDF Optimization
* **Detailed Description:** High-level optimization engine that removes unused objects, recompresses streams, downsamples images, and linearizes documents for Fast Web View.
* **Uses types:** `OptimizationParams`, `FlattenTransparencyParams`, `CompressionType`

#### Properties (`OptimizationParams`)
- `compress_streams: bool`: Recompresses object and content streams via Flate.
- `remove_unreferenced_objects: bool`: Performs dead-object garbage collection and cross-reference table compacting.
- `downsample_images: bool`: Bicubic downsampling of high-resolution raster images to target DPI.
- `target_image_dpi: Option<f64>`: Target resolution cap (e.g. `150.0`).
- `linearize: bool`: Optimizes byte layout for Fast Web View (byte serving).
- `flatten_transparency: Option<FlattenTransparencyParams>`: Flattens transparent vector artwork.

---

## Domain 12: Digital Signatures & Security

### `SignDoc`
* **Category:** Digital Signatures & Security
* **Detailed Description:** Cryptographic parameters required to apply an ISO 32000-2 / PAdES-compliant digital signature.
* **Properties:**
  - `cert_p12_bytes: Vec<u8>`: PKCS#12 certificate and private key payload.
  - `password: Option<String>`: Passphrase for decrypting the PKCS#12 bundle.
  - `signer_name: Option<String>`: Common Name (CN) of signer.
  - `reason: Option<String>`: Signing justification.
  - `location: Option<String>`: Geographical location of signing.
  - `timestamp_server_url: Option<String>`: RFC 3161 Timestamp Authority (TSA) endpoint.

### `PermissionsFlags` & `EncryptionType`
* **Category:** Digital Signatures & Security
* **Detailed Description:**
  - `EncryptionType`: Standard encryption algorithms (`AES128`, `AES256`).
  - `PermissionsFlags`: Bitmask governing allowable user actions (`allow_print`, `allow_high_quality_print`, `allow_modify`, `allow_copy`, `allow_annotate`, `allow_fill_forms`, `allow_extract_access`, `allow_assemble`).

---

## Domain 13: Metadata & File Attachments

### `FileAttachment`
* **Category:** Metadata & Attachments
* **Detailed Description:** Embedded file payload stored inside a PDF portfolio or document catalog.
* **Properties:**
  - `file_name: String`: Filename including extension.
  - `description: Option<String>`: File description.
  - `mime_type: Option<String>`: MIME content type.
  - `size_bytes: u64`: File size in bytes.
  - `data: Vec<u8>`: Raw file contents.

### `WatermarkParams`
* **Category:** Metadata & Attachments
* **Detailed Description:** Configuration for applying decorative or security watermarks to pages.
* **Properties:**
  - `text: String`: Watermark label (e.g. `"CONFIDENTIAL"`).
  - `opacity: f64`: Alpha transparency level (`0.0` to `1.0`).
  - `rotation_degrees: f64`: Diagonal rotation angle (default `45.0`).
  - `font_size: f64`: Text font size in points.
  - `color: Color`: Watermark fill color.
  - `on_top: bool`: Render above or below existing page content.

---

## Domain 14: High-Level Pipeline Operations

### `merge`
Combines multiple documents into a single PDF. Automatically spills outputs $> 10$ MB to disk.

```rust
// Rust Example
use pdfcraft_sdk::prelude::*;

let client = LocalClient::default();
let sources = vec![
    DocumentSource::from_path("cover.pdf"),
    DocumentSource::from_path("content.pdf"),
];
let result = client.merge(&sources, MergeOptions::default())?;
result.save_to_path("combined.pdf")?;
```

```python
# Python Example
from pdfcraft.local import LocalClient
from pdfcraft.types import DocumentSource, MergeOptions

client = LocalClient()
sources = [DocumentSource.from_path("cover.pdf"), DocumentSource.from_path("content.pdf")]
result = client.merge(sources, MergeOptions(output_filename="combined.pdf"))
result.save_to_path("combined.pdf")
```

```typescript
// TypeScript Example
import { PdfCraftLocalClient } from "@pdfcraft/sdk";

const client = new PdfCraftLocalClient();
const result = await client.mergeDocuments(["cover.pdf", "content.pdf"]);
await result.saveToPath("combined.pdf");
```

---

### `split`
Partitions a PDF document into separate files based on page counts, split points, or bookmarks.

```rust
// Rust Example
let split_result = client.split(
    &DocumentSource::from_path("book.pdf"),
    SplitMode::EveryNPages(10),
)?;
println!("Generated {} split files", split_result.files.len());
```

---

### `render_page`
Renders a page to a subpixel-antialiased raster image at a specified physical DPI.

```rust
// Rust Example
let image_result = client.render_page(
    &DocumentSource::from_path("blueprint.pdf"),
    1, // 1-based page
    RenderOptions { dpi: 300.0, format: ImageFormat::Png },
)?;
image_result.save_to_path("page_1.png")?;
```

---

## Domain 15: Error Taxonomy & RFC 7807 Mapping

Every error emitted by the SDK is strongly typed and maps directly to RFC 7807 Problem Details HTTP status codes when served via the REST daemon:

| SDK Error Variant | HTTP Status | RFC 7807 `type` | Meaning & Recovery Rule |
|---|---|---|---|
| `InvalidArgument(msg)` | 400 Bad Request | `urn:pdfcraft:error:invalid-argument` | Missing parameter, empty file array, or page index 0. |
| `FileNotFound(msg)` | 404 Not Found | `urn:pdfcraft:error:file-not-found` | Specified file path does not exist on disk. |
| `Encrypted(msg)` | 422 Unprocessable | `urn:pdfcraft:error:encrypted` | PDF requires user or owner password for opening. |
| `CorruptedPdf(msg)` | 422 Unprocessable | `urn:pdfcraft:error:corrupted-pdf` | File lacks valid PDF headers or cross-reference structure. |
| `QuotaExceeded(msg)` | 413 Payload Too Large | `urn:pdfcraft:error:quota-exceeded` | Input file exceeds `max_request_bytes` threshold. |
| `Timeout(msg)` | 504 Gateway Timeout | `urn:pdfcraft:error:timeout` | Execution deadline exceeded on long-running task. |
| `EnginePanic(msg)` | 500 Internal Error | `urn:pdfcraft:error:engine-panic` | Contained engine panic. Process remains healthy. |

---

## Runnable Code Examples & CLI Demonstration

Executable reference scripts are stored under [`docs/sdk/examples/`](examples/):

### Python End-to-End Document Lifecycle CLI
* **Source Script:** [`docs/sdk/examples/python/e2e_document_lifecycle.py`](examples/python/e2e_document_lifecycle.py)
* **Description:** A complete, self-contained CLI script exercising all 5 stages of the document lifecycle:
  1. **Document Creation:** Initializes `DocumentSession.create()`
  2. **Page Operations:** Adds pages, rotates Page 2 by 90° (landscape), adjusts MediaBox dimensions, and deletes Page 3 with automatic index renumbering.
  3. **Typography & Styling:** Places text across standard fonts (`Helvetica-Bold`, `Helvetica`, `Times-Roman`, `Courier`), verifies `Style.to_string()` contract, and executes inline multi-style token transitions (`StyleTransition`).
  4. **Text Search:** Performs pattern search via bounded `DocTextFinder` and reports hit coordinates (`DocTextFinderMatch`).
  5. **Image Management:** Embeds synthetic raster images as PDF XObjects, locates them by ID, and removes temporary watermarks.
  6. **Serialization & Render:** Exports to ISO 32000-2 PDF and renders a 150 DPI preview PNG via `pdftoppm`.

**Run from the command line:**
```bash
# Basic run generating dist/example_lifecycle.pdf and preview PNG
python3 docs/sdk/examples/python/e2e_document_lifecycle.py

# Custom output paths and rendering options
python3 docs/sdk/examples/python/e2e_document_lifecycle.py \
  --output dist/custom_output.pdf \
  --preview dist/custom_preview.png \
  --dpi 200
```

### Python End-to-End OCR & Markdown Pipeline
* **Source Script:** [`docs/sdk/examples/python/ocr_document_to_markdown.py`](examples/python/ocr_document_to_markdown.py)
* **Description:** Ingests PDF documents, renders visual rasters at 150 DPI, extracts text layouts, runs Optical Character Recognition (Tesseract OCR), and produces structured Markdown files with frontmatter metadata.
* **Run from the command line:**
```bash
# Ingest and convert sample documents to Markdown in sample-docs/outputs/
python3 docs/sdk/examples/python/ocr_document_to_markdown.py

# Process custom document
python3 docs/sdk/examples/python/ocr_document_to_markdown.py \
  "sample-docs/Optional-Car-Wordings-BCAA policy info.pdf" \
  --output-dir sample-docs/outputs \
  --max-pages 10
```

### Python README-Level Showcase Specimen Generator
* **Source Script:** [`docs/sdk/examples/python/e2e_showcase_example.py`](examples/python/e2e_showcase_example.py)
* **Description:** Programmatically synthesizes the multi-page specimen document featuring all capabilities shown in the main `README.md` (Scripts of the World, vertical Japanese, two-column editorial flow with drop caps, interactive AcroForms, PKCS#7 signature blocks, APPROVED stamps, and threaded comment margin cards).
* **Run from the command line:**
```bash
# Generate 5-page PDF specimen and high-resolution page previews in sample-docs/outputs/
python3 docs/sdk/examples/python/e2e_showcase_example.py \
  --output sample-docs/outputs/readme_showcase_specimen.pdf \
  --render-previews
```

---

## Documentation Architecture: Domain Hub vs Monolith

### Recommendation: Domain Folder Architecture with Hub Index

When managing enterprise API documentation for 266+ canonical types across 18 domains (matching Datalogics APDFL / Adobe PDFL), **a folder of dedicated domain pages with a root hub index** is strongly recommended over a single monolithic file:

```
docs/sdk/
├── README.md               # Master Hub: Architecture, quickstart, cross-language mapping
├── document.md             # Domain: Document Layer (Document, Page, Bookmark, ViewDestination)
├── text.md                 # Domain: Text & Fonts (Style, StyleTransition, Word, DocTextFinder)
├── graphics.md             # Domain: Content & Graphics State (Rect, Matrix, Color, Path)
├── annotations.md          # Domain: Annotations & Actions (GoToAction, Link, Redaction)
├── images.md               # Domain: Image Processing (XObject, raster conversion)
├── forms.md                # Domain: Forms & Data Exchange (Field, TextField, FDF/XFDF)
├── security.md             # Domain: Digital Signatures & Security (SignDoc, Encryption)
├── optimization.md         # Domain: PDF Optimization & Flate Compression
└── lowlevel.md             # Domain: Low-Level COS Layer (CosObj, CosDict, CosStream)
```

### Strategic Rationale:
1. **Maintainability & Git Conflict Isolation:** Domain teams can update typography (`text.md`) or cryptographic signing (`security.md`) independently without cross-domain merge conflicts on a 30,000-line monolithic file.
2. **Modern Documentation Site Compatibility:** Static site generators (VitePress, Docusaurus, MkDocs, mdBook) require individual page routes for deep linking, breadcrumb navigation, search indexation, and multi-tab code snippets (Rust / Python / TypeScript).
3. **Context Window & Developer Experience:** Developers investigating a specific task (e.g. text redaction or PDF form filling) only need to load that single focused domain document rather than scrolling through an exhaustive 40-page reference.
4. **Authoritative Parity Mapping:** Each domain maps to the Rust types in `crates/sdk/src/` and to runnable test fixtures.

