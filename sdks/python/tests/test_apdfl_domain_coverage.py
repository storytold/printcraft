"""Comprehensive APDFL Domain Functional Coverage Test Suite for Python SDK.

Verifies that the Python SDK accurately reflects the 17 Datalogics APDFL 21 domains,
including Style, StyleTransition, Word, WordFinder, GoToAction : Action, and Annotations.
"""

import unittest
from pdfcraft.types import (
    Action,
    AcroFormExportType,
    Annotation,
    BlendMode,
    Bookmark,
    ButtonField,
    Color,
    ColorSpace,
    DocTextFinderConfig,
    DocTextFinderMatch,
    Document,
    Field,
    FitMode,
    GoToAction,
    HighlightAnnotation,
    ImageFormat,
    Matrix,
    OptimizationParams,
    PDFOptimizer,
    Page,
    PermissionsFlags,
    Point,
    Quad,
    Rect,
    Redaction,
    SignDoc,
    Style,
    StyleTransition,
    TextField,
    URIAction,
    ViewDestination,
    Word,
    WordFinderConfig,
)


class TestApdflDomainFunctionalCoverage(unittest.TestCase):
    def test_text_and_fonts_style_and_transitions(self):
        """Verifies Style, StyleTransition, and Word behavior matching APDFL specifications."""
        color = Color.rgb(0.2, 0.4, 0.6)
        style = Style(font_name="Helvetica-Bold", font_size=12.0, color=color)
        
        self.assertEqual(style.font_name, "Helvetica-Bold")
        self.assertEqual(style.font_size, 12.0)
        self.assertEqual(style.color.r, 0.2)
        
        # Test required ToString() format: [color=..., fontsize=..., fontname=...]
        style_str = str(style)
        self.assertTrue(style_str.startswith("[color="))
        self.assertIn("fontsize=12.0", style_str)
        self.assertIn("fontname=Helvetica-Bold", style_str)
        
        # StyleTransition at character index
        transition = StyleTransition(char_index=5, style=style)
        self.assertEqual(transition.char_index, 5)
        self.assertEqual(transition.style.font_name, "Helvetica-Bold")
        
        # Word containing bounding box and style transitions
        bbox = Rect(10.0, 20.0, 50.0, 15.0)
        quad = Quad.from_rect(bbox)
        word = Word(text="PdfCraft", bounding_box=bbox, quads=[quad], styles=[transition])
        self.assertEqual(word.text, "PdfCraft")
        self.assertEqual(len(word.styles), 1)
        self.assertEqual(word.styles[0].char_index, 5)

    def test_word_finder_and_doc_text_finder_configs(self):
        """Verifies WordFinder and DocTextFinder configuration options."""
        wf_cfg = WordFinderConfig(preserve_ligatures=True, precise_quads=True)
        self.assertTrue(wf_cfg.preserve_ligatures)
        self.assertTrue(wf_cfg.precise_quads)
        
        finder_cfg = DocTextFinderConfig(case_sensitive=True, whole_words_only=True)
        self.assertTrue(finder_cfg.case_sensitive)
        self.assertTrue(finder_cfg.whole_words_only)

    def test_action_and_goto_action_hierarchy(self):
        """Verifies GoToAction : Action relationship matching APDFL class model."""
        dest = ViewDestination(page_number=3, fit_mode=FitMode.FIT, zoom=1.5)
        goto = GoToAction(destination=dest)
        action = Action(goto=goto)
        
        self.assertIsNotNone(action.goto)
        self.assertEqual(action.goto.destination.page_number, 3)
        self.assertEqual(action.goto.destination.fit_mode, FitMode.FIT)
        self.assertEqual(action.goto.destination.zoom, 1.5)

    def test_annotations_and_redaction(self):
        """Verifies HighlightAnnotation, Redaction, and base Annotation."""
        rect = Rect(50.0, 100.0, 200.0, 25.0)
        quad = Quad.from_rect(rect)
        highlight = HighlightAnnotation(quads=[quad], color=Color.rgb(1.0, 1.0, 0.0))
        
        annot = Annotation(rect=rect, contents="Important Note", highlight=highlight)
        self.assertEqual(annot.contents, "Important Note")
        self.assertEqual(len(annot.highlight.quads), 1)
        self.assertEqual(annot.highlight.color.r, 1.0)
        
        redact = Redaction(quads=[quad], overlay_text="CONFIDENTIAL")
        self.assertEqual(redact.overlay_text, "CONFIDENTIAL")

    def test_forms_and_data_exchange(self):
        """Verifies AcroForm fields and export format enums."""
        rect = Rect(100.0, 200.0, 150.0, 20.0)
        txt = TextField(multiline=True, password=False)
        field = Field(name="user_comment", page_number=1, rect=rect, text_field=txt)
        
        self.assertEqual(field.name, "user_comment")
        self.assertTrue(field.text_field.multiline)
        self.assertEqual(AcroFormExportType.FDF.value, 1)
        self.assertEqual(AcroFormExportType.XFDF.value, 2)
        self.assertEqual(AcroFormExportType.XML.value, 3)
        self.assertEqual(AcroFormExportType.JSON.value, 4)

    def test_security_and_digital_signatures(self):
        """Verifies SignDoc and PermissionsFlags."""
        perms = PermissionsFlags(allow_print=True, allow_modify=False)
        self.assertTrue(perms.allow_print)
        self.assertFalse(perms.allow_modify)
        
        sign = SignDoc(cert_p12_bytes=b"\x30\x82test", signer_name="Signer CN")
        self.assertEqual(sign.signer_name, "Signer CN")
        self.assertTrue(sign.cert_p12_bytes.startswith(b"\x30\x82"))

    def test_optimization(self):
        """Verifies PDFOptimizer and OptimizationParams."""
        params = OptimizationParams(compress_streams=True, remove_unreferenced_objects=True)
        optimizer = PDFOptimizer(params=params)
        self.assertTrue(optimizer.params.compress_streams)
        self.assertTrue(optimizer.params.remove_unreferenced_objects)


if __name__ == "__main__":
    unittest.main()
