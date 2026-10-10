# PDF specification and standards parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** minor (signature trust: the EU Trusted Lists load as an opt-in file; earlier: first spec-coverage checklist) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

How much of ISO 32000-2 (PDF 2.0) and the PDF subset standards PdfCraft implements, set against
what Acrobat Pro implements. This is PdfCraft's equivalent of a "geometry" or "codec" checklist:
the parts of the format users never see named but always feel. Feature-level detail is in
`parity/acrobat-features.toml`; the gaps are ranked in [gaps.md](gaps.md).

Legend: **own** (our code), **borrowed** (the vendored `hayro` bootstrap or `lopdf`), **partial**,
**none**. "Evidence" names tests or checklist ids.

**PDF specification and standards ≈ 60% ready** (estimated): syntax, encryption, annotations,
AcroForm, redaction and signatures are our own and well tested; graphics, fonts and image codecs
are borrowed; PDF 2.0 additions and most subset standards are missing. **150–260 h** to parity
(within the features total; most of it is the renderer and font engine, and Preflight).

## ISO 32000-2 by clause

| Clause | Topic | Acrobat | PdfCraft | Evidence / what's missing |
|---|---|---|---|---|
| §7.2–7.5 | Syntax, objects, file structure, xref tables and streams, object streams, hybrid files, incremental updates | full | **own**, tolerant, repairs damaged files | `crates/cos`; corpus 958 files; fuzzed nightly |
| §7.4 | Stream filters: Flate, LZW, ASCII85, ASCIIHex, RunLength, predictors | full | **own** | `crates/filters` |
| §7.4 | Image filters: DCT, JPX, JBIG2, CCITTFax | full | **borrowed** (decode only) | `core.image-filters` partial; own codecs M1.1/M1.2; no JBIG2/CCITT encoding (Optimizer) |
| §7.5.8 | Linearization | R W | R only | `core.linearization` planned |
| §7.6.4 | Standard security handler R2–R6 (RC4 40/128, AES-128, AES-256), crypt filters, `/EncryptMetadata`, SASLprep | full | **own**, read and write | `crates/crypt`; **written files rejected by Acrobat (#774)** |
| §7.6.5 | Public-key security handler | full | none | `core.open-public-key`, `protect.certificate-encryption` planned |
| §7.6.2 | Unencrypted signature `/Contents` | yes | yes | signing encrypted documents (#753) |
| §7.9.2 | Text strings incl. PDF 2.0 UTF-8 | full | PDFDocEncoding and UTF-16; UTF-8 none | `core.pdf20-utf8-strings` planned |
| §7.11 | File specifications, embedded files | full | read and save attachments; no add/delete/describe | `organize.attachments-*` planned |
| §7.12 | Extensions dictionary | yes | preserved | |
| §8 | Graphics: paths, colour spaces, patterns, shadings, images, XObjects | full | **borrowed** renderer; own content parser and serializer (`crates/content`) for editing and redaction | rendering P0s partial |
| §8.6 | ICC-based colour, output intents, colour management | full | ICC preserved; sRGB output intent for PDF/A; no colour management settings or soft proofing | `print.color-management` planned |
| §9 | Text and fonts: Type 1, TrueType, OpenType/CFF, Type 3, CID-keyed, CMaps, ToUnicode | full | **borrowed** rendering; own text extraction (word-F1 0.98); standard-14 metrics for generated appearances; embedded fonts reused for editing, else Helvetica | `view.render-type3`, `view.render-cid-fonts` partial; no own font engine; no font embedding/subsetting for arbitrary new text beyond craft-fonts Japanese |
| §10 | Rendering: transfer functions, halftones, overprint, smoothness | full | borrowed; no overprint preview | `print.overprint-preview` planned |
| §11 | Transparency: blend modes, soft masks, groups, knockout | full | borrowed (knockout shipped) | `view.render-transparency` partial |
| §12.1–12.3 | Viewer preferences, page layout, page labels, outlines, destinations | full | **own**: initial view, page labels, bookmarks edit and from structure, named destinations rewired on copy | named-destination editing panel planned |
| §12.4 | Page transitions, thumbnails, articles, presentations | full | none (thumbnails discarded or kept) | `organize.page-transitions`, `view.articles-panel` planned |
| §12.5 | Annotations | 28 types | see table below | |
| §12.6 | Actions | 19 types | see table below | |
| §12.7 | Interactive forms (AcroForm): fields, appearances, calculation order, JavaScript actions | full | **own**: all field types, regenerated appearances, Acrobat's AF functions and event order, sandboxed JavaScript (boa) | `form.js-host` partial (object-model subset); submit planned |
| §12.7.8 | XFA (deprecated in PDF 2.0; Acrobat still fills XFA) | static and dynamic | **own**, partial: template layout, FormCalc, XFA JavaScript model, datasets | `form.xfa-*` partial; no flattening |
| §12.8 | Digital signatures | see table below | | |
| §12.9 | Measurement and geospatial | full | 2D measuring with viewports and calibration; no geospatial | `comment.geospatial` planned |
| §12.10 | Document requirements | yes | none | |
| §13 | Multimedia: sound, movies, screen, rich media, 3D (U3D, PRC) | yes | preserved untouched; not played or shown | `misc.rich-media-*`, `misc.3d-view` planned (P3) |
| §14.3 | Metadata: info dictionary, XMP | full, XMP editor | info dictionary edited; XMP kept in step for PDF/A; no XMP editor or custom properties | `organize.xmp-editor` planned |
| §14.6–14.8 | Marked content, logical structure, tagged PDF | full, with editing tools | read (checker, bookmarks from structure, redaction of tags); no tag editing or autotag | gap 11 |
| §14.9 | Accessibility support: alt text, ActualText, language | full | alt text figure by figure, language, title | `a11y.alt-text-workflow` |
| §14.10 | Web capture | yes | none | |
| §14.11 | Prepress: page boxes, output intents, trapping, OPI | full | page boxes and crop; sRGB output intent | `print.*` production planned |
| §14.12 | Document parts (DPart) | yes | none | `core.pdf20-dpart` planned |
| §14.13 | Associated files (`/AF`) | yes | none | `core.pdf20-associated-files` planned |
| — | Structural validation (Arlington PDF model) | yes (Preflight syntax checks) | none | `core.arlington-validation` planned |
| — | Optional content (layers) | full | **own** toggle and default state; set-OCG-state actions; no layer editing | `view.layers-panel` |
| — | Redaction (`/Redact` annotations, applying) | full | **own**: removes glyphs, image pixels, vectors, XObject content, annotations, fields and tags; verifies no residue | `protect.redact-*`, `protect.redaction-verification` |

## Annotations (§12.5.6)

| Type | Acrobat | PdfCraft | Notes |
|---|---|---|---|
| Text (note), Popup | create, edit | create, edit; pop-ups not movable windows | `comment.popups` planned |
| Link | create, edit | create, edit, from URLs | `edit.link-*` |
| FreeText (text box, typewriter, callout) | create, rich text | create; rich text written but no per-run formatting | `comment.text-box` partial |
| Line, Square, Circle, Polygon, PolyLine | create | create, all ten line endings | |
| Highlight, Underline, Squiggly, StrikeOut | create | create | |
| Caret (insert/replace text) | create | create; not snapped to text | `comment.insert-text` partial |
| Stamp | create, custom, dynamic | create, custom, dynamic, Sign Here | stamp management planned |
| Ink | create, erase | create, erase | no pressure |
| FileAttachment | create | create | |
| Sound | create, play | preserved | P3 |
| Redact | create, apply | create, apply | own |
| Widget | create, edit | create, edit | AcroForm |
| Watermark | create | page-content watermarks (as Acrobat writes them); `/Watermark` annotations preserved | |
| Screen, Movie, RichMedia, 3D | create, play | preserved | P3 |
| PrinterMark, TrapNet | create (print production) | preserved | |
| Projection | create (PDF 2.0) | preserved | |

## Actions (§12.6.4)

| Action | Acrobat | PdfCraft |
|---|---|---|
| GoTo | yes | yes (page with Fit; named destinations and zoom partial) |
| GoToR, GoToE | yes | none (preserved; the Optimizer can remove invalid ones) |
| Launch | yes (restricted) | none |
| Thread | yes | none |
| URI | yes | yes |
| Sound, Movie, Rendition, GoTo3DView, RichMediaExecute | yes | none (P3) |
| Hide | yes | yes |
| Named | yes | safe list (print, page navigation) |
| SubmitForm | yes | explained, not sent |
| ResetForm | yes | yes |
| ImportData | yes | via Import form data, not as an action |
| JavaScript | yes | yes in fields and buttons (sandboxed); document actions none |
| SetOCGState | yes | yes |
| Trans | yes | none |

## Digital signatures (§12.8, ETSI EN 319 142 PAdES)

| Capability | Acrobat | PdfCraft |
|---|---|---|
| Sub-filters `adbe.pkcs7.detached`, `ETSI.CAdES.detached`; read `adbe.pkcs7.sha1`, `adbe.x509.rsa_sha1` | yes | yes |
| PAdES B-B | yes | yes (PKCS #12, macOS Keychain, Windows store with PIN) |
| PAdES B-T (signature timestamp), document timestamps | yes, with configured TSA | partial: works when a TSA is given; no timestamp-server preferences |
| PAdES B-LT, B-LTA (DSS/VRI, OCSP/CRL) | yes, fetched online | partial: evidence supplied by the caller; no online fetching |
| DocMDP certification | yes | yes |
| FieldMDP (lock fields) | yes | none |
| UR3 usage rights | preserved, Reader-extending is Adobe-only | none (P3) |
| Seed values | yes | none |
| Validation: chain, key usage, EKU, critical extensions, changes after signing, signed version | yes | yes; name constraints and issuer critical extensions missing |
| Trust: own store, OS store, AATL, EUTL | yes | own store; EUTL as an opt-in file and optional built-in roots (off by default, no UI); AATL out of scope; OS store planned |
| Smart cards and tokens (PKCS #11, CryptoTokenKit) | yes | Windows CNG store (with PIN prompt) only |

## Subset standards

| Standard | Acrobat | PdfCraft | Hours |
|---|---|---|---|
| PDF/A-1a/b (ISO 19005-1) | verify, convert | none | |
| PDF/A-2a/b/u, PDF/A-3a/b/u | verify, convert | **partial**: 2b and 3b structural rules and conversion (sRGB intent; not CMYK) | |
| PDF/A-4, 4e, 4f | verify, convert | none | |
| PDF/X-1a, X-3, X-4 (ISO 15930) | verify, convert | none | |
| PDF/UA-1, PDF/UA-2 (ISO 14289) | verify (Preflight), checker | Accessibility Checker (32 rules, Acrobat's own), no PDF/UA verification | |
| PDF/E, PDF/VT | verify, convert | none | |
| Preflight (profiles, fixups, reports) | hundreds of profiles | none | |
| Factur-X / ZUGFeRD (e-invoice in PDF/A-3) | Preflight checks | none | |
| **All subset standards** | | | **65–115** (gap 9) |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | minor | Trust row: the EU Trusted Lists and optional built-in roots exist as opt-in sets (off by default, no UI); the OS store is still planned |
| 2026-10-10 | major | Created from the code, `vendor/README.md` and `parity/acrobat-features.toml`, against ISO 32000-2 and Acrobat Pro 26.002.21931's documented capabilities |
