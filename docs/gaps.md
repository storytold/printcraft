# Where PdfCraft falls short of Acrobat Pro

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** minor (signatures: the EU Trusted Lists load as an opt-in trust list, gap 10 narrowed; earlier: alpha markers removed after the gate was re-judged: no gap blocks alpha) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

Every known shortfall, one entry each, ranked by what it costs users. This is the work list: agents
choose from the top unless the owner says otherwise, and prefer these over new P2/P3 checklist
features. The numbers behind it are in [target-app-parity.md](target-app-parity.md); the per-feature
detail is in `parity/acrobat-features.toml` (`cargo xtask parity --partial` lists what each partial
feature lacks).

No gap blocks the [alpha gate](roadmap.md#alpha-gate); gaps 1 and 8 hold the convert-and-share
workflow's partial sub-cases and are the first beta items.

Hours are Opus 5.5 agent wall-clock hours. "Doc" names the parity document the gap belongs to.
When a gap closes, delete its entry, note it in the [ROADMAP.md](../ROADMAP.md) progress log and
update the numbers.

## Summary

| # | Gap | Kind | Hours | Doc |
|---|---|---|---|---|
| 1 | Encrypted files we write don't open in Acrobat | file format | 8–15 | [file-format-parity.md](file-format-parity.md) |
| 2 | Rendering is borrowed and its fidelity unmeasured | feature, spec | 110–190 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 3 | Editing existing text is fragile on real files | feature | 100–180 | [target-app-parity.md](target-app-parity.md) |
| 4 | No printing on Windows or the web | feature, platform | 15–30 | [hardware-parity.md](hardware-parity.md) |
| 5 | Chinese UI shows missing-glyph boxes in releases | localization | 6–12 | [localization-parity.md](localization-parity.md) |
| 6 | Open-issue backlog of wrong results in shipped features | stability | 45–80 | this file |
| 7 | OCR reads only unaccented Latin | feature | 40–75 | [file-format-parity.md](file-format-parity.md) |
| 8 | Office export weak, Office import absent | file format | 55–100 | [file-format-parity.md](file-format-parity.md) |
| 9 | No Preflight; PDF/A partial; PDF/X and PDF/UA absent | standards | 65–115 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 10 | Signatures: no PKCS #11, timestamp servers or online revocation | feature, hardware | 40–70 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 11 | Accessibility: checker only, no tagging tools | feature | 40–70 | [target-app-parity.md](target-app-parity.md) |
| 12 | No measured fidelity harness against Acrobat | quality | (in 2) | [target-app-parity.md](target-app-parity.md) |
| 13 | Performance budgets and lazy loading of very large files | performance | 25–45 | this file |
| 14 | Arabic not mirrored; no Hindi, Indonesian, Korean, Vietnamese | localization | 30–50 | [localization-parity.md](localization-parity.md) |
| 15 | Forms: JavaScript object model, actions, submit, XFA depth | feature | 40–75 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 16 | UI precision gaps (multi-select move, rulers, accelerators, windows) | UI/UX | 30–60 | [ui-parity.md](ui-parity.md) |
| 17 | Certificate security and other protect gaps | feature, spec | 15–30 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 18 | Print production tools absent | feature | 25–45 | [target-app-parity.md](target-app-parity.md) |
| 19 | Preferences mostly missing | UI/UX | 15–25 | [ui-parity.md](ui-parity.md) |
| 20 | Scanners, GPU page rendering, pen pressure, Read Out Loud | hardware | 25–50 | [hardware-parity.md](hardware-parity.md) |
| 21 | Compare: no side-by-side view or filters | feature | 10–20 | [target-app-parity.md](target-app-parity.md) |
| 22 | PDF 2.0 extras, linearization, Arlington validation | spec | 20–35 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 23 | Local AI provider (summarize, ask, alt text, redaction suggestions) | AI | 20–40 | [target-app-parity.md](target-app-parity.md) |
| 24 | Checklist not diffed against Acrobat's own menus | measurement | 4–8 | [target-app-parity.md](target-app-parity.md) |

## The gaps

### 1. Encrypted files we write don't open in Acrobat

- **Missing:** interoperable encryption output. A document protected with a password in PdfCraft
  opens in PdfCraft, SumatraPDF, PDF24 and LibreOffice, but Adobe Reader, Acrobat and PDF-XChange
  report it as damaged.
- **Evidence:** [#774](https://github.com/storytold/pdfcraft/issues/774) (2026-10-10, confirmed by a
  second user). Our encryption tests round-trip through our own reader and `qpdf --check`, which
  is more lenient than Acrobat.
- **Impact:** blocking. Passwords are the most common reason to touch security; a file a recipient
  can't open is data loss in practice. Partial sub-case of the convert-and-share gate row (unencrypted sharing works), so not an alpha blocker; the first beta item.
- **Fix:** find the divergence (likely `/Encrypt` dictionary or R6 `/Perms`/`/OE`/`/UE` details, or
  a string/stream encrypted that must not be), then add an interop suite that opens every kind of
  output we write in other readers (pdf.js, MuPDF and Poppler as oracle processes; Acrobat by hand
  on the owner's Mac).
- **Estimate:** 8–15 h. **Doc:** [file-format-parity.md](file-format-parity.md).

### 2. Rendering is borrowed and its fidelity unmeasured

- **Missing:** our own renderer (M2: `model` crate, font engine, DisplayList devices, ADR-0004), or
  an owner decision to adopt another engine; and any measurement of rendering against Acrobat.
- **Evidence:** pages are drawn by vendored `hayro` 0.7 with 22 local patches to `hayro-interpret`,
  9 to `hayro`, 3 to `hayro-jbig2` and 13 to `hayro-syntax` (`vendor/README.md`); `lopdf` still
  backs document inspection. All 7 rendering P0 features are `partial`. User reports: TikZ pattern
  fills drawn wrong ([#794](https://github.com/storytold/pdfcraft/issues/794)), blurry text at
  1080p/1440p ([#730](https://github.com/storytold/pdfcraft/issues/730)), fixed zoom sizes differ
  from Acrobat ([#739](https://github.com/storytold/pdfcraft/issues/739)). An outside author
  proposes his MIT/Apache renderer as the bridge
  ([#841](https://github.com/storytold/pdfcraft/issues/841)).
- **Impact:** every page a user sees. Wrong glyphs, shadings or blend modes erode trust faster than
  any missing feature.
- **Fix:** first a fidelity harness (gap 12), then the renderer: own devices (150–250 h in the old
  plan) or an adopted engine plus our own font and text layers.
- **Estimate:** 110–190 h (includes the harness). **Owner decision needed** on #841 vs M2.
  **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 3. Editing existing text is fragile on real files

- **Missing:** robust in-place editing of text PdfCraft didn't write: subset fonts without the
  needed glyphs, CJK and right-to-left text, rotated text, lists, superscript/subscript, keeping
  structure tags, find and replace in edit mode; vector and object editing; arrange and align.
- **Evidence:** `edit.text-edit` is partial (font reused or Helvetica substituted);
  `edit.rtl-cjk-editing`, `edit.rotated-text-editing`, `edit.keep-tags-on-edit`,
  `edit.vector-edit`, `edit.edit-object-tool` are planned. Users:
  [#766](https://github.com/storytold/pdfcraft/issues/766) (editing Arabic text),
  [#844](https://github.com/storytold/pdfcraft/issues/844) (can't move several elements),
  [#787](https://github.com/storytold/pdfcraft/issues/787) (move images between pages),
  [#791](https://github.com/storytold/pdfcraft/issues/791).
- **Impact:** "Edit PDF" is one of Acrobat Pro's top reasons to buy. Substituting Helvetica changes
  how a document looks.
- **Estimate:** 100–180 h (the longest feature pole, M7). **Doc:** [target-app-parity.md](target-app-parity.md).

### 4. No printing on Windows or the web

- **Missing:** a Windows spooler (and printer properties), browser printing.
- **Evidence:** [#756](https://github.com/storytold/pdfcraft/issues/756): on Windows 11 the Print
  dialog only saves a PDF. `crates/print/README.md`: "Not yet: Windows and web spoolers".
- **Impact:** Windows is most of Acrobat's market. Printing is a P0 workflow.
- **Estimate:** 15–30 h. **Doc:** [hardware-parity.md](hardware-parity.md).

### 5. Chinese UI shows missing-glyph boxes in releases

- **Missing:** a Simplified Chinese (`Hans`) UI face in release builds and a working fallback.
- **Evidence:** [#826](https://github.com/storytold/pdfcraft/issues/826),
  [#688](https://github.com/storytold/pdfcraft/issues/688),
  [#728](https://github.com/storytold/pdfcraft/issues/728),
  [#758](https://github.com/storytold/pdfcraft/issues/758),
  [#689](https://github.com/storytold/pdfcraft/issues/689) (Flatpak). `AGENTS.md` §1.1 bars Noto
  CJK (Source Han rebranded), so craft-fonts needs another openly licensed Hans face.
- **Impact:** the second-largest language group sees a broken interface although the catalog is 97%
  translated.
- **Estimate:** 6–12 h (font sourcing in craft-fonts plus the fallback chain). **Doc:**
  [localization-parity.md](localization-parity.md).

### 6. Open-issue backlog of wrong results in shipped features

- **Missing:** fixes for ≈ 25 filed reports of features marked `shipped` that give wrong results,
  plus triage of ≈ 220 other open issues.
- **Evidence:** 245 open issues on 2026-10-10, e.g. XFDF exchange loses polygon vertices and turns
  callouts into text boxes (#809, #819), page deletion leaves null destinations (#817, #821),
  automatic URL linking panics on Unicode text (#816), autosave skips new unsaved PDFs (#815),
  saving ignores typing in an open paragraph (#811), automation reports success after replacing an
  earlier result (#799, #800), Shift-Cmd-G goes forward (#810).
- **Impact:** every one is a user who lost trust. #816 is a crash, which outranks feature work
  (`AGENTS.md` §4).
- **Estimate:** 45–80 h (≈ 0.2–0.4 h per issue). **Doc:** this file.

### 7. OCR reads only unaccented Latin

- **Missing:** every other script and accented Latin; editable-text output; deskew, rotation,
  despeckle, background and camera clean-up; MRC compression; pluggable engines; OCR in the
  browser; scanner input.
- **Evidence:** `ocr.ocr-languages` partial ("the models read only the Latin alphabet without
  accents"); 19 of 26 OCR features planned; [#744](https://github.com/storytold/pdfcraft/issues/744)
  (Dutch). Acrobat ships on-device recognition for dozens of languages.
- **Impact:** scanned documents are a core Pro workflow outside English.
- **Estimate:** 40–75 h, partly blocked on openly licensed models. **Doc:**
  [file-format-parity.md](file-format-parity.md).

### 8. Office export weak, Office import absent

- **Missing:** Word export that keeps layout; Excel and PowerPoint export; creating PDFs from Office,
  HTML, web pages and PostScript; SVG/XML/EPS/PS/JPEG 2000 export.
- **Evidence:** `create.export-docx` partial; [#773](https://github.com/storytold/pdfcraft/issues/773)
  (an invoice converted to Word loses its layout, gains table borders and duplicates pages);
  `create.export-xlsx`, `create.export-pptx`, `create.from-office`, `create.from-html` planned.
- **Impact:** "Export PDF" and "Create PDF" are two of Acrobat's most used tools.
- **Gate:** lossy or missing sub-cases of the convert-and-share workflow (partial, not blocking).
  First beta items: Create PDF from Office (LibreOffice sidecar, 10–20 h) and a Word export that
  keeps layout (20–35 h).
- **Estimate:** 55–100 h. **Doc:** [file-format-parity.md](file-format-parity.md).

### 9. No Preflight; PDF/A partial; PDF/X and PDF/UA absent

- **Missing:** Preflight (profiles, single checks and fixups, results tree, reports, droplets);
  PDF/A-1, PDF/A-4 and conformance levels a/u; PDF/X verify and convert; PDF/UA-1/2 verify; PDF/E,
  PDF/VT; output-intent choice; transparency flattening.
- **Evidence:** 12 Preflight features and 9 standards features planned; `optimize.pdfa-validate`,
  `optimize.pdfa-convert` partial (2b/3b structural rules; CMYK documents get no output intent).
  Acrobat ships hundreds of built-in Preflight profiles.
- **Impact:** blocks print shops, archives and accessibility compliance work.
- **Estimate:** 65–115 h. **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 10. Signatures: no PKCS #11, timestamp servers or online revocation

- **Missing:** PKCS #11 tokens and smart cards on macOS and Linux; timestamp-server preferences;
  fetching OCSP/CRL online for LTV; FieldMDP and locking fields on signing; the OS trust store; a UI
  for the EU Trusted Lists (they load as an opt-in file through `sign_trust`, off by default);
  signature graphics and saved appearances; compare signed version; signing in the browser.
- **Evidence:** `sign.pkcs11`, `sign.timestamp-servers`, `sign.field-mdp` and
  `sign.os-trust` planned; `sign.pades-bt`, `sign.ltv`, `sign.ocsp-crl` partial (caller-supplied
  evidence); `sign.eutl`, `sign.builtin-roots` partial (opt-in files, no UI). Users ask for
  Acrobat-like validation
  ([#824](https://github.com/storytold/pdfcraft/issues/824),
  [#775](https://github.com/storytold/pdfcraft/issues/775),
  [#718](https://github.com/storytold/pdfcraft/issues/718)).
- **Impact:** government and legal users sign with tokens and need B-LT/B-LTA.
- **Estimate:** 40–70 h; tokens need hardware to test. **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 11. Accessibility: checker only, no tagging tools

- **Missing:** autotag (document and fields), Tags, Order and Content panels, tag properties,
  Reading Order tool, artifact marking, table editor, role map, Make Accessible action, Read Out
  Loud; full keyboard operation of PdfCraft itself and a screen-reader audit.
- **Evidence:** all 32 checker rules shipped, but 21 tagging and reading features planned;
  `a11y.keyboard-only` (P0) planned, `a11y.app-screen-reader` partial.
- **Impact:** remediation work is impossible; PdfCraft itself is not yet usable without a mouse.
- **Estimate:** 40–70 h. **Doc:** [target-app-parity.md](target-app-parity.md).

### 12. No measured fidelity harness against Acrobat

- **Missing:** side-by-side comparison of rendering, text extraction, form calculation and
  signature validation against Acrobat on synthetic fixtures.
- **Evidence:** none exists; only text extraction has an oracle (word-F1 0.98 against `pdftotext`).
  Acrobat captures must stay local in `plan/acrobat/` (clean-room, `AGENTS.md` §1.1).
- **Impact:** until it exists, every quality number here is a judgement.
- **Estimate:** counted in gap 2 (≈ 20–30 h of it). **Doc:** [target-app-parity.md](target-app-parity.md).

### 13. Performance budgets and lazy loading of very large files

- **Missing:** `misc.performance-budgets` (P0) and `core.lazy-loading` (P0): open GB files within a
  memory budget, with budgets checked on Tier-1 platforms.
- **Evidence:** #307 brought a 9,156-page file from > 10 GiB to ≈ 500 MiB inspection, but
  file-backed input and global memory budgets remain open; blurry fonts at common resolutions
  (#730).
- **Estimate:** 25–45 h. **Doc:** this file.

### 14. Arabic not mirrored; no Hindi, Indonesian, Korean, Vietnamese

- **Missing:** a right-to-left layout (panels, menus, alignment), bidirectional text in the text
  layout itself (wrapping, typed text); catalogs for Hindi, Indonesian, Korean and Vietnamese
  (Acrobat ships Korean; it ships Arabic in its Middle East edition; it ships none of the other
  three).
- **Evidence:** [localization.md](localization.md) §Arabic; `misc.rtl-ui` planned.
- **Estimate:** 30–50 h (Arabic RTL 20–35, four catalogs 10–15 plus review). **Doc:**
  [localization-parity.md](localization-parity.md).

### 15. Forms: JavaScript object model, actions, submit, XFA depth

- **Missing:** the wider Acrobat JavaScript object model (annotations, layers, dialogs), document
  actions, the Actions tab triggers that run JavaScript, form submission, the debugger, barcode
  fields, rich-text values; XFA flattening and complete dynamic-form fidelity.
- **Evidence:** `form.js-host` partial, `form.document-actions`, `form.submit-form`,
  `edit.action-javascript`, `form.js-debugger`, `form.xfa-flatten` planned; XFA layout, FormCalc
  and data partial.
- **Estimate:** 40–75 h. **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 16. UI precision gaps

- **Missing:** moving several selected objects together (#844), nudging with arrow keys, single-key tool accelerators,
  rulers, grids and guides, loupe and dynamic zoom, multiple windows and split view, comment
  pop-ups that can be moved, connector lines, customisable quick tools, column text selection
  (#740); shortcut display and touchpad scrolling bugs (#746, #759, #721), macOS window controls
  (#789).
- **Estimate:** 30–60 h. **Doc:** [ui-parity.md](ui-parity.md).

### 17. Certificate security and other protect gaps

- **Missing:** encrypting for certificate recipients (and opening such files), security policies,
  attachment-only encryption, redaction pattern locales and partial-word redaction, folder
  redaction, protected view, URL and attachment trust.
- **Estimate:** 15–30 h. **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 18. Print production tools absent

- **Missing:** Output Preview, separations, total area coverage, overprint preview, ink manager,
  colour conversion, printer marks, fix hairlines, flattener preview, colour management settings;
  print as image; PostScript output.
- **Evidence:** 18 print-production features planned (L area).
- **Estimate:** 25–45 h (part of the L area's 30–55). **Doc:** [target-app-parity.md](target-app-parity.md).

### 19. Preferences mostly missing

- **Missing:** most of Acrobat's twenty or so preference pages (General, Documents, Commenting, Forms, Identity,
  Signatures, Security, Measuring, Units and guides, Convert, …). Today: Interface language,
  Documents and view, Identity, Fill & Sign, JavaScript.
- **Estimate:** 15–25 h. **Doc:** [ui-parity.md](ui-parity.md).

### 20. Scanners, GPU page rendering, pen pressure, Read Out Loud

- **Missing:** TWAIN/WIA/ICA/SANE scanning; GPU-accelerated page rendering (pages are rasterized on
  the CPU and uploaded); pressure-sensitive ink; platform text-to-speech.
- **Estimate:** 25–50 h; scanners need hardware. **Doc:** [hardware-parity.md](hardware-parity.md).

### 21. Compare: no side-by-side view or filters

- **Missing:** synchronised side-by-side view, filters by change type, page ranges, scanned
  documents.
- **Estimate:** 10–20 h. **Doc:** [target-app-parity.md](target-app-parity.md).

### 22. PDF 2.0 extras, linearization, Arlington validation

- **Missing:** UTF-8 text strings, associated files (`/AF`), document parts, structure namespaces;
  linearized save (Fast Web View); deterministic save; structural validation against the Arlington
  model.
- **Estimate:** 20–35 h. **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 23. Local AI provider

- **Missing:** the opt-in provider interface (local model or the user's own endpoint) and the
  features on it: summarize, ask with page citations, alternate-text and redaction suggestions.
  Acrobat's AI Assistant is a cloud service and out of scope.
- **Estimate:** 20–40 h. **Doc:** [target-app-parity.md](target-app-parity.md).

### 24. Checklist not diffed against Acrobat's own menus

- **Missing:** a menu and tool listing captured from Acrobat (names only, `plan/acrobat/`, never
  committed) diffed against the checklist, so that features the checklist never listed show up.
  Also correct `misc.localization` and `misc.installers`, which are still `planned` though both
  ship.
- **Estimate:** 4–8 h. **Doc:** [target-app-parity.md](target-app-parity.md).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | minor | Gap 10: the EU Trusted Lists are no longer missing but loadable as an opt-in file (`sign.eutl`, `sign.builtin-roots` partial); still missing the OS trust store and any UI for the trust sets |
| 2026-10-10 | minor | Removed the alpha markers: with the cross-app gate rule, gaps 1 and 8 are partial sub-cases, not blockers; stage alpha |
| 2026-10-10 | minor | Marked the alpha blockers (gap 1 and the Office part of gap 8) after the core-workflow gate put the stage at pre-alpha |
| 2026-10-10 | major | Created. Ranked 24 gaps from the 2026-10-10 re-measure, the open GitHub issues and the former ROADMAP.md §Where we're lacking and where we're going (renderer, hardening, fidelity, editing, Pro workflows, 1.0 polish), which this file replaces |
