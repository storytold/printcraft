//! Document inspection: everything panels need that is not pixels.
//!
//! Page geometry comes from hayro (which resolves inheritance and rotation); the rest comes from
//! the lazy COS reader through a read-only lopdf object adapter. The original lopdf loader
//! remains the compatibility fallback. When neither can inspect a renderable file, panels
//! are empty and `warnings` says why.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro::hayro_syntax::DecryptionError;
use hayro::hayro_syntax::LoadPdfError;
use hayro::hayro_syntax::Pdf;
use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId};
use pdfcraft_cos::page_labels::{MAX_LABEL_BYTES, MAX_LABEL_TOTAL_BYTES, MAX_LABEL_TREE_DEPTH, MAX_LABEL_TREE_WORK, MAX_PREFIX_BYTES, alpha, roman};

use crate::OpenError;
use crate::structure::{LazyStructure, Structure};

/// lopdf decodes object and cross-reference streams while it loads, with no limit unless one is
/// set: a few hundred bytes of nested FlateDecode then inflate to gigabytes. Real object and
/// xref streams are far below this.
const LOAD_STREAM_LIMIT: usize = 256 << 20;

/// At most this many `/State` entries of a set-layer-visibility action are read.
const MAX_LAYER_STATE: usize = 1024;

/// At most this many layers are read from all of `/RBGroups` together.
const MAX_LAYER_GROUP_ENTRIES: usize = 4096;

/// `DocInfo::outline` holds the first this many bookmarks in outline order. Opening a document
/// reads only this window; the rest are listed by page through `pdfcraft_organize`.
const OUTLINE_WINDOW: usize = 10_000;

/// Deepest level `DocInfo::outline` nests (the top level is 0); deeper bookmarks are left out.
const OUTLINE_DEPTH: usize = 32;

fn load_options(password: Option<&str>) -> LoadOptions {
    LoadOptions { password: password.map(str::to_owned), max_decompressed_size: Some(LOAD_STREAM_LIMIT), ..LoadOptions::default() }
}

#[derive(Clone, Debug, Default)]
pub struct DocInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub pdf_version: String,
    pub file_size: usize,
    pub encrypted: bool,
    pub tagged: bool,
    pub has_javascript: bool,
    /// The form is (also) an XFA form, which isn't read yet (`form.xfa-*` in the parity list).
    pub xfa: Option<Xfa>,
    pub pages: Vec<PageInfo>,
    pub outline: Vec<OutlineItem>,
    /// The outline has bookmarks that `outline` leaves out (see `OUTLINE_WINDOW`, `OUTLINE_DEPTH`).
    pub outline_more: bool,
    pub annotations: Vec<Annotation>,
    pub fields: Vec<Field>,
    pub links: Vec<Link>,
    pub layers: Vec<Layer>,
    /// Radio-button layer groups (`/OCProperties /D /RBGroups`): turning one layer of a group on
    /// turns the others off.
    pub layer_groups: Vec<Vec<(u32, u16)>>,
    pub fonts: Vec<FontInfo>,
    pub attachments: Vec<Attachment>,
    pub warnings: Vec<String>,
}

/// What kind of XFA form a document has (`/AcroForm /XFA`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Xfa {
    /// The pages and fields are ordinary PDF; the XFA packets describe the same form (and hold
    /// its data, which Acrobat shows in preference to the fields).
    Static,
    /// The form is laid out from the XFA packets at open (`/NeedsRendering`, or no fields): the
    /// PDF pages are only a placeholder.
    Dynamic,
}

#[derive(Clone, Debug)]
pub struct PageInfo {
    /// Displayed size in points, after `/Rotate` and `/UserUnit`.
    pub width: f32,
    pub height: f32,
    /// Page label (`/PageLabels`), falling back to the 1-based page number.
    pub label: String,
    /// Effective crop box in user space [x0, y0, x1, y1] (the visible region).
    pub crop: [f32; 4],
    /// Clockwise page rotation in degrees (0, 90, 180, 270).
    pub rotation: u16,
}

impl PageInfo {
    /// A point in view space (points, y down, after `/Rotate`; what the text layer uses) →
    /// PDF user space.
    pub fn view_to_user(&self, x: f32, y: f32) -> [f32; 2] {
        let [cx0, cy0, cx1, cy1] = self.crop;
        let (a, b) = (x / self.width.max(1e-3), y / self.height.max(1e-3));
        let (u, v) = match self.rotation {
            90 => (b, 1.0 - a),
            180 => (1.0 - a, 1.0 - b),
            270 => (1.0 - b, a),
            _ => (a, b),
        };
        [cx0 + u * (cx1 - cx0), cy1 - v * (cy1 - cy0)]
    }

    /// PDF user space → view space (the inverse of [`Self::view_to_user`]).
    pub fn user_to_view(&self, x: f32, y: f32) -> [f32; 2] {
        let [cx0, cy0, cx1, cy1] = self.crop;
        let (u, v) = ((x - cx0) / (cx1 - cx0).max(1e-3), (cy1 - y) / (cy1 - cy0).max(1e-3));
        let (a, b) = match self.rotation {
            90 => (1.0 - v, u),
            180 => (1.0 - u, 1.0 - v),
            270 => (v, 1.0 - u),
            _ => (u, v),
        };
        [a * self.width, b * self.height]
    }

    /// A destination's user-space point (each coordinate possibly unspecified) → the same point
    /// as a fraction of the displayed page `[x, y]` (0..1 from its top-left corner, y down),
    /// after `/Rotate` and a further clockwise `view_rotation` (0, 90, 180 or 270). On a page
    /// turned a quarter, the user `x` decides the displayed `y` and vice versa, so each output
    /// is `None` when the input it comes from is (or is not finite). Points off the page are
    /// clamped to its edge.
    pub fn dest_fraction(&self, x: Option<f32>, y: Option<f32>, view_rotation: u16) -> [Option<f32>; 2] {
        let [cx0, cy0, cx1, cy1] = self.crop;
        let frac = |v: f32| if v.is_finite() { Some(v.clamp(0.0, 1.0)) } else { None };
        // Unrotated fractions across (u, from the crop's left) and down (v, from its top).
        let u = x.and_then(|x| frac((x - cx0) / (cx1 - cx0).max(1e-3)));
        let v = y.and_then(|y| frac((cy1 - y) / (cy1 - cy0).max(1e-3)));
        let flip = |f: Option<f32>| f.map(|f| 1.0 - f);
        match (u32::from(self.rotation) + u32::from(view_rotation)) % 360 {
            90 => [flip(v), u],
            180 => [flip(u), flip(v)],
            270 => [v, flip(u)],
            _ => [u, v],
        }
    }

    /// A view-space rectangle → the normalized user-space rectangle covering the same area of the
    /// page (`[x0, y0, x1, y1]`, `x0 <= x1`, `y0 <= y1`). Under `/Rotate` the corners swap roles,
    /// so a view rectangle's top-left is not in general the user rectangle's `[x0, y1]`.
    pub fn view_rect_to_user(&self, r: [f32; 4]) -> [f32; 4] {
        let (a, b) = (self.view_to_user(r[0], r[1]), self.view_to_user(r[2], r[3]));
        [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
    }

    /// A view-space rectangle as a text-markup quad in user space: top-left, top-right,
    /// bottom-left, bottom-right as read on screen.
    pub fn view_rect_to_quad(&self, r: [f32; 4]) -> [f64; 8] {
        let mut q = [0.0; 8];
        for (i, (x, y)) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])].into_iter().enumerate() {
            let [ux, uy] = self.view_to_user(x, y);
            q[2 * i] = ux as f64;
            q[2 * i + 1] = uy as f64;
        }
        q
    }
}

/// A clickable link annotation.
#[derive(Clone, Debug)]
pub struct Link {
    pub page: usize,
    pub rect: [f32; 4],
    pub target: LinkTarget,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    /// A destination in this document: the 0-based page and where on it the view goes.
    Page(usize, DestView),
    Uri(String),
    /// A set-layer-visibility action (`SetOCGState`, ISO 32000-2 §12.6.4.13): each change in
    /// order, naming the layer by its optional content group. With `preserve_rb`, a layer turned
    /// on turns off the other layers of its radio-button groups.
    SetLayers {
        changes: Vec<(LayerOp, (u32, u16))>,
        preserve_rb: bool,
    },
    Other(String),
}

/// What a set-layer-visibility action does to a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerOp {
    On,
    Off,
    Toggle,
}

/// Where an explicit destination (ISO 32000-2 §12.3.2.2, Table 149) places its page in the
/// window. Coordinates are PDF user space; `None` is a `null` (or unusable) operand, which keeps
/// that part of the view as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DestView {
    /// The top of the page at the current zoom: a destination with no position, or one whose
    /// type is missing or unknown.
    #[default]
    Top,
    /// `/XYZ left top zoom`: the point (left, top) at the window's upper-left corner, at `zoom`
    /// (1.0 = 100%; 0 or null keeps the current zoom).
    Xyz { left: Option<f32>, top: Option<f32>, zoom: Option<f32> },
    /// `/Fit`, and `/FitB` (fit the page's content box; read as the whole page).
    Fit,
    /// `/FitH top`, and `/FitBH`: fit the width, with `top` at the window's top edge.
    FitH { top: Option<f32> },
    /// `/FitV left`, and `/FitBV`: fit the height, with `left` at the window's left edge.
    FitV { left: Option<f32> },
    /// `/FitR left bottom right top`: fit the rectangle `[x0, y0, x1, y1]` (normalized).
    FitR { rect: [f32; 4] },
}

impl DestView {
    /// The view an explicit destination array asks for, from its elements after the page
    /// (`/XYZ left top zoom`, `/FitH top`, …). Hostile input never fails: a missing or unknown
    /// type, or a `/FitR` without four usable numbers, is [`DestView::Top`]; a non-numeric,
    /// non-finite or missing operand is `None`; a zoom that is not positive is `None`.
    pub fn from_operands(kind: Option<&[u8]>, operands: &[Option<f32>]) -> Self {
        let n = |i: usize| operands.get(i).copied().flatten().filter(|v| v.is_finite());
        match kind {
            Some(b"XYZ") => Self::Xyz { left: n(0), top: n(1), zoom: n(2).filter(|z| *z > 0.0) },
            Some(b"Fit" | b"FitB") => Self::Fit,
            Some(b"FitH" | b"FitBH") => Self::FitH { top: n(0) },
            Some(b"FitV" | b"FitBV") => Self::FitV { left: n(0) },
            Some(b"FitR") => match (n(0), n(1), n(2), n(3)) {
                (Some(x0), Some(y0), Some(x1), Some(y1)) => Self::FitR { rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)] },
                _ => Self::Top,
            },
            _ => Self::Top,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<usize>,
    /// Where on `page` the bookmark goes ([`DestView::Top`] for a bookmark without a page).
    pub view: DestView,
    pub children: Vec<OutlineItem>,
    pub open: bool,
}

#[derive(Clone, Debug)]
pub struct Annotation {
    pub page: usize,
    pub subtype: String,
    pub author: Option<String>,
    pub contents: Option<String>,
    pub modified: Option<String>,
    /// `/NM` of this annotation, and of the one it replies to (`/IRT`), for threading.
    pub name: Option<String>,
    pub in_reply_to: Option<String>,
    /// Rect in PDF user space: [x0, y0, x1, y1].
    pub rect: [f32; 4],
    pub color: Option<[f32; 3]>,
    /// Position in the page's `/Annots` (how edits address the comment).
    pub index: usize,
    /// For a status reply (`/State`, Acrobat's "Set status"): the state it sets.
    pub state: Option<String>,
    /// Text markup quadrilaterals (`/QuadPoints`), 8 numbers each.
    pub quads: Vec<[f32; 8]>,
    /// The Locked flag (`/F` bit 8): the comment can't be moved, resized, restyled or deleted.
    pub locked: bool,
    /// `/IT`, the intent: `FreeTextCallout`, `PolygonCloud`, `FreeTextTypeWriter`…
    pub intent: Option<String>,
    /// A Fill & Sign mark: typed text, a check, cross, dot or line, or a typed, drawn or picture
    /// signature or initials (the rules of `pdfcraft_annot::is_fill_sign`).
    pub fill_sign: bool,
}

impl Annotation {
    /// A checkmark reply (`/StateModel /Marked`): "Marked" or "Unmarked" rather than a review status.
    pub fn is_mark(&self) -> bool {
        matches!(self.state.as_deref(), Some("Marked" | "Unmarked"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    CheckBox,
    Radio,
    PushButton,
    Combo,
    List,
    Signature,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    pub value: Option<String>,
    pub page: Option<usize>,
    pub tooltip: Option<String>,
    pub has_actions: bool,
    /// Rect of the first widget, in user space.
    pub rect: Option<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct Layer {
    /// Object number and generation of the optional content group.
    pub id: (u32, u16),
    pub name: String,
    pub visible: bool,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub description: Option<String>,
    pub size: Option<usize>,
    pub source: AttachmentSource,
}

/// Where an attachment lives (so its bytes can be fetched on demand).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentSource {
    /// Document-level: `/Names /EmbeddedFiles` entry with this key.
    Document { key: String },
    /// A FileAttachment annotation: page index and position in the page's `/Annots`.
    Annotation { page: usize, index: usize },
}

/// A font used by the document (Document Properties ▸ Fonts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontInfo {
    /// BaseFont without the subset tag.
    pub name: String,
    /// Type1, TrueType, Type0, Type3, MMType1, CIDFontType0/2 …
    pub kind: String,
    pub embedded: bool,
    pub subset: bool,
    pub encoding: Option<String>,
}

/// Inspect a document. Fails only if the renderer itself cannot open the file (or needs a
/// password). Never panics: parser crashes are caught and reported.
pub fn inspect(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Result<DocInfo, OpenError> {
    let pdf = catch_unwind(AssertUnwindSafe(|| Pdf::new_with_password(bytes.clone(), password.unwrap_or(""))))
        .map_err(|p| OpenError::Invalid(format!("the parser crashed: {}", crate::raster::panic_message(&p))))?
        .map_err(|e| match e {
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
                if password.is_some() {
                    OpenError::WrongPassword
                } else {
                    OpenError::NeedsPassword
                }
            }
            LoadPdfError::Decryption(other) => OpenError::Unsupported(format!("encryption not supported: {other:?}")),
            LoadPdfError::Invalid => OpenError::Invalid("no readable page tree or cross-reference data was found".into()),
        })?;
    let mut info = DocInfo { file_size: bytes.len(), pdf_version: version_label(&format!("{:?}", pdf.version())), ..Default::default() };
    let pages = catch_unwind(AssertUnwindSafe(|| {
        let mut out = Vec::new();
        for (i, page) in pdf.pages().iter().enumerate() {
            let (w, h) = page.render_dimensions();
            let c = page.intersected_crop_box();
            let rotation = match page.rotation() {
                hayro::hayro_syntax::page::Rotation::None => 0,
                hayro::hayro_syntax::page::Rotation::Horizontal => 90,
                hayro::hayro_syntax::page::Rotation::Flipped => 180,
                hayro::hayro_syntax::page::Rotation::FlippedHorizontal => 270,
            };
            let crop = [c.x0 as f32, c.y0 as f32, c.x1 as f32, c.y1 as f32];
            // Degenerate boxes still get a usable placeholder size so layout never divides by zero.
            let (w, h) = if w.is_finite() && h.is_finite() && w >= 1.0 && h >= 1.0 { (w, h) } else { (612.0, 792.0) };
            out.push(PageInfo { width: w, height: h, label: (i + 1).to_string(), crop, rotation });
        }
        out
    }));
    match pages {
        Ok(p) => info.pages = p,
        Err(p) => return Err(OpenError::Invalid(format!("the page tree could not be read: {}", crate::raster::panic_message(&p)))),
    }
    let options = load_options(password);
    // Geometry no longer needs the renderer parser. Do not overlap its allocations with
    // structural inspection, including the compatibility fallback for damaged files.
    drop(pdf);
    inspect_structure(&mut info, |tmp| {
        // The lazy reader first; lopdf's repair/tolerance path when it can't expose the same
        // page tree or an object a panel needs. No partial result is published.
        let lazy = catch_unwind(AssertUnwindSafe(|| {
            let doc = LazyStructure::new(bytes.clone(), password).ok()?;
            let mut t = DocInfo { pages: tmp.pages.clone(), ..Default::default() };
            let inspector = Inspector::new(&doc);
            if inspector.page_index.len() != t.pages.len() {
                return None;
            }
            inspector.fill(&mut t);
            (!doc.failed()).then_some(t)
        }))
        .ok()
        .flatten();
        if let Some(t) = lazy {
            *tmp = t;
            return Ok(());
        }
        let doc = Document::load_mem_with_options(&bytes, options).map_err(|e| e.to_string())?;
        Inspector::new(&doc).fill(tmp);
        Ok(())
    });
    if info.pages.is_empty() {
        return Err(OpenError::Invalid("the document has no pages".into()));
    }
    Ok(info)
}

// Commit auxiliary metadata only after inspection succeeds; renderer geometry is the fallback.
fn inspect_structure(info: &mut DocInfo, fill: impl FnOnce(&mut DocInfo) -> Result<(), String>) {
    let structure = catch_unwind(AssertUnwindSafe(|| {
        let mut tmp = DocInfo { pages: info.pages.clone(), ..Default::default() };
        fill(&mut tmp)?;
        Ok::<_, String>(tmp)
    }));
    match structure {
        Ok(Ok(mut filled)) => {
            filled.file_size = info.file_size;
            filled.pdf_version = std::mem::take(&mut info.pdf_version);
            *info = filled;
        }
        Ok(Err(e)) => info.warnings.push(format!("Some document structure (bookmarks, comments, fields) could not be read: {e}")),
        Err(p) => {
            info.warnings.push(format!("Document structure inspection crashed and was skipped: {}", crate::raster::panic_message(&p)));
        }
    }
}

struct Inspector<'a> {
    doc: &'a dyn Structure,
    page_index: HashMap<ObjectId, usize>,
    /// Named destinations (`/Names /Dests` tree), keyed by raw string bytes, built once.
    /// Looking each name up by walking the tree was O(links × names): minutes on manuals.
    named: HashMap<Vec<u8>, &'a Object>,
}

impl<'a> Inspector<'a> {
    fn new(doc: &'a dyn Structure) -> Self {
        let page_index = doc.get_pages().into_iter().map(|(n, id)| (id, n as usize - 1)).collect();
        let mut me = Self { doc, page_index, named: HashMap::new() };
        let mut named = HashMap::new();
        if let Some(tree) =
            doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).and_then(|o| me.dict(o)).and_then(|n| n.get(b"Dests").ok()).and_then(|o| me.dict(o))
        {
            me.name_tree_raw(tree, &mut HashSet::new(), 0, &mut named);
        }
        me.named = named;
        me
    }

    fn fill(&self, info: &mut DocInfo) {
        // lopdf drops /Encrypt from the trailer once it has decrypted the file.
        info.encrypted = self.doc.encrypted();
        if let Some(d) = self.doc.trailer().get(b"Info").ok().and_then(|o| self.dict(o)) {
            info.title = self.text(d, b"Title");
            info.author = self.text(d, b"Author");
            info.subject = self.text(d, b"Subject");
            info.keywords = self.text(d, b"Keywords");
            info.creator = self.text(d, b"Creator");
            info.producer = self.text(d, b"Producer");
        }
        let Ok(catalog) = self.doc.catalog() else { return };
        info.tagged = catalog
            .get(b"MarkInfo")
            .ok()
            .and_then(|o| self.dict(o))
            .and_then(|m| m.get(b"Marked").ok())
            .and_then(|o| o.as_bool().ok())
            .unwrap_or(false);
        match self.page_labels(catalog, &mut info.pages) {
            Ok(notes) => info.warnings.extend(notes.into_iter().map(|note| format!("Page labels: {note}."))),
            Err(reason) => info.warnings.push(format!("Page labels were skipped: {reason}; using physical page numbers.")),
        }
        if let Some(first) = catalog.get(b"Outlines").ok().and_then(|o| self.dict(o)).and_then(|d| d.get(b"First").ok()) {
            (info.outline, info.outline_more) = self.outline_window(first);
        }
        self.annotations(info);
        let mut field_seen = HashSet::new();
        if let Some(form) = catalog.get(b"AcroForm").ok().and_then(|o| self.dict(o))
            && let Ok(fields) = form.get(b"Fields").and_then(|o| self.resolve(o).as_array())
        {
            for f in fields {
                self.field(f, None, &mut info.fields, &mut field_seen, 0);
            }
        }
        // Widgets no `/Fields` entry reaches are fields too (Acrobat and the browsers fill them);
        // each is adopted through its topmost unlisted `/Parent`, so a split field stays one.
        let mut pages: Vec<_> = self.page_index.iter().collect();
        pages.sort_by_key(|(_, i)| **i);
        for (&pid, _) in pages {
            let Ok(page_dict) = self.doc.get_dictionary(pid) else { continue };
            let Ok(Object::Array(annots)) = page_dict.get(b"Annots").map(|o| self.resolve(o)) else { continue };
            for a in annots {
                let Some(d) = self.dict(a) else { continue };
                if self.name(d, b"Subtype").as_deref() != Some("Widget")
                    || (d.get(b"FT").is_err() && d.get(b"T").is_err() && d.get(b"Parent").is_err())
                {
                    continue;
                }
                if let Object::Reference(id) = a
                    && let Some(root) = self.unlisted_field(*id, &field_seen)
                {
                    self.field(&Object::Reference(root), None, &mut info.fields, &mut field_seen, 0);
                }
            }
        }
        info.has_javascript |= info.fields.iter().any(|f| f.has_actions);
        let needs_rendering = catalog.get(b"NeedsRendering").ok().and_then(|o| self.resolve(o).as_bool().ok()).unwrap_or(false);
        if catalog.get(b"AcroForm").ok().and_then(|o| self.dict(o)).is_some_and(|form| form.get(b"XFA").is_ok()) {
            info.xfa = Some(if needs_rendering || info.fields.is_empty() { Xfa::Dynamic } else { Xfa::Static });
        }
        self.layers(catalog, &mut info.layers);
        info.layer_groups = self.layer_groups(catalog);
        self.fonts(&mut info.fonts);
        if let Some(names) = catalog.get(b"Names").ok().and_then(|o| self.dict(o)) {
            if let Some(ef) = names.get(b"EmbeddedFiles").ok().and_then(|o| self.dict(o)) {
                let mut seen = HashSet::new();
                for (name, spec) in self.name_tree(ef, &mut seen, 0) {
                    info.attachments.push(self.attachment(name.clone(), spec, AttachmentSource::Document { key: name }));
                }
            }
            info.has_javascript |= names.get(b"JavaScript").is_ok();
        }
    }

    // ── object helpers ──────────────────────────────────────────────────────────────────────

    fn resolve(&self, o: &'a Object) -> &'a Object {
        self.doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
    }

    fn dict(&self, o: &'a Object) -> Option<&'a Dictionary> {
        match self.resolve(o) {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    fn text(&self, d: &Dictionary, key: &[u8]) -> Option<String> {
        let o = self.resolve(d.get(key).ok()?);
        let s = match o {
            Object::String(bytes, _) => text_string(bytes),
            Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
            _ => return None,
        };
        let s = s.trim_matches('\0').trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    fn name(&self, d: &Dictionary, key: &[u8]) -> Option<String> {
        self.resolve(d.get(key).ok()?).as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned())
    }

    /// Whether an annotation is a Fill & Sign mark. Keep in step with
    /// `pdfcraft_annot::is_fill_sign`, which the engine's comment list uses.
    fn is_fill_sign(&self, d: &Dictionary, subtype: &str) -> bool {
        let yes = |key: &[u8]| matches!(d.get(key).map(|o| self.resolve(o)), Ok(Object::Boolean(true)));
        if yes(b"PCFillSign") {
            return true;
        }
        match subtype {
            "FreeText" => self.name(d, b"IT").as_deref() == Some("FreeTextTypeWriter"),
            "Stamp" => match self.name(d, b"Name").as_deref() {
                Some("PCCheck" | "PCCross" | "PCDot" | "PCLine" | "PCTypedSignature") => true,
                Some("PCCustomSignature" | "PCCustomInitials") => yes(b"PCPictureImage"),
                _ => false,
            },
            "Ink" => self.text(d, b"Subj").as_deref() == Some("Signature"),
            _ => false,
        }
    }

    fn page_of(&self, o: &Object) -> Option<usize> {
        match o {
            Object::Reference(id) => self.page_index.get(id).copied(),
            // Some producers write page *numbers* in remote-style destinations.
            Object::Integer(n) => usize::try_from(*n).ok(),
            _ => None,
        }
    }

    // ── destinations ────────────────────────────────────────────────────────────────────────

    /// The page and view of a destination: an explicit array (`[page /XYZ left top zoom]`, …),
    /// a dictionary with `/D`, or a named destination that resolves to one.
    fn dest(&self, dest: &Object, depth: u32) -> Option<(usize, DestView)> {
        if depth > 8 {
            return None;
        }
        match self.resolve(dest) {
            Object::Array(a) => {
                let page = a.first().and_then(|p| self.page_of(p))?;
                let kind = a.get(1).map(|k| self.resolve(k)).and_then(|k| k.as_name().ok());
                let operands: Vec<Option<f32>> = a.iter().skip(2).take(4).map(|o| self.resolve(o).as_float().ok()).collect();
                Some((page, DestView::from_operands(kind, &operands)))
            }
            Object::Dictionary(d) => d.get(b"D").ok().and_then(|d| self.dest(d, depth + 1)),
            Object::String(key, _) | Object::Name(key) => self.named_dest(key).and_then(|d| self.dest(d, depth + 1)),
            _ => None,
        }
    }

    fn named_dest(&self, key: &[u8]) -> Option<&'a Object> {
        let catalog = self.doc.catalog().ok()?;
        if let Some(v) = self.named.get(key) {
            return Some(v);
        }
        // PDF 1.1 style /Dests dictionary.
        catalog.get(b"Dests").ok().and_then(|o| self.dict(o)).and_then(|d| d.get(key).ok())
    }

    /// A name tree's leaves keyed by their raw key bytes (first occurrence wins).
    fn name_tree_raw(&self, node: &'a Dictionary, seen: &mut HashSet<*const Dictionary>, depth: u32, out: &mut HashMap<Vec<u8>, &'a Object>) {
        if depth > 32 || !seen.insert(node as *const _) {
            return;
        }
        if let Ok(Object::Array(pairs)) = node.get(b"Names").map(|o| self.resolve(o)) {
            for pair in pairs.chunks(2) {
                if let [k, v] = pair {
                    let key = match self.resolve(k) {
                        Object::String(s, _) => s.clone(),
                        Object::Name(n) => n.clone(),
                        _ => continue,
                    };
                    out.entry(key).or_insert(v);
                }
            }
        }
        if let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| self.resolve(o)) {
            for k in kids {
                if let Some(d) = self.dict(k) {
                    self.name_tree_raw(d, seen, depth + 1, out);
                }
            }
        }
    }

    fn name_tree(&self, node: &'a Dictionary, seen: &mut HashSet<*const Dictionary>, depth: u32) -> Vec<(String, &'a Object)> {
        let mut out = Vec::new();
        if depth > 32 || !seen.insert(node as *const _) {
            return out;
        }
        if let Ok(Object::Array(pairs)) = node.get(b"Names").map(|o| self.resolve(o)) {
            for pair in pairs.chunks(2) {
                if let [k, v] = pair {
                    let key = match self.resolve(k) {
                        Object::String(bytes, _) => text_string(bytes),
                        _ => String::new(),
                    };
                    out.push((key, v));
                }
            }
        }
        if let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| self.resolve(o)) {
            for k in kids {
                if let Some(d) = self.dict(k) {
                    out.extend(self.name_tree(d, seen, depth + 1));
                }
            }
        }
        out
    }

    // ── outline ─────────────────────────────────────────────────────────────────────────────

    /// The window `DocInfo::outline` keeps: the first `OUTLINE_WINDOW` bookmarks in outline order,
    /// nested no deeper than `OUTLINE_DEPTH`, and whether any bookmark was left out. Iterative, and
    /// each bookmark is read once, so the work and memory stay within the window.
    fn outline_window(&self, first: &'a Object) -> (Vec<OutlineItem>, bool) {
        let mut seen = HashSet::new();
        // The listed bookmarks in outline order, with their depth.
        let mut listed: Vec<(usize, OutlineItem)> = Vec::new();
        // One entry per open level: its next sibling, or `None` once that run of siblings has ended.
        let mut levels: Vec<Option<&'a Object>> = vec![Some(first)];
        let mut more = false;
        while let Some(depth) = levels.len().checked_sub(1) {
            let Some(slot) = levels.get_mut(depth) else { break };
            let Some(o) = slot.take() else {
                levels.pop();
                continue;
            };
            if let Object::Reference(id) = o
                && !seen.insert(*id)
            {
                continue; // cycle: this run of siblings ends here
            }
            let Some(d) = self.dict(o) else { continue };
            if listed.len() >= OUTLINE_WINDOW {
                more = true;
                break;
            }
            *slot = d.get(b"Next").ok();
            let dest = d
                .get(b"Dest")
                .ok()
                .and_then(|dest| self.dest(dest, 0))
                .or_else(|| d.get(b"A").ok().and_then(|a| self.dict(a)).and_then(|a| a.get(b"D").ok()).and_then(|dest| self.dest(dest, 0)));
            let (page, view) = dest.map_or((None, DestView::Top), |(p, v)| (Some(p), v));
            match d.get(b"First").ok() {
                Some(child) if depth < OUTLINE_DEPTH => levels.push(Some(child)),
                Some(_) => more = true,
                None => {}
            }
            let open = d.get(b"Count").ok().and_then(|c| c.as_i64().ok()).is_some_and(|c| c > 0);
            let title = self.text(d, b"Title").unwrap_or_default();
            listed.push((depth, OutlineItem { title, page, view, children: Vec::new(), open }));
        }
        (nest_outline(listed), more)
    }

    // ── page labels (ISO 32000-2 §12.4.2) ───────────────────────────────────────────────────

    fn label_object(&self, object: &'a Object) -> Result<&'a Object, &'static str> {
        self.doc.dereference(object).map(|(_, o)| o).map_err(|_| "a number tree reference is missing or cyclic")
    }

    fn label_dict(&self, object: &'a Object) -> Result<Option<&'a Dictionary>, &'static str> {
        Ok(match self.label_object(object)? {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        })
    }

    /// Apply `/PageLabels` to `pages`. `Err`: the tree is unusable and no label is applied.
    /// `Ok(notes)`: labels applied, with what was repaired on the way (a duplicated start page,
    /// a range whose labels are too long to show).
    fn page_labels(&self, catalog: &Dictionary, pages: &mut [PageInfo]) -> Result<Vec<String>, &'static str> {
        let mut notes = Vec::new();
        let Ok(tree) = catalog.get(b"PageLabels") else { return Ok(notes) };
        let Some(tree) = self.label_dict(tree)? else { return Ok(notes) };
        let mut ranges: BTreeMap<usize, &Dictionary> = BTreeMap::new();
        let mut stack = vec![(tree, 0usize)];
        let mut seen = HashSet::new();
        let mut left = MAX_LABEL_TREE_WORK;
        while let Some((node, depth)) = stack.pop() {
            label_work(&mut left, 1)?;
            if depth > MAX_LABEL_TREE_DEPTH {
                return Err("the number tree is too deep");
            }
            if !seen.insert(node as *const Dictionary) {
                return Err("the number tree repeats a node or contains a cycle");
            }
            if let Ok(nums) = node.get(b"Nums")
                && let Object::Array(nums) = self.label_object(nums)?
            {
                label_work(&mut left, nums.len().div_ceil(2))?;
                for pair in nums.chunks(2) {
                    if let [k, v] = pair
                        && let (Ok(start), Some(d)) = (self.label_object(k)?.as_i64(), self.label_dict(v)?)
                        && let Ok(start) = usize::try_from(start)
                        && ranges.insert(start, d).is_some()
                    {
                        // The later range wins, as in readers that overwrite.
                        notes.push(format!("a range starts on page {} twice; the later one is used", start.saturating_add(1)));
                    }
                }
            }
            if let Ok(kids) = node.get(b"Kids")
                && let Object::Array(kids) = self.label_object(kids)?
            {
                label_work(&mut left, kids.len())?;
                if kids.len() > MAX_LABEL_TREE_WORK.saturating_sub(stack.len()) {
                    return Err("the number tree has too many pending nodes");
                }
                if !kids.is_empty() && depth == MAX_LABEL_TREE_DEPTH {
                    return Err("the number tree is too deep");
                }
                for kid in kids {
                    if let Some(d) = self.label_dict(kid)? {
                        stack.push((d, depth + 1));
                    }
                }
            }
        }
        // Decode each bounded prefix once, rather than once for every page in its range.
        let mut specs = BTreeMap::new();
        for (start, d) in ranges {
            let first = d.get(b"St").ok().and_then(|o| self.resolve(o).as_i64().ok()).unwrap_or(1).max(1) as u64;
            let prefix = match d.get(b"P").ok().map(|o| self.resolve(o)) {
                Some(Object::String(raw, _) | Object::Name(raw)) => {
                    if raw.len() > MAX_PREFIX_BYTES {
                        return Err("a raw prefix exceeds 2050 bytes");
                    }
                    self.text(d, b"P").unwrap_or_default()
                }
                _ => String::new(),
            };
            if prefix.len() > MAX_LABEL_BYTES {
                return Err("a decoded prefix exceeds 1024 UTF-8 bytes");
            }
            let style = d.get(b"S").ok().and_then(|o| self.resolve(o).as_name().ok());
            specs.insert(start, (first, prefix, style));
        }
        let mut staged = Vec::new();
        let mut total = 0usize;
        for (i, _) in pages.iter().enumerate() {
            let Some((&start, (first, prefix, style))) = specs.range(..=i).next_back() else { continue };
            let n = if matches!(*style, Some(b"D" | b"R" | b"r" | b"A" | b"a")) {
                let offset = u64::try_from(i - start).map_err(|_| "a page number is too large")?;
                first.checked_add(offset).ok_or("a page number overflows")?
            } else {
                0 // /St is unused when the label consists only of its prefix.
            };
            let remaining = MAX_LABEL_BYTES - prefix.len();
            let number = match *style {
                Some(b"D") => Some(n.to_string()),
                Some(b"R" | b"r") => roman(n, remaining),
                Some(b"A" | b"a") => alpha(n, remaining),
                _ => Some(String::new()),
            }
            .filter(|number| number.len() <= remaining);
            // A label too long to show keeps that page's physical number; other ranges stay.
            let Some(mut number) = number else {
                let note = format!(
                    "labels from page {} exceed {MAX_LABEL_BYTES} UTF-8 bytes; those pages show their physical page numbers",
                    start.saturating_add(1)
                );
                if !notes.contains(&note) {
                    notes.push(note);
                }
                continue;
            };
            if matches!(*style, Some(b"R" | b"A")) {
                number.make_ascii_uppercase();
            }
            let mut label = prefix.clone();
            label.push_str(&number);
            if label.is_empty() {
                continue; // Keep the existing physical number for an empty custom label.
            }
            total = total.checked_add(label.len()).ok_or("the total label size overflows")?;
            if total > MAX_LABEL_TOTAL_BYTES {
                return Err("custom labels exceed 4 MiB in total");
            }
            staged.push((i, label));
        }
        // A malformed later range must not leave a partially applied custom label sequence.
        for (i, label) in staged {
            if let Some(page) = pages.get_mut(i) {
                page.label = label;
            }
        }
        Ok(notes)
    }

    // ── annotations ─────────────────────────────────────────────────────────────────────────

    fn annotations(&self, info: &mut DocInfo) {
        for (&id, &page) in &self.page_index {
            let Ok(page_dict) = self.doc.get_dictionary(id) else { continue };
            let Ok(Object::Array(annots)) = page_dict.get(b"Annots").map(|o| self.resolve(o)) else { continue };
            for (index, a) in annots.iter().enumerate() {
                let Some(d) = self.dict(a) else { continue };
                let Some(subtype) = self.name(d, b"Subtype") else { continue };
                if subtype == "FileAttachment"
                    && let Ok(fs) = d.get(b"FS")
                {
                    let name = self.text(d, b"Contents").unwrap_or_else(|| "attachment".into());
                    info.attachments.push(self.attachment(name, fs, AttachmentSource::Annotation { page, index }));
                }
                if subtype == "Link" {
                    if let Some(link) = self.link(page, d) {
                        if matches!(&link.target, LinkTarget::Other(s) if s == "JavaScript") {
                            info.has_javascript = true;
                        }
                        info.links.push(link);
                    }
                    continue;
                }
                // Not comments: form widgets, pop-ups and non-markup annotations such as the
                // Screen annotation that drives a LaTeX `animate` player (keep in step with
                // `pdfcraft_annot::is_comment_subtype`).
                if matches!(subtype.as_str(), "Widget" | "Popup" | "Screen" | "Movie" | "RichMedia" | "3D" | "PrinterMark" | "TrapNet" | "Watermark")
                {
                    continue;
                }
                let rect = rect4(self.resolve(d.get(b"Rect").unwrap_or(&Object::Null)));
                let mut color = match d.get(b"C").map(|o| self.resolve(o)) {
                    Ok(Object::Array(c)) if c.len() == 3 => {
                        let f = |i: usize| c[i].as_float().unwrap_or(0.0);
                        Some([f(0), f(1), f(2)])
                    }
                    _ => None,
                };
                // A text box's /C is its background; its text colour is in /DA.
                if subtype == "FreeText"
                    && let Some(da) = self.text(d, b"DA")
                {
                    let t: Vec<&str> = da.split_whitespace().collect();
                    if let Some(i) = t.iter().position(|x| *x == "rg")
                        && i >= 3
                    {
                        let f = |k: usize| t[k].parse::<f32>().unwrap_or(0.0);
                        color = Some([f(i - 3), f(i - 2), f(i - 1)]);
                    }
                }
                let quads = match d.get(b"QuadPoints").map(|o| self.resolve(o)) {
                    Ok(Object::Array(q)) => {
                        let v: Vec<f32> = q.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
                        v.as_chunks::<8>().0.to_vec()
                    }
                    _ => Vec::new(),
                };
                let in_reply_to = d.get(b"IRT").ok().and_then(|o| self.dict(o)).and_then(|p| self.text(p, b"NM"));
                let fill_sign = self.is_fill_sign(d, &subtype);
                info.annotations.push(Annotation {
                    page,
                    subtype,
                    author: self.text(d, b"T"),
                    contents: self.text(d, b"Contents"),
                    modified: self.text(d, b"M").map(|m| pretty_date(&m)),
                    name: self.text(d, b"NM"),
                    in_reply_to,
                    rect,
                    color,
                    index,
                    state: self.text(d, b"State"),
                    quads,
                    locked: d.get(b"F").ok().and_then(|f| self.resolve(f).as_i64().ok()).unwrap_or(0) & 128 != 0,
                    intent: self.name(d, b"IT"),
                    fill_sign,
                });
            }
        }
        info.annotations.sort_by(|a, b| a.page.cmp(&b.page).then(b.rect[3].total_cmp(&a.rect[3])));
    }

    fn link(&self, page: usize, d: &Dictionary) -> Option<Link> {
        let rect = rect4(self.resolve(d.get(b"Rect").ok()?));
        let target = if let Ok(dest) = d.get(b"Dest") {
            let (page, view) = self.dest(dest, 0)?;
            LinkTarget::Page(page, view)
        } else {
            let a = d.get(b"A").ok().and_then(|a| self.dict(a))?;
            match self.name(a, b"S").as_deref() {
                Some("GoTo") => {
                    let (page, view) = self.dest(a.get(b"D").ok()?, 0)?;
                    LinkTarget::Page(page, view)
                }
                Some("URI") => {
                    LinkTarget::Uri(a.get(b"URI").ok().and_then(|u| self.resolve(u).as_str().ok()).map(|b| String::from_utf8_lossy(b).into_owned())?)
                }
                Some("SetOCGState") => self.layer_state(a),
                Some(other) => LinkTarget::Other(other.to_string()),
                None => return None,
            }
        };
        Some(Link { page, rect, target })
    }

    /// A set-OCG-state action: `/State` is `ON`, `OFF` or `Toggle`, each followed by the groups
    /// it applies to; `/PreserveRB` is true unless it is `false`.
    fn layer_state(&self, a: &Dictionary) -> LinkTarget {
        let mut changes = Vec::new();
        if let Ok(Object::Array(items)) = a.get(b"State").map(|o| self.resolve(o)) {
            let mut op = None;
            for item in items.iter().take(MAX_LAYER_STATE) {
                match item {
                    Object::Name(n) => {
                        op = match n.as_slice() {
                            b"ON" => Some(LayerOp::On),
                            b"OFF" => Some(LayerOp::Off),
                            b"Toggle" => Some(LayerOp::Toggle),
                            _ => None,
                        }
                    }
                    Object::Reference(id) => changes.extend(op.map(|op| (op, *id))),
                    _ => {}
                }
            }
        }
        let preserve_rb = !matches!(a.get(b"PreserveRB").map(|o| self.resolve(o)), Ok(Object::Boolean(false)));
        LinkTarget::SetLayers { changes, preserve_rb }
    }

    // ── form fields ─────────────────────────────────────────────────────────────────────────

    fn field(
        &self,
        o: &'a Object,
        parent: Option<(&str, Option<String>, Option<i64>)>,
        out: &mut Vec<Field>,
        seen: &mut HashSet<ObjectId>,
        depth: u32,
    ) {
        if depth > 32 {
            return;
        }
        if let Object::Reference(id) = o
            && !seen.insert(*id)
        {
            return;
        }
        let Some(d) = self.dict(o) else { return };
        let (parent_name, parent_ft, parent_ff) = parent.unwrap_or(("", None, None));
        let partial = self.text(d, b"T");
        let full = match (&partial, parent_name.is_empty()) {
            (Some(t), true) => t.clone(),
            (Some(t), false) => format!("{parent_name}.{t}"),
            (None, _) => parent_name.to_string(),
        };
        let ft = self.name(d, b"FT").or(parent_ft);
        let ff = d.get(b"Ff").ok().and_then(|x| x.as_i64().ok()).or(parent_ff);
        // Non-terminal: has kids that are fields (they carry /T). Widgets-only kids are terminal.
        let kids = match d.get(b"Kids").map(|k| self.resolve(k)) {
            Ok(Object::Array(k)) => k.as_slice(),
            _ => &[],
        };
        let field_kids: Vec<_> = kids.iter().filter(|k| self.dict(k).is_some_and(|kd| kd.get(b"T").is_ok())).collect();
        if !field_kids.is_empty() {
            for k in field_kids {
                self.field(k, Some((&full, ft.clone(), ff)), out, seen, depth + 1);
            }
            return;
        }
        let flags = ff.unwrap_or(0);
        let kind = match ft.as_deref() {
            Some("Tx") => FieldKind::Text,
            Some("Btn") if flags & (1 << 16) != 0 => FieldKind::PushButton,
            Some("Btn") if flags & (1 << 15) != 0 => FieldKind::Radio,
            Some("Btn") => FieldKind::CheckBox,
            Some("Ch") if flags & (1 << 17) != 0 => FieldKind::Combo,
            Some("Ch") => FieldKind::List,
            Some("Sig") => FieldKind::Signature,
            _ => FieldKind::Unknown,
        };
        let value = match d.get(b"V").map(|v| self.resolve(v)) {
            Ok(Object::Name(n)) => Some(String::from_utf8_lossy(n).into_owned()),
            Ok(Object::String(bytes, _)) => Some(text_string(bytes)),
            Ok(Object::Array(a)) => Some(
                a.iter()
                    .filter_map(|x| match self.resolve(x) {
                        Object::String(bytes, _) => Some(text_string(bytes)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Ok(Object::Dictionary(_)) if kind == FieldKind::Signature => Some("signed".into()),
            _ => None,
        };
        // The widget is either this dict (merged field/widget) or its first kid.
        let widget = if d.get(b"Rect").is_ok() { Some(d) } else { kids.first().and_then(|k| self.dict(k)) };
        let page = widget.and_then(|w| w.get(b"P").ok()).and_then(|p| self.page_of(p)).or_else(|| self.find_widget_page(o, kids));
        let has_actions = d.get(b"AA").is_ok() || widget.is_some_and(|w| w.get(b"AA").is_ok() || w.get(b"A").is_ok());
        let rect = widget.and_then(|w| w.get(b"Rect").ok()).map(|r| rect4(self.resolve(r)));
        out.push(Field { name: full, kind, value, page, tooltip: self.text(d, b"TU"), has_actions, rect });
    }

    fn find_widget_page(&self, field: &Object, kids: &[Object]) -> Option<usize> {
        let targets: HashSet<ObjectId> =
            std::iter::once(field).chain(kids.iter()).filter_map(|o| if let Object::Reference(id) = o { Some(*id) } else { None }).collect();
        if targets.is_empty() {
            return None;
        }
        self.page_index.iter().find_map(|(&pid, &idx)| {
            let annots = self.doc.get_dictionary(pid).ok()?.get(b"Annots").ok()?;
            let arr = self.resolve(annots).as_array().ok()?;
            arr.iter().any(|a| matches!(a, Object::Reference(id) if targets.contains(id))).then_some(idx)
        })
    }

    /// The topmost field above widget `id` (itself without a `/Parent`), or `None` when it or an
    /// ancestor is already reachable from the `/Fields` tree, or a `/Parent` loop hides the top.
    fn unlisted_field(&self, id: ObjectId, seen: &HashSet<ObjectId>) -> Option<ObjectId> {
        let mut top = id;
        let mut visited = HashSet::from([id]);
        loop {
            let o = Object::Reference(top);
            let d = self.dict(&o)?;
            let parent = match d.get(b"Parent") {
                Ok(Object::Reference(p)) => Some(*p),
                _ => None,
            };
            let Some(p) = parent else {
                return if seen.contains(&top) { None } else { Some(top) };
            };
            if !visited.insert(p) || visited.len() > 64 {
                return None;
            }
            if seen.contains(&p) {
                return None;
            }
            top = p;
        }
    }

    // ── optional content ────────────────────────────────────────────────────────────────────

    fn layers(&self, catalog: &Dictionary, out: &mut Vec<Layer>) {
        let Some(props) = catalog.get(b"OCProperties").ok().and_then(|o| self.dict(o)) else { return };
        let config = props.get(b"D").ok().and_then(|o| self.dict(o));
        let off: HashSet<ObjectId> = config
            .and_then(|c| c.get(b"OFF").ok())
            .and_then(|o| self.resolve(o).as_array().ok())
            .map(|a| a.iter().filter_map(|x| x.as_reference().ok()).collect())
            .unwrap_or_default();
        let base_off = config.and_then(|c| self.name(c, b"BaseState")).as_deref() == Some("OFF");
        let on: HashSet<ObjectId> = config
            .and_then(|c| c.get(b"ON").ok())
            .and_then(|o| self.resolve(o).as_array().ok())
            .map(|a| a.iter().filter_map(|x| x.as_reference().ok()).collect())
            .unwrap_or_default();
        let Ok(Object::Array(ocgs)) = props.get(b"OCGs").map(|o| self.resolve(o)) else { return };
        for g in ocgs {
            let Some(d) = self.dict(g) else { continue };
            let id = g.as_reference().ok();
            let visible = match id {
                Some(id) if off.contains(&id) => false,
                Some(id) if on.contains(&id) => true,
                _ => !base_off,
            };
            let Some(id) = id else { continue };
            out.push(Layer { id, name: self.text(d, b"Name").unwrap_or_else(|| "Layer".into()), visible });
        }
    }

    /// The default configuration's radio-button groups (`/RBGroups`) of two or more layers.
    fn layer_groups(&self, catalog: &Dictionary) -> Vec<Vec<ObjectId>> {
        let groups = catalog
            .get(b"OCProperties")
            .ok()
            .and_then(|o| self.dict(o))
            .and_then(|p| p.get(b"D").ok())
            .and_then(|o| self.dict(o))
            .and_then(|c| c.get(b"RBGroups").ok())
            .and_then(|o| self.resolve(o).as_array().ok());
        let mut left = MAX_LAYER_GROUP_ENTRIES;
        let mut out = Vec::new();
        for g in groups.into_iter().flatten() {
            let Ok(members) = self.resolve(g).as_array() else { continue };
            let group: Vec<ObjectId> = members.iter().filter_map(|x| x.as_reference().ok()).take(left).collect();
            left = left.saturating_sub(group.len());
            if group.len() > 1 {
                out.push(group);
            }
            if left == 0 {
                break;
            }
        }
        out
    }

    fn fonts(&self, out: &mut Vec<FontInfo>) {
        let mut seen = HashSet::new();
        let mut pages: Vec<_> = self.page_index.iter().collect();
        pages.sort_by_key(|(_, i)| **i);
        for (&pid, _) in pages {
            let Ok(fonts) = self.doc.get_page_fonts(pid) else { continue };
            for (_, f) in fonts {
                let base = f
                    .get(b"BaseFont")
                    .ok()
                    .and_then(|o| o.as_name().ok())
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_else(|| "(unnamed)".into());
                let kind = self.name(f, b"Subtype").unwrap_or_default();
                let encoding = match f.get(b"Encoding").map(|o| self.resolve(o)) {
                    Ok(Object::Name(n)) => Some(String::from_utf8_lossy(n).into_owned()),
                    Ok(Object::Dictionary(_)) => Some("Custom".into()),
                    Ok(Object::Stream(_)) => Some("Embedded CMap".into()),
                    _ => None,
                };
                // Embedded: FontFile/2/3 in the descriptor (for Type0, in the descendant font).
                let descriptor_of = |d: &Dictionary| d.get(b"FontDescriptor").ok().and_then(|o| self.dict(o)).cloned();
                let desc = descriptor_of(f).or_else(|| {
                    f.get(b"DescendantFonts")
                        .ok()
                        .and_then(|o| self.resolve(o).as_array().ok())
                        .and_then(|a| a.first())
                        .and_then(|o| self.dict(o))
                        .and_then(descriptor_of)
                });
                let embedded =
                    kind == "Type3" || desc.is_some_and(|d| d.get(b"FontFile").is_ok() || d.get(b"FontFile2").is_ok() || d.get(b"FontFile3").is_ok());
                let subset = base.len() > 7 && base.as_bytes()[6] == b'+' && base[..6].bytes().all(|b| b.is_ascii_uppercase());
                let name = if subset { base[7..].to_string() } else { base };
                let info = FontInfo { name, kind, embedded, subset, encoding };
                if seen.insert((info.name.clone(), info.kind.clone(), info.embedded)) {
                    out.push(info);
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
    }

    fn attachment(&self, name: String, spec: &Object, source: AttachmentSource) -> Attachment {
        let d = self.dict(spec);
        let size =
            d.and_then(|d| d.get(b"EF").ok()).and_then(|o| self.dict(o)).and_then(|ef| ef.get(b"F").ok().or_else(|| ef.get(b"UF").ok())).and_then(
                |f| match self.resolve(f) {
                    Object::Stream(s) => s
                        .dict
                        .get(b"Params")
                        .ok()
                        .and_then(|p| self.dict(p))
                        .and_then(|p| p.get(b"Size").ok())
                        .and_then(|s| s.as_i64().ok())
                        .map(|s| s as usize)
                        .or(Some(s.content.len())),
                    _ => None,
                },
            );
        let display = d.and_then(|d| self.text(d, b"UF").or_else(|| self.text(d, b"F"))).unwrap_or(name);
        Attachment { name: display, description: d.and_then(|d| self.text(d, b"Desc")), size, source }
    }
}

/// A text string's value, decoded by the same rules as the engine's object model
/// ([`pdfcraft_cos::PdfString::to_text`]) so the panels and outline editing agree.
fn text_string(bytes: &[u8]) -> String {
    pdfcraft_cos::PdfString::literal(bytes).to_text()
}

/// Nests a pre-order list of `(depth, item)` into the outline tree.
fn nest_outline(listed: Vec<(usize, OutlineItem)>) -> Vec<OutlineItem> {
    let mut roots = Vec::new();
    // The open ancestors, outermost first; each collects its children until it closes.
    let mut open: Vec<(usize, OutlineItem)> = Vec::new();
    for (depth, item) in listed {
        while open.last().is_some_and(|(d, _)| *d >= depth) {
            close_outline(&mut open, &mut roots);
        }
        open.push((depth, item));
    }
    while !open.is_empty() {
        close_outline(&mut open, &mut roots);
    }
    roots
}

/// Closes the innermost open bookmark: it joins its parent's children, or the top level.
fn close_outline(open: &mut Vec<(usize, OutlineItem)>, roots: &mut Vec<OutlineItem>) {
    if let Some((_, item)) = open.pop() {
        match open.last_mut() {
            Some((_, parent)) => parent.children.push(item),
            None => roots.push(item),
        }
    }
}

fn rect4(o: &Object) -> [f32; 4] {
    match o {
        Object::Array(a) if a.len() == 4 => {
            let v: Vec<f32> = a.iter().map(|x| x.as_float().unwrap_or(0.0)).collect();
            [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])]
        }
        _ => [0.0; 4],
    }
}

fn label_work(left: &mut usize, count: usize) -> Result<(), &'static str> {
    *left = left.checked_sub(count).ok_or("the number tree has too many entries")?;
    Ok(())
}

/// `D:20260930104512-04'00'` → `2026-09-30 10:45`.
/// A PDF date (`D:20261001123000Z`) as "2026-10-01 12:30"; other strings unchanged.
pub fn pretty_date(s: &str) -> String {
    let d = s.trim_start_matches("D:");
    if let Some(d) = d.get(..12).filter(|d| d.bytes().all(|b| b.is_ascii_digit())) {
        // Twelve ASCII digits make every slice below a UTF-8 character boundary.
        format!("{}-{}-{} {}:{}", &d[0..4], &d[4..6], &d[6..8], &d[8..10], &d[10..12])
    } else {
        s.to_string()
    }
}

/// Fetch an attachment's bytes (decoded). Capped at 1 GiB to defuse decompression bombs.
pub fn attachment_data(bytes: &[u8], password: Option<&str>, att: &Attachment) -> Result<Vec<u8>, String> {
    let run = || -> Result<Vec<u8>, String> {
        let options = load_options(password);
        let doc = Document::load_mem_with_options(bytes, options).map_err(|e| e.to_string())?;
        let insp = Inspector::new(&doc);
        let spec: &Object = match &att.source {
            AttachmentSource::Document { key } => {
                let names = doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).and_then(|o| insp.dict(o)).ok_or("no /Names")?;
                let ef = names.get(b"EmbeddedFiles").ok().and_then(|o| insp.dict(o)).ok_or("no /EmbeddedFiles")?;
                let mut seen = HashSet::new();
                insp.name_tree(ef, &mut seen, 0).into_iter().find(|(k, _)| k == key).map(|(_, v)| v).ok_or("attachment not found")?
            }
            AttachmentSource::Annotation { page, index } => {
                let (&pid, _) = insp.page_index.iter().find(|(_, i)| **i == *page).ok_or("page not found")?;
                let annots = doc.get_dictionary(pid).map_err(|e| e.to_string())?.get(b"Annots").map_err(|e| e.to_string())?;
                let a = insp.resolve(annots).as_array().map_err(|e| e.to_string())?.get(*index).ok_or("annotation not found")?;
                insp.dict(a).and_then(|d| d.get(b"FS").ok()).ok_or("annotation has no file")?
            }
        };
        let fs = insp.dict(spec).ok_or("bad file specification")?;
        let ef = fs.get(b"EF").ok().and_then(|o| insp.dict(o)).ok_or("the file is not embedded (external reference)")?;
        let stream = ef.get(b"UF").ok().or_else(|| ef.get(b"F").ok()).map(|o| insp.resolve(o)).ok_or("no embedded stream")?;
        let stream = stream.as_stream().map_err(|e| e.to_string())?;
        if stream.dict.get(b"Filter").is_ok() {
            stream.decompressed_content_with_limit(1 << 30).map_err(|e| e.to_string())
        } else {
            Ok(stream.content.clone())
        }
    };
    catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|p| Err(format!("reading the attachment crashed: {}", crate::raster::panic_message(&p))))
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_predictor_rows_longer_than_the_data_are_refused_before_allocating() {
        // Vendored lopdf patch: a fuzzed `/Columns 4294967295` allocated two 4 GiB rows before
        // reading any data. A row that cannot fit in the data is refused up front.
        let e = lopdf::filters::png::decode_frame(&[2, 0, 0, 0], 1, 64 << 20).unwrap_err();
        assert!(e.to_string().contains("longer than the data"), "{e}");
        // A real frame still decodes: two 3-byte rows, the second Up-filtered.
        assert_eq!(lopdf::filters::png::decode_frame(&[0, 1, 2, 3, 2, 1, 1, 1], 1, 3).unwrap(), [1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn empty_png_predictor_frames_do_not_allocate_rows() {
        // usize::MAX cannot be reserved, so the old code safely errors before allocating.
        // An empty frame needs no rows regardless of the declared width.
        assert!(lopdf::filters::png::decode_frame(&[], 1, usize::MAX).unwrap().is_empty());
    }

    fn tiff_predictor_stream(data: Vec<u8>, columns: i64, colors: i64, bits: i64) -> lopdf::Stream {
        let mut params = lopdf::Dictionary::new();
        params.set("Predictor", 2i64);
        params.set("Columns", columns);
        params.set("Colors", colors);
        params.set("BitsPerComponent", bits);
        let mut dict = lopdf::Dictionary::new();
        dict.set("DecodeParms", params);
        let mut stream = lopdf::Stream::new(dict, data);
        stream.compress().unwrap();
        assert_eq!(stream.dict.get(b"Filter").unwrap().as_name().unwrap(), b"FlateDecode");
        stream
    }

    #[test]
    fn tiff_subbyte_predictor_row_width_overflow_is_refused() {
        // Each multiplication overflows before any scratch allocation in the old debug build.
        for bits in [1, 2, 4] {
            let stream = tiff_predictor_stream(vec![0; 64], i64::MAX, 3, bits);
            assert!(
                matches!(stream.decompressed_content_with_limit(64), Err(lopdf::Error::Decompress(lopdf::DecompressError::Predictor(_)))),
                "{bits}-bit row"
            );
        }
    }

    #[test]
    fn tiff_subbyte_predictor_scratch_is_bounded_by_available_samples() {
        // 1/2-bit row widths fit usize on 32- and 64-bit hosts; Colors previously made Vec<u16>
        // reject the capacity before allocating. A partial row has no preceding pixel.
        for bits in [1, 2] {
            let data = vec![0b1010_0110; 64];
            let stream = tiff_predictor_stream(data.clone(), 1, i64::try_from(isize::MAX).unwrap(), bits);
            assert_eq!(stream.decompressed_content_with_limit(64).unwrap(), data, "{bits}-bit row");
        }
    }

    #[test]
    fn tiff_subbyte_predictors_keep_components_rows_and_padding() {
        // Repeated tiny rows compress through the public Stream API. Rows remain independent,
        // differences wrap at each component depth, and trailing padding bits survive.
        for (columns, colors, bits, encoded, decoded) in [
            (16, 1, 1, vec![255, 170, 8, 255], vec![170, 204, 15, 85]),
            (6, 1, 2, vec![85, 179], vec![108, 147]),
            (3, 1, 4, vec![25, 16], vec![26, 176]),
            (2, 2, 4, vec![18, 34], vec![18, 52]),
        ] {
            let stream = tiff_predictor_stream(encoded.repeat(32), columns, colors, bits);
            assert_eq!(stream.decompressed_content_with_limit(128).unwrap(), decoded.repeat(32), "{bits}-bit, {colors} colours");
        }
    }

    // Xref-stream fixtures exercise indirect /Length lookup during document loading: stream 2's /Length is `3 0 R`,
    // `containers` lists compressed (id, container, index) entries and `objects` the other objects in the file.
    fn compressed_length_pdf(containers: &[(u32, u32, u16)], objects: &[(u32, &[u8])]) -> Vec<u8> {
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let ids = containers.iter().flat_map(|&(id, container, _)| [id, container]).chain(objects.iter().map(|&(id, _)| id));
        let xref_id = ids.max().unwrap_or(3) + 1;
        let mut records = vec![[0u8; 7]; (xref_id + 1) as usize];
        records[0] = [0, 0, 0, 0, 0, 255, 255];
        let mut append = |id: u32, body: &[u8]| {
            records[id as usize][0] = 1;
            records[id as usize][1..5].copy_from_slice(&(pdf.len() as u32).to_be_bytes());
            pdf.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
            pdf.extend_from_slice(body);
            pdf.extend_from_slice(b"\nendobj\n");
        };
        append(1, b"<< /Type /Catalog >>");
        append(2, b"<< /Length 3 0 R >>\nstream\nhello\nendstream");
        for &(id, body) in objects {
            append(id, body);
        }
        for &(id, container, index) in containers {
            records[id as usize][0] = 2;
            records[id as usize][1..5].copy_from_slice(&container.to_be_bytes());
            records[id as usize][5..7].copy_from_slice(&index.to_be_bytes());
        }
        let xref_offset = pdf.len();
        records[xref_id as usize][0] = 1;
        records[xref_id as usize][1..5].copy_from_slice(&(xref_offset as u32).to_be_bytes());
        pdf.extend_from_slice(
            format!("{xref_id} 0 obj\n<< /Type /XRef /Size {} /Root 1 0 R /W [1 4 2] /Length {} >>\nstream\n", records.len(), records.len() * 7)
                .as_bytes(),
        );
        for record in records {
            pdf.extend_from_slice(&record);
        }
        pdf.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes());
        pdf
    }

    // Stream 2's /Length starts a chain of `hops` indirect /Length lookups, each naming a new object; only the last
    // link's /Length is direct. Compressed: link k is the integer 3 + 2k (the length it measures) alone in the object
    // stream 4 + 2k, whose own /Length is link k + 1. Plain: link k is the stream 3 + k, whose /Length is link k + 1.
    fn length_chain_pdf(hops: u32, compressed: bool) -> Vec<u8> {
        let length = |hop: u32, next: u32, data: &str| if hop + 1 < hops { format!("{next} 0 R") } else { data.len().to_string() };
        let (mut containers, mut objects) = (Vec::new(), Vec::new());
        let mut value = 5; // the length of stream 2's data, "hello"
        for hop in 0..hops {
            if compressed {
                let (id, container) = (3 + 2 * hop, 4 + 2 * hop);
                let index = format!("{id} 0 ");
                let data = format!("{index}{value}");
                let dict = format!("/Type /ObjStm /N 1 /First {} /Length {}", index.len(), length(hop, id + 2, &data));
                objects.push((container, format!("<< {dict} >>\nstream\n{data}\nendstream")));
                containers.push((id, container, 0));
                value = data.len();
            } else {
                objects.push((3 + hop, format!("<< /Length {} >>\nstream\n\nendstream", length(hop, 4 + hop, ""))));
            }
        }
        let objects: Vec<_> = objects.iter().map(|(id, body)| (*id, body.as_bytes())).collect();
        compressed_length_pdf(&containers, &objects)
    }

    // Lenient loading keeps a document whose stream 2 has an unresolvable /Length: the catalog, and stream 2's
    // dictionary without its data.
    fn load_without_stream_2_data(pdf: &[u8]) -> Document {
        let doc = Document::load_mem(pdf).unwrap();
        assert!(doc.catalog().unwrap().has_type(b"Catalog"));
        assert!(doc.get_object((2, 0)).unwrap().as_stream().unwrap().content.is_empty());
        doc
    }

    #[test]
    fn compressed_object_self_container_does_not_recurse() {
        let doc = load_without_stream_2_data(&compressed_length_pdf(&[(3, 3, 0)], &[]));
        assert!(doc.get_object((3, 0)).is_err());
    }

    #[test]
    fn compressed_object_mutual_containers_do_not_recurse() {
        let doc = load_without_stream_2_data(&compressed_length_pdf(&[(3, 4, 0), (4, 3, 0)], &[]));
        assert!(doc.get_object((3, 0)).is_err());
        assert!(doc.get_object((4, 0)).is_err());
    }

    #[test]
    fn object_stream_length_cycle_does_not_recurse() {
        let pdf = compressed_length_pdf(&[(3, 4, 0)], &[(4, b"<< /Type /ObjStm /N 1 /First 4 /Length 3 0 R >>\nstream\n3 0 5\nendstream")]);
        let doc = load_without_stream_2_data(&pdf);
        // The container's /Length is the object it holds: the container is kept without data, so 3 never loads.
        assert!(doc.get_object((4, 0)).unwrap().as_stream().unwrap().content.is_empty());
        assert!(doc.get_object((3, 0)).is_err());
    }

    #[test]
    fn compressed_object_container_chain_does_not_recurse() {
        let containers: Vec<_> = (3..100_003).map(|id| (id, id + 1, 0)).collect();
        let pdf = compressed_length_pdf(&containers, &[(100_003, b"<< /Type /ObjStm /N 1 /First 4 /Length 5 >>\nstream\n3 0 5\nendstream")]);
        let doc = load_without_stream_2_data(&pdf);
        // The object stream in the file loads, but the xref puts 3 in the compressed container 4, which is refused.
        assert_eq!(doc.get_object((100_003, 0)).unwrap().as_stream().unwrap().content, b"3 0 5");
        assert!(doc.get_object((3, 0)).is_err());
    }

    #[test]
    fn compressed_objects_and_indirect_stream_length_still_load() {
        let pdf = compressed_length_pdf(
            &[(3, 4, 0), (5, 4, 1)],
            &[(4, b"<< /Type /ObjStm /N 2 /First 8 /Length 17 >>\nstream\n3 0 5 2 5 (value)\nendstream")],
        );
        let doc = Document::load_mem_with_options(&pdf, LoadOptions { strict: true, ..Default::default() }).unwrap();
        assert_eq!(doc.get_object((3, 0)).unwrap().as_i64().unwrap(), 5);
        assert_eq!(doc.get_object((5, 0)).unwrap().as_str().unwrap(), b"value");
        assert_eq!(doc.get_object((2, 0)).unwrap().as_stream().unwrap().content, b"hello");
    }

    #[test]
    fn compressed_length_in_a_container_with_an_indirect_length_still_loads() {
        // Two hops, as in a valid file: stream 2's /Length (3) is compressed in container 4, whose own /Length (5) is
        // a plain indirect integer.
        let pdf =
            compressed_length_pdf(&[(3, 4, 0)], &[(4, b"<< /Type /ObjStm /N 1 /First 4 /Length 5 0 R >>\nstream\n3 0 5\nendstream"), (5, b"5")]);
        let doc = Document::load_mem_with_options(&pdf, LoadOptions { strict: true, ..Default::default() }).unwrap();
        assert_eq!(doc.get_object((3, 0)).unwrap().as_i64().unwrap(), 5);
        assert_eq!(doc.get_object((5, 0)).unwrap().as_i64().unwrap(), 5);
        assert_eq!(doc.get_object((2, 0)).unwrap().as_stream().unwrap().content, b"hello");
    }

    #[test]
    fn compressed_length_chain_does_not_recurse() {
        // Every container is an ordinary object in the file and every hop names a new object, so neither the
        // container check nor the cycle history stops the chain; each hop recursed once more (a 512 KiB stack
        // overflowed within 150 hops). Lookups past the cap fail: the first link never loads, while the last one,
        // next to the direct /Length, does.
        let hops = 5_000;
        let doc = load_without_stream_2_data(&length_chain_pdf(hops, true));
        assert!(doc.get_object((3, 0)).is_err());
        assert!(doc.get_object((3 + 2 * (hops - 1), 0)).unwrap().as_i64().is_ok());
        // The cap itself: loading link j reads its container's /Length, a chain of 2 * (hops - 1 - j) lookups, and
        // 16 are allowed, so the 9th link from the end loads and the 10th does not.
        assert!(doc.get_object((3 + 2 * (hops - 9), 0)).is_ok());
        assert!(doc.get_object((3 + 2 * (hops - 10), 0)).is_err());
    }

    #[test]
    fn plain_stream_length_chain_does_not_recurse() {
        // The same recursion without object streams: each stream's /Length names the next stream (a 512 KiB stack
        // overflowed within 200 hops). A stream is no length, so every link is kept as a stream without data.
        let hops = 5_000;
        let doc = load_without_stream_2_data(&length_chain_pdf(hops, false));
        assert!((3..3 + hops).all(|id| doc.get_object((id, 0)).is_ok_and(|link| link.as_stream().is_ok_and(|s| s.content.is_empty()))));
    }

    fn indexed_object_stream(pairs: &[(u32, usize)], members: &[u8]) -> lopdf::Stream {
        let mut content = Vec::new();
        for &(id, offset) in pairs {
            content.extend_from_slice(format!("{id} {offset} ").as_bytes());
        }
        let mut dict = Dictionary::new();
        dict.set("Type", "ObjStm");
        dict.set("N", pairs.len() as i64);
        dict.set("First", content.len() as i64);
        content.extend_from_slice(members);
        lopdf::Stream::new(dict, content)
    }

    #[test]
    fn object_stream_member_extents_bound_owned_strings() {
        let mut members = vec![b'('; 32];
        members.extend(vec![b'x'; 1 << 20]);
        members.extend([b')'; 32]);
        // Put the innermost (complete) string first in index order, then overlapping starts
        // and duplicates. Only this first member at offset 31 may own the large string.
        let mut pairs = vec![(100, 31)];
        pairs.extend((0..31).map(|offset| (offset as u32 + 1, offset)));
        pairs.extend([(200, 31), (201, 31)]);
        let stream = indexed_object_stream(&pairs, &members);
        let parsed = lopdf::ObjectStream::new_with_limit(&stream, Some(2 << 20)).unwrap();
        assert_eq!(parsed.objects.get(&(100, 0)).unwrap().as_str().unwrap().len(), 1 << 20);
        let total: usize = parsed.objects.values().filter_map(|object| object.as_str().ok()).map(<[u8]>::len).sum();
        assert!(total <= stream.content.len(), "owned {total} string bytes from {} decoded bytes", stream.content.len());
        assert!(!parsed.objects.contains_key(&(200, 0)));
        assert!(!parsed.objects.contains_key(&(201, 0)));
    }

    #[test]
    fn object_stream_member_extents_preserve_index_order_and_whitespace() {
        let members = b" \n42\n /Name \n[true 7]\n << /Key (value) >>\n (last)";
        // Unsorted offsets, and object 9 appears twice: its last index entry still wins.
        let pairs = [(9, 0), (12, 21), (10, 5), (11, 12), (9, 41)];
        let parsed = lopdf::ObjectStream::new(&indexed_object_stream(&pairs, members)).unwrap();
        assert_eq!(parsed.objects.len(), 4);
        assert_eq!(parsed.objects.get(&(9, 0)).unwrap().as_str().unwrap(), b"last");
        assert_eq!(parsed.objects.get(&(10, 0)).unwrap().as_name().unwrap(), b"Name");
        assert_eq!(parsed.objects.get(&(11, 0)).unwrap().as_array().unwrap(), &[Object::Boolean(true), Object::Integer(7)]);
        assert_eq!(parsed.objects.get(&(12, 0)).unwrap().as_dict().unwrap().get(b"Key").unwrap().as_str().unwrap(), b"value");
    }

    #[test]
    fn view_and_user_space_round_trip_for_every_rotation() {
        for rotation in [0u16, 90, 180, 270] {
            let (w, h) = if rotation % 180 == 0 { (200.0, 300.0) } else { (300.0, 200.0) };
            let p = super::PageInfo { width: w, height: h, label: String::new(), crop: [10.0, 20.0, 210.0, 320.0], rotation };
            for (x, y) in [(0.0, 0.0), (15.0, 40.0), (w, h)] {
                let [ux, uy] = p.view_to_user(x, y);
                let [vx, vy] = p.user_to_view(ux, uy);
                assert!((vx - x).abs() < 1e-3 && (vy - y).abs() < 1e-3, "{rotation}: {x},{y} → {ux},{uy} → {vx},{vy}");
            }
        }
        // Unrotated: the view's top-left is the crop box's top-left.
        let p = super::PageInfo { width: 200.0, height: 300.0, label: String::new(), crop: [10.0, 20.0, 210.0, 320.0], rotation: 0 };
        assert_eq!(p.view_to_user(0.0, 0.0), [10.0, 320.0]);
        assert_eq!(p.view_rect_to_quad([0.0, 0.0, 10.0, 5.0]), [10.0, 320.0, 20.0, 320.0, 10.0, 315.0, 20.0, 315.0]);
    }

    use super::*;

    #[test]
    fn roman_and_alpha_labels() {
        assert_eq!(roman(4, MAX_LABEL_BYTES).as_deref(), Some("iv"));
        assert_eq!(roman(1994, MAX_LABEL_BYTES).as_deref(), Some("mcmxciv"));
        assert_eq!(alpha(1, MAX_LABEL_BYTES).as_deref(), Some("a"));
        assert_eq!(alpha(27, MAX_LABEL_BYTES).as_deref(), Some("aa"));
        assert_eq!(alpha(53, MAX_LABEL_BYTES).as_deref(), Some("aaa"));
    }

    // Synthetic label-only inspection fixtures: never construct an explosive branching tree.
    fn label_info(nums: Vec<Object>, kids: Vec<Object>) -> DocInfo {
        let mut doc = Document::with_version("1.7");
        let mut tree = Dictionary::new();
        tree.set("Nums", nums);
        tree.set("Kids", kids);
        doc.objects.insert((6, 0), Object::Dictionary(tree));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("PageLabels", Object::Reference((6, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let mut info = DocInfo {
            pages: (1..=3)
                .map(|n| PageInfo { width: 200.0, height: 300.0, crop: [0.0, 0.0, 200.0, 300.0], rotation: 0, label: n.to_string() })
                .collect(),
            ..Default::default()
        };
        Inspector::new(&doc).fill(&mut info);
        info
    }

    fn label_spec(style: &str, prefix: Vec<u8>, first: i64) -> Object {
        let mut d = Dictionary::new();
        d.set("S", Object::Name(style.as_bytes().to_vec()));
        d.set("P", Object::String(prefix, lopdf::StringFormat::Hexadecimal));
        d.set("St", first);
        Object::Dictionary(d)
    }

    fn assert_label_fallback(info: &DocInfo) {
        assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
        assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
        assert!(info.warnings[0].contains("Page labels"), "{:?}", info.warnings);
        assert!(info.warnings[0].contains("physical page numbers"), "{:?}", info.warnings);
    }

    #[test]
    fn page_label_limits_keep_normal_pdf_numbering() {
        for (style, first, expected) in [
            ("r", 4, ["iv", "v", "vi"]),
            ("R", 1994, ["MCMXCIV", "MCMXCV", "MCMXCVI"]),
            ("a", 26, ["z", "aa", "bb"]),
            ("A", 52, ["ZZ", "AAA", "BBB"]),
        ] {
            let info = label_info(vec![Object::Integer(0), label_spec(style, Vec::new(), first)], Vec::new());
            assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), expected);
            assert!(info.warnings.is_empty(), "{:?}", info.warnings);
        }
    }

    #[test]
    fn page_label_limits_reject_long_prefixes_without_partial_labels() {
        for prefix in [vec![b'x'; 1025], vec![b'x'; 2051]] {
            let nums = vec![Object::Integer(0), label_spec("D", b"ok-".to_vec(), 1), Object::Integer(1), label_spec("D", prefix, 1)];
            assert_label_fallback(&label_info(nums, Vec::new()));
        }
    }

    #[test]
    fn page_label_limits_count_decoded_utf8_bytes() {
        let mut prefix = vec![0xfe, 0xff];
        for _ in 0..342 {
            prefix.extend_from_slice(&0x2022u16.to_be_bytes());
        }
        assert_label_fallback(&label_info(vec![Object::Integer(0), label_spec("", prefix, 1)], Vec::new()));
    }

    #[test]
    fn page_label_limits_bound_roman_and_alpha_output() {
        // Just 1025 output bytes on the old path, so RED does not consume excessive memory.
        for (style, first) in [("r", 1_025_000), ("a", 26 * 1024 + 1)] {
            assert_label_fallback(&label_info(vec![Object::Integer(0), label_spec(style, Vec::new(), first)], Vec::new()));
        }
    }

    #[test]
    fn page_label_limits_detect_one_node_cycle() {
        let nums = vec![Object::Integer(0), label_spec("D", b"custom-".to_vec(), 1)];
        assert_label_fallback(&label_info(nums, vec![Object::Reference((6, 0))]));
    }

    #[test]
    fn page_label_limits_accept_utf16_ascii_at_the_output_boundary() {
        let mut prefix = vec![0xfe, 0xff];
        for _ in 0..1023 {
            prefix.extend_from_slice(&u16::from(b'x').to_be_bytes());
        }
        let info = label_info(vec![Object::Integer(0), label_spec("D", prefix, 1)], Vec::new());
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
        assert_eq!(info.pages[0].label, format!("{}1", "x".repeat(1023)));
        assert!(info.pages.iter().all(|p| p.label.len() == MAX_LABEL_BYTES));
    }

    #[test]
    fn page_label_limits_refuse_hostile_numbers_before_allocating() {
        for style in ["r", "R", "a", "A"] {
            assert_label_fallback(&label_info(vec![Object::Integer(0), label_spec(style, Vec::new(), i64::MAX)], Vec::new()));
        }
        // Decimal labels remain small even when their first number exceeds 32-bit usize.
        let info = label_info(vec![Object::Integer(0), label_spec("D", Vec::new(), i64::MAX)], Vec::new());
        assert_eq!(info.pages[0].label, i64::MAX.to_string());
        assert_eq!(info.pages[1].label, (i64::MAX as u64 + 1).to_string());
        assert!(info.warnings.is_empty());
    }

    #[test]
    fn page_label_limits_bound_wide_number_trees() {
        assert_label_fallback(&label_info(Vec::new(), vec![Object::Null; MAX_LABEL_TREE_WORK + 1]));
        let pair = [Object::Integer(0), label_spec("D", Vec::new(), 1)];
        let nums = (0..MAX_LABEL_TREE_WORK).flat_map(|_| pair.clone()).collect();
        assert_label_fallback(&label_info(nums, Vec::new()));
    }

    #[test]
    fn page_label_limits_bound_total_custom_output_atomically() {
        let doc = Document::with_version("1.7");
        let mut tree = Dictionary::new();
        tree.set("Nums", vec![Object::Integer(0), label_spec("", vec![b'x'; MAX_LABEL_BYTES], 1)]);
        let mut catalog = Dictionary::new();
        catalog.set("PageLabels", Object::Dictionary(tree));
        let count = MAX_LABEL_TOTAL_BYTES / MAX_LABEL_BYTES + 1;
        let mut pages: Vec<_> = (1..=count)
            .map(|n| PageInfo { width: 200.0, height: 300.0, crop: [0.0, 0.0, 200.0, 300.0], rotation: 0, label: n.to_string() })
            .collect();
        let err = Inspector::new(&doc).page_labels(&catalog, &mut pages).unwrap_err();
        assert!(err.contains("4 MiB"), "{err}");
        assert!(pages.iter().enumerate().all(|(i, p)| p.label == (i + 1).to_string()));
        // Exact aggregate boundary remains supported.
        pages.pop();
        Inspector::new(&doc).page_labels(&catalog, &mut pages).unwrap();
        assert!(pages.iter().all(|p| p.label.len() == MAX_LABEL_BYTES));
    }

    #[test]
    fn page_label_limits_handle_prefix_only_and_reference_cycles() {
        let info = label_info(vec![Object::Integer(0), label_spec("", b"prefix".to_vec(), i64::MAX)], Vec::new());
        assert!(info.warnings.is_empty());
        assert!(info.pages.iter().all(|p| p.label == "prefix"));
        let mut doc = Document::with_version("1.7");
        doc.objects.insert((6, 0), Object::Reference((6, 0)));
        let mut catalog = Dictionary::new();
        catalog.set("PageLabels", Object::Reference((6, 0)));
        assert!(Inspector::new(&doc).page_labels(&catalog, &mut []).unwrap_err().contains("cyclic"));
    }

    #[test]
    fn page_label_limits_bound_depth_and_count_combined_work() {
        for depth in [MAX_LABEL_TREE_DEPTH, MAX_LABEL_TREE_DEPTH + 1] {
            let mut child = Dictionary::new();
            child.set("Nums", vec![Object::Integer(0), label_spec("D", b"custom-".to_vec(), 1)]);
            let mut child = Object::Dictionary(child);
            for _ in 1..depth {
                let mut parent = Dictionary::new();
                parent.set("Kids", vec![child]);
                child = Object::Dictionary(parent);
            }
            let info = label_info(Vec::new(), vec![child]);
            if depth == MAX_LABEL_TREE_DEPTH {
                assert!(info.warnings.is_empty());
                assert_eq!(info.pages[0].label, "custom-1");
            } else {
                assert_label_fallback(&info);
            }
        }
        // Each array alone fits: their combined work still must be bounded.
        let half = MAX_LABEL_TREE_WORK / 2;
        let spec = label_spec("D", Vec::new(), 1);
        let nums = (0..half).flat_map(|i| [Object::Integer(i as i64), spec.clone()]).collect();
        assert_label_fallback(&label_info(nums, vec![Object::Null; half]));
    }

    #[test]
    fn page_label_limits_keep_the_later_of_duplicate_ranges() {
        let nums = vec![Object::Integer(0), label_spec("D", b"first-".to_vec(), 1), Object::Integer(0), label_spec("D", b"last-".to_vec(), 1)];
        let info = label_info(nums, Vec::new());
        assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["last-1", "last-2", "last-3"]);
        assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
        assert!(info.warnings[0].contains("twice"), "{:?}", info.warnings);
    }

    #[test]
    fn page_label_limits_keep_other_ranges_when_one_is_too_long() {
        // Page 1 is labelled normally; pages 2–3 would need over 1024 bytes of roman numerals.
        let nums = vec![Object::Integer(0), label_spec("D", b"ok-".to_vec(), 1), Object::Integer(1), label_spec("r", Vec::new(), 1_025_000)];
        let info = label_info(nums, Vec::new());
        assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["ok-1", "2", "3"]);
        assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
        assert!(info.warnings[0].contains("physical page numbers"), "{:?}", info.warnings);
    }

    const ATTACHMENTS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(notes.txt) 6 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R] /Resources << /Font << /F1 9 0 R >> >> >> endobj
4 0 obj << /Type /Annot /Subtype /FileAttachment /Rect [10 10 30 30] /Contents (data.csv) /FS 8 0 R >> endobj
5 0 obj << /Type /EmbeddedFile /Length 11 >> stream
hello notes
endstream endobj
6 0 obj << /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 5 0 R >> /Desc (Doc-level) >> endobj
7 0 obj << /Type /EmbeddedFile /Length 5 >> stream
a,b,c
endstream endobj
8 0 obj << /Type /Filespec /F (data.csv) /EF << /F 7 0 R >> >> endobj
9 0 obj << /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn attachments_of_both_kinds_are_listed_and_extracted() {
        let bytes = Arc::new(ATTACHMENTS.to_vec());
        let info = inspect(bytes.clone(), None).expect("opens");
        let names: Vec<_> = info.attachments.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"notes.txt") && names.contains(&"data.csv"), "{names:?}");
        for a in &info.attachments {
            let data = attachment_data(&bytes, None, a).expect("extracts");
            let expected: &[u8] = if a.name == "notes.txt" { b"hello notes" } else { b"a,b,c" };
            assert_eq!(data, expected, "{}", a.name);
        }
        assert_eq!(info.fonts, vec![FontInfo { name: "Helvetica".into(), kind: "Type1".into(), embedded: false, subset: true, encoding: None }]);
    }

    const LAYERS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R 7 0 R] /D << /OFF [6 0 R] /RBGroups [[5 0 R 6 0 R] [7 0 R] 8 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [10 0 R 11 0 R] >> endobj
5 0 obj << /Type /OCG /Name (Red) >> endobj
6 0 obj << /Type /OCG /Name (Green) >> endobj
7 0 obj << /Type /OCG /Name (Blue) >> endobj
8 0 obj [6 0 R 7 0 R] endobj
9 0 obj [/OFF 5 0 R] endobj
10 0 obj << /Type /Annot /Subtype /Link /Rect [10 10 50 30] /A << /S /SetOCGState /State [7 0 R /ON 6 0 R /Bogus 5 0 R /Toggle 5 0 R 1 7 0 R] /PreserveRB false >> >> endobj
11 0 obj << /Type /Annot /Subtype /Link /Rect [60 10 100 30] /A << /S /SetOCGState /State 9 0 R >> >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn radio_button_layer_groups_are_read() {
        let info = inspect(Arc::new(LAYERS.to_vec()), None).expect("opens");
        let layers: Vec<_> = info.layers.iter().map(|l| (l.id, l.name.as_str(), l.visible)).collect();
        assert_eq!(layers, [((5, 0), "Red", true), ((6, 0), "Green", false), ((7, 0), "Blue", true)]);
        // A group of one constrains nothing; an indirect group is read.
        assert_eq!(info.layer_groups, [vec![(5, 0), (6, 0)], vec![(6, 0), (7, 0)]]);
    }

    const ORPHAN_FIELDS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R 6 0 R] >> endobj
4 0 obj << /Fields [] >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (alpha) /V (one) /Rect [10 100 90 120] >> endobj
6 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (beta) /Rect [10 60 90 80] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn fields_listed_only_on_the_pages_are_listed() {
        let info = inspect(Arc::new(ORPHAN_FIELDS.to_vec()), None).expect("opens");
        let listed: Vec<_> = info.fields.iter().map(|f| (f.name.as_str(), f.value.as_deref(), f.page)).collect();
        assert_eq!(listed, [("alpha", Some("one"), Some(0)), ("beta", None, Some(0))], "{:?}", info.fields);
    }

    #[test]
    fn lazy_structure_matches_the_compatibility_inspector() {
        // Each fixture as lopdf writes it, and with its objects in object streams.
        let packed = |bytes: &[u8]| {
            let doc = pdfcraft_cos::Document::open(Arc::new(bytes.to_vec())).unwrap();
            pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions { object_streams: true, ..Default::default() }).unwrap()
        };
        let fixtures = [ATTACHMENTS, LAYERS, DESTS].into_iter().flat_map(|bytes| {
            let mut original = Document::load_mem(bytes).unwrap();
            let mut plain = Vec::new();
            original.save_to(&mut plain).unwrap();
            let stm = packed(&plain);
            [plain, stm]
        });
        for bytes in fixtures {
            let lazy = LazyStructure::new(Arc::new(bytes.clone()), None).unwrap();
            let eager = Document::load_mem(&bytes).unwrap();
            let mut a = inspect(Arc::new(bytes), None).unwrap();
            let mut b = DocInfo { pages: a.pages.clone(), ..Default::default() };
            let mut c = DocInfo { pages: a.pages.clone(), ..Default::default() };
            Inspector::new(&lazy).fill(&mut b);
            Inspector::new(&eager).fill(&mut c);
            assert!(!lazy.failed());
            assert_eq!(format!("{b:?}"), format!("{c:?}"));
            a.file_size = 0;
            a.pdf_version.clear();
            assert_eq!(format!("{a:?}"), format!("{c:?}"));
        }
    }

    #[test]
    fn links_read_set_layer_actions() {
        let info = inspect(Arc::new(LAYERS.to_vec()), None).expect("opens");
        let targets: Vec<_> = info.links.iter().map(|l| l.target.clone()).collect();
        use LayerOp::{Off, On, Toggle};
        assert_eq!(
            targets,
            [
                // Groups before the first name or after an unknown one are skipped, and so is
                // anything that isn't a group.
                LinkTarget::SetLayers { changes: vec![(On, (6, 0)), (Toggle, (5, 0)), (Toggle, (7, 0))], preserve_rb: false },
                // An indirect /State; /PreserveRB defaults to true.
                LinkTarget::SetLayers { changes: vec![(Off, (5, 0))], preserve_rb: true },
            ]
        );
    }

    #[test]
    fn destination_operands_become_views() {
        use DestView::*;
        let v = |kind: &[u8], ops: &[Option<f32>]| DestView::from_operands(Some(kind), ops);
        // ISO 32000-2 Table 149, each type.
        assert_eq!(v(b"XYZ", &[Some(0.0), Some(420.0), Some(0.0)]), Xyz { left: Some(0.0), top: Some(420.0), zoom: None });
        assert_eq!(v(b"XYZ", &[Some(72.0), Some(500.0), Some(1.5)]), Xyz { left: Some(72.0), top: Some(500.0), zoom: Some(1.5) });
        assert_eq!(v(b"Fit", &[]), Fit);
        assert_eq!(v(b"FitB", &[]), Fit);
        assert_eq!(v(b"FitH", &[Some(250.0)]), FitH { top: Some(250.0) });
        assert_eq!(v(b"FitBH", &[None]), FitH { top: None });
        assert_eq!(v(b"FitV", &[Some(120.0)]), FitV { left: Some(120.0) });
        assert_eq!(v(b"FitBV", &[Some(5.0)]), FitV { left: Some(5.0) });
        // FitR's corners are normalized.
        assert_eq!(v(b"FitR", &[Some(110.0), Some(220.0), Some(10.0), Some(20.0)]), FitR { rect: [10.0, 20.0, 110.0, 220.0] });
        // Missing operands are null; so are non-finite ones, and a zoom that isn't positive.
        assert_eq!(v(b"XYZ", &[]), Xyz { left: None, top: None, zoom: None });
        assert_eq!(v(b"XYZ", &[Some(f32::NAN), Some(f32::INFINITY), Some(f32::NEG_INFINITY)]), Xyz { left: None, top: None, zoom: None });
        assert_eq!(v(b"XYZ", &[None, Some(-5.0), Some(-1.0)]), Xyz { left: None, top: Some(-5.0), zoom: None });
        assert_eq!(v(b"XYZ", &[None, None, Some(f32::NAN)]), Xyz { left: None, top: None, zoom: None });
        assert_eq!(v(b"FitH", &[Some(f32::NAN)]), FitH { top: None });
        // A FitR without four usable numbers, an unknown type or no type: the top of the page.
        assert_eq!(v(b"FitR", &[Some(1.0), Some(2.0)]), Top);
        assert_eq!(v(b"FitR", &[Some(1.0), Some(2.0), Some(f32::INFINITY), Some(4.0)]), Top);
        assert_eq!(v(b"Zoom", &[Some(3.0)]), Top);
        assert_eq!(v(b"xyz", &[Some(3.0)]), Top);
        assert_eq!(DestView::from_operands(None, &[Some(1.0)]), Top);
    }

    /// Two 300×400 pages. Bookmarks and links on page 1 go to page 2 in every destination form,
    /// named (`/Names /Dests` and the PDF 1.1 `/Dests`) and malformed.
    const DESTS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 10 0 R /Names << /Dests 40 0 R >> /Dests << /old << /D [4 0 R /FitV 120] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [30 0 R 31 0 R 32 0 R 33 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
10 0 obj << /Type /Outlines /First 11 0 R /Last 20 0 R /Count 10 >> endobj
11 0 obj << /Title (xyz) /Parent 10 0 R /Next 12 0 R /Dest [4 0 R /XYZ 0 420 0] >> endobj
12 0 obj << /Title (fith) /Parent 10 0 R /Next 13 0 R /Dest [4 0 R /FitH 250] >> endobj
13 0 obj << /Title (named) /Parent 10 0 R /Next 14 0 R /Dest (sec) >> endobj
14 0 obj << /Title (old named) /Parent 10 0 R /Next 15 0 R /Dest /old >> endobj
15 0 obj << /Title (action) /Parent 10 0 R /Next 16 0 R /A << /S /GoTo /D [4 0 R /FitR 110 220 10 20] >> >> endobj
16 0 obj << /Title (wrong types) /Parent 10 0 R /Next 17 0 R /Dest [4 0 R /XYZ (left) /Top true] >> endobj
17 0 obj << /Title (short) /Parent 10 0 R /Next 18 0 R /Dest [4 0 R /FitR 1 2] >> endobj
18 0 obj << /Title (no type) /Parent 10 0 R /Next 19 0 R /Dest [4 0 R] >> endobj
19 0 obj << /Title (huge) /Parent 10 0 R /Next 20 0 R /Dest [4 0 R /XYZ 1000000000000 -5 -1] >> endobj
20 0 obj << /Title (no page) /Parent 10 0 R /Dest [/XYZ 0 420 0] >> endobj
30 0 obj << /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest [4 0 R /XYZ 72 500 1.5] >> endobj
31 0 obj << /Type /Annot /Subtype /Link /Rect [0 20 10 30] /A << /S /GoTo /D (sec) >> >> endobj
32 0 obj << /Type /Annot /Subtype /Link /Rect [0 40 10 50] /Dest [4 0 R /FitBH 100] >> endobj
33 0 obj << /Type /Annot /Subtype /Link /Rect [0 60 10 70] /Dest [4 0 R /Fit] >> endobj
40 0 obj << /Names [(sec) << /D [4 0 R /XYZ null 300 2] >>] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn bookmarks_and_links_keep_their_destination_view() {
        use DestView::*;
        let info = inspect(Arc::new(DESTS.to_vec()), None).expect("opens");
        let outline: Vec<_> = info.outline.iter().map(|o| (o.title.as_str(), o.page, o.view)).collect();
        assert_eq!(
            outline,
            [
                ("xyz", Some(1), Xyz { left: Some(0.0), top: Some(420.0), zoom: None }),
                ("fith", Some(1), FitH { top: Some(250.0) }),
                ("named", Some(1), Xyz { left: None, top: Some(300.0), zoom: Some(2.0) }),
                ("old named", Some(1), FitV { left: Some(120.0) }),
                ("action", Some(1), FitR { rect: [10.0, 20.0, 110.0, 220.0] }),
                // Hostile operands fall back without failing the bookmark.
                ("wrong types", Some(1), Xyz { left: None, top: None, zoom: None }),
                ("short", Some(1), Top),
                ("no type", Some(1), Top),
                ("huge", Some(1), Xyz { left: Some(1e12), top: Some(-5.0), zoom: None }),
                ("no page", None, Top),
            ]
        );
        let links: Vec<_> = info.links.iter().map(|l| l.target.clone()).collect();
        assert_eq!(
            links,
            [
                LinkTarget::Page(1, Xyz { left: Some(72.0), top: Some(500.0), zoom: Some(1.5) }),
                LinkTarget::Page(1, Xyz { left: None, top: Some(300.0), zoom: Some(2.0) }),
                LinkTarget::Page(1, FitH { top: Some(100.0) }),
                LinkTarget::Page(1, Fit),
            ]
        );
    }

    #[test]
    fn outline_titles_decode_every_text_string_encoding() {
        // Issue #142: CJK bookmark titles showed as mojibake or with a stray BOM.
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
10 0 obj << /Type /Outlines /First 11 0 R /Last 16 0 R /Count 6 >> endobj
11 0 obj << /Title <FEFF7B2C4E007AE0> /Parent 10 0 R /Next 12 0 R /Dest [3 0 R /Fit] >> endobj
12 0 obj << /Title <EFBBBFE79BAEE5BD95> /Parent 10 0 R /Next 13 0 R /Dest [3 0 R /Fit] >> endobj
13 0 obj << /Title <EFBBBF41FF42> /Parent 10 0 R /Next 14 0 R /Dest [3 0 R /Fit] >> endobj
14 0 obj << /Title <E6A682E8BFB0> /Parent 10 0 R /Next 15 0 R /Dest [3 0 R /Fit] >> endobj
15 0 obj << /Title <436166E9> /Parent 10 0 R /Next 16 0 R /Dest [3 0 R /Fit] >> endobj
16 0 obj << /Title <FEFFFEFF0041> /Parent 10 0 R /Dest [3 0 R /Fit] >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let info = inspect(Arc::new(pdf.to_vec()), None).expect("opens");
        let titles: Vec<_> = info.outline.iter().map(|o| o.title.as_str()).collect();
        // UTF-16BE, UTF-8 with BOM, invalid UTF-8 after a BOM (lossy, not dropped), BOM-less
        // raw UTF-8, PDFDocEncoding Latin-1, and a doubled BOM.
        assert_eq!(titles, ["第一章", "目录", "A\u{FFFD}B", "概述", "Café", "A"]);
    }

    /// A file whose objects are `bodies` in order (object 1 first), with a cross-reference table.
    fn pdf_of(bodies: &[String]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
        out
    }

    /// A one-page file whose outline is `items` (objects 5, 6, … in order).
    fn outline_file(items: &[String]) -> Vec<u8> {
        let mut bodies = vec![
            "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_string(),
            "<< /Type /Page /Parent 2 0 R >>".to_string(),
            format!("<< /Type /Outlines /First 5 0 R /Count {} >>", items.len()),
        ];
        bodies.extend(items.iter().cloned());
        pdf_of(&bodies)
    }

    /// The titles of `items`, with children in brackets (`Parent[Child]`).
    fn shape(items: &[OutlineItem]) -> Vec<String> {
        items.iter().map(|o| if o.children.is_empty() { o.title.clone() } else { format!("{}[{}]", o.title, shape(&o.children).join(",")) }).collect()
    }

    #[test]
    fn outline_window_keeps_the_first_bookmarks_and_says_when_more_follow() {
        const COUNT: usize = OUTLINE_WINDOW + 5;
        let items: Vec<String> = (0..COUNT)
            .map(|k| {
                let prev = if k == 0 { String::new() } else { format!(" /Prev {} 0 R", 4 + k) };
                let next = if k + 1 == COUNT { String::new() } else { format!(" /Next {} 0 R", 6 + k) };
                format!("<< /Title (Item {k}) /Parent 4 0 R{prev}{next} /Dest [3 0 R /Fit] >>")
            })
            .collect();
        let info = inspect(Arc::new(outline_file(&items)), None).expect("opens");
        assert_eq!(info.outline.len(), OUTLINE_WINDOW);
        assert_eq!(info.outline.last().map(|o| o.title.as_str()), Some("Item 9999"));
        assert!(info.outline_more);

        let one = inspect(Arc::new(outline_file(&["<< /Title (Only) /Parent 4 0 R /Dest [3 0 R /Fit] >>".into()])), None).expect("opens");
        assert_eq!(shape(&one.outline), ["Only"]);
        assert!(!one.outline_more);
    }

    #[test]
    fn outline_window_nests_no_deeper_than_its_depth_limit() {
        // A chain of 40 bookmarks, each the first child of the one before.
        let items: Vec<String> = (0..40)
            .map(|k| {
                let parent = if k == 0 { 4 } else { 4 + k };
                let first = if k + 1 == 40 { String::new() } else { format!(" /First {} 0 R", 6 + k) };
                format!("<< /Title (Deep {k}) /Parent {parent} 0 R{first} /Dest [3 0 R /Fit] >>")
            })
            .collect();
        let info = inspect(Arc::new(outline_file(&items)), None).expect("opens");
        let mut levels = 0;
        let mut level = info.outline.first();
        while let Some(item) = level {
            levels += 1;
            level = item.children.first();
        }
        assert_eq!(levels, OUTLINE_DEPTH + 1);
        assert!(info.outline_more);
    }

    #[test]
    fn outline_window_ends_cycles_and_broken_links_without_looping() {
        let listed = |items: &[&str]| {
            let items: Vec<String> = items.iter().map(|s| s.to_string()).collect();
            shape(&inspect(Arc::new(outline_file(&items)), None).expect("opens").outline)
        };
        assert_eq!(listed(&["<< /Title (Self) /Parent 4 0 R /Next 5 0 R >>"]), ["Self"]);
        assert_eq!(listed(&["<< /Title (Self) /Parent 4 0 R /First 5 0 R >>"]), ["Self"]);
        assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /Next 6 0 R >>", "<< /Title (B) /Parent 4 0 R /Next 5 0 R >>"]), ["A", "B"]);
        assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /First 6 0 R >>", "<< /Title (B) /Parent 5 0 R /First 5 0 R >>"]), ["A[B]"]);
        assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /Next 99 0 R >>"]), ["A"]);
        assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /Next 6 0 R >>", "42"]), ["A"]);
    }

    #[test]
    fn destination_points_map_to_the_displayed_page() {
        // A crop box away from the origin: x 100..400, y 50..450.
        let mut p = PageInfo { width: 300.0, height: 400.0, label: "1".into(), crop: [100.0, 50.0, 400.0, 450.0], rotation: 0 };
        assert_eq!(p.dest_fraction(Some(100.0), Some(450.0), 0), [Some(0.0), Some(0.0)]);
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.5), Some(0.25)]);
        // Unspecified stays unspecified; off-page and huge values are clamped to the page.
        assert_eq!(p.dest_fraction(None, Some(350.0), 0), [None, Some(0.25)]);
        assert_eq!(p.dest_fraction(Some(-1e30), Some(1e30), 0), [Some(0.0), Some(0.0)]);
        assert_eq!(p.dest_fraction(Some(f32::INFINITY), Some(f32::NAN), 0), [None, None]);
        // /Rotate 90 (clockwise): user y runs left to right across the displayed page, user x
        // top to bottom. A view rotation adds to it.
        p.rotation = 90;
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.75), Some(0.5)]);
        assert_eq!(p.dest_fraction(None, Some(350.0), 0), [Some(0.75), None]);
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 90), [Some(0.5), Some(0.75)]);
        p.rotation = 180;
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.5), Some(0.75)]);
        p.rotation = 270;
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.25), Some(0.5)]);
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 90), [Some(0.5), Some(0.25)]);
        // A degenerate crop box never divides by zero.
        let flat = PageInfo { crop: [0.0, 0.0, 0.0, 0.0], rotation: 0, ..p };
        assert!(flat.dest_fraction(Some(5.0), Some(-5.0), 0).iter().all(|f| f.is_some_and(|f| (0.0..=1.0).contains(&f))));
    }

    #[test]
    fn structure_failure_preserves_renderer_pages() {
        for crash in [true, false] {
            let mut info = DocInfo {
                file_size: 321,
                pdf_version: "1.7".into(),
                pages: vec![PageInfo { width: 300.0, height: 200.0, label: "1".into(), crop: [10.0, 20.0, 210.0, 320.0], rotation: 90 }],
                ..Default::default()
            };
            inspect_structure(&mut info, |tmp| {
                tmp.pages.clear();
                tmp.title = Some("partially inspected".into());
                if crash {
                    panic!("synthetic structure inspection failure");
                }
                Err("synthetic structure inspection failure".into())
            });

            assert_eq!(info.pages.len(), 1);
            let page = &info.pages[0];
            assert_eq!((page.width, page.height, page.label.as_str(), page.crop, page.rotation), (300.0, 200.0, "1", [10.0, 20.0, 210.0, 320.0], 90));
            assert_eq!(info.file_size, 321);
            assert_eq!(info.pdf_version, "1.7");
            assert!(info.title.is_none());
            assert_eq!(info.warnings.len(), 1);
            assert!(info.warnings[0].contains("synthetic structure inspection failure"));
            assert_eq!(info.warnings[0].contains("crashed"), crash);
        }
    }

    #[test]
    fn invalid_dates_with_unicode_are_unchanged() {
        for input in ["", "D:", "D:20260930104", "D:202609x01045", "yesterday"] {
            assert_eq!(pretty_date(input), input);
        }
        // Cover every position before the 12-byte prefix, including characters that
        // straddle its end. None of these strings is an ASCII PDF date.
        for character in ['é', '€', '😀'] {
            for prefix_len in 0..12 {
                let input = format!("D:{}{character}123456789012", "1".repeat(prefix_len));
                assert_eq!(pretty_date(&input), input, "{character} after {prefix_len} digits");
            }
        }
    }

    #[test]
    fn unicode_annotation_date_does_not_prevent_opening() {
        use lopdf::dictionary;

        let date = "D:12345678901éX";
        let mut encoded_date = vec![0xFE, 0xFF];
        encoded_date.extend(date.encode_utf16().flat_map(u16::to_be_bytes));
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let annot_id = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![10.into(), 10.into(), 30.into(), 30.into()],
            "M" => Object::String(encoded_date, lopdf::StringFormat::Hexadecimal),
        });
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
            "Annots" => vec![annot_id.into()],
        });
        doc.objects.insert(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }
            .into(),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("write synthetic fixture");

        let info = inspect(Arc::new(bytes), None).expect("opens despite a non-date /M string");
        assert_eq!(info.pages.len(), 1);
        assert_eq!((info.pages[0].width, info.pages[0].height), (200.0, 300.0));
        assert_eq!(info.annotations.len(), 1);
        assert_eq!(info.annotations[0].modified.as_deref(), Some(date));
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
    }

    #[test]
    fn dates_are_prettified() {
        assert_eq!(pretty_date("D:20260930104512-04'00'"), "2026-09-30 10:45");
        assert_eq!(pretty_date("yesterday"), "yesterday");
    }

    #[test]
    fn jspdf_high_precision_mediabox_does_not_overflow_dimensions() {
        let pdf = b"%PDF-1.3
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 3874.9606299212600788 5493.5433070866147318] >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let info = inspect(std::sync::Arc::new(pdf.to_vec()), None).expect("opens");
        assert_eq!(info.pages.len(), 1);
        let p = &info.pages[0];
        assert!((p.width - 3874.96).abs() < 0.1, "width expected ~3874.96, got {}", p.width);
        assert!((p.height - 5493.54).abs() < 0.1, "height expected ~5493.54, got {}", p.height);
    }
}

/// "1.7" from the parser's version name ("Pdf17", "V1_7", …).
fn version_label(name: &str) -> String {
    let digits: Vec<char> = name.chars().filter(char::is_ascii_digit).collect();
    match digits[..] {
        [a, b] => format!("{a}.{b}"),
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn version_labels_read_as_numbers() {
        assert_eq!(super::version_label("Pdf17"), "1.7");
        assert_eq!(super::version_label("V2_0"), "2.0");
        assert_eq!(super::version_label("Unknown"), "Unknown");
    }
}
