"""Verification test suite for PdfCraft Python SDK (Task 5.1)."""

import unittest
from pathlib import Path

from pdfcraft.errors import InvalidArgumentError, PdfCraftError
from pdfcraft.local import LocalClient
from pdfcraft.rest import RestClient
from pdfcraft.types import DocumentSource, MergeOptions, RenderOptions


class TestPdfCraftPythonSdk(unittest.TestCase):
    def setUp(self):
        self.local = LocalClient()
        self.rest = RestClient(base_url="http://127.0.0.1:9999")

    def test_local_merge_in_memory(self):
        doc1 = b"%PDF-1.4 sample 1"
        doc2 = b"%PDF-1.4 sample 2"
        res = self.local.merge([doc1, doc2])
        self.assertTrue(res.is_memory)
        self.assertEqual(res.mime_type, "application/pdf")
        self.assertTrue(res.data.startswith(b"%PDF-1.7"))
        self.assertIn(doc1, res.data)
        self.assertIn(doc2, res.data)

    def test_local_empty_merge_raises_invalid_argument(self):
        with self.assertRaises(InvalidArgumentError):
            self.local.merge([])

    def test_local_split(self):
        res = self.local.split(b"%PDF-doc", every_n_pages=1)
        self.assertEqual(len(res.files), 2)
        self.assertTrue(all(isinstance(f, Path) for f in res.files))

    def test_local_render_page_dpi(self):
        res = self.local.render_page(b"%PDF-doc", 1, RenderOptions(dpi=300.0))
        self.assertTrue(res.is_memory)
        self.assertEqual(res.mime_type, "image/png")
        self.assertIn(b"DPI_300", res.data)

    def test_local_render_page_zero_rejected(self):
        with self.assertRaises(InvalidArgumentError):
            self.local.render_page(b"%PDF-doc", 0)

    def test_rest_problem_details_mapping(self):
        # Verify 400 Bad Request mapping
        with self.assertRaises(InvalidArgumentError):
            self.rest._handle_error_response(b'{"status": 400, "detail": "Invalid page", "code": "BAD_REQUEST"}', 400)

        # Verify 413 Quota Exceeded mapping
        with self.assertRaises(PdfCraftError) as ctx:
            self.rest._handle_error_response(b'{"status": 413, "detail": "Limit reached", "code": "QUOTA_EXCEEDED"}', 413)
        self.assertEqual(ctx.exception.status_code, 413)


if __name__ == "__main__":
    unittest.main()
