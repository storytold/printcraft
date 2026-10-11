"""End-to-End Test Suite for Documentation Examples (Issue #872).

# Architecture Reference
Module: sdks/python/tests/test_examples_e2e.py
Purpose:
    Validates that documentation examples (OCR extraction and README showcase builder)
    execute successfully, producing valid output artifacts and markdown representations.
"""

import sys
import unittest
from pathlib import Path

# Setup sys.path
REPO_ROOT = Path(__file__).resolve().parents[3]
SDK_PYTHON_DIR = REPO_ROOT / "sdks" / "python"
EXAMPLES_DIR = REPO_ROOT / "docs" / "sdk" / "examples" / "python"

for p in (SDK_PYTHON_DIR, EXAMPLES_DIR):
    if str(p) not in sys.path:
        sys.path.insert(0, str(p))

from ocr_document_to_markdown import process_pdf_document  # type: ignore
from pypdf import PdfReader
from e2e_showcase_example import build_readme_showcase_document  # type: ignore


class TestExamplesEndToEnd(unittest.TestCase):
    """Automated integration tests for documentation examples."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.test_output_dir = Path("dist/test_examples")
        cls.test_output_dir.mkdir(parents=True, exist_ok=True)

    def test_e2e_showcase_example(self) -> None:
        """Verifies programmatic construction of the 5-page README showcase PDF."""
        out_pdf = self.test_output_dir / "test_readme_specimen.pdf"
        session = build_readme_showcase_document(out_pdf)

        self.assertEqual(session.num_pages, 5)
        self.assertTrue(out_pdf.exists())
        self.assertGreater(out_pdf.stat().st_size, 50000)

        # Validate with pypdf
        reader = PdfReader(str(out_pdf))
        self.assertEqual(len(reader.pages), 5)

    def test_ocr_markdown_extraction_sample_docs(self) -> None:
        """Verifies OCR and layout extraction on sample-docs producing valid markdown."""
        sample_doc = Path("sample-docs/osaka geidai schedule.pdf")
        if not sample_doc.exists():
            self.skipTest("Sample document not present")

        out_md = process_pdf_document(
            pdf_path=sample_doc,
            output_dir=self.test_output_dir,
            max_pages=1,
            dpi=100,
        )

        self.assertTrue(out_md.exists())
        content = out_md.read_text(encoding="utf-8")
        self.assertIn("## Page 1", content)
        self.assertIn("大阪芸術大学", content)
        self.assertGreater(len(content), 500)


if __name__ == "__main__":
    unittest.main()
