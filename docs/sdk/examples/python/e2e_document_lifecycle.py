#!/usr/bin/env python3
"""PdfCraft End-to-End Document Lifecycle Example (Python SDK / APDFL Parity).

This standalone script demonstrates how to drive the full document manipulation
lifecycle using the PdfCraft Python SDK:
  1. Create a document from scratch (`DocumentSession.create()`)
  2. Add, modify (rotate 90°, resize MediaBox), and remove pages
  3. Insert styled text using standard fonts, colors, and `StyleTransition`
  4. Perform pattern search using bounded `DocTextFinder`
  5. Embed raster images as PDF XObjects, locate, and remove/replace images
  6. Serialize to a valid PDF file and render a preview PNG

Usage:
  python3 docs/sdk/examples/python/e2e_document_lifecycle.py
  python3 docs/sdk/examples/python/e2e_document_lifecycle.py --output my_doc.pdf --preview my_doc.png
"""

from __future__ import annotations

import argparse
import io
import sys
from pathlib import Path

# Automatically add local SDK package to sys.path so the example runs out of the box
REPO_ROOT = Path(__file__).resolve().parents[4]
SDK_PYTHON_DIR = REPO_ROOT / "sdks" / "python"
if str(SDK_PYTHON_DIR) not in sys.path:
    sys.path.insert(0, str(SDK_PYTHON_DIR))

try:
    from PIL import Image
except ImportError:
    print("Error: Pillow is required to run this example. Install with: pip install pillow", file=sys.stderr)
    sys.exit(1)

from pdfcraft import (
    Color,
    DocTextFinderConfig,
    DocumentSession,
    Rect,
    Style,
    StyleTransition,
)


def create_sample_badge(width: int = 160, height: int = 80, rgb_color: tuple[int, int, int] = (26, 128, 142)) -> bytes:
    """Generates an in-memory PNG badge with diagonal pattern stripes."""
    img = Image.new("RGB", (width, height), color=rgb_color)
    for x in range(width):
        for y in range(height):
            if (x + y) % 16 < 2:
                img.putpixel((x, y), (255, 255, 255))
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def run_document_lifecycle(
    output_pdf: Path,
    preview_png: Path | None = None,
    dpi: int = 150,
) -> None:
    """Executes the complete document lifecycle from start to finish."""
    print("=" * 70)
    print("PdfCraft End-to-End Document Lifecycle Demonstration")
    print("=" * 70)

    # --------------------------------------------------------------------------
    # Step 1: Initialize New Document
    # --------------------------------------------------------------------------
    print("\n[Step 1/6] Initializing DocumentSession...")
    session = DocumentSession.create()
    print(f"  • Created empty session (initial page count: {session.num_pages})")

    # --------------------------------------------------------------------------
    # Step 2: Page Operations (Add, Modify, Remove)
    # --------------------------------------------------------------------------
    print("\n[Step 2/6] Performing Page Management Operations...")
    p1 = session.add_page(width=612.0, height=792.0)  # Standard Letter
    p2 = session.add_page(width=612.0, height=792.0)
    p3 = session.add_page(width=595.0, height=842.0)  # Temporary A4 sheet
    print(f"  • Added 3 pages (total pages: {session.num_pages})")

    # Modify Page 2: rotate 90° clockwise to landscape orientation and expand width
    session.modify_page(2, rotation_degrees=90, media_box=Rect(0.0, 0.0, 792.0, 612.0))
    print(f"  • Modified Page 2: orientation = 90° landscape, dimensions = 792x612 pt")

    # Remove Page 3
    removed = session.remove_page(3)
    print(f"  • Removed Page 3 (final page count: {session.num_pages})")

    # --------------------------------------------------------------------------
    # Step 3: Typography, Styling & Style Transitions
    # --------------------------------------------------------------------------
    print("\n[Step 3/6] Adding Typographical Content & Style Transitions...")

    # Document Title in Helvetica-Bold
    title_style = Style(
        font_name="Helvetica-Bold",
        font_size=22.0,
        color=Color.rgb(0.08, 0.18, 0.36),  # Deep Navy
    )
    print(f"  • Title Style: {title_style}")
    session.add_text(1, "PdfCraft Enterprise SDK Lifecycle", 40.0, 720.0, title_style)

    # Subtitle in Helvetica
    sub_style = Style(font_name="Helvetica", font_size=12.0, color=Color.rgb(0.35, 0.40, 0.45))
    session.add_text(1, "Programmatic Document Assembly & APDFL Parity Verification", 40.0, 698.0, sub_style)

    # Section 1: Typography Variations
    h2_style = Style(font_name="Helvetica-Bold", font_size=14.0, color=Color.rgb(0.12, 0.45, 0.65))
    session.add_text(1, "1. Typographical Font & Color Styling", 40.0, 655.0, h2_style)

    times_style = Style(font_name="Times-Roman", font_size=11.0, color=Color.rgb(0.1, 0.1, 0.1))
    session.add_text(1, "- Serif Body: Built with clean-room pure-Rust vector typography engines.", 50.0, 635.0, times_style)

    courier_style = Style(font_name="Courier", font_size=10.0, color=Color.rgb(0.15, 0.55, 0.25))
    session.add_text(1, "- Monospace Code: cargo test -p pdfcraft-sdk && python verify.py", 50.0, 615.0, courier_style)

    # Inline Style Transition (Crimson Bold -> Charcoal Regular)
    st_bold = StyleTransition(
        char_index=0,
        style=Style(font_name="Helvetica-Bold", font_size=11.0, color=Color.rgb(0.85, 0.20, 0.10)),
    )
    st_regular = StyleTransition(
        char_index=17,
        style=Style(font_name="Helvetica", font_size=11.0, color=Color.rgb(0.2, 0.2, 0.2)),
    )
    session.add_styled_line(
        page_number=1,
        x=50.0,
        y=590.0,
        base_text="CRITICAL NOTICE: Inline style transition demonstration across tokens.",
        transitions=[st_bold, st_regular],
    )
    print("  • Placed styled text across Helvetica, Times-Roman, and Courier")
    print("  • Placed inline multi-style transition line")

    # --------------------------------------------------------------------------
    # Step 4: Text Pattern Search (DocTextFinder)
    # --------------------------------------------------------------------------
    print("\n[Step 4/6] Executing DocTextFinder Pattern Search...")
    search_query = "Enterprise SDK"
    finder_cfg = DocTextFinderConfig(case_sensitive=True)
    matches = session.find_text(search_query, finder_cfg)
    print(f"  • Searched for '{search_query}': found {len(matches)} match(es)")
    for match in matches:
        print(f"    - Page {match.page_number}: '{match.matched_text}' at (x={match.bounding_box.x:.1f}, y={match.bounding_box.y:.1f})")

    # --------------------------------------------------------------------------
    # Step 5: Embedded Raster Graphics & Removal
    # --------------------------------------------------------------------------
    print("\n[Step 5/6] Managing Embedded Raster Graphics...")
    session.add_text(1, "2. Embedded Raster Graphics & Asset Management", 40.0, 545.0, h2_style)

    badge_bytes = create_sample_badge(160, 80, rgb_color=(26, 128, 142))
    watermark_bytes = create_sample_badge(100, 50, rgb_color=(230, 140, 30))

    id_badge = session.add_image(
        page_number=1,
        image_source=badge_bytes,
        rect=Rect(50.0, 430.0, 160.0, 80.0),
        image_id="VerificationBadge",
    )
    id_watermark = session.add_image(
        page_number=1,
        image_source=watermark_bytes,
        rect=Rect(240.0, 445.0, 100.0, 50.0),
        image_id="DraftWatermark",
    )
    print(f"  • Embedded Image 1: ID '{id_badge}' (160x80 pt)")
    print(f"  • Embedded Image 2: ID '{id_watermark}' (100x50 pt)")
    print(f"  • Current images on Page 1: {len(session.get_images(1))}")

    # Remove temporary watermark
    session.remove_image(1, "DraftWatermark")
    print(f"  • Removed image 'DraftWatermark' (remaining images on Page 1: {len(session.get_images(1))})")

    caption_style = Style(font_name="Helvetica", font_size=9.0, color=Color.rgb(0.4, 0.4, 0.4))
    session.add_text(1, "Embedded Verification Badge (Draft Watermark was programmatically removed).", 50.0, 415.0, caption_style)

    # Section 3: Architecture Invariants
    session.add_text(1, "3. APDFL Parity & Architectural Compliance", 40.0, 370.0, h2_style)
    notes = [
        "- ISO 32000-2 & Arlington schema compliance with zero closed-source binary dependencies.",
        "- Clean-room Safe Rust guarantee (#![forbid(unsafe_code)]) and Never Crash error discipline.",
        "- Canonical 17 Datalogics APDFL domains verified across Rust, Python, and TypeScript.",
        "- Headless testability and multi-engine parity verified without Adobe proprietary assets.",
    ]
    y_pos = 350.0
    for note in notes:
        session.add_text(1, note, 50.0, y_pos, times_style)
        y_pos -= 18.0

    # Page 2 content
    session.add_text(2, "Page 2: Landscape Orientation (Rotated 90°)", 50.0, 540.0, title_style)
    session.add_text(2, "Demonstrating dynamic page rotation and dimension modification.", 50.0, 515.0, sub_style)

    # --------------------------------------------------------------------------
    # Step 6: Serialize to PDF & Render Preview
    # --------------------------------------------------------------------------
    print("\n[Step 6/6] Serializing to Output PDF...")
    output_pdf.parent.mkdir(parents=True, exist_ok=True)
    res = session.save(output_pdf)
    print(f"  ✓ PDF serialized successfully: {output_pdf} ({res.size_bytes:,} bytes)")

    if preview_png is not None:
        print(f"\n[Preview] Rendering Page 1 to PNG via pdftoppm (DPI: {dpi})...")
        try:
            rendered = session.render_page(1, preview_png, dpi=float(dpi))
            print(f"  ✓ High-resolution preview rendered: {rendered} ({rendered.stat().st_size:,} bytes)")
        except Exception as e:
            print(f"  ⚠ Note: preview rendering skipped or pdftoppm unavailable: {e}")

    print("\n" + "=" * 70)
    print("Lifecycle Execution Complete!")
    print(f"Output PDF:     {output_pdf.resolve()}")
    if preview_png and preview_png.exists():
        print(f"Output Preview: {preview_png.resolve()}")
    print("=" * 70)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="PdfCraft End-to-End Programmatic Document Lifecycle Example",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument(
        "--output",
        "-o",
        type=Path,
        default=Path("dist/example_lifecycle.pdf"),
        help="Path where the generated PDF document will be written",
    )
    parser.add_argument(
        "--preview",
        "-p",
        type=Path,
        default=Path("dist/example_lifecycle_preview.png"),
        help="Path where the rendered Page 1 preview PNG will be saved",
    )
    parser.add_argument(
        "--no-preview",
        action="store_true",
        help="Skip raster preview rendering",
    )
    parser.add_argument(
        "--dpi",
        type=int,
        default=150,
        help="Rendering DPI for raster preview",
    )

    args = parser.parse_args()
    preview_target = None if args.no_preview else args.preview

    run_document_lifecycle(
        output_pdf=args.output,
        preview_png=preview_target,
        dpi=args.dpi,
    )


if __name__ == "__main__":
    main()
