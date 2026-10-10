# PdfCraft parity with Adobe Acrobat Pro

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** minor (readiness table with hours per audience; full number restated as the standard's additive weighted sum, 47% → 49%; earlier: second look: cross-cutting double count removed, mainstream-practitioner number added, core-workflow gate applied → alpha) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

The authoritative assessment of how close PdfCraft is to Acrobat Pro. [ROADMAP.md](../ROADMAP.md)
summarizes it; [gaps.md](gaps.md) lists every shortfall one by one; the deep-dive checklists are
[pdf-spec-parity.md](pdf-spec-parity.md), [file-format-parity.md](file-format-parity.md),
[ui-parity.md](ui-parity.md), [hardware-parity.md](hardware-parity.md) and
[localization-parity.md](localization-parity.md).

**Scope.** Parity means the *offline* feature set of Acrobat Pro. Adobe's cloud services (Document
Cloud storage, Acrobat Sign requests, shared review, the AI Assistant, Adobe Express, the Adobe
Approved Trust List) are out of scope: 23 such features are tracked as `na` in
`parity/acrobat-features.toml` and excluded from every percentage.

## Headline

| | Value | Kind |
|---|---|---|
| **Feature breadth**, tier-weighted | **≈ 67%** | measured (checklist; weights below) |
| Feature breadth, unweighted | 56.9% (423 shipped + 74 partial of 808) | measured (`cargo xtask parity` arithmetic) |
| Must-have (P0) features | 94.2% (226 + 23 partial of 252) | measured |
| Feature breadth, mainstream practitioner | ≈ 70% tier-weighted (63.2% unweighted: 292 + 44 partial of 497; P0 95.3%) | measured ([split below](#mainstream-practitioner-vs-full-acrobat-pro)) |
| **Ready for real work**, full Acrobat Pro | **≈ 49%** (range 45–53%) | estimated (weighted sum over dimensions, [below](#by-dimension)) |
| **Ready for real work**, mainstream practitioner | **≈ 55%** (range 50–60%) | estimated ([split below](#mainstream-practitioner-vs-full-acrobat-pro)) |
| **Ready for real work**, essentials user | **≈ 66%** (range 60–70%) | estimated ([below](#essentials-user)) |
| **Stage** | **alpha** (passes the core-workflow gate) | see [Stage](#stage) |
| Remaining to beta | ≈ 400–700 Opus 5.5 agent-hours | estimated |
| Remaining to full parity | ≈ 780–1,400 Opus 5.5 agent-hours | estimated |

### Readiness by audience

| Audience | Ready | Opus 5.5 agent-hours to ≈ 95% | Work that dominates |
|---|---|---|---|
| Full Acrobat Pro (ready for real work) | ≈ 49% | ≈ 780–1,400 h (≈ 300–550 h wall clock with 3–5 agents; 60–70% parallelizes) | Own renderer and fidelity harness, editing existing content, OCR and Office conversion, Preflight and PDF/A/X/UA, accessibility tagging, signatures for institutions |
| Mainstream practitioner | ≈ 55% | ≈ 500–900 h (≈ 200–400 h wall clock; ≈ 60% parallelizes) | Rendering fidelity, editing existing text (CJK, RTL, subsets), conversion (Word layout, Excel, PowerPoint, Office→PDF), OCR languages, Windows printing, the bug backlog |
| Essentials user | ≈ 66% | ≈ 180–330 h (≈ 90–180 h wall clock; ≈ 50% parallelizes) | Rendering fidelity on everyday files, Windows printing, launch and GPU stability, encrypted output Acrobat opens (#774), simple text fixes, Word export layout, Chinese UI glyphs |

Hours are calibrated as in [Effort and calibration](#effort-and-calibration) (≈ 1k kept lines per agent-hour for new features, 3–10× slower for hardening and fidelity). The audiences are nested subsets, so essentials ≤ mainstream ≤ full.

The gap between the numbers is the story of this release: most of what an Acrobat user reaches
for *exists*, but much of it is shallower than Acrobat's, rendering is still borrowed, quality has
never been measured side by side with Acrobat, and the first public releases (0.4.0, 0.5.0) drew
245 open issues in two days, including one that makes our encrypted files unreadable in Acrobat
([#774](https://github.com/storytold/pdfcraft/issues/774)).

## How this was measured

- **Target identified from the installed app.** `/Applications/Adobe Acrobat DC/Adobe Acrobat.app`,
  `CFBundleShortVersionString` 26.002.21931. Inspected without launching it and without opening
  anything inside `Contents/Resources/JavaScripts/` (clean-room rules, `AGENTS.md` §2): the
  `Info.plist` `CFBundleDocumentTypes` (the formats it opens: see
  [file-format-parity.md](file-format-parity.md)), the `.lproj` folder names (24 localizations,
  plus the MEA/MEH/NAF Middle East and North Africa editions named in `Resources/Sequences/`), the
  names in `Contents/Plugins/` (Accessibility, AcroForm, Checkers, Comments, DigSig, EScript,
  MakeAccessible, PPKLite, ReadOutLoud, Reflow, SaveAsRTF, Search, Spelling, WebLink, …),
  `Contents/Frameworks/` (AdobeJP2K, Adobe3D, AdobeCoolType, AdobeLinguistic, the OCR-capable
  `OnDeviceIntelligence_SDK`, …) and the `PDF Optimizer` and `Sequences` (Action Wizard) folders.
  Names and listings only.
- **Feature breadth from the checklist.** `parity/acrobat-features.toml` tracks 831 Acrobat Pro
  features (808 in scope). `cargo xtask parity` (in `xtask ci`) rejects any `shipped` claim whose
  command, automation tool or cited test does not exist. The numbers here were computed from the
  file on `origin/main` at 5d590604 (2026-10-10) with the same arithmetic as the tool (partial
  counts half). The checklist itself was written from Acrobat's public documentation, not diffed
  against a menu dump, so it can miss features; that risk is one of the gaps.
- **Ready for real work by judgement, from evidence:** the code (152k lines of Rust in `crates/`,
  `apps/` and `xtask`, 1,765 tests, 184 registered commands, 141 automation tools); the vendored
  renderer's patch list (`vendor/README.md`); user reports on GitHub (245 open, 107 closed issues on
  2026-10-10); release packaging (`.github/workflows/release.yml`); and knowledge of Acrobat Pro's
  feature set from Adobe's public documentation.
- **Not measured, and the largest uncertainty:** no side-by-side comparison of rendering, text
  extraction or form behaviour against Acrobat exists yet. Every "ready" figure below is an
  estimate.

### Weights

Feature breadth weights the checklist's tiers by how much Acrobat users rely on them: **P0 50%,
P1 30%, P2 15%, P3 5%** → 0.50 × 94.2 + 0.30 × 61.0 + 0.15 × 12.0 + 0.05 × 2.3 ≈ **67%**.

Ready for real work weights the areas by how often an Acrobat Pro user reaches for them (judgement:
Adobe publishes no usage data): view 16%, comment 12%, edit 12%, organize 10%, forms 10%,
create/export 9%, sign 7%, protect 5%, print 5%, OCR 4%, core 3%, optimize/standards 3%,
accessibility 2%, automation/preferences 2%. That gives the Features dimension, ≈ 49%.

The **full ready-for-real-work number is an additive weighted sum over the dimensions** (standard
method; weights by what decides whether a professional can switch): features 40%, file formats and
conversion 12%, UI/UX 10%, PDF specification 8%, stability 8%, performance 6%, localization 5%,
platforms 5%, hardware 4%, ecosystem 1%, AI 1%:
0.40 × 49 + 0.12 × 40 + 0.10 × 55 + 0.08 × 60 + 0.08 × 50 + 0.06 × 40 + 0.05 × 59 + 0.05 × 70 +
0.04 × 35 + 0.01 × 30 + 0.01 × 0 = 49.25 → **≈ 49%**.

History: the first version of this file subtracted 9 points from the area figure for cross-cutting
shortfalls (40%); the second look found that a double count (Windows printing, encrypted-output
interop and Chinese glyphs were already scored in their areas) and used a 2-point deduction (47%);
this version replaces the deduction with the standard's weighted sum over dimensions (49%). Method
aligned with the standard, no new evidence. Multiplicative discounts are used only for the
mainstream and essentials numbers.

## Mainstream practitioner vs full Acrobat Pro

The checklist covers all of Acrobat Pro, including surfaces a typical practitioner rarely opens.
Splitting it (measured from `parity/acrobat-features.toml`):

| Slice | Features | Shipped | Partial | Unweighted | Tier-weighted | P0 |
|---|---|---|---|---|---|---|
| Mainstream: view, comment (without measuring), edit, organize, forms (without XFA and JavaScript tooling), protect and redact, OCR, create and export, compress (Reduce File Size, Optimizer), print (without print production) | 497 | 292 | 44 | 63.2% | ≈ 70% | 95.3% (193) |
| Specialist: core internals, digital-signature PKI, accessibility remediation, automation and preferences, Preflight and PDF/A/X/UA, print production, measuring, XFA and JavaScript tooling | 311 | 131 | 30 | 46.9% | ≈ 63% | 90.7% (59) |
| All | 808 | 423 | 74 | 56.9% | ≈ 67% | 94.2% (252) |

So the checklist is **not** dominated by niche surface (38% of rows), and the P0 tier, which is
mostly mainstream, is at 94–95% in both slices. What holds the mainstream number down is P1 depth
(69%) and the P2 long tail (13%: rulers, loupe, portfolios, spell check, Bates across files).

**Ready for real work, mainstream practitioner ≈ 55%**, weighting the mainstream areas by use and
scoring only their mainstream features: view 20% × 60, comment 15% × 70, edit 15% × 35,
organize 12% × 75, fill and sign 12% × 70, convert 10% × 30, protect and redact 6% × 55,
OCR 4% × 20, compress 3% × 60, print 3% × 50 = 55.6%. Read-only, review, form-filling and page
workflows are close to Acrobat; editing existing content, conversion and OCR are not.

### Essentials user

Someone who uses Acrobat for the everyday basics and never opens the rest. Scored on those features
only, by depth, then discounted for what such a user actually runs into. Advanced options, Pro
workflows, exchange edge cases and everything outside the mainstream slice are excluded.

| Core feature | Weight | Depth | Evidence |
|---|---|---|---|
| Open the PDFs people send and read them (render, zoom, scroll, page through) | 20% | 80% | Repair of damaged files, 958-file corpus; rendering borrowed but broadly right (#794 is a TikZ pattern) |
| Find, select and copy text | 8% | 85% | Find, options, results list; word-F1 0.98 |
| Print | 8% | 55% | Good on macOS and Linux; nothing on Windows, where most such users are (#756) |
| Highlight, sticky notes, text comments | 12% | 85% | All markup types, replies, properties |
| Fill in forms | 10% | 85% | Every AcroForm field type, Acrobat's formatting and calculations |
| Fill & Sign: typed, drawn or image signature | 10% | 85% | Fill & Sign tools; flatten on save |
| Rotate, delete, reorder and extract pages | 8% | 85% | Organize grid, drag reorder, thumbnails |
| Combine files | 6% | 85% | Combine tab with bookmarks per file |
| Fix a typo in existing text | 5% | 40% | Latin paragraphs only; font may be substituted |
| Export to Word or images | 5% | 45% | Images good; Word loses layout (#773) |
| Reduce file size | 3% | 75% | Reduce File Size, Optimizer |
| Protect with a password | 3% | 30% | Works in PdfCraft; Acrobat and Reader reject the file (#774) |
| Undo, save, autosave | 2% | 90% | Undo after save, atomic saves, crash recovery |

Weighted depth: **≈ 75%**. Discounts an essentials user hits: launch and stability (54 issues about
installing, launching, GPU and windows; OpenGL fallback) −5 points; discoverability and clarity
(an Acrobat-like shell with Home and All tools, but tool-mode friction such as #825 and #721) −2;
opening what people send (Chinese UI glyph boxes #826, CJK documents #758) −2.
**Essentials user ≈ 66%** (60–70%).

### User sentiment (second look, 2026-10-10)

Practitioners' praise is real and consistent with these numbers:

- **Adoption:** 7,595 GitHub stars, 2,661 forks; release downloads 177,301 (v0.4.0) and 39,447
  (v0.5.0, its first day).
- **Praise:** 23 of 359 issues written by users carry explicit praise ("amazing bit of software"
  #693, "ridiculously fast and awesome PDF editor" #320, "brilliant" #542, "fantastic and very useful"
  #369, "Gigantic stuff… I love it" #449). Switching reports: #179 switched from Adobe Reader on
  Windows ("covers nearly everything I used Reader for"); #232 replacing macOS Preview; #273
  "open source alternative to Acrobat"; #195 would "replace Acrobat completely" once hardware-token
  signing exists.
- **What users hit** (issue titles, excluding 35 agent-filed `fix(...)` reports): installation,
  launch and GPU/window problems 54, rendering and display 46, editing text and images 41, CJK and
  other fonts and glyphs 41, signing and certificates 35, printing 23, forms 18, conversion and
  export 16, OCR 5. Roughly two thirds are bugs (≈ 213 vs ≈ 111 feature requests), concentrated in
  core paths rather than niche features.

The praise comes overwhelmingly from Reader-class work (view, annotate, fill, sign, organize), where
the mainstream number is highest; the requests and bugs cluster on editing, CJK, printing,
conversion and signing tokens, which is where the gaps are. That supports raising the full number
from 40% to 47% (the double count; restated as 49% by the weighted-sum method) and reporting the mainstream 55% beside it; it does not support
calling PdfCraft a replacement for Acrobat Pro yet.

## By dimension

Hours are Opus 5.5 agent wall-clock hours, low–high. Rows marked *within features* are part of the
features total and are shown for orientation; the others add to it.

| Dimension | Ready | Weight | Remaining (h) | Counted | Evidence | Doc |
|---|---|---|---|---|---|---|
| Features (14 areas) | ≈ 49% depth; 67% breadth | 40% | 610–1,100 | base | Area table below | this file |
| UI/UX fidelity | ≈ 55% | 10% | 30–60 | additional | Acrobat-style shell, All tools, Home, command palette; missing single-key accelerators, rulers/guides, multiple windows, popups; user reports #739, #744, #746, #759, #789, #844 | [ui-parity.md](ui-parity.md) |
| File formats and conversion | ≈ 40% | 12% | 120–220 | within features | PDF read/write strong but encrypted output fails in Acrobat (#774); export Word is layout-poor (#773); no Excel/PowerPoint export, no Office/HTML/PostScript import | [file-format-parity.md](file-format-parity.md) |
| PDF specification and standards | ≈ 60% | 8% | 150–260 | within features | ISO 32000 syntax, encryption, annotations and AcroForm strong; rendering and fonts borrowed from `hayro`; PDF 2.0 extras, linearization, public-key security absent; PDF/A partial, PDF/X/UA none | [pdf-spec-parity.md](pdf-spec-parity.md) |
| Hardware | ≈ 35% | 4% | 25–50 | additional | Page raster is CPU only (Acrobat has GPU rendering); Windows CNG and macOS Keychain keys sign, no PKCS #11; no scanners; Windows has no printing (#756) | [hardware-parity.md](hardware-parity.md) |
| Localization | ≈ 59% over the 12 key languages | 5% | 35–60 | additional | 15 catalogs, 13 at 94–100%; Chinese UI shows tofu in releases (#826, #688); Arabic not mirrored; no Hindi, Indonesian, Korean, Vietnamese | [localization-parity.md](localization-parity.md) |
| Performance | ≈ 40% | 6% | 25–45 | additional | 500-page smooth scrolling and large-file memory work (#307) done; no performance budgets (`misc.performance-budgets`), no lazy loading of GB files; blurry text at 1080p/1440p (#730) | [gaps.md](gaps.md) |
| Stability | ≈ 50% | 8% | 45–80 | additional | Never-crash rules and nightly fuzzing; 245 open issues, many of them `fix(...)` reports of wrong results in shipped features (#799–#821) | [gaps.md](gaps.md) |
| Platforms | ≈ 70% | 5% | 15–30 | additional | macOS (signed, notarized), Windows x64/x86/ARM64 (signed), Linux (AppImage, deb, rpm, Flatpak), FreeBSD, web (WASM). Acrobat has no Linux, FreeBSD or full web editor; we lack Windows and web printing, web OCR and web recovery | [hardware-parity.md](hardware-parity.md) |
| Ecosystem and plug-ins | ≈ 30% | 1% | 10–20 | within features | No plug-in SDK or folder-level JavaScript; Action Wizard partial. Agent automation (CLI, MCP, UI control channel) has no Acrobat equivalent | [gaps.md](gaps.md) |
| AI features | ≈ 0% (in scope) | 1% | 20–40 | within features | Acrobat's AI Assistant is cloud-only (out of scope). Local, opt-in provider interface (`misc.ai-provider`) not started | [gaps.md](gaps.md) |
| **Total (weighted sum)** | **≈ 49%** | 100% | **≈ 780–1,400** | | | |

## By feature area

Breadth is measured from the checklist (partial = ½). Ready is estimated. Hours are remaining to
full parity in that area.

| Area | Features | Shipped | Partial | Breadth | Ready | Remaining (h) | What's strong / what's missing |
|---|---|---|---|---|---|---|---|
| A Core model and fidelity | 56 | 42 | 2 | 76.8% | 70% | 25–45 | Parser, repair, R2–R6 encryption, incremental and atomic saves, undo, autosave, crash recovery, revisions. Missing: encrypted output that Acrobat opens (#774), lazy loading of GB files, linearization, PDF 2.0 extras, Arlington validation, public-key security, own image codecs |
| B View and navigation | 92 | 52 | 11 | 62.5% | 55% | 110–190 | Shell, layouts, zoom, find, panels, tiles, web build, 500-page scrolling. **Pages are drawn by the vendored `hayro`** (22 + 9 local patches): all 7 rendering P0s are partial. Missing: own renderer or a decided replacement (#841), multiple windows, split view, rulers/guides, loupe, reflow, search across files |
| C Content editing | 61 | 29 | 7 | 53.3% | 30% | 100–180 | Edit existing paragraphs (font reused or Helvetica substituted, rewrapped), added text and images stay editable, header/footer, watermarks, backgrounds, Bates, links. Missing: robust editing of real-world text (subset fonts, CJK, RTL #766, rotated), lists, vector/object editing, arrange/align, spell check, multi-select move (#844) |
| D Organize pages | 76 | 45 | 3 | 61.2% | 70% | 12–25 | Organize grid, insert/extract/replace/split/combine with links, fields, layers and bookmarks carried over; page labels, boxes, bookmarks from structure. Missing: transitions, attachments editing, portfolios, struct-tree merge, field-name conflicts; open fixes #817, #818, #821 |
| E Comments and review | 65 | 46 | 9 | 77.7% | 70% | 12–25 | Every markup type with appearances, replies, status, filters, stamps (custom, dynamic), XFDF/FDF, summaries, flatten, 2D measuring. Missing: rich-text runs, popup windows, summary layouts with pages, per-type hiding; XFDF fidelity bugs #807, #809, #819 |
| F Forms and JavaScript | 79 | 58 | 8 | 78.5% | 65% | 40–75 | Fill and author every AcroForm field type, Acrobat's AF functions in Acrobat's event order, sandboxed JavaScript (boa), data exchange, Fill & Sign; XFA static/dynamic layout, FormCalc and XFA data (partial). Missing: wider JavaScript object model, document actions, submit, debugger, barcode fields, XFA flattening and full dynamic fidelity |
| G Protect, redact, sanitize | 41 | 23 | 2 | 58.5% | 55% | 15–30 | Passwords and permissions, Remove security, redaction that removes glyphs, pixels, vectors, annotations and tags, with verification; word lists, patterns, FOIA/Privacy Act codes; sanitize. Missing: certificate security, pattern locales, custom code sets, partial-word redaction, folder redaction, protected view |
| H Digital signatures | 45 | 23 | 10 | 62.2% | 45% | 40–70 | PAdES B-B signing and validation (PKCS #12, macOS Keychain, Windows store with PIN prompts), certification, encrypted documents, certificate usage checks; B-T, LTV, OCSP/CRL with caller-supplied evidence. Missing: PKCS #11 and smart cards outside Windows, timestamp-server preferences, online revocation fetching, FieldMDP and field locking, OS trust store, EU Trusted Lists, compare signed version |
| I Scan and OCR | 26 | 5 | 2 | 23.1% | 15% | 40–75 | Searchable image (ocrs, Latin script without accents) on pages, ranges and many files. Missing: every other language (#744 Dutch; accents, CJK, Cyrillic, Arabic), editable-text output, deskew, rotation, despeckle, camera images, MRC compression, scanners |
| J Create and export | 34 | 12 | 3 | 39.7% | 25% | 55–100 | Create from images, multipage TIFF, text, clipboard, blank, many files at once; export PNG/JPEG/TIFF, text, RTF; Word and HTML partial. Missing: Office/HTML/web/PostScript to PDF, Excel and PowerPoint export, Word layout fidelity (#773), SVG/XML/EPS/PS/JPEG 2000 export, accessible text |
| K Optimize and standards | 58 | 15 | 10 | 34.5% | 20% | 65–115 | Reduce File Size and the Optimizer (images, discards, clean-up, space audit); PDF/A-2b/3b verify and convert (partial). Missing: Preflight (profiles, fixups, reports), PDF/A-1/4 and a/u levels, PDF/X, PDF/UA, PDF/E, PDF/VT, transparency flattening, font unembedding, monochrome JBIG2/CCITT |
| L Print | 39 | 15 | 1 | 39.7% | 35% | 30–55 | Acrobat's sizing, n-up, booklet, poster, comments & forms, preview, print-ready PDF, CUPS on macOS/Linux and the Windows spooler (#756). Missing: **printing on the web**, print as image, PostScript, all print-production tools (output preview, separations, ink manager, printer marks, colour conversion) |
| M Accessibility | 61 | 38 | 1 | 63.1% | 30% | 40–70 | Accessibility Checker (all 32 rules, report, fixes), alternate text, title and language. Missing: autotag, Tags/Order/Content panels, Reading Order tool, artifact marking, table editor, Read Out Loud, full keyboard operation of the app, screen-reader audit |
| N Automation, preferences, misc | 75 | 20 | 5 | 30.0% | 35% | 25–45 | CLI, MCP, UI control channel, 141 automation tools, Action Wizard (run, create), compare (text, visual, report). Missing: most Preferences pages, side-by-side compare, Action Wizard management, custom commands, AI providers, rich media/3D, hosted web app |
| **All** | **808** | **423** | **74** | **56.9%** | **≈ 49%** | **610–1,100** | Plus 175–325 h of cross-cutting work (dimension table) |

Shipped features rest on thin evidence: 141 of 423 cite exactly one test, and only a handful cite
an external oracle (`pdftotext`, `pdfsig`, OpenSSL, `qpdf --check`, a corpus).

The checklist lags reality in two places: `misc.localization` is still `planned` although 15
catalogs ship, and `misc.installers` is `planned` although signed macOS, Windows and Linux packages
ship (`.github/workflows/release.yml`). Neither moves the numbers by more than 0.2 points; fix them
with evidence when next touched.

## Stage

**alpha.** Ready for real work, ≈ 49%, is inside the 40–75% band, and every one of Acrobat Pro's
six everyday workflows can be completed end to end on macOS. Under the gate rule shared by all the
Crafting Apps, a workflow blocks only when a typical user can't complete it at all; lossy exchange
or a missing sub-case is a beta item. **Converting and sharing** is therefore partial, not
blocking: unencrypted PDFs we save open everywhere and export to Word, RTF, HTML, text and images
works, but password-protected output is rejected by Acrobat and Reader (#774), there is no in-app
Create PDF from Office, and Word export loses layout (#773). The gate table, with evidence, is in
[roadmap.md](roadmap.md#alpha-gate). (Earlier the same day this row was judged blocking and the
stage set to pre-alpha; see the revision history.)

| Core workflow | Works end to end? |
|---|---|
| View, search and print | yes |
| Organize, combine and split pages | yes |
| Comment and review | yes |
| Fill and sign forms | yes |
| Edit text and images | partial, not blocking (Latin text with embedded or standard fonts) |
| Convert and share (Office ↔ PDF, password protection) | partial, not blocking (encryption interop #774, Office→PDF and Word layout #773 are beta items) |

**To reach beta** (≈ 75% ready, no blocking gap in PDF interchange), ≈ 26 points and
**≈ 400–700 agent-hours**:

| Beta requirement | Hours |
|---|---|
| Encrypted output opens in Acrobat and Reader (#774), with an interop suite that opens our outputs in other readers | 8–15 |
| Rendering: a fidelity harness against Acrobat captures (local, clean-room) and fixing what it finds; settle the renderer (own M2 devices, or #841's proposal); Chinese UI faces in releases | 80–140 |
| Editing existing text robust on real files: subset fonts, CJK, RTL, rotated text | 70–120 |
| Printing on Windows and the web | 15–30 |
| OCR in the major languages (accents, Cyrillic, CJK), deskew and rotation | 35–60 |
| Export: Word that keeps layout; Excel and PowerPoint | 45–80 |
| Signatures: timestamp servers, online revocation and LTV, PKCS #11, OS trust | 30–55 |
| PDF/A (all parts and levels), PDF/UA validation, a first Preflight | 45–80 |
| Accessibility tagging: autotag, Tags panel, Reading Order | 35–60 |
| Open-issue backlog, stability and performance budgets | 40–70 |
| **Total** | **≈ 400–700** |

## Effort and calibration

**Calibrated against this repository's own history.**

- The repo is 11 days old (first commit 2026-09-30 05:17), with 591 commits and 371 merged pull
  requests (all since 2026-10-04). Rust in `crates/`, `apps/` and `xtask/` grew from 37.7k lines
  (2026-10-02) to 82.6k (10-04), 99.2k (10-08) and 152.7k (10-10).
- The previous ROADMAP measured about **1.0k kept, tested lines per agent-hour** (72k lines in
  roughly 65–75 agent-hours). New features land at about that rate: medium P1 features such as
  redaction word lists (#757, 314 lines), bookmarks from structure (#754, 588 lines) and signing
  encrypted documents (#753, 308 lines) each took about 1–3 agent-hours including tests, with 2–4 h
  from PR to merge, mostly CI. Large arcs take longer: XFA dynamic layout, its JavaScript and
  FormCalc (#121, #230, #301; ≈ 13k lines) about 15–25 agent-hours; the Arabic interface
  (#692, 2.7k lines) about 3–5.
- Hardening and fidelity work is 3–10× slower per line than new features: the 2026-10-07 → 10-10
  block landed ≈ 330 commits but moved the checklist by only 12 shipped features (411 → 423), because most of
  it was never-crash work, Windows fixes, localization and XFA.

The previous estimate (2026-10-07) was **600–1,100 h** to full parity. Since then XFA, signature
timestamps/LTV and signing encrypted documents progressed (≈ −60 h), but the first public releases
surfaced work the estimate did not hold: 245 open issues, missing Windows printing, Acrobat
interoperability of encrypted output, Chinese UI glyphs and Word-export layout (≈ +200–300 h).
Hence **≈ 780–1,400 h** now. Roughly 60–70% parallelizes across crates (3–5 agents: ≈ 300–550 h
wall clock).

**Needs a human:** the renderer decision (own M2 devices vs an outside renderer, #841); Acrobat
oracle captures on the owner's Mac (synthetic fixtures only); native-speaker review of every
catalog; smart cards and PKCS #11 tokens, scanners and Windows printers to test against; any
licensed OCR or AI model.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | minor | Readiness table: hours to ≈ 95% per audience (full 780–1,400, mainstream 500–900, essentials 180–330). Full number restated as the standard's additive weighted sum over dimensions with written weights: 47% → 49%. Method aligned with the standard, no new evidence |
| 2026-10-10 | minor | Added the essentials-user score (≈ 66%): weighted depth of 13 everyday features, discounted for launch, discoverability and opening others' files |
| 2026-10-10 | minor | Consistency check across apps: re-judged the convert-and-share gate row with the shared rule (blocking only when the workflow can't be completed at all; lossy exchange and missing sub-cases are beta items). Encrypted output (#774) and Office→PDF are sub-cases, Word export is lossy not absent → partial, not blocking; stage pre-alpha → **alpha**. Numbers unchanged |
| 2026-10-10 | minor | Second look requested by the owner after practitioners praised PdfCraft: user sentiment gathered (stars, downloads, 23 praising issues, switching reports, issue themes); checklist split into mainstream (497) and specialist (311) features; the 9-point cross-cutting deduction removed as a double count (Windows printing, interop and glyphs were already scored in their areas). Ready for real work 40% → 47%; mainstream practitioner ≈ 55% added. Core-workflow alpha gate applied: convert and share fails → stage alpha → **pre-alpha**, ≈ 40–70 h from alpha |
| 2026-10-10 | major | Created from ROADMAP.md §Estimate summary and §Honest assessment (2026-10-05/07). Full re-measure against Acrobat Pro 26.002.21931: checklist recomputed (56.9% unweighted, ≈ 67% tier-weighted), ready for real work ≈ 40%, stage alpha, hours re-calibrated (780–1,400 h to parity, 400–700 h to beta), dimension and area tables |
| 2026-10-07 | minor | (in ROADMAP.md) 806 tracked features, 51.0% shipped, effort-weighted ≈ 30–35%, 600–1,100 h |
| 2026-10-05 | major | (in ROADMAP.md) First honest assessment by dimension and area |
