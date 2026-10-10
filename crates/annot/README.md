# pdfcraft-annot

Comments (annotations), ISO 32000-2 §12.5. Layer L3, depends on `pdfcraft-cos`, `pdfcraft-fonts` and `pdfcraft-model`.

## What it does

- **Builders** for the comment types Acrobat's commenting tools create: sticky note (`Text` +
  `Popup`), highlight, underline, strikethrough, squiggly, rectangle (`Square`), oval (`Circle`),
  line and arrow (`Line` with `/LE`), freehand drawing (`Ink`) and text box (`FreeText`).
- **Appearance streams** (`appearance::build`) generated from the annotation dictionary, so new
  and restyled comments look the same in every viewer. Highlights multiply (`/BM /Multiply`).
  Note icons are PdfCraft's own drawings. Text boxes use standard Helvetica (WinAnsi), with
  line breaking by approximate Helvetica proportions (no font program is bundled). Text WinAnsi
  can't write (Japanese, Czech, Greek, ...) is split into runs: WinAnsi runs stay in Helvetica and
  the rest uses Type 3 fonts (`/PCJp0`...) of craft-fonts glyphs with a ToUnicode map, nothing
  embedded; without craft-fonts those characters stay `?`. `appearance::text_measure` and
  `text_wrap` measure text the way it is drawn, so boxes are sized with them. `set_appearance`
  makes the fonts' streams and descriptors indirect objects.
- **Edits:** reply (`/IRT`), set review status (a `/State` reply, as Acrobat's "Set status"
  does), change text, move, resize, restyle, delete (with pop-up and replies).

## API sketch

```rust
let index = add_annotation(&mut doc, &NewAnnotation { page, shape, style, contents, author }, &meta)?;
add_reply(&mut doc, page, index, "Agreed", "Ada", &meta)?;
set_review_state(&mut doc, page, index, ReviewState::Accepted, "Ada", &meta)?;
move_annotation(&mut doc, page, index, dx, dy, &meta)?;
set_style(&mut doc, page, index, Some(rgb), Some(0.5), Some(2.0), &meta)?;
delete_annotation(&mut doc, page, index)?;
signature_image(&doc, page, index)?; // embedded image XObject of Fill & Sign signature/initials
```

A comment is addressed by `(page, index in /Annots)`, the same pair
`pdfcraft_render::Annotation` reports. `Meta` carries the date and the `/NM` id so edits stay
deterministic; the engine supplies them.

## Fidelity rules

- Unknown keys are kept. Inline annotation dictionaries are promoted to indirect objects only
  when an edit needs a reference to them.
- `set_style` refuses (and changes nothing) when it cannot redraw the comment, e.g. stamps,
  cloudy borders or callouts, instead of leaving a stale appearance.
- Replies get the parent's `/Rect` and an empty appearance: they appear in comment lists but
  never paint a second icon.
- Resizing a stamp changes its `/Rect` and preserves its original appearance and resources;
  viewers scale that appearance into the new rectangle. Locked stamps refuse the edit.
- Natural-size image stamps placed by the engine counterrotate their appearance on rotated
  pages. Image restyling retains that appearance matrix. Explicit rectangles, image signatures
  and PDF-page stamps still use their existing placement behavior; page-rotation support for
  those paths is not complete.

## Not yet

Callouts, polygons and polylines, clouds, carets (insert/replace text), stamps, file
attachments, rich text (`/RC`, `/DS`), and the shared named-appearance cache Acrobat writes.
