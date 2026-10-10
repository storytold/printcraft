# Hardware and platform parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (first hardware and platform matrix) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

Each hardware feature Acrobat Pro uses, per platform, ours against Acrobat's. Acrobat runs on macOS
and Windows (plus a limited web viewer/editor in Document Cloud); PdfCraft runs on macOS, Windows,
Linux, FreeBSD and the web. Gaps ranked in [gaps.md](gaps.md).

**Hardware ≈ 35% ready, 25–50 h** (additional to the features total; scanners, smart cards and
printing are counted in their feature areas). **Platforms ≈ 70% ready, 15–30 h.**

## Hardware

| Feature | Acrobat (macOS / Windows) | PdfCraft macOS | PdfCraft Windows | PdfCraft Linux / FreeBSD | PdfCraft web | Evidence |
|---|---|---|---|---|---|---|
| GPU page rendering | yes (Page Display ▸ GPU-accelerated rendering) | no: CPU raster, GPU for UI compositing (wgpu, Metal) | no (wgpu DX12, GL fallback) | no (wgpu Vulkan/GL) | no (WebGL/WebGPU UI) | `crates/render`; gap 20 |
| Multi-threaded rendering | yes | yes (render pool, tiles, watchdog) | yes | yes | single thread | `view.render-priority` |
| HiDPI / Retina | yes | yes | yes, per-monitor DPI v2 (#324, #725) | yes | yes | text blurry at 1080p/1440p (#730) |
| Multiple monitors | yes, multiple windows | one window | one window; surfaces fit the GPU limit (#577) | one window | n/a | `view.multiple-windows` planned |
| Printers | all, PostScript, printer properties | CUPS spooler | **none** (#756) | CUPS spooler | **none** | `crates/print/README.md` |
| Scanners (TWAIN, WIA, ICA) | yes | none | none | none (SANE) | n/a | `ocr.scanner-acquire` planned |
| Smart cards and USB tokens | yes (PKCS #11, CryptoTokenKit, CNG) | Keychain identities (software keys) | CNG store keys, with PIN prompts for cards (#742) | none (PKCS #11 planned) | none | `sign.pkcs11` planned |
| Pen and touch | Touch mode, pen ink, Windows Ink | trackpad pinch zoom; ink from pointer, no pressure | ink from pointer, no pressure or touch mode | as macOS | pointer | touchpad scrolling not 1:1 (#759) |
| Keyboard input methods (IME) | yes | yes; Japanese and Korean IME fixes patched into winit | yes (Microsoft Korean cursor fix) | X11 and Wayland IME fixes | browser | `vendor/README.md` winit patches 2–10 |
| Screen readers | VoiceOver, NVDA, JAWS | AccessKit labels (partial) | AccessKit (partial) | AccessKit (partial) | partial | `a11y.app-screen-reader` partial |
| Text to speech (Read Out Loud) | yes | none | none | none | none | `a11y.read-aloud` planned |
| Apple Silicon / ARM64 | native | native | ARM64 build (#61) | ARM64 builds | n/a | `release.yml` |

## Platforms

| Platform | Acrobat | PdfCraft | Packaging | Gaps |
|---|---|---|---|---|
| macOS | yes | yes | signed and notarized DMG | — |
| Windows (x64, x86, ARM64) | yes (x64, ARM64) | yes | signed installers | — |
| Linux | no | yes | AppImage, deb, rpm, Flatpak, tarball | Chinese faces in Flatpak (#689) |
| FreeBSD | no | yes | package | — |
| Web | Acrobat online (cloud) | yes (WASM, offline in the browser) | static site | printing, OCR, crash recovery, PKCS #12 signing, hosted deployment |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | major | Created from the code, `vendor/README.md`, `.github/workflows/release.yml`, the print crate's README and user issues |
| 2026-10-10 | minor | Windows printing shipped (#756): the Windows row's gap is closed; the web row keeps "printing" |
