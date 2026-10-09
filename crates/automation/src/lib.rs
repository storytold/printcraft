//! pdfcraft-automation — agent control for PdfCraft (architecture §13).
//!
//! - **Layer:** L7. Headless: depends on the engine, never on a UI toolkit.
//! - [`Automation`] is a tool set over an engine [`Session`]: open, inspect, render, extract and
//!   find text, edit pages and metadata, undo/redo, save, combine, extract and split. Every tool
//!   has a JSON Schema ([`tools`]) and takes and returns JSON, so the same table drives the MCP
//!   server ([`mcp`]), `pdfcraft-cli run` and (later) the UI control channel.
//! - Pages are **1-based** in every tool, as people number them. Rectangles are in PDF points
//!   with the origin at the top-left of the displayed page.
//! - An optional root directory confines every path a tool reads or writes.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod a11y;
mod comments;
mod content;
mod conventions;
mod forms;
mod links;
#[cfg(feature = "mcp")]
pub mod mcp;
mod measure;
mod printing;
mod redact;
mod signing;
mod tools;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfcraft_engine::{CombineSource, DocId, Document, Edit, Session, commands};
use pdfcraft_platform::staging::{StagingName, create_staging, staging_suffixes};
use pdfcraft_render::{PageRenderer, PageText, RenderConfig, RenderRequest, RequestKind};
use serde_json::{Value, json};

pub use tools::{ToolDef, tools};

/// One piece of a tool's result.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    /// Structured data (MCP: a text block with the JSON plus `structuredContent`).
    Json(Value),
    /// A PNG image.
    Png { data: Vec<u8>, width: u32, height: u32 },
}

/// Why a tool call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolError {
    /// No tool has this name.
    UnknownTool(String),
    /// The arguments don't match the tool's schema.
    InvalidArgs(String),
    /// The tool ran and failed (the message is for the agent to read).
    Failed(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::UnknownTool(t) => write!(f, "unknown tool {t:?}"),
            ToolError::InvalidArgs(m) | ToolError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ToolError {}

type Result<T> = std::result::Result<T, ToolError>;

fn failed(e: impl std::fmt::Display) -> ToolError {
    ToolError::Failed(e.to_string())
}

/// Default and maximum resolution for `page_render`.
const DEFAULT_DPI: f64 = 96.0;
const MAX_DPI: f64 = 600.0;

/// Bookmarks in one page of the bookmark list, by default and at most.
const BOOKMARK_PAGE: usize = 100;
const MAX_BOOKMARK_PAGE: usize = 1000;

/// Page texts of one document version: (the working bytes, one slot per page).
type TextCache = (Arc<Vec<u8>>, Vec<Option<Arc<PageText>>>);

/// A headless PdfCraft session driven by tool calls.
pub struct Automation {
    session: Session,
    root: Option<PathBuf>,
    /// Synchronous renderers, rebuilt when a document's working bytes change.
    renderers: HashMap<DocId, (Arc<Vec<u8>>, PageRenderer)>,
    /// Extracted page text per document version (the working bytes it was taken from).
    texts: HashMap<DocId, TextCache>,
}

impl Default for Automation {
    fn default() -> Self {
        Self::new()
    }
}

impl Automation {
    pub fn new() -> Self {
        Self { session: Session::new(), root: None, renderers: HashMap::new(), texts: HashMap::new() }
    }

    /// Confine every path the tools read or write to `root` (relative paths resolve inside it).
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into().canonicalize()?;
        if !root.is_dir() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the root must be a folder"));
        }
        self.root = Some(root);
        Ok(self)
    }

    /// Use a fixed clock for saves (deterministic output in tests).
    pub fn with_clock(mut self, clock: fn() -> i64) -> Self {
        self.session = std::mem::take(&mut self.session).with_clock(clock);
        self
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Save `bytes` that a caller writes on the session's behalf, such as `pdfcraft-cli run`
    /// saving a rendered page. With a root, it is confined like a tool's own output (a relative
    /// path resolves inside it) and written atomically. Without one, the path is written as
    /// given, so `/dev/stdout` and the like still work. Returns where the file went.
    pub fn write_output(&self, path: &str, bytes: &[u8]) -> Result<PathBuf> {
        let target = self.resolve(path, true)?;
        if self.root.is_some() {
            write_atomic(&target, bytes)?;
        } else {
            std::fs::write(&target, bytes).map_err(|e| failed(format!("{path}: {e}")))?;
        }
        Ok(target)
    }

    /// Run the tool `name` with JSON `args` (an object; `null` means no arguments).
    pub fn call(&mut self, name: &str, args: &Value) -> Result<Vec<Content>> {
        let empty = json!({});
        let args = if args.is_null() { &empty } else { args };
        if !args.is_object() {
            return Err(ToolError::InvalidArgs("arguments must be a JSON object".into()));
        }
        let def = tools::find(name).ok_or_else(|| ToolError::UnknownTool(name.into()))?;
        tools::check_args(def, args)?;
        let a = Args(args);
        let out = match name {
            "doc_open" => self.doc_open(&a)?,
            "doc_list" => json!({ "documents": self.session.docs().iter().map(summary).collect::<Vec<_>>() }),
            "doc_info" => info(self.doc(&a)?),
            "doc_close" => self.doc_close(&a)?,
            "doc_save" => self.doc_save(&a)?,
            "doc_set_info" => {
                let edit = Edit::SetInfo { key: a.str("key")?.into(), value: a.str("value")?.into() };
                self.apply(&a, edit)?
            }
            "page_render" => return self.page_render(&a).map(|c| vec![c]),
            "comment_image_preview" => return self.comment_image_preview(&a),
            "text_extract" => self.text_extract(&a)?,
            "text_find" => self.text_find(&a)?,
            "page_rotate" => {
                let degrees = a.int("degrees")?;
                if degrees % 90 != 0 {
                    return Err(ToolError::InvalidArgs("degrees must be a multiple of 90".into()));
                }
                let doc = self.doc(&a)?;
                let base = match a.opt_ints("pages")? {
                    Some(_) => self.pages(&a, "pages")?,
                    None => (0..doc.info.pages.len()).collect(),
                };
                let parity = match a.opt_str("subset")?.unwrap_or("all") {
                    "all" => pdfcraft_engine::PageParity::Both,
                    "even" => pdfcraft_engine::PageParity::Even,
                    "odd" => pdfcraft_engine::PageParity::Odd,
                    s => return Err(ToolError::InvalidArgs(format!("unknown subset {s:?} (all, even, odd)"))),
                };
                let orientation = match a.opt_str("orientation")?.unwrap_or("all") {
                    "all" => pdfcraft_engine::PageOrientation::Both,
                    "landscape" => pdfcraft_engine::PageOrientation::Landscape,
                    "portrait" => pdfcraft_engine::PageOrientation::Portrait,
                    o => return Err(ToolError::InvalidArgs(format!("unknown orientation {o:?} (all, landscape, portrait)"))),
                };
                let pages = pdfcraft_engine::filter_pages(&self.doc(&a)?.info, &base, parity, orientation);
                if pages.is_empty() {
                    return Err(failed("no pages match the filters"));
                }
                let n = pages.len();
                let mut out = self.apply(&a, Edit::RotatePages { pages, degrees })?;
                out["rotated"] = json!(n);
                out
            }
            "page_delete" => {
                let pages = self.pages(&a, "pages")?;
                self.apply(&a, Edit::DeletePages { pages })?
            }
            "page_move" => {
                let pages = self.pages(&a, "pages")?;
                let to = self.position(&a, "to")?;
                self.apply(&a, Edit::MovePages { pages, to })?
            }
            "page_insert_blank" => self.insert_blank(&a)?,
            "page_insert_file" => self.insert_file(&a)?,
            "page_extract" => self.page_extract(&a)?,
            "doc_combine" => self.doc_combine(&a)?,
            "doc_create_multiple" => self.doc_create_multiple(&a)?,
            "doc_split" => self.doc_split(&a)?,
            "edit_undo" => {
                let id = self.doc(&a)?.id;
                let label = self.session.undo(id).map_err(failed)?;
                json!({ "undone": label, "document": summary(self.doc(&a)?) })
            }
            "edit_redo" => {
                let id = self.doc(&a)?.id;
                let label = self.session.redo(id).map_err(failed)?;
                json!({ "redone": label, "document": summary(self.doc(&a)?) })
            }
            "command_list" => self.command_list(&a)?,
            "command_run" => return self.command_run(&a),
            "command_batch" => self.command_batch(&a)?,
            "doc_inspect" => self.doc_inspect(&a)?,
            "render_preview" => return self.render_preview(&a).map(|c| vec![c]),
            "page_number" => {
                use pdfcraft_organize::LabelStyle as L;
                let n = self.doc(&a)?.info.pages.len();
                let (from, to) = (a.int("from")?, a.int("to")?);
                if from < 1 || to < from || to as usize > n {
                    return Err(ToolError::InvalidArgs(format!("from and to must satisfy 1 ≤ from ≤ to ≤ {n}")));
                }
                let style = match a.opt_str("style")?.unwrap_or("decimal") {
                    "decimal" => L::Decimal,
                    "upper-roman" => L::UpperRoman,
                    "lower-roman" => L::LowerRoman,
                    "upper-alpha" => L::UpperAlpha,
                    "lower-alpha" => L::LowerAlpha,
                    "none" => L::None,
                    other => return Err(ToolError::InvalidArgs(format!("unknown style {other:?}"))),
                };
                let prefix = a.opt_str("prefix")?.unwrap_or_default().to_string();
                let first = a.opt_int("start")?.unwrap_or(1).clamp(1, u32::MAX as i64) as u32;
                let mut out = self.apply(&a, Edit::NumberPages { from: from as usize - 1, to: to as usize - 1, style, prefix, first })?;
                out["labels"] = json!(self.doc(&a)?.info.pages.iter().map(|p| p.label.clone()).collect::<Vec<_>>());
                out
            }
            "bookmark_list" => self.bookmark_listing(&a)?,
            "bookmark_add" => {
                let page = self.page(&a)?;
                let parent = a.opt_path("parent")?.unwrap_or_default();
                let index = a.opt_int("position")?.map_or(usize::MAX, |p| (p.max(1) - 1) as usize);
                let title = a.str("title")?.to_string();
                self.apply(&a, Edit::AddBookmark { parent, index, title, page })?
            }
            "bookmark_rename" => {
                let (path, title) = (a.path("path")?, a.str("title")?.to_string());
                self.apply(&a, Edit::RenameBookmark { path, title })?
            }
            "bookmark_delete" => {
                let path = a.path("path")?;
                self.apply(&a, Edit::DeleteBookmark { path })?
            }
            "bookmark_move" => {
                let from = a.path("path")?;
                let to_parent = a.opt_path("parent")?.unwrap_or_default();
                let index = a.opt_int("position")?.map_or(usize::MAX, |p| (p.max(1) - 1) as usize);
                self.apply(&a, Edit::MoveBookmark { from, to_parent, index })?
            }
            "bookmark_set_page" => {
                let (path, page) = (a.path("path")?, self.page(&a)?);
                self.apply(&a, Edit::SetBookmarkPage { path, page })?
            }
            "bookmark_from_structure" => {
                let mut out = self.apply(&a, Edit::BookmarksFromStructure)?;
                let listing = self.bookmark_listing(&a)?;
                out["bookmarks"] = listing["bookmarks"].clone();
                out["next"] = listing["next"].clone();
                out
            }
            "doc_protect" => self.doc_protect(&a)?,
            "page_replace" => {
                let pages = self.pages(&a, "pages")?;
                let path = self.resolve(a.str("path")?, false)?;
                let bytes = Arc::new(std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let src_pages = match a.opt_ints("from_pages")? {
                    Some(p) => one_based(&p)?,
                    None => (0..pages.len()).collect(),
                };
                self.apply(&a, Edit::ReplacePages { pages, name, bytes, src_pages })?
            }
            "page_duplicate" => {
                let pages = self.pages(&a, "pages")?;
                self.apply(&a, Edit::DuplicatePages { pages })?
            }
            "page_set_box" => self.page_set_box(&a)?,
            "doc_create" => self.doc_create(&a)?,
            "doc_flatten" => {
                let (comments, fields) = (a.opt_bool("comments")?.unwrap_or(true), a.opt_bool("fields")?.unwrap_or(true));
                if !comments && !fields {
                    return Err(ToolError::InvalidArgs("nothing to flatten".into()));
                }
                self.apply(&a, Edit::Flatten { comments, fields })?
            }
            "doc_reduce" => {
                let id = self.doc(&a)?.id;
                let before = self.doc(&a)?.bytes.len();
                let path = self.resolve(a.str("path")?, true)?;
                let (bytes, merged) = self.session.reduced_bytes(id).map_err(failed)?;
                // As in the app, a copy that isn't smaller is not written (#490).
                let written = bytes.len() < before;
                if written {
                    write_atomic(&path, &bytes)?;
                }
                json!({ "path": path.to_string_lossy(), "written": written, "bytes_before": before, "bytes_after": bytes.len(), "merged_objects": merged })
            }
            "doc_optimize" => self.doc_optimize(&a)?,
            "doc_initial_view" => self.doc_initial_view(&a)?,
            "doc_revisions" => self.doc_revisions(&a)?,
            "text_lines" => {
                let page = self.page(&a)?;
                let doc = self.doc(&a)?;
                let info = &doc.info.pages[page];
                let r = |x: f32| (x as f64 * 100.0).round() / 100.0;
                let lines: Vec<Value> = doc
                    .text_lines(page)
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let (u, v) = (info.user_to_view(l.rect[0] as f32, l.rect[1] as f32), info.user_to_view(l.rect[2] as f32, l.rect[3] as f32));
                        json!({
                            "line": i + 1,
                            "text": l.text,
                            "rect": [r(u[0].min(v[0])), r(u[1].min(v[1])), r(u[0].max(v[0])), r(u[1].max(v[1]))],
                            "font": l.base_font,
                            "size": (l.size * 100.0).round() / 100.0,
                        })
                    })
                    .collect();
                json!({ "page": page + 1, "count": lines.len(), "lines": lines })
            }
            "page_images" => {
                let page = self.page(&a)?;
                let doc = self.doc(&a)?;
                let info = &doc.info.pages[page];
                let r = |x: f32| (x as f64 * 100.0).round() / 100.0;
                let list: Vec<Value> = doc
                    .page_images(page)
                    .iter()
                    .enumerate()
                    .map(|(i, im)| {
                        let (u, v) = (info.user_to_view(im.rect[0] as f32, im.rect[1] as f32), info.user_to_view(im.rect[2] as f32, im.rect[3] as f32));
                        json!({ "image": i + 1, "rect": [r(u[0].min(v[0])), r(u[1].min(v[1])), r(u[0].max(v[0])), r(u[1].max(v[1]))], "pixels": [im.width, im.height], "name": im.name, "kind": if im.is_form { "form" } else { "image" } })
                    })
                    .collect();
                json!({ "page": page + 1, "count": list.len(), "images": list })
            }
            "image_edit" | "image_save" => {
                let page = self.page(&a)?;
                let n = self.doc(&a)?.page_images(page).len();
                let k = a.int("image")?;
                if k < 1 || k as usize > n {
                    return Err(ToolError::InvalidArgs(format!("image {k} is out of range: page {} has {n} images", page + 1)));
                }
                let index = k as usize - 1;
                if name == "image_save" {
                    let (ext, bytes) = self.doc(&a)?.page_image_file(page, index).map_err(failed)?;
                    let mut path = self.resolve(a.str("path")?, true)?;
                    if path.is_dir() {
                        // Adding the extension to a folder's name would write beside it: "." is the
                        // root, so that would land outside it.
                        return Err(failed(format!("{}: is a folder, not a file", path.display())));
                    }
                    if path.extension().is_none() {
                        path.set_extension(ext);
                    }
                    write_atomic(&path, &bytes)?;
                    json!({ "path": path.to_string_lossy(), "format": ext, "bytes": bytes.len() })
                } else {
                    use pdfcraft_engine::ImageEdit;
                    let change = match a.str("action")? {
                        "move" => {
                            let r: Vec<f64> =
                                a.get("rect").and_then(Value::as_array).map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                            let r = <[f64; 4]>::try_from(r).map_err(|_| ToolError::InvalidArgs("move needs rect: 4 numbers".into()))?;
                            // Top-left-origin points → user space.
                            let info = &self.doc(&a)?.info.pages[page];
                            let (u0, u1) = (info.view_to_user(r[0] as f32, r[1] as f32), info.view_to_user(r[2] as f32, r[3] as f32));
                            ImageEdit::Move([u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64])
                        }
                        "rotate" => ImageEdit::Rotate(a.opt_int("quarters")?.unwrap_or(1) as i32),
                        "flip_horizontal" => ImageEdit::Flip { horizontal: true },
                        "flip_vertical" => ImageEdit::Flip { horizontal: false },
                        "delete" => ImageEdit::Delete,
                        "replace" => {
                            let path = self.resolve(a.str("path")?, false)?;
                            let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
                            ImageEdit::Replace {
                                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                bytes: Arc::new(bytes),
                            }
                        }
                        other => return Err(ToolError::InvalidArgs(format!("unknown action {other:?}"))),
                    };
                    self.apply(&a, Edit::EditPageImage { page, index, change })?
                }
            }
            "text_paragraphs" => {
                let page = self.page(&a)?;
                let doc = self.doc(&a)?;
                let info = &doc.info.pages[page];
                let r = |x: f32| (x as f64 * 100.0).round() / 100.0;
                let blocks: Vec<Value> = doc
                    .text_blocks(page)
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let (u, v) = (info.user_to_view(b.rect[0] as f32, b.rect[1] as f32), info.user_to_view(b.rect[2] as f32, b.rect[3] as f32));
                        json!({
                            "paragraph": i + 1,
                            "text": b.text,
                            "lines": b.lines.iter().map(|l| l + 1).collect::<Vec<_>>(),
                            "rect": [r(u[0].min(v[0])), r(u[1].min(v[1])), r(u[0].max(v[0])), r(u[1].max(v[1]))],
                            "font": b.base_font,
                            "size": (b.size * 100.0).round() / 100.0,
                        })
                    })
                    .collect();
                json!({ "page": page + 1, "count": blocks.len(), "paragraphs": blocks })
            }
            "text_edit" if a.get("paragraph").is_some() => {
                let page = self.page(&a)?;
                let n = self.doc(&a)?.text_blocks(page).len();
                let k = a.int("paragraph")?;
                if k < 1 || k as usize > n {
                    return Err(ToolError::InvalidArgs(format!("paragraph {k} is out of range: page {} has {n} paragraphs", page + 1)));
                }
                let block = self.doc(&a)?.text_blocks(page)[k as usize - 1].clone();
                let text = a.opt_str("text")?.map(str::to_owned).unwrap_or(block.text);
                let mut style = pdfcraft_engine::BlockStyle {
                    bold: a.opt_bool("bold")?,
                    size: a.opt_num("size")?,
                    underline: a.opt_bool("underline")?,
                    line_spacing: a.opt_num("line_spacing")?,
                    char_spacing: a.opt_num("char_spacing")?,
                    scale: a.opt_num("scale")?,
                    width: a.opt_num("width")?,
                    ..Default::default()
                };
                let (dx, dy) = (a.opt_num("dx")?, a.opt_num("dy")?);
                if dx.is_some() || dy.is_some() {
                    style.offset = Some([dx.unwrap_or(0.0), dy.unwrap_or(0.0)]);
                }
                if let Some(f) = a.opt_str("font")? {
                    let family = match f {
                        "helvetica" => pdfcraft_engine::FontFamily::Helvetica,
                        "times" => pdfcraft_engine::FontFamily::Times,
                        "courier" => pdfcraft_engine::FontFamily::Courier,
                        other => return Err(ToolError::InvalidArgs(format!("unknown font {other:?} (helvetica, times, courier)"))),
                    };
                    style.family = Some((family, a.opt_bool("bold")?.unwrap_or(false), a.opt_bool("italic")?.unwrap_or(false)));
                }
                if let Some(c) = a.opt_str("color")? {
                    style.color = Some(comments::parse_color(c)?);
                }
                if let Some(al) = a.opt_str("align")? {
                    style.align = Some(match al {
                        "left" => pdfcraft_engine::TextAlign::Left,
                        "center" => pdfcraft_engine::TextAlign::Center,
                        "right" => pdfcraft_engine::TextAlign::Right,
                        "justify" => pdfcraft_engine::TextAlign::Justify,
                        other => return Err(ToolError::InvalidArgs(format!("unknown align {other:?}"))),
                    });
                }
                let mut out = self.apply(&a, Edit::EditTextBlock { page, block: k as usize - 1, text, style })?;
                if let Some(b) = self.doc(&a)?.text_blocks(page).get(k as usize - 1) {
                    out["paragraph"] = json!({ "text": b.text, "lines": b.lines.len(), "font": b.base_font });
                }
                out
            }
            "text_edit" => {
                let page = self.page(&a)?;
                let n = self.doc(&a)?.text_lines(page).len();
                let line = a.int("line")?;
                if line < 1 || line as usize > n {
                    return Err(ToolError::InvalidArgs(format!("line {line} is out of range: page {} has {n} lines", page + 1)));
                }
                let text = a.str("text")?.to_owned();
                let mut out = self.apply(&a, Edit::EditTextLine { page, line: line as usize - 1, text })?;
                let after = self.doc(&a)?.text_lines(page);
                if let Some(l) = after.get(line as usize - 1) {
                    out["line"] = json!({ "text": l.text, "font": l.base_font });
                }
                out
            }
            "doc_audit_space" => {
                let rows: Vec<Value> = self
                    .doc(&a)?
                    .audit_space()
                    .iter()
                    .map(|u| json!({ "category": u.category.label(), "bytes": u.bytes, "percent": (u.percent * 100.0).round() / 100.0 }))
                    .collect();
                json!({ "categories": rows })
            }
            "doc_open_revision" => {
                let id = self.doc(&a)?.id;
                let n = usize::try_from(a.opt_int("revision")?.ok_or_else(|| ToolError::InvalidArgs("revision is required".into()))?).unwrap_or(0);
                let new = self.session.open_revision(id, n).map_err(failed)?;
                summary(self.session.get(new).ok_or_else(|| failed("the document vanished"))?)
            }
            "doc_export_images" | "doc_export_text" | "doc_export_all_images" => self.export(name, &a)?,
            "doc_header_footer" | "doc_watermark" | "doc_background" | "doc_remove_marks" => self.marks(name, &a)?,
            "doc_unprotect" => {
                let mut out = self.apply(&a, Edit::RemoveProtection)?;
                out["security"] = security(self.doc(&a)?);
                out
            }
            "form_fields" => self.form_fields(&a)?,
            "form_fill" => self.form_fill(&a)?,
            "form_set_image" => {
                let path = self.resolve(a.str("path")?, false)?;
                let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
                self.apply(&a, Edit::SetFieldImage { name: a.str("field")?.to_owned(), image: Arc::new(bytes) })?
            }
            "form_reset" => self.form_reset(&a)?,
            "form_add_field" => self.form_add_field(&a)?,
            "form_set_props" => self.form_set_props(&a)?,
            "form_delete_field" => self.form_delete_field(&a)?,
            "form_tab_order" => self.form_tab_order(&a)?,
            "doc_export_data" => self.doc_export_data(&a)?,
            "doc_import_data" => self.doc_import_data(&a)?,
            "redact_mark" => self.redact_mark(&a)?,
            "redact_apply" => self.redact_apply(&a)?,
            "redact_clear" => self.redact_clear(&a)?,
            "doc_hidden_info" => self.doc_hidden_info(&a)?,
            "printers" => self.printers()?,
            "printer_options" => self.printer_options(&a)?,
            "link_list" => self.link_list(&a)?,
            "link_add" => self.link_add(&a)?,
            "link_edit" => self.link_edit(&a)?,
            "link_delete" => self.link_delete(&a)?,
            "links_from_urls" => self.links_from_urls(&a)?,
            "links_remove" => self.links_remove(&a)?,
            "object_list" => self.object_list(&a)?,
            "object_move" => self.object_move(&a)?,
            "content_list" => self.content_list(&a)?,
            "page_add_text" => self.page_add_text(&a)?,
            "page_add_image" => self.page_add_image(&a)?,
            "content_update" => self.content_update(&a)?,
            "content_delete" => self.content_delete(&a)?,
            "doc_print" => self.doc_print(&a)?,
            "doc_remove_hidden" => self.doc_remove_hidden(&a)?,
            "fill_sign_add" => self.fill_sign_add(&a)?,
            "fill_sign_date_format" => self.fill_sign_date_format(&a)?,
            "measure_distance" => self.measurement_add(&a, pdfcraft_engine::measure::Kind::Distance)?,
            "measure_perimeter" => self.measurement_add(&a, pdfcraft_engine::measure::Kind::Perimeter)?,
            "measure_area" => self.measurement_add(&a, pdfcraft_engine::measure::Kind::Area)?,
            "measure_info" => self.measurement_info(&a)?,
            "measure_list" => self.measurement_list(&a)?,
            "measure_scale" => self.measurement_scale(&a)?,
            "measure_snap" => self.measurement_snap(&a)?,
            "measure_export" => self.measurement_export(&a)?,
            "comment_list" => self.comment_list(&a)?,
            "comment_add" => self.comment_add(&a)?,
            "stamp_custom" => self.stamp_custom(&a)?,
            "comment_reply" => self.comment_reply(&a)?,
            "comment_set_status" => self.comment_set_status(&a)?,
            "sign_list" => self.sign_list(&a)?,
            "accessibility_check" => self.a11y_check(&a)?,
            "ocr_recognize" => self.ocr_recognize(&a)?,
            "js_run" => self.js_run(&a)?,
            "js_document_scripts" => self.js_document_scripts(&a)?,
            "js_set_document_script" => self.js_set_document_script(&a)?,
            "js_enabled" => self.js_enabled(&a)?,
            "form_set_script" => self.form_set_script(&a)?,
            "form_merge_data" => self.form_merge_data(&a)?,
            "form_actions" => self.form_actions(&a)?,
            "form_detect_fields" => self.form_detect_fields(&a)?,
            "doc_compare" => self.doc_compare(&a)?,
            "action_list" => self.action_list()?,
            "pdfa_verify" => self.pdfa_verify(&a)?,
            "doc_export_office" => self.doc_export_office(&a)?,
            "pdfa_convert" => self.pdfa_convert(&a)?,
            "action_run" => self.action_run(&a)?,
            "doc_compare_report" => self.doc_compare_report(&a)?,
            "doc_compare_mark" => self.doc_compare_mark(&a)?,
            "form_set_actions" => self.form_set_actions(&a)?,
            "ocr_status" => self.ocr_status()?,
            "ocr_recognize_files" => self.ocr_recognize_files(&a)?,
            "accessibility_report" => self.a11y_report(&a)?,
            "accessibility_fix" => self.a11y_fix(&a)?,
            "accessibility_figures" => self.accessibility_figures(&a)?,
            "accessibility_set_alt" => self.accessibility_set_alt(&a)?,
            "sign_id_create" => self.sign_id_create(&a)?,
            "sign_document" => self.sign_document(&a)?,
            "sign_keychain_ids" => {
                #[cfg(target_os = "macos")]
                let ids: Vec<Value> = pdfcraft_engine::sign::keychain::identities(None)
                    .map_err(failed)?
                    .iter()
                    .map(|id| json!({ "id": pdfcraft_engine::sign::keychain::reference(&id.certificate), "certificate": signing::cert_json(&id.certificate) }))
                    .collect();
                #[cfg(not(target_os = "macos"))]
                let ids: Vec<Value> = Vec::new();
                json!({ "count": ids.len(), "ids": ids })
            }
            "sign_windows_ids" => {
                #[cfg(target_os = "windows")]
                let (ids, unusable): (Vec<Value>, Vec<Value>) = {
                    let listing = pdfcraft_engine::sign::windows::list().map_err(failed)?;
                    let ids = listing
                        .ids
                        .iter()
                        .map(|id| json!({ "id": pdfcraft_engine::sign::windows::reference(&id.certificate), "certificate": signing::cert_json(&id.certificate) }))
                        .collect();
                    let unusable = listing
                        .unusable
                        .iter()
                        .map(|u| json!({ "subject": u.subject, "sha256": u.fingerprint, "reason": u.reason, "no_private_key": u.no_private_key }))
                        .collect();
                    (ids, unusable)
                };
                #[cfg(not(target_os = "windows"))]
                let (ids, unusable): (Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new());
                json!({ "count": ids.len(), "ids": ids, "unusable": unusable })
            }
            "sign_trust" => self.sign_trust(&a)?,
            "comment_mark" => self.comment_mark(&a)?,
            "comment_lock" => self.comment_lock(&a)?,
            "comments_hide" => self.comments_hide(&a)?,
            "comments_summarize" => self.comments_summarize(&a)?,
            "comment_edit" => self.comment_edit(&a)?,
            "comment_delete" => self.comment_delete(&a)?,
            other => return Err(ToolError::UnknownTool(other.into())),
        };
        Ok(vec![Content::Json(out)])
    }

    // ---- documents ---------------------------------------------------------------------------

    /// One page of a document's bookmarks: `offset` (from 0) and `limit` (1 to `MAX_BOOKMARK_PAGE`).
    fn bookmark_listing(&self, a: &Args) -> Result<Value> {
        let id = a.int("doc")?;
        let id = DocId(u64::try_from(id).map_err(|_| ToolError::InvalidArgs("doc must be positive".into()))?);
        let offset = a.opt_int("offset")?.unwrap_or(0);
        let offset = usize::try_from(offset).map_err(|_| ToolError::InvalidArgs("offset must be 0 or more".into()))?;
        let limit = a.opt_int("limit")?.unwrap_or(BOOKMARK_PAGE as i64);
        let limit = usize::try_from(limit)
            .ok()
            .filter(|l| (1..=MAX_BOOKMARK_PAGE).contains(l))
            .ok_or_else(|| ToolError::InvalidArgs(format!("limit must be from 1 to {MAX_BOOKMARK_PAGE}")))?;
        let page = self
            .session
            .bookmark_page(id, offset, limit)
            .ok_or_else(|| ToolError::Failed(format!("no open document with id {} (see doc_list)", id.0)))?;
        Ok(json!({ "bookmarks": nest_rows(&page.rows), "next": page.next, "truncated": page.truncated }))
    }

    fn doc(&self, a: &Args) -> Result<&Document> {
        let id = a.int("doc")?;
        let id = u64::try_from(id).map_err(|_| ToolError::InvalidArgs("doc must be positive".into()))?;
        self.session.get(DocId(id)).ok_or_else(|| ToolError::Failed(format!("no open document with id {id} (see doc_list)")))
    }

    fn doc_open(&mut self, a: &Args) -> Result<Value> {
        let path = self.resolve(a.str("path")?, false)?;
        let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let id = self.session.open(name, Some(path.to_string_lossy().into_owned()), Arc::new(bytes), a.opt_str("password")?).map_err(failed)?;
        let doc = self.session.get(id).ok_or_else(|| failed("the document vanished"))?;
        Ok(summary(doc))
    }

    fn doc_close(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let id = doc.id;
        let discard_changes = a.opt_bool("discard_changes")?.unwrap_or(false);
        if doc.dirty && !discard_changes {
            return Err(failed("the document has unsaved changes: save it with doc_save, or pass discard_changes: true"));
        }
        self.session.close(id);
        self.renderers.remove(&id);
        self.texts.remove(&id);
        Ok(json!({ "closed": id.0 }))
    }

    fn doc_save(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let id = doc.id;
        let target = match a.opt_str("path")? {
            Some(p) => self.resolve(p, true)?,
            None => PathBuf::from(doc.path.clone().ok_or_else(|| failed("the document has never been saved: pass a path"))?),
        };
        let same_file = doc.path.as_deref().is_some_and(|p| Path::new(p) == target);
        // Redacted (or sanitized) output replaces the source only when asked to, and is always a full rewrite.
        let redacted = doc.has_unsaved_redaction();
        if redacted && same_file && !a.opt_bool("overwrite_source")?.unwrap_or(false) {
            return Err(failed(
                "the document has applied redactions or removed hidden information: saving in place would replace the original file. Save to a new path, or pass overwrite_source: true",
            ));
        }
        // Saving to a new file is a full rewrite unless asked otherwise, like Save As.
        let full = redacted || a.opt_bool("full")?.unwrap_or(!same_file);
        let flatten_fill_sign = a.opt_bool("flatten_fill_sign")?.unwrap_or(false);
        if flatten_fill_sign {
            self.apply(a, Edit::FlattenFillSign)?;
        }
        let bytes = if full { self.session.save_full_bytes(id) } else { self.session.save_bytes(id) }.map_err(failed)?;
        if redacted {
            // Checked before anything is replaced; the temporary file is owner-only.
            self.session.verify_save(id, &bytes).map_err(failed)?;
            pdfcraft_engine::write_private_atomic(&target, &bytes).map_err(|e| failed(format!("{}: {e}", target.display())))?;
        } else {
            write_atomic(&target, &bytes)?;
        }
        let path = target.to_string_lossy().into_owned();
        self.session.mark_saved(id, bytes.clone(), Some(path.clone())).map_err(failed)?;
        Ok(json!({ "path": path, "bytes": bytes.len(), "incremental": !full, "document": summary(self.doc(a)?) }))
    }

    fn doc_revisions(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let ends = doc.revision_ends();
        let sigs = doc.signatures.clone();
        let list: Vec<Value> = ends
            .iter()
            .enumerate()
            .map(|(i, end)| {
                let start = if i == 0 { 0 } else { ends[i - 1] };
                // A signature signs the revision its byte range ends in.
                let signed: Vec<&str> =
                    sigs.iter().filter(|s| s.signed && s.signed_len > start && s.signed_len <= *end).map(|s| s.field.as_str()).collect();
                json!({ "revision": i + 1, "end": end, "bytes": end - start, "signed_by": signed })
            })
            .collect();
        Ok(json!({ "revisions": list, "unsaved_changes": doc.dirty }))
    }

    fn doc_initial_view(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{InitialLayout as L, Magnification as M, Navigation as N};
        let bad = |m: String| ToolError::InvalidArgs(m);
        let mut v = self.doc(a)?.initial_view();
        let before = v.clone();
        if let Some(n) = a.opt_str("navigation")? {
            v.navigation = match n {
                "page" => N::PageOnly,
                "bookmarks" => N::Bookmarks,
                "pages" => N::Pages,
                "attachments" => N::Attachments,
                "layers" => N::Layers,
                o => return Err(bad(format!("unknown navigation {o:?}"))),
            };
        }
        if let Some(l) = a.opt_str("layout")? {
            v.layout = match l {
                "default" => L::Default,
                "single" => L::SinglePage,
                "continuous" => L::SinglePageContinuous,
                "two_up" => L::TwoUp,
                "two_up_continuous" => L::TwoUpContinuous,
                "two_up_cover" => L::TwoUpCoverPage,
                "two_up_continuous_cover" => L::TwoUpContinuousCoverPage,
                o => return Err(bad(format!("unknown layout {o:?}"))),
            };
        }
        match a.get("magnification") {
            None => {}
            Some(Value::Number(p)) => v.magnification = M::Percent(p.as_f64().unwrap_or(100.0).clamp(1.0, 6400.0)),
            Some(Value::String(m)) => {
                v.magnification = match m.as_str() {
                    "default" => M::Default,
                    "actual" => M::ActualSize,
                    "fit_page" => M::FitPage,
                    "fit_width" => M::FitWidth,
                    "fit_height" => M::FitHeight,
                    "fit_visible" => M::FitVisible,
                    o => return Err(bad(format!("unknown magnification {o:?}"))),
                }
            }
            Some(_) => return Err(bad("magnification is a name or a percentage".into())),
        }
        if let Some(p) = a.opt_int("page")? {
            v.page = (p.max(1) - 1) as usize;
        }
        for (k, f) in [
            ("fit_window", &mut v.fit_window),
            ("center_window", &mut v.center_window),
            ("full_screen", &mut v.full_screen),
            ("display_title", &mut v.display_title),
            ("hide_menubar", &mut v.hide_menubar),
            ("hide_toolbar", &mut v.hide_toolbar),
            ("hide_window_ui", &mut v.hide_window_ui),
        ] {
            if let Some(b) = a.opt_bool(k)? {
                *f = b;
            }
        }
        if let Some(l) = a.opt_str("language")? {
            v.language = (!l.trim().is_empty()).then(|| l.trim().to_string());
        }
        if let Some(b) = a.opt_str("binding")? {
            v.right_to_left = match b {
                "left" => false,
                "right" => true,
                o => return Err(bad(format!("binding is left or right, not {o:?}"))),
            };
        }
        let mut out = if v != before { self.apply(a, Edit::SetInitialView(Box::new(v.clone())))? } else { json!({}) };
        out["initial_view"] = json!({
            "navigation": format!("{:?}", v.navigation),
            "layout": format!("{:?}", v.layout),
            "magnification": format!("{:?}", v.magnification),
            "page": v.page + 1,
            "fit_window": v.fit_window,
            "center_window": v.center_window,
            "full_screen": v.full_screen,
            "display_title": v.display_title,
            "hide_menubar": v.hide_menubar,
            "hide_toolbar": v.hide_toolbar,
            "hide_window_ui": v.hide_window_ui,
            "language": v.language,
            "binding": if v.right_to_left { "right" } else { "left" },
        });
        Ok(out)
    }

    fn doc_optimize(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::optimize::{Compression, ImageSettings, Settings};
        let id = self.doc(a)?.id;
        let before = self.doc(a)?.bytes.len();
        let path = self.resolve(a.str("path")?, true)?;
        let mut settings = Settings::default();
        let images = |key: &str, base: ImageSettings| -> Result<ImageSettings> {
            let Some(v) = a.get(key) else { return Ok(base) };
            let mut s = base;
            if let Some(d) = v.get("downsample").and_then(Value::as_bool) {
                s.downsample = d;
            }
            if let Some(p) = v.get("ppi").and_then(Value::as_f64) {
                s.target_ppi = p.clamp(9.0, 2400.0);
            }
            if let Some(p) = v.get("above_ppi").and_then(Value::as_f64) {
                s.above_ppi = p.clamp(9.0, 2400.0);
            }
            s.above_ppi = s.above_ppi.max(s.target_ppi);
            if let Some(c) = v.get("compression").and_then(Value::as_str) {
                let q = v.get("quality").and_then(Value::as_u64).unwrap_or(60).clamp(1, 100) as u8;
                s.compression = match c {
                    "jpeg" => Compression::Jpeg(q),
                    "zip" | "flate" => Compression::Flate,
                    "retain" => Compression::Retain,
                    other => return Err(ToolError::InvalidArgs(format!("unknown compression {other:?} (jpeg, zip, retain)"))),
                };
            }
            Ok(s)
        };
        settings.color = images("color", settings.color)?;
        settings.gray = images("gray", settings.gray)?;
        for (key, flag) in [
            ("discard_thumbnails", &mut settings.discard_thumbnails),
            ("discard_alternate_images", &mut settings.discard_alternate_images),
            ("discard_tags", &mut settings.discard_tags),
            ("discard_print_settings", &mut settings.discard_print_settings),
            ("flate_unencoded", &mut settings.flate_unencoded),
            ("remove_invalid_links", &mut settings.remove_invalid_links),
            ("remove_unreferenced_dests", &mut settings.remove_unreferenced_dests),
        ] {
            if let Some(b) = a.opt_bool(key)? {
                *flag = b;
            }
        }
        let discard: Vec<pdfcraft_engine::Hidden> = match a.get("discard") {
            None => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or_else(|| ToolError::InvalidArgs("discard must be an array".into()))?
                .iter()
                .map(|x| {
                    x.as_str()
                        .and_then(pdfcraft_engine::Hidden::from_id)
                        .ok_or_else(|| ToolError::InvalidArgs(format!("unknown discard category {x}")))
                })
                .collect::<Result<_>>()?,
        };
        let (bytes, r) = self.session.optimized_bytes(id, &settings, &discard).map_err(failed)?;
        write_atomic(&path, &bytes)?;
        let o = &r.optimize;
        Ok(json!({
            "path": path.to_string_lossy(),
            "bytes_before": before,
            "bytes_after": bytes.len(),
            "images": o.images,
            "images_resampled": o.images_resampled,
            "images_recompressed": o.images_recompressed,
            "image_bytes_before": o.image_bytes_before,
            "image_bytes_after": o.image_bytes_after,
            "thumbnails": o.thumbnails,
            "alternate_images": o.alternate_images,
            "tags_removed": o.tags_removed,
            "streams_compressed": o.streams_compressed,
            "invalid_links": o.invalid_links,
            "invalid_bookmarks": o.invalid_bookmarks,
            "unreferenced_dests": o.unreferenced_dests,
            "unused_xobjects": o.unused_xobjects,
            "merged_objects": r.merged,
            "discarded": r.discarded.iter().map(|(h, n)| json!({ "category": h.id(), "count": n })).collect::<Vec<_>>(),
        }))
    }

    /// A watermark or background picture: `file` (an image or a PDF) and `file_page` (1-based).
    fn mark_file(&self, a: &Args) -> Result<Option<pdfcraft_engine::MarkFile>> {
        let Some(p) = a.opt_str("file")? else { return Ok(None) };
        let path = self.resolve(p, false)?;
        let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let page = a.opt_int("file_page")?.unwrap_or(1).max(1) as usize - 1;
        Ok(Some(pdfcraft_engine::MarkFile {
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            bytes: Arc::new(bytes),
            page,
        }))
    }

    fn apply(&mut self, a: &Args, edit: Edit) -> Result<Value> {
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        Ok(summary(self.doc(a)?))
    }

    fn marks(&mut self, tool: &str, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{Background, HeaderFooter, MarkKind, Watermark};
        let n = self.doc(a)?.info.pages.len();
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..n).collect(),
        };
        let color =
            |key: &str, default: [f64; 3]| -> Result<[f64; 3]> { Ok(a.opt_str(key)?.map(comments::parse_color).transpose()?.unwrap_or(default)) };
        let replace = a.opt_bool("replace")?.unwrap_or(false);
        let edit = match tool {
            "doc_header_footer" => {
                let mut hf = HeaderFooter::default();
                for (k, key) in ["header_left", "header_center", "header_right", "footer_left", "footer_center", "footer_right"].iter().enumerate() {
                    hf.text[k] = a.opt_str(key)?.unwrap_or_default().to_string();
                }
                if let Some(s) = a.opt_num("font_size")? {
                    hf.font_size = s;
                }
                hf.color = color("color", hf.color)?;
                if let Some(m) = a.get("margins").and_then(Value::as_array) {
                    let m: Vec<f64> = m.iter().filter_map(Value::as_f64).collect();
                    hf.margins = <[f64; 4]>::try_from(m).map_err(|_| ToolError::InvalidArgs("margins must be 4 numbers".into()))?;
                }
                if let Some(s) = a.opt_int("start_number")? {
                    hf.start_number = s.clamp(1, u32::MAX as i64) as u32;
                }
                Edit::AddHeaderFooter { pages, settings: hf, replace }
            }
            "doc_watermark" => {
                let d = Watermark::default();
                let file = self.mark_file(a)?;
                let text = match &file {
                    Some(_) => a.opt_str("text")?.unwrap_or_default().to_string(),
                    None => a.str("text")?.to_string(),
                };
                let wm = Watermark {
                    text,
                    source: None,
                    scale: a.opt_num("scale")?.unwrap_or(d.scale).clamp(0.01, 1.0),
                    font_size: a.opt_num("font_size")?.unwrap_or(0.0),
                    color: color("color", d.color)?,
                    opacity: a.opt_num("opacity")?.unwrap_or(d.opacity),
                    rotation: a.opt_num("rotation")?.unwrap_or(d.rotation),
                    behind: a.opt_bool("behind")?.unwrap_or(false),
                    offset: [0.0; 2],
                };
                Edit::AddWatermark { pages, settings: wm, replace, file }
            }
            "doc_background" => Edit::AddBackground {
                pages,
                settings: Background {
                    color: color("color", [1.0; 3])?,
                    opacity: a.opt_num("opacity")?.unwrap_or(1.0),
                    scale: a.opt_num("scale")?.unwrap_or(1.0).clamp(0.01, 1.0),
                    ..Background::default()
                },
                replace,
                file: self.mark_file(a)?,
            },
            _ => {
                let kind = match a.str("kind")? {
                    "header_footer" => MarkKind::HeaderFooter,
                    "watermark" => MarkKind::Watermark,
                    "background" => MarkKind::Background,
                    other => return Err(ToolError::InvalidArgs(format!("unknown kind {other:?}"))),
                };
                Edit::RemoveMarks { kind }
            }
        };
        self.apply(a, edit)
    }

    fn export(&mut self, tool: &str, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..doc.info.pages.len()).collect(),
        };
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let mut ex = pdfcraft_engine::export::Exporter::new(doc);
        if tool == "doc_export_text" {
            let path = self.resolve(a.str("path")?, true)?;
            let text = ex.text_of(&pages).map_err(failed)?;
            write_atomic(&path, text.as_bytes())?;
            return Ok(json!({ "path": path.to_string_lossy(), "pages": pages.len(), "bytes": text.len() }));
        }
        let folder = self.resolve(a.str("folder")?, true)?;
        std::fs::create_dir_all(&folder).map_err(|e| failed(format!("{}: {e}", folder.display())))?;
        if tool == "doc_export_all_images" {
            let min = u32::try_from(a.opt_int("min_size")?.unwrap_or(0).max(0)).unwrap_or(u32::MAX);
            let out = pdfcraft_engine::export::extract_images(&doc.export_source(), &pages, min).map_err(failed)?;
            let mut files = Vec::new();
            for (k, img) in out.images.iter().enumerate() {
                let path = child(&folder, &pdfcraft_engine::export::image_file_name(&stem, img, k + 1));
                write_atomic(&path, &img.data)?;
                files.push(json!({ "path": path.to_string_lossy(), "page": img.page + 1, "width": img.width, "height": img.height }));
            }
            let skipped: Vec<Value> = out.skipped.iter().map(|(p, r, why)| json!({ "page": p + 1, "object": r.num, "reason": why })).collect();
            return Ok(json!({ "count": files.len(), "files": files, "skipped": skipped }));
        }
        let dpi = a.opt_num("dpi")?.unwrap_or(150.0);
        let quality = a.opt_int("quality")?.unwrap_or(85).clamp(1, 100) as u8;
        let format = match a.opt_str("format")?.unwrap_or("png") {
            "png" => pdfcraft_engine::export::ImageFormat::Png,
            "jpeg" | "jpg" => pdfcraft_engine::export::ImageFormat::Jpeg { quality },
            "tiff" | "tif" => pdfcraft_engine::export::ImageFormat::Tiff,
            f => return Err(ToolError::InvalidArgs(format!("unknown format {f:?} (png, jpeg, tiff)"))),
        };
        let mut files = Vec::new();
        let mut lowered = Vec::new();
        for p in pages {
            let img = ex.image(p, dpi, format).map_err(failed)?;
            let path = child(&folder, &format!("{stem}_page_{}.{}", p + 1, format.extension()));
            write_atomic(&path, &img)?;
            files.push(path.to_string_lossy().into_owned());
            // A page too large for the renderer at `dpi` is drawn at the most it allows.
            let used = ex.dpi_used(p, dpi);
            if used < dpi.clamp(18.0, 1200.0) - 0.5 {
                lowered.push(json!({ "page": p + 1, "dpi": used.floor() }));
            }
        }
        if lowered.is_empty() {
            Ok(json!({ "count": files.len(), "files": files }))
        } else {
            Ok(json!({ "count": files.len(), "files": files, "lower_dpi": lowered }))
        }
    }

    fn doc_create(&mut self, a: &Args) -> Result<Value> {
        let (name, bytes) = match a.str("from")? {
            "blank" => {
                let n = a.opt_int("pages")?.unwrap_or(1).clamp(1, 10_000) as usize;
                let (w, h) = (a.opt_num("width")?.unwrap_or(612.0), a.opt_num("height")?.unwrap_or(792.0));
                ("Untitled.pdf".to_string(), self.session.create_blank(w, h, n).map_err(failed)?)
            }
            "images" => {
                let resolution = a.opt_num("dpi")?.map_or(pdfcraft_engine::ImageResolution::Embedded, pdfcraft_engine::ImageResolution::Dpi);
                let mut images = Vec::new();
                for p in a.strs("paths")? {
                    let path = self.resolve(p, false)?;
                    let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
                    images.push((path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), bytes));
                }
                let name = if images.len() == 1 {
                    format!("{}.pdf", images[0].0.rsplit_once('.').map_or(images[0].0.as_str(), |(s, _)| s))
                } else {
                    "Images.pdf".into()
                };
                (name, self.session.create_from_images_with_resolution(&images, resolution).map_err(failed)?)
            }
            "text" => {
                let (title, text) = match (a.opt_str("text")?, a.opt_str("path")?) {
                    (Some(t), _) => ("Text".to_string(), t.to_string()),
                    (None, Some(p)) => {
                        let path = self.resolve(p, false)?;
                        let t = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
                        (path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), pdfcraft_engine::decode_text(&t))
                    }
                    (None, None) => return Err(ToolError::InvalidArgs("text needs `text` or `path`".into())),
                };
                (format!("{title}.pdf"), self.session.create_from_text(&title, &text).map_err(failed)?)
            }
            other => return Err(ToolError::InvalidArgs(format!("unknown source {other:?}"))),
        };
        let name = a.opt_str("name")?.map(str::to_owned).unwrap_or(name);
        let id = self.session.open_new(name, bytes).map_err(failed)?;
        Ok(summary(self.session.get(id).ok_or_else(|| failed("the document vanished"))?))
    }

    fn page_set_box(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{BoxSpec, PageBox};
        let doc = self.doc(a)?;
        let n = doc.info.pages.len();
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..n).collect(),
        };
        let which = match a.opt_str("box")? {
            None => PageBox::Crop,
            Some(b) => PageBox::from_name(b).ok_or_else(|| ToolError::InvalidArgs(format!("unknown box {b:?}")))?,
        };
        let nums = |key: &str| -> Result<Option<[f64; 4]>> {
            a.get(key)
                .map(|v| {
                    let v: Vec<f64> = v.as_array().map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                    <[f64; 4]>::try_from(v).map_err(|_| ToolError::InvalidArgs(format!("{key} must be 4 numbers")))
                })
                .transpose()
        };
        let spec = match (nums("margins")?, nums("rect")?) {
            (Some(_), Some(_)) => return Err(ToolError::InvalidArgs("pass margins or rect, not both".into())),
            (Some(m), None) => BoxSpec::Margins(m),
            (None, Some(r)) => {
                // Top-left-origin points on the displayed page → user space (one page at a time).
                if pages.len() != 1 {
                    return Err(ToolError::InvalidArgs("rect applies to a single page; use margins for several".into()));
                }
                let p = &doc.info.pages[pages[0]];
                let (u0, u1) = (p.view_to_user(r[0] as f32, r[1] as f32), p.view_to_user(r[2] as f32, r[3] as f32));
                BoxSpec::Rect([u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64])
            }
            (None, None) => BoxSpec::Remove,
        };
        let mut out = self.apply(a, Edit::SetPageBox { pages, which, spec })?;
        out["page_sizes"] = json!(self.doc(a)?.info.pages.iter().map(|p| [p.width, p.height]).collect::<Vec<_>>());
        Ok(out)
    }

    fn doc_protect(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{Algorithm, Changes, Printing, Protection};
        let d = Protection::default();
        // Restrictions only exist behind a permissions password (ISO 32000-2 §7.6.4.4: /P is
        // enforced against the owner password; without one everything stays allowed). Refuse a
        // restriction that could not take effect instead of writing an unrestricted file (#134).
        let restriction = ["printing", "changes", "copy", "accessibility"].into_iter().find(|k| a.get(k).is_some());
        if let (None, Some(key)) = (a.opt_str("permissions_password")?, restriction) {
            return Err(ToolError::InvalidArgs(format!(
                "`{key}` needs `permissions_password`: with open_password alone the document is encrypted but nothing is restricted"
            )));
        }
        let p = Protection {
            open_password: a.opt_str("open_password")?.map(str::to_owned),
            permissions_password: a.opt_str("permissions_password")?.map(str::to_owned),
            printing: match a.opt_str("printing")? {
                None => d.printing,
                Some("none") => Printing::None,
                Some("low") => Printing::Low,
                Some("high") => Printing::High,
                Some(o) => return Err(ToolError::InvalidArgs(format!("unknown printing {o:?}"))),
            },
            changes: match a.opt_str("changes")? {
                None => d.changes,
                Some("none") => Changes::None,
                Some("pages") => Changes::Pages,
                Some("fill-sign") => Changes::FillSign,
                Some("comment-fill-sign") => Changes::CommentFillSign,
                Some("any-except-extract") => Changes::AnyExceptExtract,
                Some(o) => return Err(ToolError::InvalidArgs(format!("unknown changes {o:?}"))),
            },
            copy: a.opt_bool("copy")?.unwrap_or(d.copy),
            accessibility: a.opt_bool("accessibility")?.unwrap_or(d.accessibility),
            algorithm: match a.opt_str("compatibility")? {
                None | Some("aes-256") => Algorithm::Aes256,
                Some("aes-128") => Algorithm::Aes128,
                Some("rc4-128") => Algorithm::Rc4_128,
                Some("rc4-40") => Algorithm::Rc4_40,
                Some(o) => return Err(ToolError::InvalidArgs(format!("unknown compatibility {o:?}"))),
            },
            encrypt_metadata: a.opt_bool("encrypt_metadata")?.unwrap_or(d.encrypt_metadata),
        };
        let mut out = self.apply(a, Edit::Protect(p))?;
        out["security"] = security(self.doc(a)?);
        Ok(out)
    }

    fn insert_blank(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let at = self.position(a, "at")?;
        // Default to the size of the neighbouring page, as Acrobat does; Letter for empty files.
        let near = doc.info.pages.get(at.saturating_sub(1)).or(doc.info.pages.first());
        let (w, h) = near.map(|p| (f64::from(p.width), f64::from(p.height))).unwrap_or((612.0, 792.0));
        let width = a.opt_num("width")?.unwrap_or(w);
        let height = a.opt_num("height")?.unwrap_or(h);
        if !(1.0..=14400.0).contains(&width) || !(1.0..=14400.0).contains(&height) {
            return Err(ToolError::InvalidArgs("width and height must be between 1 and 14400 points".into()));
        }
        self.apply(a, Edit::InsertBlankPage { at, width, height })
    }

    fn insert_file(&mut self, a: &Args) -> Result<Value> {
        let at = self.position(a, "at")?;
        let path = self.resolve(a.str("path")?, false)?;
        let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let pages = a.opt_ints("pages")?.map(|p| one_based(&p)).transpose()?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.apply(a, Edit::InsertPagesFrom { name, bytes: Arc::new(bytes), pages, at })
    }

    /// Write freshly created bytes to `out` and/or open them as a new document.
    fn deliver(&mut self, a: &Args, name: &str, bytes: Arc<Vec<u8>>) -> Result<Value> {
        let mut result = json!({ "bytes": bytes.len() });
        let out = a.opt_str("out")?;
        if let Some(out) = out {
            let path = self.resolve(out, true)?;
            write_atomic(&path, &bytes)?;
            result["path"] = json!(path.to_string_lossy());
        }
        if a.opt_bool("open")?.unwrap_or(out.is_none()) {
            let id = self.session.open_new(name, bytes).map_err(failed)?;
            result["document"] = summary(self.session.get(id).ok_or_else(|| failed("the document vanished"))?);
        }
        Ok(result)
    }

    fn doc_create_multiple(&mut self, a: &Args) -> Result<Value> {
        let paths = a.strs("paths")?;
        if paths.is_empty() || paths.len() > pdfcraft_engine::MAX_CREATE_FILES {
            return Err(ToolError::InvalidArgs(format!("paths must list 1 to {} files", pdfcraft_engine::MAX_CREATE_FILES)));
        }
        let separate = match a.opt_str("mode")? {
            None | Some("combine") => false,
            Some("separate") => true,
            Some(m) => return Err(ToolError::InvalidArgs(format!("mode must be \"combine\" or \"separate\", not {m:?}"))),
        };
        if separate {
            return self.create_separate(a, &paths);
        }
        let ranges: Vec<Option<String>> = match a.get("pages") {
            None | Some(Value::Null) => vec![None; paths.len()],
            Some(Value::Array(v)) if v.len() == paths.len() => v.iter().map(|x| x.as_str().map(str::to_owned)).collect(),
            Some(_) => return Err(ToolError::InvalidArgs("pages must list a range (or null) for each path".into())),
        };
        let mut sources = Vec::new();
        for (p, range) in paths.into_iter().zip(ranges) {
            let path = self.resolve(p, false)?;
            let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let (_, pdf) = self.session.convert_to_pdf(&name, &Arc::new(bytes)).map_err(failed)?;
            let title = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            sources.push((title, pdf, range));
        }
        let bytes = self.session.combine_ranges(&sources).map_err(failed)?;
        self.deliver(a, "Combined", bytes)
    }

    /// One PDF per file, written into `out_dir`; a file that fails doesn't stop the others.
    fn create_separate(&mut self, a: &Args, paths: &[&str]) -> Result<Value> {
        let dir = self.resolve(a.str("out_dir").map_err(|_| ToolError::InvalidArgs("mode \"separate\" needs out_dir".into()))?, true)?;
        std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
        let mut out = Vec::new();
        for &p in paths {
            let converted = self.resolve(p, false).map_err(|e| e.to_string()).and_then(|src| {
                let bytes = std::fs::read(&src).map_err(|e| format!("{}: {e}", src.display()))?;
                let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let stem = src.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
                let (kind, pdf) = self.session.convert_to_pdf(&name, &Arc::new(bytes)).map_err(|e| e.to_string())?;
                Ok((kind, stem, pdf))
            });
            out.push(match converted {
                Ok((pdfcraft_engine::SourceKind::Pdf, ..)) => json!({ "path": p, "skipped": "already a PDF" }),
                Ok((_, stem, pdf)) => {
                    let target = unused(&dir, &stem);
                    write_atomic(&target, &pdf)?;
                    json!({ "path": p, "output": target.to_string_lossy(), "bytes": pdf.len() })
                }
                Err(e) => json!({ "path": p, "error": e }),
            });
        }
        Ok(json!({ "files": out }))
    }

    fn page_extract(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let (id, name) = (doc.id, format!("{} (extract)", doc.name));
        let stem = Path::new(&doc.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "page".into());
        let pages = self.pages(a, "pages")?;
        let mut out = if a.opt_bool("separate")?.unwrap_or(false) {
            // Each page as its own file.
            let dir = self.resolve(a.str("out_dir").map_err(|_| ToolError::InvalidArgs("separate files need out_dir".into()))?, true)?;
            std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
            let mut files = Vec::new();
            for &p in &pages {
                let bytes = self.session.extract(id, &[p]).map_err(failed)?;
                let path = child(&dir, &format!("{stem}-page{}.pdf", p + 1));
                write_atomic(&path, &bytes)?;
                files.push(path.to_string_lossy().into_owned());
            }
            json!({ "files": files })
        } else {
            let bytes = self.session.extract(id, &pages).map_err(failed)?;
            self.deliver(a, &name, bytes)?
        };
        if a.opt_bool("delete")?.unwrap_or(false) {
            let deleted = self.apply(a, Edit::DeletePages { pages })?;
            out["original"] = deleted;
        }
        Ok(out)
    }

    fn doc_combine(&mut self, a: &Args) -> Result<Value> {
        let paths = a.strs("paths")?;
        if paths.len() < 2 {
            return Err(ToolError::InvalidArgs("combine needs at least two files".into()));
        }
        let ranges: Vec<Option<String>> = match a.get("pages") {
            None | Some(Value::Null) => vec![None; paths.len()],
            Some(Value::Array(v)) if v.len() == paths.len() => v
                .iter()
                .enumerate()
                .map(|(index, value)| match value {
                    Value::Null => Ok(None),
                    Value::String(range) => Ok(Some(range.clone())),
                    _ => Err(ToolError::InvalidArgs(format!("pages[{index}] must be a range string or null"))),
                })
                .collect::<Result<_>>()?,
            Some(_) => return Err(ToolError::InvalidArgs("pages must list a range (or null) for each path".into())),
        };
        let passwords: Vec<Option<String>> = match a.get("passwords") {
            None | Some(Value::Null) => vec![None; paths.len()],
            Some(Value::Array(v)) if v.len() == paths.len() && v.iter().all(|x| x.is_string() || x.is_null()) => {
                v.iter().map(|x| x.as_str().map(str::to_owned)).collect()
            }
            Some(_) => return Err(ToolError::InvalidArgs("passwords must list a password (or null) for each path".into())),
        };
        // Entries sharing a group number are one file split into parts placed apart.
        let groups: Option<Vec<Option<u64>>> = match a.get("groups") {
            None | Some(Value::Null) => None,
            Some(Value::Array(v)) if v.len() == paths.len() && v.iter().all(|x| x.is_u64() || x.is_null()) => {
                Some(v.iter().map(Value::as_u64).collect())
            }
            Some(_) => return Err(ToolError::InvalidArgs("groups must list a number (or null) for each path".into())),
        };
        let mut sources: Vec<CombineSource> = Vec::new();
        let mut firsts: Vec<(u64, PathBuf, usize)> = Vec::new();
        for (i, (p, range)) in paths.into_iter().zip(ranges).enumerate() {
            let path = self.resolve(p, false)?;
            let group = groups.as_ref().and_then(|g| g.get(i).copied().flatten());
            let first = group.and_then(|g| firsts.iter().find(|f| f.0 == g));
            let bytes = match first {
                Some((g, first_path, at)) => {
                    if *first_path != path {
                        return Err(ToolError::InvalidArgs(format!("groups[{i}]: group {g} is {}, not {}", first_path.display(), path.display())));
                    }
                    sources.get(*at).map(|s| s.1.clone()).ok_or_else(|| failed("group source missing"))?
                }
                None => {
                    if let Some(g) = group {
                        firsts.push((g, path.clone(), sources.len()));
                    }
                    Arc::new(std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?)
                }
            };
            let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            sources.push((name, bytes, range));
        }
        let passwords: Vec<Option<&str>> = passwords.iter().map(Option::as_deref).collect();
        let bytes = match groups {
            None => self.session.combine_unlocked(&sources, &passwords),
            Some(groups) => {
                // A path without a group is a file of its own: give it a key no group uses.
                let mut next = groups.iter().flatten().max().map_or(Some(0), |m| m.checked_add(1));
                let mut keys = Vec::with_capacity(groups.len());
                for g in &groups {
                    keys.push(match g {
                        Some(g) => *g,
                        None => {
                            let own = next.ok_or_else(|| ToolError::InvalidArgs("group numbers are too large".into()))?;
                            next = own.checked_add(1);
                            own
                        }
                    });
                }
                self.session.combine_grouped(&sources, &keys, &passwords)
            }
        }
        .map_err(failed)?;
        self.deliver(a, "Combined", bytes)
    }

    fn doc_split(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let id = doc.id;
        let stem = Path::new(&doc.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "part".into());
        let bookmarks = a.opt_bool("bookmarks")?.unwrap_or(false);
        let max_mb = a.opt_num("max_mb")?;
        let chosen = [a.get("every").is_some(), a.get("before").is_some(), bookmarks, max_mb.is_some()].iter().filter(|x| **x).count();
        if chosen != 1 {
            return Err(ToolError::InvalidArgs(
                "pass exactly one of every (pages per file), before (page numbers), bookmarks: true, or max_mb".into(),
            ));
        }
        let mut titles: Vec<(usize, String)> = Vec::new();
        let parts = if let Some(mb) = max_mb {
            if !mb.is_finite() || mb <= 0.0 {
                return Err(ToolError::InvalidArgs("max_mb must be positive".into()));
            }
            self.session.split_by_size(id, (mb * 1_048_576.0) as usize).map_err(failed)?
        } else {
            let by = match (a.opt_int("every")?, a.opt_ints("before")?) {
                (Some(n), None) if n > 0 => pdfcraft_organize::SplitBy::PageCount(n as usize),
                (None, Some(b)) => pdfcraft_organize::SplitBy::Before(one_based(&b)?),
                _ if bookmarks => {
                    titles = self.session.bookmark_splits(id);
                    if titles.is_empty() {
                        return Err(failed("the document has no top-level bookmarks"));
                    }
                    pdfcraft_organize::SplitBy::Before(titles.iter().map(|t| t.0).collect())
                }
                _ => return Err(ToolError::InvalidArgs("every must be a positive page count".into())),
            };
            self.session.split(id, &by).map_err(failed)?
        };
        let dir = self.resolve(a.str("out_dir")?, true)?;
        std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
        let mut files = Vec::new();
        let mut used = std::collections::HashSet::new();
        for (i, (first, last, bytes)) in parts.iter().enumerate() {
            let safe = |t: &str| t.chars().map(|c| if c.is_alphanumeric() || " -_.,()".contains(c) { c } else { '_' }).collect::<String>();
            let file = match titles.iter().find(|t| t.0 + 1 == *first) {
                Some((_, t)) => format!("{stem}-{}.pdf", safe(t)),
                None => format!("{stem}-part{}.pdf", i + 1),
            };
            // Equal titles (or titles that sanitize alike) get -2, -3, ... so no part overwrites another; compared case-insensitively.
            let mut unique = file.clone();
            let mut n = 1;
            while !used.insert(unique.to_lowercase()) {
                n += 1;
                unique = format!("{}-{n}.pdf", file.trim_end_matches(".pdf"));
            }
            let path = child(&dir, &unique);
            write_atomic(&path, bytes)?;
            files.push(json!({ "path": path.to_string_lossy(), "first_page": first, "last_page": last }));
        }
        Ok(json!({ "files": files }))
    }

    // ---- pages and text ----------------------------------------------------------------------

    /// 1-based page list → validated 0-based indices.
    fn pages(&self, a: &Args, key: &str) -> Result<Vec<usize>> {
        let n = self.doc(a)?.info.pages.len();
        let pages = one_based(&a.ints(key)?)?;
        if pages.is_empty() {
            return Err(ToolError::InvalidArgs(format!("{key} must list at least one page")));
        }
        if let Some(p) = pages.iter().find(|p| **p >= n) {
            return Err(ToolError::InvalidArgs(format!("page {} is out of range: the document has {n} pages", p + 1)));
        }
        Ok(pages)
    }

    /// A 1-based insertion position (1 = before the first page, n+1 = after the last) → 0-based.
    fn position(&self, a: &Args, key: &str) -> Result<usize> {
        let n = self.doc(a)?.info.pages.len();
        let at = a.int(key)?;
        if at < 1 || at as usize > n + 1 {
            return Err(ToolError::InvalidArgs(format!("{key} must be between 1 and {} (the document has {n} pages)", n + 1)));
        }
        Ok(at as usize - 1)
    }

    fn renderer(&mut self, id: DocId) -> Result<&mut PageRenderer> {
        let doc = self.session.get(id).ok_or_else(|| failed("no such document"))?;
        let fresh = || {
            let config = RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() };
            (doc.display.clone(), PageRenderer::new(doc.display.clone(), config))
        };
        let entry = self.renderers.entry(id).or_insert_with(fresh);
        if !Arc::ptr_eq(&entry.0, &doc.display) {
            *entry = fresh();
        }
        Ok(&mut entry.1)
    }

    fn page_render(&mut self, a: &Args) -> Result<Content> {
        let id = self.doc(a)?.id;
        let page = self.page(a)?;
        let dpi = a.opt_num("dpi")?.unwrap_or(DEFAULT_DPI);
        if !(1.0..=MAX_DPI).contains(&dpi) {
            return Err(ToolError::InvalidArgs(format!("dpi must be between 1 and {MAX_DPI}")));
        }
        let out = self.renderer(id)?.render(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: (dpi / 72.0) as f32, tag: 0 });
        if let Some(e) = out.error {
            return Err(failed(format!("page {} could not be rendered: {e}", page + 1)));
        }
        let data = encode_png(out.width, out.height, &out.rgba)?;
        Ok(Content::Png { data, width: out.width, height: out.height })
    }

    fn page(&self, a: &Args) -> Result<usize> {
        let n = self.doc(a)?.info.pages.len();
        let p = a.int("page")?;
        if p < 1 || p as usize > n {
            return Err(ToolError::InvalidArgs(format!("page {p} is out of range: the document has {n} pages")));
        }
        Ok(p as usize - 1)
    }

    /// The text of `pages` (0-based), from the cache or extracted in parallel.
    fn page_texts(&mut self, id: DocId, pages: &[usize]) -> Result<Vec<Arc<PageText>>> {
        let doc = self.session.get(id).ok_or_else(|| failed("no such document"))?;
        let (bytes, n) = (doc.bytes.clone(), doc.info.pages.len());
        let password: Option<Arc<str>> = doc.password.as_deref().map(Arc::from);
        let entry = self.texts.entry(id).or_insert_with(|| (bytes.clone(), Vec::new()));
        if !Arc::ptr_eq(&entry.0, &bytes) || entry.1.len() != n {
            *entry = (bytes.clone(), vec![None; n]);
        }
        let mut missing: Vec<usize> = pages.iter().copied().filter(|p| entry.1.get(*p).is_some_and(Option::is_none)).collect();
        missing.sort_unstable();
        missing.dedup();
        for (p, text) in extract_parallel(&bytes, password, &missing) {
            let text = Arc::new(text.map_err(|e| failed(format!("page {}: {e}", p + 1)))?);
            if let Some(slot) = entry.1.get_mut(p) {
                *slot = Some(text);
            }
        }
        pages.iter().map(|p| entry.1.get(*p).cloned().flatten().ok_or_else(|| failed(format!("page {} has no text", p + 1)))).collect()
    }

    fn text_extract(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let id = doc.id;
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..doc.info.pages.len()).collect(),
        };
        // Column select (#740): only the text inside the rectangle, row by row.
        let rect = a.nums::<4>("rect")?.map(|r| r.map(|v| v as f32));
        let texts = self.page_texts(id, &pages)?;
        let text = |t: &PageText| match rect {
            Some(r) => t.column_text(&t.glyphs_in(r)),
            None => t.plain_text(),
        };
        let out: Vec<Value> = pages.iter().zip(texts).map(|(p, t)| json!({ "page": p + 1, "text": text(&t) })).collect();
        Ok(json!({ "pages": out }))
    }

    fn text_find(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let (id, n) = (doc.id, doc.info.pages.len());
        let query = a.str("query")?;
        let limit = a.opt_int("limit")?.unwrap_or(500).max(1) as usize;
        let texts = self.page_texts(id, &(0..n).collect::<Vec<_>>())?;
        let mut matches = Vec::new();
        'pages: for (p, text) in texts.iter().enumerate() {
            for r in text.find(query) {
                if matches.len() == limit {
                    break 'pages;
                }
                matches.push(json!({ "page": p + 1, "text": text.text_of(r.clone()), "rects": text.line_rects(r) }));
            }
        }
        Ok(json!({ "query": query, "count": matches.len(), "matches": matches }))
    }

    fn command_list(&self, a: &Args) -> Result<Value> {
        let active = match a.opt_int("doc")? {
            Some(_) => Some(self.doc(a)?.id),
            None => None,
        };
        let filter = a.opt_str("filter")?.unwrap_or("").to_lowercase();
        let enabled_only = a.opt_bool("enabled_only")?.unwrap_or(false);
        let list: Vec<Value> = commands::COMMANDS
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "label": commands::current_label(c, &self.session, active),
                    "menu": c.menu,
                    "shortcut": c.shortcut.map(|s| s.label(cfg!(target_os = "macos"))),
                    "enabled": commands::is_enabled(c, &self.session, active),
                    "tool": tools::tool_for_command(c.id),
                    "params": tools::tool_for_command(c.id).and_then(tools::find).map(|t| &t.input_schema),
                })
            })
            .collect();
        let list: Vec<Value> = list
            .into_iter()
            .filter(|c| {
                (!enabled_only || c.get("enabled").and_then(Value::as_bool) == Some(true))
                    && (filter.is_empty()
                        || ["id", "label", "menu"].iter().any(|k| c.get(k).is_some_and(|v| v.to_string().to_lowercase().contains(&filter))))
            })
            .collect();
        Ok(json!({ "commands": list }))
    }

    // ---- paths -------------------------------------------------------------------------------

    /// Resolve a user-supplied path, enforcing the root (if any). `for_write` allows a file that
    /// does not exist yet (its nearest existing ancestor must be inside the root).
    ///
    /// Every path outside the root gets the same refusal, so a confined client can't learn what
    /// exists out there (#136): `..` is resolved by name first, another network share or device
    /// namespace is refused without touching it, and the deepest existing ancestor (links
    /// followed) must be inside the root before anything about the rest is reported.
    fn resolve(&self, path: &str, for_write: bool) -> Result<PathBuf> {
        let p = Path::new(path);
        let Some(root) = &self.root else { return Ok(p.to_path_buf()) };
        let outside = || failed(format!("{path} is outside the allowed directory {}", root.display()));
        let joined = lexical(&root.join(p)).ok_or_else(outside)?;
        if foreign_share(&joined, root) {
            return Err(outside());
        }
        let mut existing = joined.as_path();
        let mut rest = Vec::new();
        while !existing.exists() {
            if existing.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(outside()); // a broken link: going on would tell whether its target exists
            }
            match (existing.file_name(), existing.parent()) {
                (Some(name), Some(parent)) => {
                    rest.push(name);
                    existing = parent;
                }
                _ => return Err(outside()), // not even a drive or share that exists
            }
        }
        let mut real = existing.canonicalize().map_err(|_| outside())?;
        if !real.starts_with(root) {
            return Err(outside());
        }
        if !rest.is_empty() && !for_write {
            // Missing, below a folder inside the root: say why, as the system reports it.
            real = joined.canonicalize().map_err(|e| failed(format!("{path}: {e}")))?;
            if !real.starts_with(root) {
                return Err(outside()); // it appeared, as a link out, since the check above
            }
            return Ok(real);
        }
        real.extend(rest.iter().rev());
        Ok(real)
    }
}

/// `path` with `.` and `..` resolved by name, before the filesystem is consulted (Windows does
/// the same, and so does joining onto a canonical root there). `None` if a `..` would climb
/// above the start of the path.
fn lexical(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let mut parts: Vec<Component> = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match parts.last() {
                Some(Component::Normal(_)) => {
                    parts.pop();
                }
                _ => return None,
            },
            other => parts.push(other),
        }
    }
    Some(parts.iter().collect())
}

/// Whether `path` names another network share or device namespace than `root` (Windows
/// `\\host\share`, `\\?\UNC\…`, `\\.\…`, `\\?\…`). Those are refused by name: even checking
/// that one exists would contact the host. Drive letters are left to the normal check.
fn foreign_share(path: &Path, root: &Path) -> bool {
    use std::path::{Component, Prefix};
    fn share(p: &Path) -> Option<String> {
        let Some(Component::Prefix(prefix)) = p.components().next() else { return None };
        let (kind, a, b) = match prefix.kind() {
            Prefix::Disk(_) | Prefix::VerbatimDisk(_) => return None,
            Prefix::UNC(host, share) | Prefix::VerbatimUNC(host, share) => ("unc", host, share),
            Prefix::DeviceNS(name) => ("device", name, std::ffi::OsStr::new("")),
            Prefix::Verbatim(name) => ("verbatim", name, std::ffi::OsStr::new("")),
        };
        Some(format!("{kind}\\{}\\{}", a.to_string_lossy(), b.to_string_lossy()).to_lowercase())
    }
    share(path).is_some_and(|s| share(root).as_ref() != Some(&s))
}

// ---- JSON helpers ----------------------------------------------------------------------------

/// Typed access to validated arguments.
struct Args<'a>(&'a Value);

impl Args<'_> {
    fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }
    fn missing(key: &str) -> ToolError {
        ToolError::InvalidArgs(format!("missing argument {key}"))
    }
    fn wrong(key: &str, what: &str) -> ToolError {
        ToolError::InvalidArgs(format!("{key} must be {what}"))
    }
    fn str(&self, key: &str) -> Result<&str> {
        self.opt_str(key)?.ok_or_else(|| Self::missing(key))
    }
    fn opt_str(&self, key: &str) -> Result<Option<&str>> {
        self.get(key).map(|v| v.as_str().ok_or_else(|| Self::wrong(key, "a string"))).transpose()
    }
    fn int(&self, key: &str) -> Result<i64> {
        self.opt_int(key)?.ok_or_else(|| Self::missing(key))
    }
    fn opt_int(&self, key: &str) -> Result<Option<i64>> {
        self.get(key).map(|v| v.as_i64().ok_or_else(|| Self::wrong(key, "an integer"))).transpose()
    }
    fn opt_num(&self, key: &str) -> Result<Option<f64>> {
        self.get(key).map(|v| v.as_f64().filter(|f| f.is_finite()).ok_or_else(|| Self::wrong(key, "a number"))).transpose()
    }
    fn opt_bool(&self, key: &str) -> Result<Option<bool>> {
        self.get(key).map(|v| v.as_bool().ok_or_else(|| Self::wrong(key, "true or false"))).transpose()
    }
    fn ints(&self, key: &str) -> Result<Vec<i64>> {
        self.opt_ints(key)?.ok_or_else(|| Self::missing(key))
    }
    fn opt_ints(&self, key: &str) -> Result<Option<Vec<i64>>> {
        let Some(v) = self.get(key) else { return Ok(None) };
        let arr = v.as_array().ok_or_else(|| Self::wrong(key, "an array of integers"))?;
        arr.iter().map(|x| x.as_i64().ok_or_else(|| Self::wrong(key, "an array of integers"))).collect::<Result<Vec<_>>>().map(Some)
    }
    /// A 1-based bookmark path → 0-based indices.
    fn path(&self, key: &str) -> Result<Vec<usize>> {
        let p = self.opt_path(key)?.ok_or_else(|| Self::missing(key))?;
        if p.is_empty() {
            return Err(ToolError::InvalidArgs(format!("{key} must not be empty")));
        }
        Ok(p)
    }
    fn opt_path(&self, key: &str) -> Result<Option<Vec<usize>>> {
        let Some(v) = self.opt_ints(key)? else { return Ok(None) };
        v.iter().map(|i| if *i >= 1 { Ok(*i as usize - 1) } else { Err(Self::wrong(key, "1-based positions")) }).collect::<Result<Vec<_>>>().map(Some)
    }
    fn strs(&self, key: &str) -> Result<Vec<&str>> {
        let v = self.get(key).ok_or_else(|| Self::missing(key))?;
        let arr = v.as_array().ok_or_else(|| Self::wrong(key, "an array of strings"))?;
        arr.iter().map(|x| x.as_str().ok_or_else(|| Self::wrong(key, "an array of strings"))).collect()
    }
}

/// Bookmark rows in listing order as JSON: a row holds the rows that follow it on the page at the
/// next level down in `children`, and paths and pages are 1-based.
fn nest_rows(rows: &[pdfcraft_engine::BookmarkRow]) -> Vec<Value> {
    // The open rows, outermost first, with their depth and the children gathered so far.
    let mut open: Vec<(usize, Value, Vec<Value>)> = Vec::new();
    let mut roots = Vec::new();
    for row in rows {
        let depth = row.path.len().saturating_sub(1);
        while open.last().is_some_and(|(d, _, _)| *d >= depth) {
            close_row(&mut open, &mut roots);
        }
        let path: Vec<usize> = row.path.iter().map(|p| p.saturating_add(1)).collect();
        let fields = json!({ "path": path, "title": row.title, "page": row.page.map(|p| p.saturating_add(1)), "open": row.open });
        open.push((depth, fields, Vec::new()));
    }
    while !open.is_empty() {
        close_row(&mut open, &mut roots);
    }
    roots
}

/// Closes the innermost open row: it joins the row above it as a child, or the top level.
fn close_row(open: &mut Vec<(usize, Value, Vec<Value>)>, roots: &mut Vec<Value>) {
    if let Some((_, mut fields, children)) = open.pop() {
        fields["children"] = Value::Array(children);
        match open.last_mut() {
            Some((_, _, siblings)) => siblings.push(fields),
            None => roots.push(fields),
        }
    }
}

fn one_based(pages: &[i64]) -> Result<Vec<usize>> {
    pages
        .iter()
        .map(|p| if *p >= 1 { Ok(*p as usize - 1) } else { Err(ToolError::InvalidArgs(format!("page numbers start at 1 (got {p})"))) })
        .collect()
}

/// The document's security as the next save writes it (never includes passwords).
fn security(d: &Document) -> Value {
    match d.security_summary() {
        None => json!({ "protected": false }),
        Some(s) => {
            let p = s.permissions;
            json!({
                "protected": true,
                "method": s.method,
                "pending": s.pending,
                "printing": if !p.print() { "none" } else if p.print_high_quality() { "high" } else { "low" },
                "modify": p.modify(), "assemble": p.assemble(), "copy": p.copy(), "annotate": p.annotate(),
                "fill_forms": p.fill_forms(), "accessibility": p.extract_for_accessibility(),
            })
        }
    }
}

/// What every tool that changes a document returns.
fn summary(d: &Document) -> Value {
    json!({
        "doc": d.id.0,
        "name": d.name,
        "path": d.path,
        "pages": d.info.pages.len(),
        "dirty": d.dirty,
        "editable": d.read_only_reason.is_none(),
        "read_only_reason": d.read_only_reason,
        "encrypted": d.info.encrypted,
        "undo": d.can_undo(),
        "redo": d.can_redo(),
    })
}

fn info(d: &Document) -> Value {
    let i = &d.info;
    let page1 = |p: usize| p + 1;
    let mut security = security(d);
    if let Some(s) = d.security_summary() {
        security["opened_as_owner"] = json!(s.owner);
    }
    json!({
        "document": summary(d),
        "pdf_version": i.pdf_version,
        "file_size": i.file_size,
        "title": i.title, "author": i.author, "subject": i.subject, "keywords": i.keywords,
        "creator": i.creator, "producer": i.producer,
        "tagged": i.tagged,
        "has_javascript": i.has_javascript,
        // XFA forms: "static" (the PDF's own fields work; the XFA data is ignored) or "dynamic"
        // (laid out from the template by PdfCraft, see xfa_layout; placeholder pages when that failed).
        "xfa": i.xfa.map(|x| match x { pdfcraft_render::Xfa::Static => "static", pdfcraft_render::Xfa::Dynamic => "dynamic" }),
        "xfa_layout": d.xfa.as_ref().map(|x| json!({ "pages": x.pages, "fields": x.fields, "warnings": x.warnings })),
        // What was rewritten from, or could not be written to, the XFA data.
        "xfa_warnings": d.xfa_warnings,
        "security": security,
        "pages": i.pages.iter().enumerate().map(|(n, p)| json!({
            "page": n + 1, "label": p.label, "width": p.width, "height": p.height, "rotation": p.rotation,
        })).collect::<Vec<_>>(),
        "outline": outline(&i.outline),
        "outline_more": i.outline_more,
        // Rectangles use the tools' convention (top-left of the displayed page, like
        // comment_list and link_list), not raw PDF user space, so they can be fed back to
        // geometry-taking tools (#129).
        "annotations": i.annotations.iter().map(|a| json!({
            "page": page1(a.page), "type": a.subtype, "author": a.author, "contents": a.contents,
            "modified": a.modified, "name": a.name, "in_reply_to": a.in_reply_to, "rect": view_rect(i, a.page, a.rect),
        })).collect::<Vec<_>>(),
        "fields": i.fields.iter().map(|f| json!({
            "name": f.name, "kind": format!("{:?}", f.kind), "value": f.value, "page": f.page.map(page1),
            "tooltip": f.tooltip, "has_actions": f.has_actions,
        })).collect::<Vec<_>>(),
        "links": i.links.iter().map(|l| json!({
            "page": page1(l.page), "rect": view_rect(i, l.page, l.rect),
            "target": match &l.target {
                pdfcraft_render::LinkTarget::Page(p, _) => json!({ "page": page1(*p) }),
                pdfcraft_render::LinkTarget::Uri(u) => json!({ "uri": u }),
                pdfcraft_render::LinkTarget::SetLayers { changes, preserve_rb } => json!({
                    "layers": changes.iter().map(|(op, ocg)| json!({
                        "layer": i.layers.iter().find(|l| l.id == *ocg).map(|l| l.name.as_str()),
                        "state": match op {
                            pdfcraft_render::LayerOp::On => "on",
                            pdfcraft_render::LayerOp::Off => "off",
                            pdfcraft_render::LayerOp::Toggle => "toggle",
                        },
                    })).collect::<Vec<_>>(),
                    "preserve_rb": preserve_rb,
                }),
                pdfcraft_render::LinkTarget::Other(o) => json!({ "other": o }),
            },
        })).collect::<Vec<_>>(),
        "layers": i.layers.iter().map(|l| json!({ "name": l.name, "visible": l.visible })).collect::<Vec<_>>(),
        "attachments": i.attachments.iter().map(|a| json!({ "name": a.name, "description": a.description, "size": a.size })).collect::<Vec<_>>(),
        "fonts": i.fonts.iter().map(|f| json!({
            "name": f.name, "kind": f.kind, "embedded": f.embedded, "subset": f.subset, "encoding": f.encoding,
        })).collect::<Vec<_>>(),
        "warnings": i.warnings,
        "repairs": d.repair_log(),
    })
}

/// A user-space rectangle on 0-based `page` in displayed-page coordinates; unchanged when the
/// page is unknown (a malformed annotation that points at no page).
fn view_rect(i: &pdfcraft_render::DocInfo, page: usize, rect: [f32; 4]) -> [f32; 4] {
    i.pages.get(page).map_or(rect, |p| comments::rect_to_view(p, rect))
}

fn outline(items: &[pdfcraft_render::OutlineItem]) -> Value {
    Value::Array(items.iter().map(|o| json!({ "title": o.title, "page": o.page.map(|p| p + 1), "children": outline(&o.children) })).collect())
}

fn encode_png(width: u32, height: u32, premultiplied: &[u8]) -> Result<Vec<u8>> {
    let mut rgba = premultiplied.to_vec();
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(failed)?;
    w.write_image_data(&rgba).map_err(failed)?;
    w.finish().map_err(failed)?;
    Ok(out)
}

/// Extract the text of `pages` (0-based) with one renderer per worker thread.
fn extract_parallel(bytes: &Arc<Vec<u8>>, password: Option<Arc<str>>, pages: &[usize]) -> Vec<(usize, std::result::Result<PageText, String>)> {
    let extract = |r: &mut PageRenderer, p: usize| {
        let out = r.render(RenderRequest { page: p, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0 });
        match out.error {
            Some(e) => Err(e),
            None => Ok(out.text.map(|t| (*t).clone()).unwrap_or_default()),
        }
    };
    let config = RenderConfig { password, ..Default::default() };
    let workers = if cfg!(target_arch = "wasm32") { 1 } else { std::thread::available_parallelism().map_or(1, |n| n.get()).min(8) };
    // Small jobs aren't worth a second parse of the document.
    if workers == 1 || pages.len() < 8 {
        let mut r = PageRenderer::new(bytes.clone(), config);
        return pages.iter().map(|p| (*p, extract(&mut r, *p))).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<(usize, std::result::Result<PageText, String>)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut r = PageRenderer::new(bytes.clone(), config.clone());
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(p) = pages.get(i) else { break };
                        done.push((*p, extract(&mut r, *p)));
                    }
                    done
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    out.sort_by_key(|(p, _)| *p);
    out
}

/// The file `name` inside `dir`, where `name` comes from a document or an argument: separators,
/// colons (a Windows drive or stream) and control characters become `_`, and a name of only dots
/// gets a leading `_`, so it is one plain file name and can't lead out of `dir`.
fn child(dir: &Path, name: &str) -> PathBuf {
    let mut safe: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':') || c.is_control() { '_' } else { c }).collect();
    if safe.chars().all(|c| c == '.') {
        safe.insert(0, '_');
    }
    dir.join(safe)
}

/// `<stem>.pdf` in `dir`, or `<stem> (2).pdf` and so on when that name is taken.
fn unused(dir: &Path, stem: &str) -> PathBuf {
    let first = child(dir, &format!("{stem}.pdf"));
    if !first.exists() {
        return first;
    }
    (2..10_000u32).map(|n| child(dir, &format!("{stem} ({n}).pdf"))).find(|p| !p.exists()).unwrap_or(first)
}

/// Write via a temporary file in the same directory, then rename over the target.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_with(path, bytes, staging_suffixes())
}

/// [`write_atomic`], trying the staging names that `suffixes` give.
fn write_atomic_with(path: &Path, bytes: &[u8], suffixes: impl IntoIterator<Item = u64>) -> Result<()> {
    use std::io::Write;
    // A folder can't be replaced, and staging beside it could land outside the root: "." names
    // the root itself, whose parent isn't ours to write in.
    if path.is_dir() {
        return Err(failed(format!("{}: is a folder, not a file", path.display())));
    }
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| failed(format!("{}: not a file path", path.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
    let (tmp, file) =
        create_staging(dir, &name.to_string_lossy(), StagingName::SuffixThenTag, suffixes).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
    // Closed at the end of the block, before the rename.
    let written = {
        let mut file = file;
        file.write_all(bytes)
    };
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(failed(format!("{}: {e}", tmp.display())));
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        failed(format!("{}: {e}", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_platform::staging::STAGING_ATTEMPTS;

    #[test]
    fn dot_dot_resolves_by_name_and_never_climbs_above_the_start() {
        let base = std::env::temp_dir();
        let l = |rel: &str| lexical(&base.join(rel));
        assert_eq!(l("a/./b/../c"), Some(base.join("a").join("c")));
        assert_eq!(l("missing/../../x"), lexical(&base.join("..").join("x")));
        assert_eq!(l("a/.."), Some(lexical(&base).unwrap()));
        let deep = "../".repeat(base.components().count() + 1);
        assert_eq!(lexical(&base.join(&deep)), None);
        assert_eq!(lexical(Path::new("..")), None);
    }

    #[test]
    fn names_from_documents_become_one_plain_file_name() {
        let dir = std::env::temp_dir().join("out");
        for name in ["../../x.png", "/abs/x.png", r"..\..\x.png", "C:x.png", r"\\host\share\x.png", "a:stream", "..", ".", "", "tab\there", "ok.png"]
        {
            let p = child(&dir, name);
            assert_eq!(p.parent(), Some(dir.as_path()), "{name:?} -> {}", p.display());
            assert_eq!(p.components().count(), dir.components().count() + 1, "{name:?} -> {}", p.display());
        }
        assert_eq!(child(&dir, "ok.png"), dir.join("ok.png"));
        assert_eq!(child(&dir, "../x.png"), dir.join(".._x.png"));
    }

    #[cfg(windows)]
    #[test]
    fn other_shares_and_device_paths_are_refused_by_name() {
        let local = Path::new(r"\\?\C:\work\root");
        for p in [
            r"\\host\share\x.pdf",
            "//host/share/x.pdf",
            r"\/host/share/x.pdf",
            r"\\?\UNC\host\share\x.pdf",
            r"\\.\pipe\x",
            r"\\.\C:\work\root\x.pdf",
            r"\\?\GLOBALROOT\Device\x",
        ] {
            assert!(foreign_share(Path::new(p), local), "{p}");
        }
        for p in [r"C:\work\root\x.pdf", r"c:\elsewhere\x.pdf", r"\\?\C:\work\root\x.pdf", r"D:\x.pdf", r"C:x.pdf"] {
            assert!(!foreign_share(Path::new(p), local), "{p}");
        }
        // A root on a share accepts that share, however it is spelled, and nothing else.
        let shared = Path::new(r"\\?\UNC\Server\Docs\root");
        assert!(!foreign_share(Path::new(r"\\server\docs\root\x.pdf"), shared));
        assert!(!foreign_share(Path::new(r"\\?\UNC\SERVER\DOCS\x.pdf"), shared));
        assert!(foreign_share(Path::new(r"\\server\other\x.pdf"), shared));
        assert!(foreign_share(Path::new(r"\\attacker\docs\x.pdf"), shared));
    }

    /// A fresh, empty folder for one staging test.
    fn staging_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-staging-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn staged(dir: &Path, suffix: u64) -> PathBuf {
        dir.join(format!(".out.pdf.{suffix:016x}.pdfcraft-tmp"))
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    /// A symbolic link to a file, where the system allows one (Windows needs Developer Mode or an
    /// administrator for it).
    fn file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(std::io::Error::other(format!("no symbolic links here: {} {}", target.display(), link.display())))
        }
    }

    #[test]
    fn staging_never_writes_through_a_file_planted_at_its_name() {
        let dir = staging_dir("planted");
        // A file elsewhere that a planted link points at.
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, "PRECIOUS").unwrap();
        let target = dir.join("out.pdf");
        // A hard link needs no privileges on any system, and writing to it writes to `outside`.
        std::fs::hard_link(&outside, staged(&dir, 1)).unwrap();
        std::fs::write(staged(&dir, 2), "PLANTED").unwrap();
        write_atomic_with(&target, b"NEW", [1, 2, 3]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert_eq!(read(&outside), "PRECIOUS");
        assert_eq!(read(&staged(&dir, 1)), "PRECIOUS");
        assert_eq!(read(&staged(&dir, 2)), "PLANTED");
        assert!(!staged(&dir, 3).exists(), "the staging file was renamed into place");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn staging_never_writes_through_a_symbolic_link_planted_at_its_name() {
        let dir = staging_dir("symlink");
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, "PRECIOUS").unwrap();
        let target = dir.join("out.pdf");
        if let Err(e) = file_symlink(&outside, &staged(&dir, 1)) {
            eprintln!("symbolic links not checked: {e}");
            return;
        }
        write_atomic_with(&target, b"NEW", [1, 2]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert_eq!(read(&outside), "PRECIOUS");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn staging_never_creates_a_file_through_a_dangling_link_at_its_name() {
        let dir = staging_dir("dangling");
        let unborn = dir.join("created-through-a-link.txt");
        let target = dir.join("out.pdf");
        if let Err(e) = file_symlink(&unborn, &staged(&dir, 1)) {
            eprintln!("symbolic links not checked: {e}");
            return;
        }
        write_atomic_with(&target, b"NEW", [1, 2]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert!(!unborn.exists(), "nothing was created through the dangling link");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn staging_gives_up_rather_than_reuse_a_taken_name() {
        let dir = staging_dir("taken");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        for s in 1..=STAGING_ATTEMPTS as u64 {
            std::fs::write(staged(&dir, s), "PLANTED").unwrap();
        }
        assert!(write_atomic_with(&target, b"NEW", 1..).is_err());
        assert_eq!(read(&target), "OLD");
        for s in 1..=STAGING_ATTEMPTS as u64 {
            assert_eq!(read(&staged(&dir, s)), "PLANTED");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The names in a folder: what a test can see was left behind.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn staging_names_differ_between_saves_and_fit_long_names() {
        let suffixes: std::collections::HashSet<u64> = staging_suffixes().take(64).collect();
        assert_eq!(suffixes.len(), 64);
        assert_ne!(staging_suffixes().next(), staging_suffixes().next(), "each save draws new names");
        // 60 four-byte characters: a 244-byte name, within every system's limit. Its staging name
        // must be too (on Linux the whole name in it would be 275 bytes).
        let dir = staging_dir("long");
        let name = format!("{}.pdf", "\u{1F600}".repeat(60));
        write_atomic(&dir.join(&name), b"NEW").unwrap();
        assert_eq!(read(&dir.join(&name)), "NEW");
        assert_eq!(listing(&dir), [name], "no staging file is left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_at_the_staging_name_is_left_alone() {
        let dir = staging_dir("folder");
        std::fs::create_dir(staged(&dir, 1)).unwrap();
        std::fs::write(staged(&dir, 1).join("inside.txt"), "PLANTED").unwrap();
        let target = dir.join("out.pdf");
        write_atomic_with(&target, b"NEW", [1, 2]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert_eq!(read(&staged(&dir, 1).join("inside.txt")), "PLANTED");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows refuses to replace a read-only file, so the rename fails: the staging file must not
    /// be left behind.
    #[cfg(windows)]
    #[test]
    fn a_failed_rename_removes_the_staging_file() {
        let dir = staging_dir("readonly");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        let mut perms = std::fs::metadata(&target).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&target, perms.clone()).unwrap();
        let e = write_atomic_with(&target, b"NEW", [7]).unwrap_err();
        assert!(e.to_string().contains(&target.display().to_string()), "the rename failed, not the staging: {e}");
        assert_eq!(read(&target), "OLD");
        assert_eq!(listing(&dir), ["out.pdf"], "the staging file was removed");
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&target, perms).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
