# pdfcraft-fonts

Layer L2. Font helpers for the appearance streams PdfCraft generates (comments, form fields).

Today it holds what generated appearances need without a font program:

- `helvetica_width`: an approximation of Helvetica's proportions by character class. No metrics
  file or font program from any vendor is bundled; widths are PdfCraft's own estimates, good
  enough for line breaking and alignment, not for typesetting.
- `wrap`: greedy line breaking with that measure (paragraphs on newlines, long words split).
- `win_ansi`: Unicode → WinAnsiEncoding bytes (`?` for characters it can't represent), and
  `literal` to write bytes as a PDF literal string.

It also holds the fonts of the optional [craft-fonts](https://github.com/storytold/craft-fonts)
build input. `build.rs` embeds them as `CRAFT_FONTS` when the build sets
`CRAFT_FONTS_DIR=<checkout>` (and fails if it can't while `CRAFT_FONTS_REQUIRED=1`, as releases
set). Without that variable `CRAFT_FONTS` is empty and everything below copes:

- `ui_japanese_fonts`: the `Jpan` faces for the interface, BIZ UDPGothic first.
- `ui_chinese_fonts`: the `Hans` faces for the interface, in manifest order (without one,
  Chinese characters the Japanese faces lack show the replacement glyph).
- `ui_cjk_fonts(prefer_hans)`: both in fallback order for the UI language (Chinese group first
  in Chinese mode, so one line never mixes faces with different vertical metrics).
- `ui_arabic_fonts`: the `Arab` faces for Arabic-script interface text (file names, document
  titles), in manifest order.
- `ui_telugu_fonts`: the `Telu` faces for Telugu interface text (the Telugu catalog, file names,
  document titles), in manifest order.
- `document_japanese_font` / `japanese_glyph`: the face (Shippori Mincho, then BIZ UDMincho) whose
  outlines become the Type 3 fallback font for Japanese text written into PDFs. Without it,
  `japanese_glyph` returns `GlyphError::NoFont` and the editor reports a clear error.
- `document_japanese_font_for_style` / `japanese_glyph_from`: select and draw a real Japanese
  fallback face for edited text. Sans sources prefer BIZ UDPGothic Regular/Bold; serif sources
  keep Mincho. Type 3 paths and advances both come from that selected face. Missing weights
  fall back to a real Regular face, never synthetic bold; no italic face is supplied. The
  default `document_japanese_font` / `japanese_glyph` APIs still use regular document Mincho.
- `document_arabic_font` / `shape_arabic` / `arabic_glyph`: the first `Arab` face, shaped with
  harfrust (joining forms, ligatures, mark positions) into clusters for Arabic text written into
  PDFs; the editor draws each cluster as one Type 3 glyph with a ToUnicode entry. Without the face
  they return `GlyphError::NoFont` and the editor reports a clear error.
- `document_japanese_fonts_for_style`: every `Jpan` face in that order of preference, so the
  editor can use the first one that has all of the replacement's glyphs (the faces differ in
  coverage, e.g. of Cyrillic).
- `SHIPPORI_MINCHO`: Shippori Mincho's bytes, or `None`.
- `japanese_type3_plan` / `build_type3` (`type3.rs`): the Type 3 fallback font for text written into
  generated appearances (FreeText, Fill & Sign). The plan picks one face as the editor does (the first
  document face with a glyph for every character, else the first) and sorts the characters into the
  ones it can draw and the ones it can't; the fonts and the widths come only from that plan, so
  what is measured is what is drawn. Codes 1–240 per font, at most four fonts (960 characters);
  the rest are reported as missing. `build_type3` hands the glyph streams (Flate), the ToUnicode
  stream and the font descriptor to a callback, which returns the object to store: `doc.add` for
  an indirect object, or the object itself when the caller makes it indirect later (annot does).
  Greek and Cyrillic letters are respaced to their ink width, as in the editor. Without craft-fonts
  `japanese_type3_plan` returns `None` and callers keep writing `?`.

Generated Japanese Type 3 fonts record the fallback face's family and style in an indirect
`FontDescriptor`. `pdf::Metrics::base_font` uses its `FontName` when the Type 3 dictionary has
no `BaseFont`, so text inspection and edit-tool results identify the fallback after save/reopen.
The descriptor does not change glyph programs or replace the Type 3 `FontBBox` metrics when
`Ascent`/`Descent` are omitted (PDF Reference 1.7, tables 5.9 and 5.19). The small web input has
only Gothic Regular, so it cannot preserve bold. No new font assets or changes to that input are
needed.

wasm32 builds embed only BIZ UDPGothic Regular and any `Arab` or `Telu` face, to keep the web build small: the web build
currently has no Chinese face, so Chinese there still shows the replacement glyph.
Font files are never
committed here (`AGENTS.md` §1.4; team members: [craftrules `standards/fonts.md`](https://github.com/storytold/craftrules/blob/main/standards/fonts.md), internal).

The full font subsystem (parsing, shaping, subsetting and embedding) arrives with M2.2/M7.
