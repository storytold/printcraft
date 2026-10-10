//! The layout engine: places a template's subforms, draws and fields on pages (XFA 3.3 part 2,
//! "Layout"). Supports positioned, top-to-bottom, left-to-right and table/row layouts, page
//! masters with content areas, `breakBefore`/`breakAfter`, and overflow leaders (header rows
//! repeated after a page break). Repeating subforms get their initial instances.
//!
//! Output is a list of pages of resolution-independent items; `pdf` turns them into PDF.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine;

use crate::XfaError;
use crate::data::{DataNode, data_path, is_on, iso_to_pattern};
use crate::model::*;
use crate::text::{self, Block, Face, Span};

/// Most pages a form may lay out to.
pub const MAX_PAGES: usize = 500;
/// Most items (text spans, lines, widgets…) across all pages.
const MAX_ITEMS: usize = 400_000;
/// Deepest nesting followed.
const MAX_DEPTH: usize = 64;
/// Most size measurements of one layout. Measurements of containers are memoized, so a real
/// form stays far below this; it bounds the work a hostile template can cause.
const MAX_MEASURES: usize = 2_000_000;
/// Most initial instances of one repeating subform.
const MAX_INSTANCES: usize = 50;
/// Largest picture taken from the data (decoded bytes), as for template pictures.
const MAX_DATA_IMAGE: usize = 32 << 20;
/// Placeholder for the page count in text laid out before the count is known.
const PAGE_COUNT_MARK: char = '\u{E000}';

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Fill { rect: Rect, color: [f64; 3] },
    Line { from: (f64, f64), to: (f64, f64), width: f64, color: [f64; 3], dashed: bool },
    Text(Span),
    Image { rect: Rect, data: Arc<Vec<u8>>, content_type: String },
    Widget(Box<Widget>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum WidgetKind {
    Text,
    /// A text field whose characters are shown as asterisks.
    Password,
    /// A drop-down (`list_box` false) or list box: `options` are (saved value, shown text).
    Choice {
        options: Vec<(String, String)>,
        list_box: bool,
        multi: bool,
        editable: bool,
    },
    /// A signature field, to be signed in the app.
    Signature,
    /// A date field with its Acrobat format pattern (`yyyy-mm-dd`).
    Date(String),
    CheckBox {
        on: String,
        round: bool,
    },
    Radio {
        group: String,
        on: String,
        round: bool,
    },
    Button {
        caption: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderShape {
    None,
    Full,
    Underline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WidgetBorder {
    pub color: Option<[f64; 3]>,
    pub fill: Option<[f64; 3]>,
    pub shape: BorderShape,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Reset,
    Print,
    SaveAs,
    Url(String),
    /// Any other click script: run by the XFA scripting engine.
    Script(String),
}

/// An interactive field to create as an AcroForm widget.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub rect: Rect,
    pub kind: WidgetKind,
    /// The PDF field name (unique in the form, no periods).
    pub name: String,
    /// The template's scripting object model path, for data round trips.
    pub som: String,
    pub tooltip: Option<String>,
    pub face: Face,
    pub h_align: HAlign,
    pub multiline: bool,
    pub max_chars: Option<usize>,
    pub read_only: bool,
    pub border: WidgetBorder,
    /// The field's value: saved (bound) values for choice lists, newline-separated when several.
    pub value: Option<String>,
    /// The template's default value (what a reset restores); data values are not defaults.
    pub default: Option<String>,
    pub action: Option<Action>,
    /// Check boxes: the on and off values of the data; radio buttons: the on value.
    pub items: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Form {
    pub pages: Vec<Page>,
    pub fields: usize,
    pub warnings: Vec<String>,
}

/// Inherited text defaults and the SOM path of the enclosing containers.
#[derive(Clone)]
struct Ctx {
    font: Font,
    para: Para,
    som: String,
}

impl Ctx {
    fn child(&self, c: &Common) -> Ctx {
        Ctx { font: self.font.apply(&c.font), para: self.para.apply(&c.para), som: self.som.clone() }
    }

    fn with_som(&self, c: &Common, index: usize) -> Ctx {
        let mut out = self.child(c);
        if let Some(n) = &c.name {
            if !out.som.is_empty() {
                out.som.push('.');
            }
            out.som.push_str(&format!("{n}[{index}]"));
        }
        out
    }
}

/// (address of a subform or area in the template, available width bits, depth).
type MeasureKey = (usize, u64, usize);

/// A cached size, with the inherited text defaults it was measured with.
struct Measured {
    font: Font,
    para: Para,
    size: (f64, f64),
}

struct Cursor {
    area: usize,
    content: usize,
    rect: Rect,
    y: f64,
    /// Something from the flow was placed in this content area.
    placed: bool,
}

struct Layouter<'t> {
    tpl: &'t Template,
    data: Option<&'t DataNode>,
    /// Shared so a page's master items keep their addresses (the measurement cache keys on them).
    areas: Rc<Vec<PageArea>>,
    uses: Vec<usize>,
    pages: Vec<Page>,
    cur: Option<Cursor>,
    names: HashMap<String, usize>,
    /// Field values by `id`, for embedded fields in rich text.
    values: HashMap<String, String>,
    warnings: Vec<String>,
    warned: HashSet<String>,
    fields: usize,
    items: usize,
    /// Container sizes by (address of the subform or area in the template, available width,
    /// depth), with the inherited font and paragraph they were measured with.
    measured: RefCell<HashMap<MeasureKey, Measured>>,
    /// Measurements made so far (see [`MAX_MEASURES`]).
    measures: Cell<usize>,
}

/// How many instances of a repeating subform to lay out: what the data holds once it holds
/// any (rows added or removed), else the template's initial count; never below one, never
/// above the maximum.
pub fn instance_count(occur: &Occur, from_data: usize) -> usize {
    let n = if from_data > 0 { from_data } else { occur.initial };
    n.min(occur.max.unwrap_or(usize::MAX)).clamp(1, MAX_INSTANCES)
}

/// Index of child `i` among the earlier siblings with the same name (the SOM index).
fn sib_index(children: &[Node], i: usize) -> usize {
    let name = children.get(i).and_then(|c| c.common().name.as_deref());
    children.iter().take(i).filter(|c| c.common().name.as_deref() == name && name.is_some()).count()
}

/// Lay a template out, with the values and repeat counts of `data` (the `xfa:data` element).
pub fn layout(tpl: &Template, data: Option<&DataNode>) -> Result<Form, XfaError> {
    let areas = match &tpl.root.page_set {
        Some(ps) if !ps.areas.is_empty() => ps.areas.clone(),
        _ => vec![PageArea {
            name: None,
            width: 612.0,
            height: 792.0,
            content: vec![Rect::new(36.0, 36.0, 540.0, 720.0)],
            items: Vec::new(),
            occur_max: None,
        }],
    };
    let mut values = HashMap::new();
    // One copy of the root for the whole layout: measurements are cached by node address.
    let root_node = Node::Subform(Box::new(tpl.root.clone()));
    collect_values(std::slice::from_ref(&root_node), &mut values, 0);
    let mut l = Layouter {
        tpl,
        data,
        uses: vec![0; areas.len()],
        areas: Rc::new(areas),
        pages: Vec::new(),
        cur: None,
        names: HashMap::new(),
        values,
        warnings: Vec::new(),
        warned: HashSet::new(),
        fields: 0,
        items: 0,
        measured: RefCell::new(HashMap::new()),
        measures: Cell::new(0),
    };
    let ctx = Ctx { font: Font::default(), para: Para::default(), som: String::new() };
    l.new_page()?;
    if tpl.root.common.presence.occupies() {
        l.flow_subform(&root_node, None, &ctx, 0, 0)?;
    }
    l.check_measures()?;
    let total = l.pages.len();
    let count = total.to_string();
    for p in &mut l.pages {
        for it in &mut p.items {
            if let Item::Text(s) = it
                && s.text.contains(PAGE_COUNT_MARK)
            {
                s.text = s.text.replace(PAGE_COUNT_MARK, &count);
            }
        }
    }
    Ok(Form { pages: l.pages, fields: l.fields, warnings: l.warnings })
}

fn collect_values(nodes: &[Node], out: &mut HashMap<String, String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for n in nodes {
        match n {
            Node::Field(f) => {
                if let Some(id) = &f.common.id {
                    out.insert(id.clone(), f.value.plain().unwrap_or_default());
                }
            }
            Node::Subform(s) => {
                collect_values(&s.children, out, depth + 1);
                if let Some(ps) = &s.page_set {
                    for a in &ps.areas {
                        collect_values(&a.items, out, depth + 1);
                    }
                }
            }
            Node::Area(a) => collect_values(&a.children, out, depth + 1),
            Node::ExclGroup(_) | Node::Draw(_) => {}
        }
    }
}

fn clamp_opt(v: f64, min: Option<f64>, max: Option<f64>) -> f64 {
    let mut v = v;
    if let Some(m) = min {
        v = v.max(m);
    }
    if let Some(m) = max
        && m > 0.0
    {
        v = v.min(m);
    }
    v.max(0.0)
}

/// Does `node` flow (may be split across pages) rather than being placed as one block?
fn splittable(node: &Node) -> bool {
    matches!(node, Node::Subform(s) if matches!(s.layout, Layout::Tb | Layout::Table) && s.common.h.is_none())
}

fn is_row(node: &Node) -> bool {
    matches!(node, Node::Subform(s) if s.layout == Layout::Row)
}

/// The Acrobat date pattern for an XFA picture clause such as `date{YYYY-MM-DD}`.
fn date_pattern(picture: Option<&str>) -> String {
    let p = picture.unwrap_or("");
    let inner = p.find('{').and_then(|a| p[a + 1..].find('}').map(|b| &p[a + 1..a + 1 + b])).unwrap_or(p);
    let inner = inner.trim();
    if inner.is_empty() || inner.chars().any(|c| !"YMDEJ-/., :".contains(c)) {
        return "yyyy-mm-dd".into();
    }
    inner.replace('E', "d").to_ascii_lowercase()
}

fn button_action(scripts: &[Script]) -> Option<Action> {
    let s = scripts.iter().find(|s| s.activity == "click")?;
    let t = &s.text;
    // Only a single-statement idiom becomes a native PDF action; anything else is run by the
    // XFA scripting engine.
    let one = t.trim().trim_end_matches(';');
    let single = !one.contains(';') && !one.contains('\n');
    if !single {
        return (!t.trim().is_empty()).then(|| Action::Script(t.clone()));
    }
    if t.contains("resetData") {
        Some(Action::Reset)
    } else if t.contains("host.print") || t.contains("execMenuItem(\"Print\")") {
        Some(Action::Print)
    } else if t.contains("SaveAs") || t.contains("execMenuItem(\"Save\")") {
        Some(Action::SaveAs)
    } else if let Some(i) = t.find("launchURL(") {
        let rest = &t[i + "launchURL(".len()..];
        let q = rest.find(['"', '\''])?;
        let quote = rest[q..].chars().next()?;
        let body = &rest[q + 1..];
        let end = body.find(quote)?;
        Some(Action::Url(body[..end].to_string()))
    } else if t.trim().is_empty() {
        None
    } else {
        Some(Action::Script(t.clone()))
    }
}

/// A PDF field name: no periods (they separate hierarchy levels), no control characters.
fn clean_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !c.is_control()).map(|c| if c == '.' { '_' } else { c }).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "field".into() } else { s }
}

impl Layouter<'_> {
    fn warn_once(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        if self.warned.insert(m.clone()) && self.warnings.len() < 100 {
            self.warnings.push(m);
        }
    }

    fn page_no(&self) -> usize {
        self.pages.len()
    }

    fn embed(&self, id: &str) -> String {
        match self.tpl.page_roles.iter().find(|(i, _)| i == id).map(|(_, r)| *r) {
            Some(PageRole::Number) => self.page_no().to_string(),
            Some(PageRole::Count) => PAGE_COUNT_MARK.to_string(),
            None => self.values.get(id).cloned().unwrap_or_default(),
        }
    }

    /// `TooLarge` once the measurement budget is spent (sizes measured after that are guesses).
    fn check_measures(&self) -> Result<(), XfaError> {
        if self.measures.get() > MAX_MEASURES {
            return Err(XfaError::TooLarge("the form is nested too deeply to lay out".into()));
        }
        Ok(())
    }

    fn push(&mut self, item: Item) -> Result<(), XfaError> {
        self.check_measures()?;
        self.items += 1;
        if self.items > MAX_ITEMS {
            return Err(XfaError::TooLarge("the form has too many objects to lay out".into()));
        }
        if let Some(p) = self.pages.last_mut() {
            p.items.push(item);
        }
        Ok(())
    }

    fn push_block(&mut self, b: Block) -> Result<(), XfaError> {
        for s in b.spans {
            if s.face.underline {
                let w = s.face.width(&s.text);
                let y = s.baseline + s.face.size * 0.1;
                self.push(Item::Line { from: (s.x, y), to: (s.x + w, y), width: (s.face.size * 0.06).max(0.3), color: s.face.color, dashed: false })?;
            }
            if !s.text.trim().is_empty() {
                self.push(Item::Text(s))?;
            }
        }
        Ok(())
    }

    // ───────────────────────────────────────────────────────────────────────── pages

    fn new_page(&mut self) -> Result<(), XfaError> {
        if self.pages.len() >= MAX_PAGES {
            return Err(XfaError::TooLarge(format!("the form lays out to more than {MAX_PAGES} pages")));
        }
        let idx =
            (0..self.areas.len()).find(|&i| self.areas[i].occur_max.is_none_or(|m| self.uses[i] < m)).unwrap_or(self.areas.len().saturating_sub(1));
        let areas = Rc::clone(&self.areas);
        let Some(area) = areas.get(idx) else {
            return Err(XfaError::Malformed("the template has no page area".into()));
        };
        if let Some(u) = self.uses.get_mut(idx) {
            *u += 1;
        }
        self.pages.push(Page { width: area.width, height: area.height, items: Vec::new() });
        let content = area.content.first().copied().unwrap_or(Rect::new(0.0, 0.0, area.width, area.height));
        self.cur = Some(Cursor { area: idx, content: 0, rect: content, y: content.y, placed: false });
        // Master page content: positioned draws and fields.
        let ctx = Ctx { font: Font::default(), para: Para::default(), som: area.name.clone().map(|n| format!("{n}[0]")).unwrap_or_default() };
        let page_rect = Rect::new(0.0, 0.0, area.width, area.height);
        for (i, item) in area.items.iter().enumerate() {
            self.draw_positioned_child(item, page_rect, &ctx, 1, sib_index(&area.items, i))?;
        }
        Ok(())
    }

    /// Move to the next content area, or the next page.
    fn next_area(&mut self) -> Result<(), XfaError> {
        if let Some(c) = &self.cur
            && let Some(area) = self.areas.get(c.area)
            && let Some(next) = area.content.get(c.content + 1).copied()
        {
            let i = c.content + 1;
            self.cur = Some(Cursor { area: c.area, content: i, rect: next, y: next.y, placed: false });
            return Ok(());
        }
        self.new_page()
    }

    fn cursor(&self) -> (Rect, f64, bool) {
        match &self.cur {
            Some(c) => (c.rect, c.y, c.placed),
            None => (Rect::new(0.0, 0.0, 612.0, 792.0), 0.0, false),
        }
    }

    /// Move the cursor down (margins and spacing: the area still counts as empty).
    fn advance(&mut self, dy: f64) {
        if let Some(c) = &mut self.cur {
            c.y += dy.max(0.0);
        }
    }

    /// Move past something that was placed.
    fn place(&mut self, dy: f64) {
        if let Some(c) = &mut self.cur {
            c.y += dy.max(0.0);
            c.placed = true;
        }
    }

    /// Make room for `h`: stay, or move to the next content area / page. Returns whether a
    /// break happened.
    fn make_room(&mut self, h: f64) -> Result<bool, XfaError> {
        let (rect, y, placed) = self.cursor();
        if y + h <= rect.bottom() + 0.01 || !placed {
            return Ok(false);
        }
        self.next_area()?;
        Ok(true)
    }

    fn page_has_flow(&self) -> bool {
        self.cur.as_ref().is_some_and(|c| c.placed || c.content > 0)
    }

    // ───────────────────────────────────────────────────────────────────────── flow

    /// Flow `node` (a subform; anything else is ignored) into the content areas.
    fn flow_subform(&mut self, node: &Node, avail: Option<(f64, f64)>, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let Node::Subform(sf) = node else { return Ok(()) };
        self.check_measures()?;
        if sf.break_before_page && self.page_has_flow() {
            self.new_page()?;
        }
        // Data adds instances of a repeating subform (rows added in another viewer).
        let instances = self.instances(node, &ctx.som);
        let ctx_parent = ctx;
        for i in 0..instances {
            let ctx = ctx_parent.with_som(&sf.common, sib + i);
            let (rect, _, _) = self.cursor();
            let (x, w) = avail.unwrap_or((rect.x, rect.w));
            let w = clamp_opt(sf.common.w.unwrap_or(w), sf.common.min_w, sf.common.max_w).min(w.max(1.0));
            if !matches!(sf.layout, Layout::Tb | Layout::Table) || sf.common.h.is_some() {
                // Not splittable: one block.
                let (_, h) = self.size_of(node, w, ctx_parent, depth);
                self.make_room(h)?;
                let (_, y, _) = self.cursor();
                self.draw_node(node, Rect::new(x, y, w, h), ctx_parent, depth, sib + i)?;
                self.place(h);
                continue;
            }
            let m = sf.common.margin;
            self.advance(m.top);
            let inner_x = x + m.left;
            let inner_w = (w - m.horizontal()).max(1.0);
            match sf.layout {
                Layout::Table => self.flow_table(sf, inner_x, inner_w, &ctx, depth)?,
                _ => {
                    for (ci, child) in sf.children.iter().enumerate() {
                        if !child.common().presence.occupies() {
                            continue;
                        }
                        let csib = sib_index(&sf.children, ci);
                        if let Node::Subform(_) = child
                            && splittable(child)
                        {
                            self.flow_subform(child, Some((inner_x, inner_w)), &ctx, depth + 1, csib)?;
                            continue;
                        }
                        let (cw, ch) = self.size_of(child, inner_w, &ctx, depth + 1);
                        self.make_room(ch)?;
                        let (_, y, _) = self.cursor();
                        self.draw_node(child, Rect::new(inner_x, y, cw, ch), &ctx, depth + 1, csib)?;
                        self.place(ch);
                    }
                }
            }
            self.advance(m.bottom);
        }
        if sf.break_after_page {
            self.new_page()?;
        }
        Ok(())
    }

    fn columns(&self, sf: &Subform, inner_w: f64) -> Vec<f64> {
        if !sf.column_widths.is_empty() {
            return sf.column_widths.clone();
        }
        let cells = sf.children.iter().filter_map(|c| if let Node::Subform(r) = c { Some(r.children.len()) } else { None }).max().unwrap_or(1).max(1);
        vec![inner_w / cells as f64; cells]
    }

    fn flow_table(&mut self, sf: &Subform, x: f64, inner_w: f64, ctx: &Ctx, depth: usize) -> Result<(), XfaError> {
        let cols = self.columns(sf, inner_w);
        let leader =
            sf.overflow_leader.as_ref().and_then(|name| sf.children.iter().position(|c| c.common().name.as_deref() == Some(name) && is_row(c)));
        for (ci, child) in sf.children.iter().enumerate() {
            if !child.common().presence.occupies() {
                continue;
            }
            let csib = sib_index(&sf.children, ci);
            if let Node::Subform(row) = child
                && row.layout == Layout::Row
            {
                // Rows added as data (in another viewer) repeat the row.
                let instances = self.instances(child, &ctx.som);
                for inst in 0..instances {
                    let h = self.row_height(row, &cols, inner_w, ctx, depth + 1);
                    if self.make_room(h)?
                        && let Some(li) = leader
                        && li != ci
                        && let Some(Node::Subform(l)) = sf.children.get(li)
                    {
                        let lh = self.row_height(l, &cols, inner_w, ctx, depth + 1);
                        let (_, y, _) = self.cursor();
                        self.draw_row(l, &cols, Rect::new(x, y, inner_w, lh), ctx, depth + 1, sib_index(&sf.children, li))?;
                        self.place(lh);
                    }
                    let (_, y, _) = self.cursor();
                    self.draw_row(row, &cols, Rect::new(x, y, inner_w, h), ctx, depth + 1, csib + inst)?;
                    self.place(h);
                }
                continue;
            }
            if splittable(child)
                && let Node::Subform(_) = child
            {
                self.flow_subform(child, Some((x, inner_w)), ctx, depth + 1, csib)?;
                continue;
            }
            let (cw, ch) = self.size_of(child, inner_w, ctx, depth + 1);
            self.make_room(ch)?;
            let (_, y, _) = self.cursor();
            self.draw_node(child, Rect::new(x, y, cw, ch), ctx, depth + 1, csib)?;
            self.place(ch);
        }
        Ok(())
    }

    // ───────────────────────────────────────────────────────────────────────── sizing

    /// The size a node takes when given `avail_w` of width. Containers are measured once per
    /// (node, width, depth): measuring a width-less container measures its content twice (for
    /// its width, then its height), which would otherwise be exponential in the nesting.
    /// How many instances of `node` to lay out under the container at `parent_som`: a
    /// repeating subform's `occur` and data instances ([`instance_count`]); one for anything else.
    fn instances(&self, node: &Node, parent_som: &str) -> usize {
        let Node::Subform(sf) = node else { return 1 };
        let from_data = match (&sf.common.name, self.data) {
            (Some(name), Some(d)) if sf.occur.max != Some(1) => d.count(&data_path(self.tpl, parent_som), name),
            _ => 0,
        };
        instance_count(&sf.occur, from_data)
    }

    fn size_of(&self, node: &Node, avail_w: f64, ctx: &Ctx, depth: usize) -> (f64, f64) {
        let c = node.common();
        let spent = self.measures.get().saturating_add(1);
        self.measures.set(spent);
        if depth > MAX_DEPTH || spent > MAX_MEASURES {
            return (c.w.unwrap_or(avail_w), c.h.unwrap_or(0.0));
        }
        // Only containers recurse; their addresses are stable for the whole layout (the
        // template is borrowed, the root and page areas are held by the layouter).
        let key = match node {
            Node::Subform(s) => Some((&**s as *const Subform as usize, avail_w.to_bits(), depth)),
            Node::Area(a) => Some((&**a as *const Area as usize, avail_w.to_bits(), depth)),
            _ => None,
        };
        if let Some(key) = key
            && let Ok(cache) = self.measured.try_borrow()
            && let Some(m) = cache.get(&key)
            && m.font == ctx.font
            && m.para == ctx.para
        {
            return m.size;
        }
        let size = self.measure(node, avail_w, ctx, depth);
        if let Some(key) = key
            && let Ok(mut cache) = self.measured.try_borrow_mut()
        {
            cache.insert(key, Measured { font: ctx.font.clone(), para: ctx.para.clone(), size });
        }
        size
    }

    fn measure(&self, node: &Node, avail_w: f64, ctx: &Ctx, depth: usize) -> (f64, f64) {
        let c = node.common();
        // The container's own SOM, under which its repeating children count their instances.
        // Measurements are cached per template node, so instance 0 stands for all of them.
        let som = ctx.with_som(c, 0).som;
        let ctx = ctx.child(c);
        // A container without a width is as wide as its content (a radio group beside its
        // question, say); a leaf without one takes what is available.
        let w = match c.w {
            Some(w) => w,
            None => match node {
                Node::Subform(_) | Node::ExclGroup(_) | Node::Area(_) => {
                    let inner = (avail_w - c.margin.horizontal()).max(1.0);
                    (self.content_width(node, inner, &ctx, depth) + c.margin.horizontal()).min(avail_w)
                }
                Node::Field(_) | Node::Draw(_) => avail_w,
            },
        };
        let w = clamp_opt(w, c.min_w, c.max_w);
        let h = match c.h {
            Some(h) => h,
            None => {
                let inner_w = (w - c.margin.horizontal()).max(1.0);
                let content = match node {
                    Node::Draw(d) => self.value_height(&d.value, inner_w, &ctx),
                    Node::Field(f) => self.field_height(f, inner_w, &ctx),
                    Node::ExclGroup(g) => self.group_height(g, inner_w, &ctx, depth),
                    Node::Area(a) => self.positioned_height(&a.children, inner_w, &ctx, depth),
                    Node::Subform(s) => match s.layout {
                        Layout::Positioned => self.positioned_height(&s.children, inner_w, &ctx, depth),
                        // Each instance of a repeating child takes its height again.
                        Layout::Tb => s
                            .children
                            .iter()
                            .filter(|k| k.common().presence.occupies())
                            .map(|k| self.size_of(k, inner_w, &ctx, depth + 1).1 * self.instances(k, &som) as f64)
                            .sum(),
                        Layout::LrTb | Layout::RlTb => self.packed_height(&s.children, inner_w, &ctx, depth),
                        Layout::Table => {
                            let cols = self.columns(s, inner_w);
                            s.children
                                .iter()
                                .filter(|k| k.common().presence.occupies())
                                .map(|k| match k {
                                    Node::Subform(r) if r.layout == Layout::Row => self.row_height(r, &cols, inner_w, &ctx, depth + 1),
                                    _ => self.size_of(k, inner_w, &ctx, depth + 1).1,
                                })
                                .sum()
                        }
                        Layout::Row => self.row_height(s, &self.columns(s, inner_w), inner_w, &ctx, depth + 1),
                    },
                };
                content + c.margin.vertical()
            }
        };
        (w, clamp_opt(h, c.min_h, c.max_h))
    }

    /// The natural width of a container's content within `avail_w`.
    fn content_width(&self, node: &Node, avail_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        let group: Vec<Node>;
        let (children, layout): (&[Node], Layout) = match node {
            Node::Subform(s) => {
                if s.layout == Layout::Table && !s.column_widths.is_empty() {
                    return s.column_widths.iter().sum::<f64>().min(avail_w);
                }
                (&s.children, s.layout)
            }
            Node::ExclGroup(g) => {
                // Fields are leaves: measuring these copies caches nothing.
                group = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
                (&group, g.layout)
            }
            Node::Area(a) => (&a.children, Layout::Positioned),
            _ => return avail_w,
        };
        let visible = children.iter().filter(|k| k.common().presence.occupies());
        match layout {
            Layout::LrTb | Layout::RlTb | Layout::Row => {
                let sum: f64 = visible.map(|k| self.size_of(k, avail_w, ctx, depth + 1).0).sum();
                if sum <= avail_w + 0.05 { sum } else { avail_w }
            }
            Layout::Positioned => {
                visible.map(|k| k.common().x.unwrap_or(0.0) + self.size_of(k, avail_w, ctx, depth + 1).0).fold(0.0, f64::max).min(avail_w)
            }
            Layout::Tb | Layout::Table => visible.map(|k| self.size_of(k, avail_w, ctx, depth + 1).0).fold(0.0, f64::max).min(avail_w),
        }
    }

    fn value_height(&self, v: &Value, w: f64, ctx: &Ctx) -> f64 {
        match v {
            Value::Text(t) => text::measure_height(&text::plain(t), w, &ctx.para, &Face::of(&ctx.font), &|id| self.embed(id)),
            Value::Rich(r) => text::measure_height(r, w, &ctx.para, &Face::of(&ctx.font), &|id| self.embed(id)),
            Value::Image { data, .. } => crate::image::size(data).map_or(w, |(iw, ih)| if iw > 0 { w * ih as f64 / iw as f64 } else { w }),
            Value::Line { .. } | Value::Rectangle(_) | Value::Empty => 0.0,
        }
    }

    fn caption_reserve(&self, f: &Field, inner_w: f64, ctx: &Ctx) -> Option<(Placement, f64)> {
        let cap = f.caption.as_ref()?;
        if !cap.presence.occupies() || f.ui == Ui::Button {
            return None;
        }
        if cap.value.is_empty() && cap.reserve.is_none() {
            return None;
        }
        let placement = if cap.placement == Placement::Inline { Placement::Left } else { cap.placement };
        let font = ctx.font.apply(&cap.font);
        let para = ctx.para.apply(&cap.para);
        let reserve = cap.reserve.unwrap_or_else(|| match placement {
            Placement::Top | Placement::Bottom => {
                let rich = match &cap.value {
                    Value::Rich(r) => r.clone(),
                    other => text::plain(&other.plain().unwrap_or_default()),
                };
                text::measure_height(&rich, inner_w, &para, &Face::of(&font), &|id| self.embed(id))
            }
            _ => (Face::of(&font).width(&cap.value.plain().unwrap_or_default()) + 2.0).min(inner_w * 0.6),
        });
        Some((placement, reserve))
    }

    fn field_height(&self, f: &Field, inner_w: f64, ctx: &Ctx) -> f64 {
        let font = ctx.font.apply(&f.common.font);
        let lines = if f.multiline { 3.0 } else { 1.0 };
        let ui = match f.ui {
            Ui::CheckButton => f.check_size.unwrap_or(10.0).max(text::line_height(font.size)),
            _ => text::line_height(font.size) * lines + 4.0 + f.ui_margin.vertical(),
        };
        match self.caption_reserve(f, inner_w, ctx) {
            Some((Placement::Top | Placement::Bottom, r)) => ui + r,
            Some((_, r)) => {
                let cap = f.caption.as_ref().map(|c| {
                    let rich = match &c.value {
                        Value::Rich(r) => r.clone(),
                        other => text::plain(&other.plain().unwrap_or_default()),
                    };
                    text::measure_height(&rich, (inner_w - r).max(1.0), &ctx.para.apply(&c.para), &Face::of(&ctx.font.apply(&c.font)), &|id| {
                        self.embed(id)
                    })
                });
                ui.max(cap.unwrap_or(0.0))
            }
            None => ui,
        }
    }

    fn group_height(&self, g: &ExclGroup, inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        let nodes: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
        match g.layout {
            Layout::Tb => nodes.iter().filter(|k| k.common().presence.occupies()).map(|k| self.size_of(k, inner_w, ctx, depth + 1).1).sum(),
            Layout::Positioned => self.positioned_height(&nodes, inner_w, ctx, depth),
            _ => self.packed_height(&nodes, inner_w, ctx, depth),
        }
    }

    fn positioned_height(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        children
            .iter()
            .filter(|k| k.common().presence.occupies())
            .map(|k| {
                let c = k.common();
                let (_, h) = self.size_of(k, (inner_w - c.x.unwrap_or(0.0)).max(1.0), ctx, depth + 1);
                c.y.unwrap_or(0.0) + h
            })
            .fold(0.0, f64::max)
    }

    /// Left-to-right packing: rows of children, wrapped at the available width.
    fn pack(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize) -> (Vec<(usize, Rect)>, f64) {
        let mut out = Vec::new();
        let (mut x, mut y, mut line_h) = (0.0, 0.0, 0.0);
        for (i, k) in children.iter().enumerate() {
            if !k.common().presence.occupies() {
                continue;
            }
            let (w, h) = self.size_of(k, inner_w, ctx, depth + 1);
            if x > 0.0 && x + w > inner_w + 0.05 {
                y += line_h;
                x = 0.0;
                line_h = 0.0;
            }
            out.push((i, Rect::new(x, y, w, h)));
            x += w;
            line_h = line_h.max(h);
        }
        (out, y + line_h)
    }

    fn packed_height(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        self.pack(children, inner_w, ctx, depth).1
    }

    /// Cell rectangles of a row (relative to the row's inner box) and the row height.
    fn cells(&self, row: &Subform, cols: &[f64], inner_w: f64, ctx: &Ctx, depth: usize) -> (Vec<(usize, Rect)>, f64) {
        let ctx = ctx.child(&row.common);
        let mut out = Vec::new();
        let mut col = 0usize;
        let mut x = 0.0;
        let mut h: f64 = 0.0;
        for (i, k) in row.children.iter().enumerate() {
            if !k.common().presence.occupies() {
                continue;
            }
            let span = k.common().col_span.max(1);
            let w = if col < cols.len() { cols.iter().skip(col).take(span).sum::<f64>() } else { k.common().w.unwrap_or((inner_w - x).max(1.0)) };
            let (_, ch) = self.size_of(k, w, &ctx, depth + 1);
            out.push((i, Rect::new(x, 0.0, w, ch)));
            h = h.max(ch);
            x += w;
            col += span;
        }
        let h = match row.common.h {
            Some(rh) => rh,
            None => h + row.common.margin.vertical(),
        };
        (out, clamp_opt(h, row.common.min_h, row.common.max_h))
    }

    fn row_height(&self, row: &Subform, cols: &[f64], inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        self.cells(row, cols, inner_w, ctx, depth).1
    }

    // ───────────────────────────────────────────────────────────────────────── drawing

    fn draw_row(&mut self, row: &Subform, cols: &[f64], rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        if let Some(b) = &row.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.with_som(&row.common, sib);
        let inner = rect.inset(&row.common.margin);
        let (cells, _) = self.cells(row, cols, inner.w, &ctx, depth);
        for (i, r) in cells {
            if let Some(k) = row.children.get(i) {
                // Cells stretch to the row height.
                self.draw_node(k, Rect::new(inner.x + r.x, inner.y, r.w, inner.h), &ctx, depth + 1, sib_index(&row.children, i))?;
            }
        }
        Ok(())
    }

    fn draw_positioned_child(&mut self, k: &Node, inner: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        let c = k.common();
        if !c.presence.occupies() {
            return Ok(());
        }
        let (x, y) = (c.x.unwrap_or(0.0), c.y.unwrap_or(0.0));
        let (w, h) = self.size_of(k, (inner.w - x).max(1.0), ctx, depth + 1);
        self.draw_node(k, Rect::new(inner.x + x, inner.y + y, w, h), ctx, depth + 1, sib)
    }

    fn draw_node(&mut self, node: &Node, rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let c = node.common();
        if c.presence == Presence::Invisible {
            return Ok(());
        }
        match node {
            Node::Draw(d) => self.draw_draw(d, rect, ctx),
            Node::Field(f) => self.draw_field(f, rect, ctx, None, sib),
            Node::ExclGroup(g) => self.draw_group(g, rect, ctx, depth, sib),
            Node::Area(a) => {
                let ctx = ctx.with_som(&a.common, sib);
                for (i, k) in a.children.iter().enumerate() {
                    self.draw_positioned_child(k, rect, &ctx, depth + 1, sib_index(&a.children, i))?;
                }
                Ok(())
            }
            Node::Subform(s) => {
                if let Some(b) = &s.common.border {
                    self.draw_border(b, rect)?;
                }
                let parent_ctx = ctx;
                let ctx = ctx.with_som(&s.common, sib);
                let inner = rect.inset(&s.common.margin);
                match s.layout {
                    Layout::Positioned => {
                        for (i, k) in s.children.iter().enumerate() {
                            self.draw_positioned_child(k, inner, &ctx, depth + 1, sib_index(&s.children, i))?;
                        }
                    }
                    Layout::Tb => {
                        let mut y = inner.y;
                        for (i, k) in s.children.iter().enumerate() {
                            if !k.common().presence.occupies() {
                                continue;
                            }
                            let (w, h) = self.size_of(k, inner.w, &ctx, depth + 1);
                            // A repeating subform here (a table in an area of a positioned page,
                            // say) gets its instances, as in a flowed one, one under another.
                            for instance in 0..self.instances(k, &ctx.som) {
                                self.draw_node(k, Rect::new(inner.x, y, w, h), &ctx, depth + 1, sib_index(&s.children, i) + instance)?;
                                y += h;
                            }
                        }
                    }
                    Layout::LrTb | Layout::RlTb => {
                        let (placed, _) = self.pack(&s.children, inner.w, &ctx, depth);
                        for (i, r) in placed {
                            if let Some(k) = s.children.get(i) {
                                let x = if s.layout == Layout::RlTb { inner.right() - r.x - r.w } else { inner.x + r.x };
                                self.draw_node(k, Rect::new(x, inner.y + r.y, r.w, r.h), &ctx, depth + 1, sib_index(&s.children, i))?;
                            }
                        }
                    }
                    Layout::Table => {
                        let cols = self.columns(s, inner.w);
                        let mut y = inner.y;
                        for (i, k) in s.children.iter().enumerate() {
                            if !k.common().presence.occupies() {
                                continue;
                            }
                            let csib = sib_index(&s.children, i);
                            if let Node::Subform(r) = k
                                && r.layout == Layout::Row
                            {
                                let h = self.row_height(r, &cols, inner.w, &ctx, depth + 1);
                                self.draw_row(r, &cols, Rect::new(inner.x, y, inner.w, h), &ctx, depth + 1, csib)?;
                                y += h;
                            } else {
                                let (w, h) = self.size_of(k, inner.w, &ctx, depth + 1);
                                self.draw_node(k, Rect::new(inner.x, y, w, h), &ctx, depth + 1, csib)?;
                                y += h;
                            }
                        }
                    }
                    Layout::Row => {
                        let cols = self.columns(s, inner.w);
                        self.draw_row(s, &cols, rect, parent_ctx, depth, sib)?;
                    }
                }
                Ok(())
            }
        }
    }

    fn draw_border(&mut self, b: &Border, rect: Rect) -> Result<(), XfaError> {
        if let Some(fill) = b.fill() {
            self.push(Item::Fill { rect, color: fill.0 })?;
        }
        let vis = b.visible_edges();
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
        // Top, right, bottom, left; lines sit inside the box.
        let segs = [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x0, y1), (x1, y1)), ((x0, y0), (x0, y1))];
        for (i, seg) in segs.iter().enumerate() {
            if !vis[i] {
                continue;
            }
            let e = b.edges[i];
            let t = e.thickness.clamp(0.1, 20.0);
            let (dx, dy) = match i {
                0 => (0.0, t / 2.0),
                1 => (-t / 2.0, 0.0),
                2 => (0.0, -t / 2.0),
                _ => (t / 2.0, 0.0),
            };
            let color = if e.stroke == Stroke::Relief { [0.55, 0.55, 0.55] } else { e.color.0 };
            self.push(Item::Line {
                from: (seg.0.0 + dx, seg.0.1 + dy),
                to: (seg.1.0 + dx, seg.1.1 + dy),
                width: t,
                color,
                dashed: matches!(e.stroke, Stroke::Dashed | Stroke::Dotted),
            })?;
        }
        Ok(())
    }

    fn draw_value_text(&mut self, v: &Value, rect: Rect, font: &Font, para: &Para) -> Result<(), XfaError> {
        let rich = match v {
            Value::Text(t) => text::plain(t),
            Value::Rich(r) => r.clone(),
            _ => return Ok(()),
        };
        let block = text::layout(&rich, rect, para, &Face::of(font), &|id| self.embed(id));
        self.push_block(block)
    }

    fn draw_draw(&mut self, d: &Draw, rect: Rect, ctx: &Ctx) -> Result<(), XfaError> {
        if let Some(b) = &d.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.child(&d.common);
        let inner = rect.inset(&d.common.margin);
        match &d.value {
            Value::Text(_) | Value::Rich(_) => self.draw_value_text(&d.value, inner, &ctx.font, &ctx.para),
            Value::Image { content_type, data } => self.draw_image(data, content_type, inner),
            Value::Line { slope_up, edge } => {
                if !edge.visible() {
                    return Ok(());
                }
                let (from, to) = if *slope_up {
                    ((inner.x, inner.bottom()), (inner.right(), inner.y))
                } else {
                    ((inner.x, inner.y), (inner.right(), inner.bottom()))
                };
                self.push(Item::Line { from, to, width: edge.thickness.clamp(0.1, 20.0), color: edge.color.0, dashed: edge.stroke == Stroke::Dashed })
            }
            Value::Rectangle(b) => self.draw_border(b, inner),
            Value::Empty => Ok(()),
        }
    }

    fn draw_group(&mut self, g: &ExclGroup, rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if let Some(b) = &g.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.with_som(&g.common, sib);
        // The group's data node holds the selected button's on value.
        let selected: Option<String> =
            self.data.and_then(|d| d.text_at(&data_path(self.tpl, &ctx.som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let inner = rect.inset(&g.common.margin);
        let group = self.unique_name(g.common.name.as_deref().unwrap_or("group"));
        let nodes: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
        let placed: Vec<(usize, Rect)> = match g.layout {
            Layout::Tb => {
                let mut y = 0.0;
                let mut out = Vec::new();
                for (i, k) in nodes.iter().enumerate() {
                    if !k.common().presence.occupies() {
                        continue;
                    }
                    let (w, h) = self.size_of(k, inner.w, &ctx, depth + 1);
                    out.push((i, Rect::new(0.0, y, w, h)));
                    y += h;
                }
                out
            }
            Layout::Positioned => nodes
                .iter()
                .enumerate()
                .filter(|(_, k)| k.common().presence.occupies())
                .map(|(i, k)| {
                    let c = k.common();
                    let (w, h) = self.size_of(k, (inner.w - c.x.unwrap_or(0.0)).max(1.0), &ctx, depth + 1);
                    (i, Rect::new(c.x.unwrap_or(0.0), c.y.unwrap_or(0.0), w, h))
                })
                .collect(),
            _ => self.pack(&nodes, inner.w, &ctx, depth).0,
        };
        for (i, r) in placed {
            if let Some(f) = g.fields.get(i) {
                let on = f.items.first().cloned().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| (i + 1).to_string());
                let tip = f.tooltip.clone().or_else(|| g.tooltip.clone());
                let chosen = selected.as_deref() == Some(on.as_str());
                self.draw_field(f, Rect::new(inner.x + r.x, inner.y + r.y, r.w, r.h), &ctx, Some((&group, &on, tip, chosen)), sib_index(&nodes, i))?;
            }
        }
        Ok(())
    }

    /// A picture scaled to fit `inner`, keeping its shape, from its top left.
    fn draw_image(&mut self, data: &[u8], content_type: &str, inner: Rect) -> Result<(), XfaError> {
        let fitted = match crate::image::size(data) {
            Some((iw, ih)) if iw > 0 && ih > 0 && inner.w > 0.0 && inner.h > 0.0 => {
                let k = (inner.w / iw as f64).min(inner.h / ih as f64);
                Rect::new(inner.x, inner.y, iw as f64 * k, ih as f64 * k)
            }
            _ => inner,
        };
        self.push(Item::Image { rect: fitted, data: Arc::new(data.to_vec()), content_type: content_type.to_string() })
    }

    fn unique_name(&mut self, base: &str) -> String {
        let base = clean_name(base);
        let n = self.names.entry(base.clone()).or_insert(0);
        *n += 1;
        if *n == 1 { base } else { format!("{base}_{n}") }
    }

    fn draw_field(
        &mut self,
        f: &Field,
        rect: Rect,
        ctx: &Ctx,
        radio: Option<(&str, &str, Option<String>, bool)>,
        sib: usize,
    ) -> Result<(), XfaError> {
        if let Some(b) = &f.common.border
            && f.ui != Ui::Button
        {
            self.draw_border(b, rect)?;
        }
        let fctx = ctx.child(&f.common);
        let inner = rect.inset(&f.common.margin);
        let (mut ui, mut cap_rect) = (inner, None);
        if let Some((placement, reserve)) = self.caption_reserve(f, inner.w, ctx) {
            let r = reserve.max(0.0);
            match placement {
                Placement::Top => {
                    cap_rect = Some(Rect::new(inner.x, inner.y, inner.w, r.min(inner.h)));
                    ui = Rect::new(inner.x, inner.y + r, inner.w, inner.h - r);
                }
                Placement::Bottom => {
                    cap_rect = Some(Rect::new(inner.x, inner.bottom() - r.min(inner.h), inner.w, r.min(inner.h)));
                    ui = Rect::new(inner.x, inner.y, inner.w, inner.h - r);
                }
                Placement::Right => {
                    cap_rect = Some(Rect::new(inner.right() - r.min(inner.w), inner.y, r.min(inner.w), inner.h));
                    ui = Rect::new(inner.x, inner.y, inner.w - r, inner.h);
                }
                _ => {
                    cap_rect = Some(Rect::new(inner.x, inner.y, r.min(inner.w), inner.h));
                    ui = Rect::new(inner.x + r, inner.y, inner.w - r, inner.h);
                }
            }
        }
        if let (Some(cr), Some(cap)) = (cap_rect, &f.caption)
            && cap.presence == Presence::Visible
        {
            let font = ctx.font.apply(&cap.font);
            let para = ctx.para.apply(&cap.para);
            self.draw_value_text(&cap.value, cr, &font, &para)?;
        }
        let som_path = || {
            if fctx.som.is_empty() {
                format!("{}[{sib}]", f.common.name.as_deref().unwrap_or("field"))
            } else {
                format!("{}.{}[{sib}]", fctx.som, f.common.name.as_deref().unwrap_or("field"))
            }
        };
        match f.ui {
            Ui::Unknown => {
                self.warn_once("a field of an unknown kind is left blank");
                return Ok(());
            }
            Ui::ImageEdit => {
                // The picture the data holds (base64, as Designer saves it) or the template's.
                let field_name = f.common.name.clone().unwrap_or_else(|| "an image field".into());
                let text: Vec<u8> = self
                    .data
                    .and_then(|d| d.text_at(&data_path(self.tpl, &som_path())))
                    .map(|t| t.bytes().filter(|b| !b.is_ascii_whitespace()).collect())
                    .unwrap_or_default();
                let from_data = if text.is_empty() {
                    None
                } else if text.len() > MAX_DATA_IMAGE / 3 * 4 + 4 {
                    self.warn_once(format!("{field_name}: the picture in the data is larger than 32 MB and is left out"));
                    None
                } else {
                    match base64::engine::general_purpose::STANDARD.decode(&text) {
                        Ok(d) if crate::image::kind(&d).is_some() => Some(d),
                        Ok(_) => {
                            self.warn_once(format!("{field_name}: the picture in the data is not a JPEG, PNG or GIF and is left out"));
                            None
                        }
                        Err(_) => {
                            self.warn_once(format!("{field_name}: the data holds no readable picture (not base64) and is left out"));
                            None
                        }
                    }
                };
                let box_ = ui.inset(&f.ui_margin);
                match (&from_data, &f.value) {
                    (Some(d), _) => self.draw_image(d, "", box_)?,
                    (None, Value::Image { content_type, data }) => self.draw_image(data, content_type, box_)?,
                    _ => {}
                }
                self.warn_once("image fields show their picture; a new picture can't be chosen yet");
                return Ok(());
            }
            Ui::Barcode => {
                // The value as text, where the bars would be.
                self.warn_once("barcode fields show their value as text, not as bars");
                let value = self
                    .data
                    .and_then(|d| d.text_at(&data_path(self.tpl, &som_path())))
                    .map(str::to_string)
                    .or_else(|| f.value.plain())
                    .unwrap_or_default();
                if !value.trim().is_empty() {
                    let mut para = fctx.para.clone();
                    para.h_align = HAlign::Center;
                    self.draw_value_text(&Value::Text(value), ui.inset(&f.ui_margin), &fctx.font, &para)?;
                }
                return Ok(());
            }
            _ => {}
        }
        if f.access == Access::NonInteractive && f.ui != Ui::Button {
            // Shown as text, not as a field.
            return self.draw_value_text(&f.value, ui.inset(&f.ui_margin), &fctx.font, &fctx.para);
        }
        let face = match (&f.ui, &f.caption) {
            // Buttons show their caption in the caption's font.
            (Ui::Button, Some(cap)) => Face::of(&ctx.font.apply(&cap.font)),
            _ => Face::of(&fctx.font),
        };
        let h_align = match (&f.ui, &f.caption) {
            (Ui::Button, Some(cap)) => ctx.para.apply(&cap.para).h_align,
            _ => fctx.para.h_align,
        };
        let name = self.unique_name(f.common.name.as_deref().unwrap_or("field"));
        let som = som_path();
        let read_only = matches!(f.access, Access::ReadOnly | Access::Protected);
        // The data's value wins over the template's default.
        let data_value: Option<String> =
            self.data.and_then(|d| d.value_at(&data_path(self.tpl, &som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        // A choice list's default is a saved value too.
        let template_default = f.value.plain().map(|v| f.saved_value(v.trim())).filter(|v| !v.is_empty());
        let plain_value = data_value.clone().or_else(|| template_default.clone());
        let (kind, rect, border, value, tooltip) = match f.ui {
            Ui::CheckButton => {
                let s = f.check_size.unwrap_or(10.0).min(ui.w.max(1.0)).min(ui.h.max(1.0));
                let y = match fctx.para.v_align {
                    VAlign::Top => ui.y + f.ui_margin.top,
                    VAlign::Middle => ui.y + (ui.h - s) / 2.0,
                    VAlign::Bottom => ui.bottom() - s - f.ui_margin.bottom,
                };
                let r = Rect::new(ui.x + f.ui_margin.left, y, s, s);
                let on = f.items.first().cloned().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "1".into());
                let (kind, tip, checked) = match radio {
                    Some((group, on_value, tip, chosen)) => (
                        WidgetKind::Radio { group: group.to_string(), on: on_value.to_string(), round: f.check_round },
                        tip.or_else(|| f.tooltip.clone()),
                        chosen,
                    ),
                    None => (
                        WidgetKind::CheckBox { on: on.clone(), round: f.check_round },
                        f.tooltip.clone(),
                        plain_value.as_deref().is_some_and(|v| is_on(v, &on)),
                    ),
                };
                let border = widget_border(f.ui_border.as_ref(), true);
                (kind, r, border, checked.then(|| on.clone()), tip)
            }
            Ui::Button => {
                let caption = f.caption.as_ref().and_then(|c| c.value.plain()).unwrap_or_default().trim().to_string();
                // A button's box is its field border; without one it is invisible (a link
                // over text, say).
                let border = match &f.common.border {
                    Some(b) => widget_border(Some(b), false),
                    None => WidgetBorder { color: None, fill: None, shape: BorderShape::None, width: 0.0 },
                };
                (WidgetKind::Button { caption }, ui, border, None, f.tooltip.clone())
            }
            Ui::DateTimeEdit => {
                let pattern = date_pattern(f.picture.as_deref());
                // Data holds ISO dates; the field shows them in its pattern.
                let shown = plain_value.as_ref().map(|v| if data_value.is_some() { iso_to_pattern(v, &pattern) } else { v.clone() });
                (WidgetKind::Date(pattern), ui, widget_border(f.ui_border.as_ref(), false), shown, f.tooltip.clone())
            }
            Ui::ChoiceList => {
                let options: Vec<(String, String)> = f
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, shown)| (f.item_values.get(i).cloned().unwrap_or_else(|| shown.clone()), shown.clone()))
                    .collect();
                // The data holds saved values (several on separate lines); shown text is taken
                // too, as a user typing into an editable list leaves it.
                let value = plain_value.as_ref().map(|v| f.saved_value(v));
                let kind = WidgetKind::Choice { options, list_box: f.choice.list_box, multi: f.choice.multi, editable: f.choice.editable };
                (kind, ui, widget_border(f.ui_border.as_ref(), false), value, f.tooltip.clone())
            }
            Ui::Signature => (WidgetKind::Signature, ui, widget_border(f.ui_border.as_ref(), false), None, f.tooltip.clone()),
            Ui::PasswordEdit => (WidgetKind::Password, ui, widget_border(f.ui_border.as_ref(), false), plain_value, f.tooltip.clone()),
            _ => (WidgetKind::Text, ui, widget_border(f.ui_border.as_ref(), false), plain_value, f.tooltip.clone()),
        };
        if rect.w < 1.0 || rect.h < 1.0 {
            return Ok(());
        }
        let action = if f.ui == Ui::Button { button_action(&f.scripts) } else { None };
        let items: Vec<String> = match &kind {
            WidgetKind::CheckBox { on, .. } => vec![on.clone(), f.items.get(1).cloned().unwrap_or_default()],
            WidgetKind::Radio { on, .. } => vec![on.clone()],
            _ => Vec::new(),
        };
        self.fields += 1;
        self.push(Item::Widget(Box::new(Widget {
            rect,
            kind,
            name,
            som,
            tooltip,
            face,
            h_align,
            multiline: f.multiline,
            max_chars: f.max_chars,
            read_only,
            border,
            value,
            default: template_default,
            action,
            items,
        })))
    }
}

/// The widget's own border from the `<ui>` border element. Without one, text fields and check
/// boxes get a solid box, as Designer draws them.
fn widget_border(b: Option<&Border>, check: bool) -> WidgetBorder {
    let Some(b) = b else {
        return WidgetBorder {
            color: Some([0.0, 0.0, 0.0]),
            fill: if check { Some([1.0, 1.0, 1.0]) } else { None },
            shape: BorderShape::Full,
            width: 0.5,
        };
    };
    let vis = b.visible_edges();
    let shape = match vis {
        [false, false, false, false] => BorderShape::None,
        [false, false, true, false] => BorderShape::Underline,
        _ => BorderShape::Full,
    };
    let edge = (0..4).find(|&i| vis[i]).map(|i| b.edges[i]).unwrap_or_default();
    WidgetBorder {
        color: (shape != BorderShape::None).then_some(edge.color.0),
        fill: b.fill().map(|c| c.0),
        shape,
        width: edge.thickness.clamp(0.1, 12.0),
    }
}
