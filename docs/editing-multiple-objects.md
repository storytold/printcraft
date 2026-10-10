# Move several objects together

In **Edit a PDF → Edit text & images**, select several items on the same page:

- **Shift-click**, **Ctrl-click**, or **Command-click** toggles an item. An ordinary selected
  paragraph, image, or added item becomes the first member when you extend its selection.
- Drag through empty page space to draw a selection rectangle. Items intersecting the rectangle
  are selected; hold one of those modifiers to add to the selection.
- Drag the selection to move all its items by the same distance. Blue outlines preview the move;
  releasing commits it as **one undo step**. Escape cancels the gesture or clears the selection.
- Switching tabs, opening Home or a modal dialog, or losing window focus cancels a drag preview.
  Returning to the document keeps the selection without committing a release from elsewhere.
- Modifier-clicks prefer added items and text over background artwork, so labels remain selectable.
- Clear the selection or double-click an item to return to ordinary text editing and image resizing.
  Finish or discard a text draft before starting a multi-selection.

The supported items are the existing paragraph boxes, raster Image XObjects, whole Form artwork,
and text/images added by PdfCraft. A Form remains one item, including its nested artwork and
resources. Added items appear once in the selection inventory. Selections stay on one page and
contain at most 1,000 items; pages with more than 10,000 recognised objects are refused for group
selection. Content streams must decode completely within 64 MiB each and 128 MiB per page, with
a 1,024-level graphics-state limit and 100,000 content operations. These limits apply to both the
original page and the moved result; unreadable or missing streams are refused. Moving between pages,
arbitrary paths, inline images, group resizing, align/distribute,
and vertical or clipping text are outside this operation's scope. A refusal changes nothing.

Added streams move by changing their outer placement, preserving drawing bytes, fonts, crop paths,
and image transforms even if the page was rotated after the item was added. Their original local
coordinate basis is retained in `/PCAdded/MoveOrigin` so metadata and source placement agree after
repeated moves. Ordinary content updates replace that stream and reset the basis.

Existing text moves without retyping, font substitution, or reflow: its original encoded glyph
strings and kerning are replayed with temporary numeric `TJ` adjustments and text rise. The reader
performs the original glyph advances, so following unselected text keeps its position. Paragraph
indices can change when detection groups lines differently after a move. The UI remaps its own
selection only when the new geometry matches uniquely; other document edits clear stale selection.

## Engine

`Document::editable_objects(page)` returns a mixed inventory with zero-based `ObjectTarget`
references and user-space rectangles. Apply `Edit::MoveObjects { page, objects, offset }` through
`Session`. Targets resolve together before mutation. Permission checks, dirty state, history,
incremental saving, and refreshing use the same transaction as other edits.

`Document::object_move_offset(page, [dx, dy])` converts a displayed-page vector (points, right/down)
to the user-space vector required by the edit, accounting for the crop box, `/Rotate`, and `/UserUnit`.
A frontend with an additional view rotation converts its pointer positions to displayed page space
first. Duplicate, missing, non-finite, oversized, or unsupported targets fail atomically. Engine callers
re-list after changes; the automation generation guard detects stale references before the edit.

## CLI and MCP

Both frontends use the same tools. `object_list` returns a document `generation` and items with
`kind` (`added`, `text`, or `image`), 1-based `index`, `type`, displayed `rect`, and optional `text`.
Treat those references as belonging to that generation and pass it back when moving:

```json
[
  {"tool":"doc_open","args":{"path":"input.pdf"}},
  {"tool":"object_list","args":{"doc":1,"page":1}}
]
```

After choosing references from the returned list, use a move call like this in the same MCP
session. For a CLI script, include `doc_open` in that script before the move:

```json
[
  {"tool":"object_move","args":{
    "doc":1,"page":1,"generation":0,
    "objects":[{"kind":"text","index":1},{"kind":"image","index":1}],
    "offset":[24,18]
  }},
  {"tool":"doc_save","args":{"doc":1,"path":"moved.pdf"}}
]
```

Use the actual generation and references from your document, rather than assuming these example
values. `offset` is in displayed points: positive x moves right, positive y moves down. The optional
generation guard rejects a changed document before interpreting references. Re-list after any edit,
undo, redo, or save; there is no claim that an index persists across generations. Run a script with
`pdfcraft-cli run --script steps.json --root DIR` or call these tools through the opt-in MCP server.

Regression coverage: `crates/edit/src/tests.rs` (glyph bytes, neighbours, split streams, shared Forms,
rotated/cropped/UserUnit pages and refusal), `crates/engine/src/tests.rs` (permissions, history,
rendering and save/reopen), `crates/automation/tests/automation.rs` (generation guards and hostile
references), and `crates/ui-egui/tests/object_selection.rs` (real-shell selection and gestures).
