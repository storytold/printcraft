# PdfCraft roadmap: milestones and what's next

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** minor (localized shortcut labels, #496; alpha gate added; convert-and-share re-judged with the cross-app rule → alpha) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

Forward-looking: the milestones, the current focus and what comes next. The parity numbers behind
it are in [target-app-parity.md](target-app-parity.md), the ranked work list in [gaps.md](gaps.md),
and the one-page summary in [ROADMAP.md](../ROADMAP.md). Detailed task lists and acceptance tests
live in `plan/execution-plan.md` (local-only, gitignored).

Keep it current: update the milestone row and the [ROADMAP.md](../ROADMAP.md) progress log at the
end of every session; re-estimate when a milestone lands.

## Alpha gate

The stage standard's gate: Acrobat Pro's everyday core workflows must each work end to end on the
main platform (macOS), with the work saved and reopened. Any `no`, or a `partial` that blocks the
workflow, keeps PdfCraft pre-alpha; those rows are the alpha checklist. Assessed 2026-10-10 with the
rule applied across the Crafting Apps: a row blocks only when a typical user cannot complete the
workflow at all with the app on its main platform for typical inputs; degraded fidelity, lossy
exchange with Acrobat users or a missing sub-case is `partial, not blocking` (a beta item).

| Core workflow | Works end to end? | Evidence | Hours to pass |
|---|---|---|---|
| View, search and print | yes | Render, find, select, panels, tiles; 500-page scrolling test; Print with CUPS on macOS and Linux (Windows printing missing, #756, a beta item). Rendering borrowed from `hayro` | 0 |
| Organize, combine and split pages | yes | `organize.*` P0s shipped with tests; links, fields, layers and bookmarks carried over; open fixes #817, #821 are edge cases | 0 |
| Comment and review | yes | Every markup type with appearances, replies, status, XFDF/FDF; comments saved in the PDF reopen in other readers. XFDF fidelity bugs #809, #819 | 0 |
| Fill and sign forms | yes | All AcroForm field types, Acrobat's AF functions, Fill & Sign, PAdES signatures validated with `pdfsig`/OpenSSL | 0 |
| Edit text and images | partial, not blocking | Editing a paragraph of Latin text works (embedded font reused, else Helvetica substituted, rewrapped); added text and images stay editable. Fails for CJK/RTL (#766), can't move several objects (#844); a beta item (gap 3) | 0 (beta: 100–180) |
| Convert and share (Office ↔ PDF, password protection) | partial, not blocking | Unencrypted PDFs we save open in Acrobat and other readers (958-file corpus round trip, `qpdf --check`); export to Word, RTF, HTML, text and images works, lossily (#773: an invoice's layout lost in Word); Create PDF works from images, text, the clipboard and many files at once. Missing sub-cases: password-protected output that Acrobat and Reader accept (#774; the file opens in PdfCraft, SumatraPDF, PDF24 and LibreOffice), an in-app Create PDF from Office (Office on macOS saves PDF itself), Excel/PowerPoint export. All beta items (gaps 1, 8) | 0 (beta: 40–70 for #774, Office→PDF and a layout-keeping Word export) |

**Result: alpha.** Every core workflow can be completed on macOS; two are lossy or missing sub-cases
(editing CJK/RTL text; encrypted output for Acrobat users, Office→PDF, Word layout), which are the
first beta items. Ready for real work, ≈ 49%, is inside the alpha band. (A first application of the
gate the same day judged convert and share as blocking and put PdfCraft at pre-alpha; re-judged with
the shared rule, encryption and Office→PDF are sub-cases and Word export is lossy, not absent.)

## Current focus

In order. Each item names its entry in [gaps.md](gaps.md).

1. **Convert and share:** encrypted output that Acrobat opens (gap 1, #774, small and the most
   damaging), then Create PDF from Office and a Word export that keeps layout (gap 8, #773).
2. **Fix the crash and wrong-result reports** filed against shipped features (gap 6), starting with
   the panic in #816.
3. **Printing on Windows** (gap 4, #756) and **a Chinese UI face in releases** (gap 5, #826).
4. **A fidelity harness against Acrobat, then the renderer decision** (gaps 12 and 2). Owner
   decision needed: our own M2 devices, or #841's proposal as the bridge.
5. **Editing existing content** (gap 3, M7): fonts, subsets, reflow, CJK and RTL.
6. **Pro workflows:** OCR languages (gap 7), Office export/import (gap 8), Preflight and PDF/A/X/UA
   (gap 9), signatures with tokens, timestamp servers and online LTV (gap 10), accessibility
   tagging (gap 11).
7. **1.0 polish (M14):** performance budgets and lazy loading (gap 13), right-to-left layout and the
   missing languages (gap 14), keyboard-only operation, a screen-reader audit.

Decided against for now (owner, 2026-10-05): self-installing updates and an update check at start.
Help ▸ Check for updates is manual only.

## Milestones

"Plan size" is the original single-agent sizing. "Done" is the estimated share of that milestone's
*acceptance criteria* that are met, not lines of code. "Remaining" is re-measured on 2026-10-10 at
the measured rate (see [target-app-parity.md](target-app-parity.md#effort-and-calibration)); the
total matches the parity estimate.

| M | Milestone | Plan size (h) | Done | Remaining (h, 2026-10-10) | Notes |
|---|---|---|---|---|---|
| M0 | Skeleton: workspace, xtask gates, CI | 15–30 | 98% | 1–2 | GitHub workflow, `deny.toml` (licence audit of every dependency), parity checklist (826 features, `xtask parity`) done. Missing: remaining crate stubs, testkit/oracle crates |
| M1 | COS: filters, crypt, parser, xref, writer | 120–200 | 80% | 20–40 | Done:<br>- filters and crypt: every standard-security revision R2–R6 (RC4, AES-128/256), SASLprep, permissions, creating encryption;<br>- cos: parse and repair (including invalid xref object-number ranges), decoded-stream limits including unfiltered data, decrypt on load, re-encrypt on save, incremental and full writing.<br>Corpus: open/edit/save passes on 958 files, and all 7 password-protected files open.<br>Full saves now pack objects into compressed object streams. Fuzzing runs nightly (`xtask fuzz`). Missing: ≥ 250 tests, own image codecs |
| M2 | Model, render, text | 200–350 | 15% | 130–230 | hayro bootstrap renderer (vendored patches); inspector predictor scratch is bounded by available input, and JPX sample rescaling handles 1–32 bits safely. Text extraction reaches word-F1 0.98 against pdftotext. Missing: model crate, fonts, DisplayList, renderer independent of hayro. Inspector page-label traversal and output are resource-bounded, with physical-number fallback warnings |
| M3 | Viewer app (native + web) | 80–150 | 90% | 10–20 | Acrobat-style shell, find, select, panels, tiles, web build, UI control channel for agents (opt-in). Missing: 60 fps test on a 500-page document, snapshot tests of every panel<br>Done since: fit visible, document title in the window; Linux middle-button scrolling; shared three-state theme preferences in the toolbar, View menu and control channel; late browser startup downloads preserve the current document and Save target (#171). |
| M4 | Engine, history, save, organize | 100–180 | 95% | 8–15 | Done:<br>- command registry (menus, shortcuts and palette all use it);<br>- undo/redo; incremental, atomic and encrypted saves;<br>- autosave and crash recovery;<br>- organize, combine, extract, split and insert-from-file, with identical fonts and images stored once;<br>- CLI `edit/combine/extract/split`.<br>Done since: Combine files in its own tab (sortable, resizable columns; folders; early warnings; unlocking protected files), bookmark editing, page labels (Number pages), CLI `run`, Set Page Boxes, Crop tool, Duplicate pages, Ctrl+A / Cmd+A to select all pages in Organize, Replace pages, bookmarks from tagged headings. Missing: page transitions, recovery on the web |
| M5 | Comments (all annotation types, XFDF) | 120–200 | 88% | 12–25 | Done: notes, highlight/underline/strikeout/squiggly, text boxes, ink, lines, arrows, rectangles, ovals, stamps (dynamic, Sign Here, Standard Business), with appearance streams; replies, status, checkmarks, locking, move/resize/restyle/delete; properties dialog; panel filter and sort; hide all; summaries (comments only); XFDF/FDF import and export; flatten; Fill & Sign; agent tools.<br>Done since: polygons, connected lines, clouds, callouts, inserted text, colour and checkmark filters; image signature pointer previews, selection handles and live image movement/resizing with proportional corners and independent edge handles. Missing: custom stamps, replace-text proposals, summary layouts with the page |
| M6 | Forms + JavaScript | 160–320 | 50% | 40–75 | Done: filling all field types with regenerated appearances; Prepare a form (every field type, multi-select/shared properties, move/resize/delete, Field Properties General/Appearance/Position/Options/Format/Validate/Calculate); Acrobat's AF format/keystroke/validate/calculate functions and simplified field notation run natively in Acrobat's event order; tab order; push-button actions; flatten; agent tools. Form data exchange (FDF, XFDF, XML, CSV, text) done.<br>Done since: JavaScript engine (boa, sandboxed) running custom keystroke/validate/calculate/format and button scripts, document JavaScripts, console. Missing: Actions tab, wider object model (annotations, layers, dialogs), XFA |
| M7 | Content editing (text, images, header/footer, watermark) | 250–500 | 20% | 100–180 | Done: header & footer, watermarks, backgrounds, Bates; added text and images that stay editable (move, resize, format, rotate, flip, crop, replace); links (Link tool, Link Properties, create from URLs, remove all). Missing: editing existing text and images in place (the longest pole), image/PDF watermarks |
| M8 | Security + redaction | 100–180 | 68% | 15–30 | Done: opening protected documents, permissions, Protect Using Password, Remove security; redaction (mark text/areas/pages, Search & Redact with patterns, apply removing glyphs, image pixels, vectors, XObject content, annotations and fields, verification, full rewrite on save); Remove hidden information and Sanitize.<br>Done since: redaction codes (U.S. FOIA and Privacy Act sets as overlay text, in Redaction properties and `redact_mark`). Missing: certificate security, custom redaction code sets and pattern locales, DCT re-encoding |
| M9 | Signatures (PAdES, validation) | 160–280 | 60% | 40–70 | Done: new `sign` crate (DER, X.509, CMS, PKCS #12 on RustCrypto; aws-lc-rs for RSA private keys); PAdES B-B signing (visible, invisible, existing fields, certification with DocMDP); validation with trust store and changes-after-signing classification; self-signed digital IDs; Signatures panel, message bar, sign dialogs; agent tools. Checked with pdfsig and OpenSSL.<br>Done since: macOS Keychain and Windows Current User Personal store signing (software-backed CNG keys), certificate viewer, signed documents protected from rewrites. Missing: timestamps (B-T), LTV (DSS, OCSP, CRL), FieldMDP, smart cards, PKCS #11 |
| M10 | OCR, create, export, print | 200–350 | 33% | 120–215 | Done: create from blank/text/PNG/JPEG/TIFF (multi-page)/GIF/BMP; export PNG/JPEG/TIFF and text; Print (Acrobat's sizing, n-up, booklet, poster, comments & forms, preview, CUPS spooler, print-ready PDF).<br>Done since: embedded/72/custom DPI choices for image imports; export all images; OCR (searchable image for pages, ranges and multiple files); single-sided cut-and-stack imposition with cut marks, through Print and doc_print; Create PDF from multiple files (PDFs, images and text, edited as pages in the grid). Missing: OCR languages beyond Latin, editable-text OCR output, Office export/import, Windows/web printing |
| M11 | Optimize, preflight, PDF/A/X/UA, print production | 200–350 | 18% | 80–140 | Done: new `optimize` crate: Reduce File Size and the PDF Optimizer (images measured where drawn, bicubic downsampling, JPEG/ZIP recompression only when smaller, discard objects and user data, Flate clean-up, resource merging, object streams). Missing: fonts and transparency panels, space audit, preflight, PDF/A/X/UA |
| M12 | Accessibility, compare, measure, search, XFA | 200–380 | 30% | 90–160 | Done: new `a11y` crate with the Accessibility Checker (all 32 rules, report, Fix/Skip/Explain, options dialog and results panel, agent tools).<br>Done since: 2D distance, perimeter and area measurements with persistent viewport calibration, snapping, live information and CSV export. Missing: autotag, Tags/Order/Content panels, Reading Order tool, alt-text workflow, compare, geospatial/3D measurement, search index, XFA |
| M13 | Automation (MCP, Action Wizard, CLI) + AI providers | 60–120 | 45% | 25–45 | Done: MCP resources (document info, text, page images); headless tool table (123 tools incl. signing, optimizing, initial view, links, stamps, data exchange, comment review, forms authoring and scripts, redaction, sanitize, print, add content), opt-in MCP server over stdio, CLI `run`/`tools` (closed stdout pipes exit cleanly), UI control channel with drag. Missing: Action Wizard, AI providers |
| M14 | 1.0 polish: performance, localization, installers | 120–250 | 15% | 80–150 | Done: PhotoCraft's translation system (`i18n/`: TSV catalogs, `tl!`, command-id and plural entries, system-language detection, strict catalog tests); every dialog, panel and notice goes through `tl!`. Catalogs of ≈ 2,000–2,140 entries for Japanese, Simplified and Traditional Chinese, Russian, Bulgarian, German, Spanish, French, Telugu, Hungarian, Ukrainian, Italian, Brazilian Portuguese and Arabic (logical-order catalog put into display order; layout not yet mirrored); Czech covers the menus. Signed and notarized macOS builds, signed Windows installers (x64, x86, ARM64), AppImage, deb, rpm, Flatpak and FreeBSD packages. #307 (bounded large-PDF memory): shared resource indexes and parser metadata, lazy inspection, bounded caches; a 9,156-page file inspects in ≈ 1.8 s at ≈ 500 MiB instead of exceeding 10 GiB. Shortcut display uses localized modifier/key names across menus, toolbar, palette, Help and tooltips (#496). Missing: a right-to-left layout, a Chinese UI face in releases, Hindi/Indonesian/Korean/Vietnamese, file-backed input and global memory budgets, performance budgets, keyboard-only operation |
| | **Total** | **2,085–3,840** | **≈ 49% ready** | **≈ 770–1,400** | Area view of the same work: [target-app-parity.md](target-app-parity.md#by-feature-area) |

## Upcoming milestones, ranked

| Rank | Next milestone | Closes | Estimate (h) |
|---|---|---|---|
| 0 | Convert and share: encrypted output opens in Acrobat, Create PDF from Office, Word export keeps layout | gap 1, part of gap 8 | 40–70 |
| 1 | Backlog and platform: crash and wrong-result reports fixed, Windows printing, Chinese UI face | gaps 4, 5, 6 | 65–120 |
| 2 | Fidelity harness and renderer decision (M2) | gaps 2, 12 | 110–190 |
| 3 | Editing existing content (M7) | gap 3 | 100–180 |
| 4 | Pro conversion: OCR languages and clean-up, Office export/import (M10) | gaps 7, 8 | 95–175 |
| 5 | Standards: Preflight, PDF/A all parts, PDF/UA, PDF/X (M11) | gap 9 | 65–115 |
| 6 | Signatures for institutions: PKCS #11, timestamp servers, online LTV (M9) | gap 10 | 40–70 |
| 7 | Accessibility tagging (M12) | gap 11 | 40–70 |
| 8 | 1.0 polish: performance, RTL, languages, keyboard-only (M14) | gaps 13, 14 | 55–95 |

Beta needs ranks 1–7 in substance (≈ 400–700 h; see [target-app-parity.md](target-app-parity.md#stage)).

## Critical path

M0 → M1 → M2 → M3 → M4 had to happen in order, and did, except that M2's own renderer was deferred
behind the `hayro` bootstrap. After M4, M5–M13 run in parallel across crates. The long poles are
M2 (renderer), M7 (content editing), M10 (OCR and Office), M11 (Preflight and standards) and M12
(tagging and XFA).

## Risks most likely to push estimates up

- Fidelity of text editing: fonts, subsets, reflow, CJK and RTL.
- XFA dynamic layout on real government and bank forms.
- Real-world signature chains, revocation and tokens.
- The rendering long tail: Type 3 fonts, broken fonts, shadings, blend modes.
- Correctness of PDF/A, PDF/X and PDF/UA conversion.
- Interoperability: files other readers (above all Acrobat) reject, as in #774.
- OCR and AI quality limited by what openly licensed models exist.
- CPU rendering performance on the web.
- Hostile input: every fuzz run so far has found new crashes or hangs.
- Unmeasured fidelity: without an Acrobat comparison harness, quality gaps surface as user reports.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | minor | M14: localized shortcut key names (#496), including runtime switching and native layout checks. Estimates unchanged. |
| 2026-10-10 | minor | Re-judged convert and share with the cross-app gate rule (only a workflow that can't be completed at all blocks): partial, not blocking → stage alpha |
| 2026-10-10 | minor | Added the alpha gate (six core workflows; convert and share fails → pre-alpha, 40–70 h to alpha) and rank 0 in the upcoming milestones |
| 2026-10-10 | major | Created from ROADMAP.md §Milestones, §Critical path, §Risks and §Where we're lacking. Merged the duplicated M1, M2, M3 and M14 rows, re-measured Done and Remaining, added Current focus and the ranked upcoming milestones |
