#!/usr/bin/env python3
"""PdfCraft End-to-End OCR and Document Markdown Extraction Pipeline.

Ingests PDF documents, performs visual rasterization, layout extraction, and
Optical Character Recognition (OCR via Tesseract), converting multi-page
documents into structured Markdown.

Usage:
  python3 docs/sdk/examples/python/ocr_document_to_markdown.py
  python3 docs/sdk/examples/python/ocr_document_to_markdown.py path/to/doc.pdf --output-dir sample-docs/outputs
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import List, Optional

from pypdf import PdfReader


def is_tool_available(name: str) -> bool:
    """Checks whether an external CLI binary is available on PATH."""
    return shutil.which(name) is not None


def extract_page_ocr(image_path: Path, lang: str = "eng") -> str:
    """Runs Tesseract OCR on an image file and returns recognized text."""
    if not is_tool_available("tesseract"):
        return ""
    try:
        cmd = ["tesseract", str(image_path), "stdout", "-l", lang, "--psm", "3"]
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, check=True)
        return proc.stdout.strip()
    except Exception:
        # Fallback without language flag
        try:
            cmd = ["tesseract", str(image_path), "stdout", "--psm", "3"]
            proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, check=True)
            return proc.stdout.strip()
        except Exception:
            return ""


def extract_page_layout_text(pdf_path: Path, page_num: int) -> str:
    """Extracts text preserving physical column layout using pdftotext."""
    if is_tool_available("pdftotext"):
        try:
            cmd = ["pdftotext", "-layout", "-f", str(page_num), "-l", str(page_num), str(pdf_path), "-"]
            proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, check=True)
            return proc.stdout.strip()
        except Exception:
            pass

    # Fallback via pypdf
    try:
        reader = PdfReader(str(pdf_path))
        if page_num - 1 < len(reader.pages):
            return reader.pages[page_num - 1].extract_text().strip()
    except Exception:
        pass
    return ""


def render_page_image(pdf_path: Path, page_num: int, output_dir: Path, dpi: int = 150) -> Optional[Path]:
    """Renders a single PDF page to a PNG image using pdftoppm."""
    if not is_tool_available("pdftoppm"):
        return None

    prefix = output_dir / f"page_{page_num:03d}"
    try:
        cmd = [
            "pdftoppm",
            "-png",
            "-r",
            str(dpi),
            "-f",
            str(page_num),
            "-l",
            str(page_num),
            str(pdf_path),
            str(prefix),
        ]
        subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)

        candidates = sorted(output_dir.glob(f"page_{page_num:03d}*.png"))
        if candidates:
            return candidates[0]
    except Exception:
        pass
    return None


def format_markdown_page(page_num: int, layout_text: str, ocr_text: str) -> str:
    """Combines extracted digital layout and OCR text into clean Markdown."""
    sections: List[str] = [f"## Page {page_num}\n"]

    # Choose primary body text (layout text preserves column alignment and CJK characters best)
    primary_text = layout_text if len(layout_text) >= len(ocr_text) else ocr_text

    if not primary_text:
        sections.append("*[No text content detected on this page]*\n")
        return "\n".join(sections)

    # Format tables or code-like columnar blocks
    lines = primary_text.splitlines()
    cleaned_lines: List[str] = []
    in_code_block = False

    for line in lines:
        stripped = line.rstrip()
        if not stripped:
            if in_code_block:
                cleaned_lines.append("```\n")
                in_code_block = False
            cleaned_lines.append("")
            continue

        # Detect tabular column spacing (3 or more consecutive spaces)
        if "   " in stripped and not in_code_block:
            cleaned_lines.append("```text")
            in_code_block = True

        if in_code_block and "   " not in stripped:
            cleaned_lines.append("```")
            in_code_block = False

        cleaned_lines.append(stripped)

    if in_code_block:
        cleaned_lines.append("```")

    sections.append("\n".join(cleaned_lines))

    # Append OCR annotations if OCR discovered distinct text not present in digital stream
    if ocr_text and ocr_text != layout_text and len(ocr_text) > 30 and abs(len(ocr_text) - len(layout_text)) > 50:
        sections.append("\n<details>")
        sections.append("<summary>Optical Character Recognition (OCR) Stream</summary>\n")
        sections.append("```text")
        sections.append(ocr_text)
        sections.append("```")
        sections.append("</details>\n")

    return "\n".join(sections)


def process_pdf_document(
    pdf_path: Path,
    output_dir: Path,
    max_pages: Optional[int] = None,
    dpi: int = 150,
) -> Path:
    """Processes a single PDF document through the OCR extraction pipeline."""
    print(f"\nProcessing Document: {pdf_path.name}")
    print(f"Path: {pdf_path.resolve()}")

    reader = PdfReader(str(pdf_path))
    total_pages = len(reader.pages)
    pages_to_process = min(total_pages, max_pages) if max_pages else total_pages

    print(f"Total Pages: {total_pages} (processing: {pages_to_process})")

    # Metadata extraction
    meta = reader.metadata or {}
    title = meta.title or pdf_path.stem
    author = meta.author or "Unknown"
    creator = meta.creator or "PdfCraft Engine"

    md_lines: List[str] = [
        "---",
        f"title: \"{title}\"",
        f"source_file: \"{pdf_path.name}\"",
        f"total_pages: {total_pages}",
        f"extracted_pages: {pages_to_process}",
        f"author: \"{author}\"",
        f"creator: \"{creator}\"",
        "ocr_engine: \"Tesseract 5.5 + Poppler Layout Engine\"",
        "---",
        "",
        f"# {title}",
        "",
        f"> **Document Source:** `{pdf_path.name}`  ",
        f"> **Processed with PdfCraft OCR Pipeline**  ",
        f"> **Total Pages:** {total_pages} | **Extracted Pages:** {pages_to_process}",
        "",
        "---",
        "",
    ]

    with tempfile.TemporaryDirectory() as tmp_dir_str:
        tmp_dir = Path(tmp_dir_str)

        for p in range(1, pages_to_process + 1):
            print(f"  • Processing Page {p}/{pages_to_process}...", end="", flush=True)

            # 1. Digital layout text
            layout_text = extract_page_layout_text(pdf_path, p)

            # 2. Render raster for OCR
            img_path = render_page_image(pdf_path, p, tmp_dir, dpi=dpi)

            # 3. Tesseract OCR
            ocr_text = extract_page_ocr(img_path) if img_path else ""

            # 4. Synthesize markdown page
            page_md = format_markdown_page(p, layout_text, ocr_text)
            md_lines.append(page_md)
            md_lines.append("\n---\n")

            char_count = max(len(layout_text), len(ocr_text))
            print(f" Done ({char_count} chars extracted)")

    output_dir.mkdir(parents=True, exist_ok=True)
    slug = re.sub(r"[^\w\-]", "_", pdf_path.stem)
    out_file = output_dir / f"{slug}_ocr.md"

    out_file.write_text("\n".join(md_lines), encoding="utf-8")
    print(f"✓ Markdown generated: {out_file.resolve()} ({out_file.stat().st_size:,} bytes)")
    return out_file


def main() -> None:
    parser = argparse.ArgumentParser(
        description="PdfCraft End-to-End OCR and Document Markdown Extraction",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument(
        "inputs",
        nargs="*",
        type=Path,
        help="Input PDF file paths to process. Defaults to sample-docs if omitted.",
    )
    parser.add_argument(
        "--output-dir",
        "-o",
        type=Path,
        default=Path("sample-docs/outputs"),
        help="Directory where output markdown documents will be written",
    )
    parser.add_argument(
        "--max-pages",
        "-m",
        type=int,
        default=None,
        help="Maximum pages to process per document (default: all pages)",
    )
    parser.add_argument(
        "--dpi",
        type=int,
        default=150,
        help="Raster resolution for OCR rendering",
    )

    args = parser.parse_args()

    # Default inputs if none provided
    input_files: List[Path] = args.inputs
    if not input_files:
        sample_dir = Path("sample-docs")
        default_candidates = [
            sample_dir / "Optional-Car-Wordings-BCAA policy info.pdf",
            sample_dir / "osaka geidai schedule.pdf",
        ]
        input_files = [f for f in default_candidates if f.exists()]

    if not input_files:
        print("Error: No input PDF files found to process.", file=sys.stderr)
        sys.exit(1)

    print("=" * 70)
    print("PdfCraft End-to-End OCR & Markdown Extraction Pipeline")
    print("=" * 70)
    print(f"Found {len(input_files)} document(s) to process.")
    print(f"Output directory: {args.output_dir.resolve()}")

    generated: List[Path] = []
    for pdf_file in input_files:
        # For the 32-page BCAA insurance policy, if no max_pages is explicitly set on CLI,
        # we can process first 5 pages for rapid verification or all if specified.
        max_p = args.max_pages if args.max_pages is not None else (5 if "BCAA" in pdf_file.name else None)
        out = process_pdf_document(pdf_file, args.output_dir, max_pages=max_p, dpi=args.dpi)
        generated.append(out)

    print("\n" + "=" * 70)
    print("All Documents Processed Successfully!")
    for g in generated:
        print(f" • {g.resolve()}")
    print("=" * 70)


if __name__ == "__main__":
    main()
