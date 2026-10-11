# pdfcraft-redact

Layer L4 (may use `edit`): redaction, architecture §11.2.

```rust
// Marks are Redact annotations (pdfcraft-annot: Shape::Redact { quads, overlay }).
let marks: Vec<Mark> = marks(&doc);                 // page, areas (user space), fill, overlay text
let report = apply(&mut doc, None)?;                 // redact only (no sanitizing); everything, or Some(&[pages])
let opts = ApplyOptions { sanitize: Sanitize::Recommended, layers: LayerPolicy::default(), allow_signed: false };
let report = apply_with(&mut doc, None, &opts)?;     // redact and sanitize, as one operation
clear_marks(&mut doc, None)?;                        // remove marks without applying
```

Applications call `apply_with`/`apply_with_proof`, whose default `ApplyOptions` use
`Sanitize::Recommended`: besides the content under the marks, it **deletes the document's
bookmarks, named destinations, file attachments, Info dictionary and XMP metadata** (full list
under "Sanitizing as part of apply"). Say so wherever the choice is offered, and pass
`Sanitize::None` or `Sanitize::Only(..)` to keep any of it. `apply` alone sanitizes nothing.

`apply` removes, under every marked area:

- **text**: glyphs are cut out of `Tj`/`TJ`/`'`/`"` and replaced by a `TJ` displacement of the
  same advance, so the rest of the line keeps its exact position. Widths come from `/Widths`,
  `/W`/`/DW` (composite fonts, embedded CMaps with codespace and CID ranges), Type 3 font
  matrices (never approximated: see below). A glyph goes when its box overlaps an area at all
  (touching an edge doesn't count); zero-size text goes when its origin is inside;
- **images**: fully covered → removed; partly covered → a copy with the covered pixels cleared
  (8/16-bit and 1/2/4-bit images, image masks; Flate/LZW/RL/A85/AHx), with every pixel its cell
  touches cleared; `/SMask` and stencil `/Mask` images are cleared into copies too, and a
  colour-key `/Mask`, `/Alternates`, `/SMaskInData`, `/OPI`, `/Metadata` and `/Thumb` are dropped
  from the copy. Codecs PdfCraft can't
  re-encode (DCT, JPX, JBIG2, CCITT) are removed whole (fail-closed); inline images under an
  area are removed;
- **vectors**: covered paths are removed; partly covered paths and shadings are clipped so
  nothing paints inside the areas;
- **form XObjects**: rewritten recursively into new objects (pages sharing the original keep it);
- **annotations** whose rectangle overlaps (with their pop-ups), and **form fields** with a
  widget under a mark;
- the marks themselves, replaced by boxes in their fill colour (and overlay text) drawn into the
  page as a `/PCMark /Redaction` stream.

A verification pass re-reads each redacted page and fails the operation if any glyph or inline
image is still under an area. Content that can't be decoded cleanly (a page content stream with
damaged or truncated data, or `/Contents` that can't be read) makes the operation fail with
`RedactError::Unreadable` rather than leave the unread part unredacted.

The thumbnail (`/Thumb`) and private data (`/PieceInfo`) of every redacted page are always
dropped, with or without sanitizing: a thumbnail is a picture of the page as it was.

A signed document is refused with `RedactError::Signed` before anything changes (redaction
rewrites the file, which would invalidate the signatures), and `apply`/`apply_with` work on a copy
that replaces the caller's document only on success, so an `Err` leaves it as it was.

## Content that can't be placed (fail closed)

When the position of what is painted can't be established, `apply` fails with
`RedactError::Unsupported { page, reason }` and changes nothing the caller keeps (the reasons
never echo page text):

- `UnresolvedFont`: text in a font without usable metrics (missing font, no `/Widths` or a code
  outside them, a composite font without `/DW`/`/W` or with a predefined CMap other than
  Identity, a standard font other than Courier without `/Widths`). The text may extend any
  distance along its line, so this fires when a mark lies on the line it sweeps;
- `VerticalWriting`: `Identity-V` / `/WMode 1` text, the same way, for the column it sweeps;
- `PatternText`, `PatternUnreadable`, `SoftMask`, `Type3Text`: a tiling pattern, soft-mask group
  or Type 3 glyph procedure (page or nested form resources) that shows text or can't be read;
  they aren't placed like page content;
- `HiddenImage`: a tiling pattern or soft-mask group that paints an image (inline, or a `Do` of
  an image), or a Type 3 glyph procedure that does the second;
- `InlineImage`: an inline image whose end can't be told (its `EI` may lie inside the data);
- `FormMatrix`: a form XObject whose `/Matrix` is there but isn't six numbers (a missing or
  malformed `/BBox` only means the pass walks the form whole; the proof can't place its text, so
  the operation then fails with `RedactError::ProofIncomplete`);
- `TooLarge`: more content than the per-page decode budget allows.

Form XObjects nested past the depth cap or referring to themselves are removed whole, as are
forms whose data is damaged (the proof can't read what such a form held, so the operation then
ends in `RedactError::ProofIncomplete` and the caller's document is unchanged). Inline `/ActualText`, `/Alt` and `/E` of marked-content blocks that
lost content are dropped from the stream, and so is the same text in a property list named from
`/Properties` (the list is rewritten without it).

## Bounds, damaged data and atomicity

- Every stream that is read for redaction is decoded strictly and within one shared cap
  (`limits.rs`: 64 MiB per stream, 256 MiB per page's streams and per scope). A damaged or
  truncated stream is `Unreadable` (never a shorter page whose tail goes unexamined), and so is a
  `/Contents` entry that can't be loaded; a reference to nothing at all is the spec's null.
  Scans that can't follow everything (more than 4096 forms or patterns) fail instead of skipping.
- A malformed `/QuadPoints` marks `/Rect` as well (a list cut short keeps its whole quadrilaterals);
  a mark with nothing readable fails with `RedactError::UnreadableMark`.
- Overlay boxes are cut to the page, repeated overlay text is capped, `/DA` numbers must be finite.
- Page thumbnails and `/PieceInfo` of redacted pages are dropped by every apply, sanitizing or not.
- A signed document is refused up front with `RedactError::Signed` (redaction rewrites the file
  and breaks the signatures); `ApplyOptions::allow_signed` rewrites it anyway.
- `apply`, `apply_with` and `apply_with_proof` work on a copy and replace the document only on
  success, so an `Err` leaves it exactly as it was.

## Redaction proof

`apply_with_proof` (and `apply`/`apply_with`, which run the same checks internally) prove the work
with code in `verify` that has its own content reader and its own text placement; it shares neither the
interpreter's font metrics nor its hit test:

```rust
let opts = ApplyOptions { sanitize: Sanitize::Recommended, layers: LayerPolicy::default(), allow_signed: false };
let (report, proof) = apply_with_proof(&mut doc, None, &opts, &ProofOptions::default())?;
assert!(proof.passed(), "{}", proof.to_text());   // always true when it returns Ok
// To look at a proof that fails, take the snapshot yourself and prove the result (see below):
let snap = Snapshot::capture(&doc, None)?;           // before: what is about to go
```

The removed glyph runs are recorded per region (they are the interpreter's own list of what it
removed, so on their own they can't catch text it failed to find; the geometric check of the
page text under each region, made with the proof's own placement of the text, covers that), then
the serialized output is swept for them in
every encoding a PDF can hold a string in (UTF-8, UTF-16BE/LE with and without a BOM, hex strings,
backslash-escaped literals), on: the raw file, every string object, every decoded stream (orphans
included), the text of every page (forms, patterns and annotation appearances too), Info, XMP,
embedded files (PDF attachments are swept recursively) and the revision structure. Images and
paths under each region are re-checked as well. When sanitizing is on (`apply_with`), it runs
first, so document information and XMP that kept removed text are cleaned before the sweep.

The proof fails closed. A survivor fails the operation with `RedactError::Residue`; a stream that
won't decode, an archive attachment, or a region whose page draws text that can't be decoded make
the proof `Unswept`/`Unverifiable`, and the operation then fails with
`RedactError::ProofIncomplete` (a proof returned by `apply_with_proof` always passes). To inspect a
proof that doesn't pass, take a `Snapshot` first, apply, call `set_overlay_streams` and `prove`
yourself. A host that writes the file itself calls `apply_with_snapshot`, which also returns the
`Snapshot`, and sweeps the bytes it is about to write with `Snapshot::prove_saved`. The
rewritten file also gets a new file identifier, and the next save is a full rewrite.

The manifest has one entry per region (page, region, glyph run and code counts, SHA-256
of the page's content after redaction, glyph-carrying operator counts, a status). It holds counts
and a digest of the result, never the removed text, unless `ProofOptions::include_plaintext` is
set. It carries no digest of the original content (a hash of the page as it was would let a
reader confirm a guess at what it said). The glyph run and code counts do disclose how much text
(its length, not its content) sat under each region, so share a proof with that in mind.

What the proof does and does not establish:

- The strings it searches for start from what the interpreter removed, so they alone can't catch a
  glyph the interpreter wrongly kept. A second check places every glyph still shown on the page with
  its own text-state tracking and the font's `/Widths` and fails a region whose area holds the
  centre of one. For a font without `/Widths` the widths are a guess, so a neighbouring glyph can be
  reported, and a region can fail the proof although the interpreter cut the right glyphs (it can
  know widths the proof doesn't use, such as a standard Courier font's). That is by design: a
  proof that can't place the text exactly must not pass.
- A string that also remains in unredacted content is counted, over the whole document, on the
  page text, the raw file and the decoded streams, and a surplus over the copies that remain fails
  the proof; Info, XMP, string objects, annotations and attachments are swept for it regardless.
- The raw file and the decoded streams are searched for strings of six or more characters only
  (three characters match `/Length`, `/MediaBox`, xref offsets and pixel data by chance), and the
  trailer's `/ID` is left out.
  Shorter strings are still looked for on the page text, in string objects, names, Info, XMP and
  text attachments.
- Streams are decoded strictly and searched decoded (page content, forms, appearances, XMP,
  attachments, and fonts, colour profiles and Flate/LZW images that do decode). The pixels of an
  image codec (DCT, JPX, JBIG2, CCITT) are searched as stored bytes only. A font program, ICC
  profile, `/Indexed` lookup table, function sample table or Flate/LZW image that is damaged or
  decodes past `limits::MAX_STREAM` is also searched as stored bytes only and counted as
  `raw only` in the surface report; that is not a failure, because redaction never edits those
  streams. Everything else stays `Unswept` when it can't be decoded in full, and fails the proof:
  page content and form streams, annotation appearances, XMP, attachments, and cross-reference
  and object streams. A damaged tail is never read as a shorter stream. Decoding and
  searching are bounded (`limits::MAX_STREAM` per stream, `limits::MAX_TOTAL_DECODED` and `limits::MAX_WORK` per proof); a document that needs
  more, or nests deeper than the walk allows, is reported as unswept.
- Images drawn from tiling patterns, soft masks or Type3 glyphs can't be placed or cleared, so a page
  that reaches one leaves its regions `Unverifiable`.
- Text that exists only as pixels is not searched.

Callers that drive the proof themselves (`apply_with_proof` does this, with a snapshot taken
inside the operation) call
`Snapshot::set_overlay_streams` with the objects `apply` created for the boxes and labels (those, and
only those, are exempt from the text sweep and the paint check), and `Snapshot::add_removed_strings`
for text removed by other routes (annotation `/Contents`, field values). Signed documents are
serialized for the proof only with `ProofOptions::allow_signed`.

## Remove hidden information / Sanitize

```rust
let found = sanitize::scan(&doc);                          // [(Hidden, count)] per category
sanitize::remove_hidden(&mut doc, &[Hidden::Comments])?;   // chosen categories
sanitize::sanitize(&mut doc)?;                             // all of them
```

Categories: metadata (`/Info`, XMP), file attachments, comments, form fields (flattened so the
values stay visible), hidden text (render modes 3 and 7, or wholly off the page), hidden layers
(content of off optional-content groups, then the groups), bookmarks, links/actions/JavaScript,
and `/PieceInfo` private data. Both make the next save a full rewrite with a new file identifier,
and fail (`RedactError::StructureTooLarge`) rather than skip containers nested too deeply to be
searched.

## Search & Redact patterns

`codes` holds the redaction code sets (U.S. FOIA and U.S. Privacy Act exemptions):
`CodeSet::from_id("foia")?.overlay(&["(b)(6)"])` gives the overlay text for the picked codes.
Custom code sets aren't supported yet.

```rust
let hits = patterns::try_find(Pattern::Iban, &page_chars)?;   // ranges into `page_chars`
let all = patterns::try_find_many(&[Pattern::Email, Pattern::Ipv4], &page_chars)?;
```

Hand-rolled scanners (no regular expressions), linear in the text, so hostile page content can't
cause backtracking blow-ups. Text is folded before matching (Unicode and full-width digits,
dashes, no-break spaces, line breaks as spaces; soft hyphens and zero-width/bidi controls
dropped; a hyphen between digits joins them across a line break) and the ranges come back as
indices into the text you passed, covering any dropped characters. Text over `MAX_INPUT_CHARS`
is an error from `try_find`/`try_find_many`, never truncated; `find` (kept for existing callers)
reports the whole text as one match in that case.

| id | matches |
| --- | --- |
| `email` | `local@domain.tld` |
| `phone` (`phone-us`) | 10 digits, or 11 with a leading 1, grouped by spaces, hyphens, dots or parentheses (or after `+`) |
| `phone-intl` | `+` or `00`, then 8-15 digits |
| `ssn` | `123-45-6789` (or spaces); area 000/666/9xx, group 00 and serial 0000 rejected; nine bare digits only after an `SSN` label |
| `credit-card` | 13-19 digits (15-digit Amex included), grouped by one space or hyphen, Luhn-checked, leading digit 2-6, not one repeated digit |
| `iban` | registry countries, exact length per country, mod-97 checked, optional spaces |
| `ipv4` | four octets 0-255 without leading zeros |
| `ipv6` | full, `::`-compressed and IPv4-tailed forms |
| `uk-postcode` | `SW1A 1AA`-style (uppercase), inward letters validated, `GIR 0AA` |
| `us-zip` | `12345` and `12345-6789`, not amounts or digit chains |
| `date-iso` | `yyyy-mm-dd`, calendar-checked (leap years) |
| `date-us` | `m/d/yy` and `m/d/yyyy`, calendar-checked |
| `date` | broad: numeric forms and month names |

### Sanitizing as part of apply

`apply(doc, pages)` only redacts (plus the page thumbnails and private data above): document
information, XMP, other pages' thumbnails, attachments and the rest stay, and a document with XFA
data is refused. Use `apply_with` to clean those too. `apply_with(doc, pages, &ApplyOptions { sanitize, layers })` also
sanitizes in the same operation, so the result is one document or an error:

- `Sanitize::Recommended` (the default of `ApplyOptions`) cleans around what was removed without
  changing what the reader sees: document information (emptied in place), XMP of the catalog,
  pages and every XObject, thumbnails, `/PieceInfo`, `/AF`, embedded files, `/Collection`,
  bookmarks, named destinations, page labels, threads, document/page scripts and every action
  that is not plain `GoTo` navigation, `/Contents` and `/RC` of annotations on redacted pages,
  multimedia annotations, form values (`/V /DV /TU`) of removed widgets and of the fields above
  them, and XFA data. Layers are held as hidden as they are (`LayerPolicy::ForceOff` switches all
  off); the report names them.
- `Sanitize::Only(categories)` removes those categories whole, as `remove_hidden` does;
  `Sanitize::None` does nothing, and a document with XFA data is then refused
  (`RedactError::Xfa`), since redaction can't reach it.

Both `scan` and the passes work from one plan (the edits are listed without changing anything,
then applied), so `Report::sanitized` counts exactly what was removed. Direct annotation
dictionaries in `/Annots` are given their own objects first. Annotations are removed when their
rectangle or their appearance (`/BBox` mapped by `/Matrix`) overlaps a mark, with their replies
(`/IRT`) and pop-ups. Structure tags lose alternate text on the element and its ancestors, marked
content in rewritten forms (`/Stm`), references to removed annotations (`/OBJR`) and their parent
tree entries. Elements nested 64 levels deep or more lose theirs unconditionally, and a structure
tree with more than two million elements, or a parent tree nested deeper than 64 levels, fails
the operation (`RedactError::StructureTooLarge`) instead of being cleaned in part. Forms replaced by rewritten copies leave the page's resources.

Pattern limits: the scanners read what the text extractor gives them, so text that exists only as
pixels isn't matched. Email addresses are ASCII only (no internationalised local parts or
domains) and the local part may not start with a dot. Month names are English only (`Sept` and
the three-letter forms are accepted). A bare 10-digit phone number or nine-digit SSN is only
matched with grouping or an `SSN` label, to keep order numbers and the like out; 13-19 digit
card numbers need the Luhn check and a plausible leading digit. Phone, postcode and ZIP rules are
US/UK/E.164 only.

Not yet: pattern locales, and re-encoding of DCT images (they are removed instead).
