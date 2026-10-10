# pdfcraft-xfa

XFA forms (XFA 3.3, the template and layout parts). Layer L3; depends on `pdfcraft-cos` and
`pdfcraft-fonts` only.

A dynamic XFA form is a PDF shell around an XML template (`/AcroForm /XFA`): one placeholder page
("requires Adobe Reader") and `/NeedsRendering true`. Viewers without an XFA engine show the
placeholder. This crate reads the template, lays it out, and writes ordinary pages and AcroForm
fields into the document, so everything else in PdfCraft (rendering, filling, saving, printing,
the automation tools) works on it unchanged. The engine does this when it opens such a form.

## What it does

- **Packets** (`read_packets`): the XDP from one stream or the `(name, stream)` array; UTF-8
  and UTF-16; capped at 64 MB.
- **Template** (`parse`): subforms, areas, fields, draws, exclusion groups, page sets with
  page areas, content areas and media; measurements in mm, cm, in, pt and px; fonts, paragraphs,
  margins, borders (edge order rules), captions with reserves, check-button items, pictures,
  rich text (`exData` XHTML: paragraphs, `br`, bold/italic/underline/size spans, embedded fields),
  inline JPEG, PNG and GIF images, lines, rectangles, occurrences, `breakBefore`/`breakAfter`, overflow
  leaders, `columnWidths` and `colSpan`.
- **Layout** (`layout`): positioned, `tb`, `lr-tb`/`rl-tb`, `table` and `row` layouts; a
  container without a width is as wide as its content; flow across content areas and pages with
  page masters chosen by occurrence; a table's header row repeats after a break; presence
  `hidden` takes no space, `invisible` takes space; repeating subforms get their initial
  instances, in flowed content and in a top-to-bottom container inside positioned content (a
  table in an area) alike; fields that compute the page number or count on layout show the numbers.
- **PDF** (`pdf::write_form`): pages with content streams in the standard 14 fonts (Arial and
  friends → Helvetica, Courier New → Courier, serif faces → Times; nothing is embedded), JPEG
  XObjects, and widgets: text (multi-line, max length, alignment), date (Acrobat's `AFDate`
  format and keystroke actions from the picture clause), check boxes and radio groups (from
  `exclGroup`), push buttons with Reset / Print / Save As / URI actions recognised from their
  scripts. Appearance streams are left empty for the forms layer to generate. Each field keeps
  its SOM path in `/PCSom`; the AcroForm gets `/PCXfaLayout` so a saved form is not laid out
  twice. The XFA packets stay, so Adobe's viewers keep rendering the form from them.

- **Data** (`data`): the `datasets` packet. On layout, a field takes its value from the data
  node at its data path: its SOM path without areas, which are named in SOM expressions but are
  not data scopes (`data_path`, as Adobe's viewers bind) (dates in ISO form, check and radio states by their on values), and a
  repeating subform or row gets as many instances as the data has. `write_datasets` merges the
  AcroForm fields' values (by `/PCSom`, or by the Designer field names of a static form) into
  the existing `xfa:data`: only the bound nodes' text changes and missing nodes are added, so
  unbound data, other namespaces, attributes and comments stay byte for byte. Values it can't
  write (a node holding structured content, absurd SOM indices, a packet that is neither UTF-8
  nor UTF-16) come back as warnings; UTF-16 packets are written back as UTF-16. `read_values`
  goes the other way, so a form filled here shows its values in Adobe's viewers and a form
  filled there shows them here. The engine does both: values on open (recorded in the
  document's warnings), the packet after every form edit.

Layout measures each container once per (node, width): width-less containers would otherwise
be measured exponentially often in their nesting. A measurement budget backs this up.

- **Scripts** (`script`): the live form as scripts see it (`form_tree`: the instance tree with
  values, presence and access, and the `ScriptEvent`s in document order, built within
  `MAX_FORM_NODES` objects and cut short, with `truncated` set, past that; `has_scripts` says
  whether a template has any, and the engine skips all of this for forms without), the presence
  and access overrides scripts make, rows added and removed in the data (`DataOp`s, applied to
  the in-memory `DataNode` while an event runs and written by `write_data_ops` as one rewrite of
  the datasets packet per event), and `rerender`, which lays the form out again from template,
  data and overrides. The scripts themselves run in the `js` crate's XFA object model; the
  engine runs initialize and calculate on open, change, exit, validate and calculate after a
  field changes, and click for buttons.

  Private keys PdfCraft writes (other viewers ignore them):

  | Key | Where | Holds |
  | --- | --- | --- |
  | `/PCSom` | generated fields | the field's SOM path, so values go back to the data |
  | `/PCXfaLayout` | AcroForm | what laying out produced, so a saved form is not laid out again |
  | `/PCXfaOverrides` | AcroForm | `<< /Presence << /som (hidden) … >> /Access << /som (readOnly) … >> >>`: presence and access scripts set, by SOM path (instance 0 of a repeating subform stands for all); applied to the template before every layout, so undo, save and reopen keep them. Adobe's viewers draw from the XFA packets and don't read it; at most 10 000 entries are read or written |
  | `/PCXfaClick` | generated push buttons | `true`: the template has a click script for this button. Such buttons carry no `/A`: XFA JavaScript is not Acrobat JavaScript, so it is never written as a `/S /JavaScript` action other viewers would run. PdfCraft finds the script by the field's `/PCSom` |

## API sketch

```rust
if pdfcraft_xfa::is_dynamic(&doc) {
    let report = pdfcraft_xfa::render_into(&mut doc)?;   // pages, fields, warnings
    for f in pdfcraft_forms::fields(&doc) { pdfcraft_forms::redraw_field(&mut doc, &f.name)?; }
}
let form = pdfcraft_xfa::layout_xml(template_xml)?;       // pages of items, for tests
```

## Deliberately not done (yet)

- **Data binding** is the default one only: explicit `bind ref` expressions, global binding and
  data descriptions are not followed, and no standalone XML or XDP data file is imported or
  exported.
- **Scripting.** JavaScript (boa) and FormCalc (a native interpreter, `pdfcraft_js::formcalc`)
  share one object model, one set of effects and one set of budgets. FormCalc covers the
  language and the common built-ins (arithmetic, logical, string, date/time, financial, unit);
  not locale-aware pictures beyond simple number and date ones, or `Get`/`Post`/`Put` (refused).
  The object model covers what forms commonly use (`xfa.form`
  navigation and SOM resolution, `rawValue`, `presence`, `access`, instance managers,
  `xfa.host` messages, reset, print, focus and URLs, `xfa.layout` page numbers, `xfa.event`);
  not `xfa.template`, data descriptions, `xfa.connectionSet`, `border`/`font`/`ui` properties
  (reads give a sink that accepts writes), `execEvent`, or the change event per keystroke.
  Validate failures show their message and keep the value, as Acrobat does for scripts.
  A hidden instance of a repeating subform hides every instance (overrides apply to the template).
  A repeating subform shows what its data holds once the data holds any instance, else its
  `initial` count, and never fewer than one (`occur min="0"` still shows one row).
  Each script runs in a fresh engine on its own thread; a form with hundreds of calculate
  scripts pays that on every field change (a shared engine per event is the obvious next step).
- **Script limits.** Forms run their scripts on open without being asked, so the engine bounds
  each event: a script's effects (values set again on the same object merge, the last wins;
  10 000 effects, 100 message boxes, 1 000 console lines per script and per event), its loop
  iterations (100 000 per call frame for initialize, calculate and validate, 1 000 000 for
  click and change), 2 000 scripts and 50 relayouts per event, and 3 s (open, changes) or 5 s
  (click) of scripts per event, after which the rest are skipped and reported. Only a click may
  print, save, open a link or move the focus; other events asking are noted in the console.
  What open-time scripts change is noted in the document's XFA warnings. Known gaps: the
  engine's loop limit is per call frame, so loops inside nested calls can run far longer; such
  a script is abandoned after 1 s (3 s for a click) on its thread, which keeps running until the
  engine's own limits end it, and the document's scripts are turned off. Memory a script
  allocates is not capped (the same holds for AcroForm JavaScript).
- **Static XFA forms** (`/NeedsRendering` absent, AcroForm fields present) keep their AcroForm;
  only their data is read and written.
- **Field kinds.** Choice lists become combo boxes (editable with `textEntry`) or list boxes
  (`open="always"`, multi-select with `open="multiSelect"`), with the shown items and the saved
  values of `<items save="1">` as the options; the data holds saved values, several on
  separate lines. Password fields are password text fields. Signature fields are `/Sig` fields
  the app can sign. Image fields show the picture the data or template holds (JPEG, PNG, GIF)
  but a new picture can't be chosen; barcode fields show their value as text, not as bars.
- Missing: `keep` constraints, `subformSet` relations, `rl-tb` is mirrored `lr-tb`, font
  metrics are the approximate Helvetica ones, and line heights are 1.15 × size.
- Acrobat cannot be an oracle here (clean-room rules): layout follows the specification, and
  fidelity has been checked by eye on real government forms, not pixel-compared.
