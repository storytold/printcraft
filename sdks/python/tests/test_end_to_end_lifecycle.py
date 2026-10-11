"""Comprehensive End-to-End Document Lifecycle Test Suite (APDFL Parity).

# Architecture Reference
Module: sdks/python/tests/test_end_to_end_lifecycle.py
Purpose:
    Validates the end-to-end programmatic document manipulation lifecycle:
      1. Create a brand new document from scratch
      2. Page management: add pages, rotate, resize media boxes, and delete unwanted pages
      3. Text operations: add text, font variations, typographical styling (Style & StyleTransition),
         and text searching with DocTextFinder
      4. Image operations: embed raster images, locate by ID, and remove/replace images
      5. Output verification: serialize to real PDF and render preview PNG for visual validation
"""

import io
import os
import shutil
import unittest
from pathlib import Path

from PIL import Image
from pypdf import PdfReader

from pdfcraft import (
    Color,
    DocTextFinderConfig,
    DocumentSession,
    Page,
    Rect,
    Style,
    StyleTransition,
)


class TestEndToEndDocumentLifecycle(unittest.TestCase):
    """Full lifecycle integration tests for PdfCraft."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.output_dir = Path("dist")
        cls.output_dir.mkdir(parents=True, exist_ok=True)
        cls.output_pdf = cls.output_dir / "e2e_showcase_lifecycle.pdf"
        cls.preview_png = cls.output_dir / "e2e_showcase_preview.png"

    def test_complete_document_lifecycle(self) -> None:
        """Executes the full 5-stage lifecycle sequence."""
        # ----------------------------------------------------------------------
        # Stage 1: Create New Document
        # ----------------------------------------------------------------------
        session = DocumentSession.create()
        self.assertEqual(session.num_pages, 0)

        # ----------------------------------------------------------------------
        # Stage 2: Add, Remove, and Modify Pages
        # ----------------------------------------------------------------------
        # Add 3 pages
        p1 = session.add_page(width=612.0, height=792.0)  # Standard Letter
        p2 = session.add_page(width=612.0, height=792.0)
        p3 = session.add_page(width=595.0, height=842.0)  # A4 size to be deleted
        self.assertEqual(session.num_pages, 3)

        # Modify Page 2: rotate 90 degrees and expand media box
        session.modify_page(2, rotation_degrees=90, media_box=Rect(0.0, 0.0, 792.0, 612.0))
        mod_p2 = session.get_page(2)
        self.assertEqual(mod_p2.rotation_degrees, 90)
        self.assertEqual(mod_p2.media_box.width, 792.0)
        self.assertEqual(mod_p2.media_box.height, 612.0)

        # Remove Page 3
        removed_page = session.remove_page(3)
        self.assertEqual(removed_page.media_box.width, 595.0)
        self.assertEqual(session.num_pages, 2)
        self.assertEqual(session.get_page(1).page_number, 1)
        self.assertEqual(session.get_page(2).page_number, 2)

        # ----------------------------------------------------------------------
        # Stage 3: Text, Fonts, Styling & Search
        # ----------------------------------------------------------------------
        # Document Header
        title_style = Style(
            font_name="Helvetica-Bold",
            font_size=22.0,
            color=Color.rgb(0.08, 0.18, 0.36),  # Deep Navy
        )
        self.assertEqual(
            str(title_style),
            "[color=RGB(0.08, 0.18, 0.36), fontsize=22.0, fontname=Helvetica-Bold]",
        )
        session.add_text(1, "PdfCraft Enterprise SDK Showcase", 40.0, 720.0, title_style)

        # Subtitle with standard Helvetica
        sub_style = Style(
            font_name="Helvetica",
            font_size=12.0,
            color=Color.rgb(0.35, 0.40, 0.45),  # Slate Grey
        )
        session.add_text(1, "End-to-End Programmatic Document Verification (APDFL Parity)", 40.0, 698.0, sub_style)

        # Section: Typographical Font Variations
        h2_style = Style(font_name="Helvetica-Bold", font_size=14.0, color=Color.rgb(0.12, 0.45, 0.65))
        session.add_text(1, "1. Typographical Font & Color Styling", 40.0, 655.0, h2_style)

        times_style = Style(font_name="Times-Roman", font_size=11.0, color=Color.rgb(0.1, 0.1, 0.1))
        session.add_text(1, "- Serif Body: Built with clean-room pure-Rust vector typography engines.", 50.0, 635.0, times_style)

        courier_style = Style(font_name="Courier", font_size=10.0, color=Color.rgb(0.15, 0.55, 0.25))  # Emerald code
        session.add_text(1, "- Monospace Code: cargo test -p pdfcraft-sdk && python verify.py", 50.0, 615.0, courier_style)

        # Multi-style Transition in a single line (StyleTransition)
        st_bold = StyleTransition(
            char_index=0,
            style=Style(font_name="Helvetica-Bold", font_size=11.0, color=Color.rgb(0.85, 0.20, 0.10)),  # Crimson
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

        # Search for text with DocTextFinder
        finder_cfg = DocTextFinderConfig(case_sensitive=True)
        matches = session.find_text("Enterprise SDK", finder_cfg)
        self.assertEqual(len(matches), 1)
        self.assertEqual(matches[0].page_number, 1)
        self.assertEqual(matches[0].matched_text, "Enterprise SDK")
        self.assertGreater(matches[0].bounding_box.width, 0)

        # Case-insensitive search
        matches_ci = session.find_text("showcase", DocTextFinderConfig(case_sensitive=False))
        self.assertEqual(len(matches_ci), 1)

        # ----------------------------------------------------------------------
        # Stage 4: Add Images & Remove Images
        # ----------------------------------------------------------------------
        h2_img = Style(font_name="Helvetica-Bold", font_size=14.0, color=Color.rgb(0.12, 0.45, 0.65))
        session.add_text(1, "2. Embedded Raster Graphics & Asset Management", 40.0, 545.0, h2_img)

        # Create 2 synthetic raster images using Pillow
        # Image A: Primary Verification Badge (Teal)
        img_a = Image.new("RGB", (160, 80), color=(26, 128, 142))
        for x in range(160):
            for y in range(80):
                if (x + y) % 16 < 2:
                    img_a.putpixel((x, y), (255, 255, 255))
        img_a_bytes = io.BytesIO()
        img_a.save(img_a_bytes, format="PNG")

        # Image B: Temporary Stamp to be removed (Amber)
        img_b = Image.new("RGB", (100, 50), color=(230, 140, 30))
        img_b_bytes = io.BytesIO()
        img_b.save(img_b_bytes, format="PNG")

        # Add both images to Page 1
        id_a = session.add_image(
            page_number=1,
            image_source=img_a_bytes.getvalue(),
            rect=Rect(50.0, 430.0, 160.0, 80.0),
            image_id="VerificationBadge",
        )
        id_b = session.add_image(
            page_number=1,
            image_source=img_b_bytes.getvalue(),
            rect=Rect(240.0, 445.0, 100.0, 50.0),
            image_id="DraftWatermark",
        )
        self.assertEqual(id_a, "VerificationBadge")
        self.assertEqual(id_b, "DraftWatermark")

        placed_images = session.get_images(1)
        self.assertEqual(len(placed_images), 2)

        # Now remove Image B ("DraftWatermark") as requested
        removed = session.remove_image(1, "DraftWatermark")
        self.assertTrue(removed)

        # Verify only 1 image remains on Page 1
        remaining_images = session.get_images(1)
        self.assertEqual(len(remaining_images), 1)
        self.assertEqual(remaining_images[0]["image_id"], "VerificationBadge")

        caption_style = Style(font_name="Helvetica", font_size=9.0, color=Color.rgb(0.4, 0.4, 0.4))
        session.add_text(1, "Embedded Verification Badge (Draft Watermark was programmatically removed).", 50.0, 415.0, caption_style)

        # Section 3: Parity Summary
        h2_sum = Style(font_name="Helvetica-Bold", font_size=14.0, color=Color.rgb(0.12, 0.45, 0.65))
        session.add_text(1, "3. APDFL Parity & Architectural Compliance", 40.0, 370.0, h2_sum)

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

        # Page 2 content (rotated landscape page)
        session.add_text(2, "Page 2: Landscape Orientation (Rotated 90°)", 50.0, 540.0, title_style)
        session.add_text(2, "Demonstrating dynamic page rotation and dimension modification.", 50.0, 515.0, sub_style)

        # ----------------------------------------------------------------------
        # Stage 5: Save & Render Visual PDF and PNG
        # ----------------------------------------------------------------------
        save_res = session.save(self.output_pdf)
        self.assertTrue(self.output_pdf.exists())
        self.assertGreater(save_res.size_bytes, 1000)

        # Validate saved PDF with pypdf reader
        reader = PdfReader(str(self.output_pdf))
        self.assertEqual(len(reader.pages), 2)
        # Verify page 2 rotation
        self.assertEqual(reader.pages[1].rotation, 90)

        # Render preview PNG of Page 1
        rendered_png = session.render_page(1, self.preview_png, dpi=150.0)
        self.assertTrue(rendered_png.exists())
        self.assertGreater(rendered_png.stat().st_size, 10000)


if __name__ == "__main__":
    import io
    unittest.main()
