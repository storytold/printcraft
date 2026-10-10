//! Comment tools: list, add, reply, set status, edit and delete (execution plan M5, AGENTS.md §3).
//!
//! Geometry follows the automation convention: points from the top-left of the displayed page,
//! y down. It is converted to PDF user space (crop box, `/Rotate`) here.

use pdfcraft_engine::{
    Edit, LineEnding, Markup, NOTE_SIZE, NewAnnotation, NoteIcon, ReviewState, Rgb, Shape, StampGroup, StampKind, Style, SummarySort,
};
use pdfcraft_render::{Annotation, PageInfo};
use serde_json::{Value, json};

use crate::{Args, Automation, Content, DEFAULT_DPI, MAX_DPI, Result, ToolError, encode_png, failed};

/// Author used when a tool call names none.
pub(crate) const DEFAULT_AUTHOR: &str = "PdfCraft";

pub(crate) fn parse_color(s: &str) -> Result<Rgb> {
    let named = match s.to_ascii_lowercase().as_str() {
        "yellow" => Some("#FFEF00"),
        "red" => Some("#E32222"),
        "orange" => Some("#FF8A00"),
        "green" => Some("#2E9E5C"),
        "blue" => Some("#0078D6"),
        "purple" => Some("#8A3FD1"),
        "pink" => Some("#FF5FA2"),
        "black" => Some("#000000"),
        "gray" | "grey" => Some("#808080"),
        "white" => Some("#FFFFFF"),
        _ => None,
    };
    let hex = named.unwrap_or(s).trim_start_matches('#');
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ToolError::InvalidArgs(format!("{s:?} is not a colour (use #RRGGBB or a name)")));
    }
    let c = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map(|v| v as f64 / 255.0).unwrap_or(0.0);
    Ok([c(0), c(2), c(4)])
}

const ENDING_NAMES: &str = "None, Square, Circle, Diamond, OpenArrow, ClosedArrow, Butt, ROpenArrow, RClosedArrow, Slash";

fn line_endings(a: &Args) -> Result<Option<Vec<LineEnding>>> {
    let Some(v) = a.get("endings") else { return Ok(None) };
    let arr = v.as_array().ok_or_else(|| ToolError::InvalidArgs("endings must be an array of line-ending names".into()))?;
    if arr.is_empty() || arr.len() > 2 {
        return Err(ToolError::InvalidArgs("endings takes one name for a callout, or two for a line or polyline".into()));
    }
    arr.iter()
        .map(|item| {
            let name = item.as_str().ok_or_else(|| ToolError::InvalidArgs("endings must be line-ending names".into()))?;
            LineEnding::parse(name).ok_or_else(|| ToolError::InvalidArgs(format!("unknown line ending {name:?} ({ENDING_NAMES})")))
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// `[start, end]` for a line, arrow or polyline. An arrow with no `endings` stays `[None, OpenArrow]`.
fn ending_pair(kind: &str, endings: Option<&[LineEnding]>) -> Result<[LineEnding; 2]> {
    match endings {
        None if kind == "arrow" => Ok([LineEnding::None, LineEnding::OpenArrow]),
        None => Ok([LineEnding::None, LineEnding::None]),
        Some(v) if v.len() == 2 => Ok([v.first().copied().unwrap_or(LineEnding::None), v.get(1).copied().unwrap_or(LineEnding::None)]),
        Some(_) => Err(ToolError::InvalidArgs("a line or polyline needs two endings, the start and the end".into())),
    }
}

fn hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", b(c[0]), b(c[1]), b(c[2]))
}

fn to_user(p: &PageInfo, x: f64, y: f64) -> [f64; 2] {
    let [ux, uy] = p.view_to_user(x as f32, y as f32);
    [ux as f64, uy as f64]
}

fn rect_to_user(p: &PageInfo, r: [f64; 4]) -> [f64; 4] {
    let (a, b) = (to_user(p, r[0], r[1]), to_user(p, r[2], r[3]));
    [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
}

/// The engine's `at` for a note or attachment icon whose top-left corner *as displayed* is the
/// view point (x, y): the top-left (`[x0, y1]`) of the icon's square in user space. Converting
/// the point alone is only right on unrotated pages — under `/Rotate` that corner of the
/// displayed square is another corner of the user-space one, and the icon lands one icon-width
/// away from where it was asked for.
fn icon_anchor(p: &PageInfo, x: f64, y: f64) -> [f64; 2] {
    let r = rect_to_user(p, [x, y, x + NOTE_SIZE, y + NOTE_SIZE]);
    [r[0], r[3]]
}

/// A user-space rectangle as the displayed-page rectangle every tool reports and accepts:
/// origin at the top-left after `/Rotate`, rounded to 1/100 pt.
pub(crate) fn rect_to_view(p: &PageInfo, r: [f32; 4]) -> [f32; 4] {
    let (a, b) = (p.user_to_view(r[0], r[1]), p.user_to_view(r[2], r[3]));
    let round = |v: f32| (v * 100.0).round() / 100.0;
    [round(a[0].min(b[0])), round(a[1].min(b[1])), round(a[0].max(b[0])), round(a[1].max(b[1]))]
}

impl Args<'_> {
    pub(crate) fn nums<const N: usize>(&self, key: &str) -> Result<Option<[f64; N]>> {
        let Some(v) = self.get(key) else { return Ok(None) };
        let wrong = || Self::wrong(key, &format!("an array of {N} numbers"));
        let arr = v.as_array().ok_or_else(wrong)?;
        let nums: Vec<f64> = arr.iter().map(|x| x.as_f64().filter(|f| f.is_finite())).collect::<Option<_>>().ok_or_else(wrong)?;
        <[f64; N]>::try_from(nums).map(Some).map_err(|_| wrong())
    }

    fn need<const N: usize>(&self, key: &str, why: &str) -> Result<[f64; N]> {
        self.nums(key)?.ok_or_else(|| ToolError::InvalidArgs(format!("{why} needs `{key}`")))
    }

    fn color(&self, key: &str) -> Result<Option<Rgb>> {
        self.opt_str(key)?.map(parse_color).transpose()
    }
}

impl Automation {
    /// One of the read-only layers the GUI uses to drag/resize an embedded image signature.
    pub(crate) fn comment_image_preview(&self, a: &Args) -> Result<Vec<Content>> {
        let (page, index) = self.comment_target(a)?;
        let dpi = a.opt_num("dpi")?.unwrap_or(DEFAULT_DPI);
        if !(1.0..=MAX_DPI).contains(&dpi) {
            return Err(ToolError::InvalidArgs(format!("dpi must be between 1 and {MAX_DPI}")));
        }
        let doc = self.doc(a)?;
        let preview = doc.image_signature_preview(page, index).map_err(failed)?.ok_or_else(|| failed("choose an image signature or initials"))?;
        let annotation = doc.info.annotations.iter().find(|c| c.page == page && c.index == index).ok_or_else(|| failed("no such comment"))?;
        let info = doc.info.pages.get(page).ok_or_else(|| failed("no such page"))?;
        let [w, h] = preview.image.size();
        let layer = a.opt_str("layer")?.unwrap_or("background");
        let image = match layer {
            "image" => Content::Png { data: preview.image.bytes().as_ref().clone(), width: w as u32, height: h as u32 },
            "background" => {
                let out = preview.render_background((dpi / 72.0) as f32).map_err(failed)?;
                Content::Png { data: encode_png(out.width, out.height, &out.rgba)?, width: out.width, height: out.height }
            }
            _ => return Err(ToolError::InvalidArgs("layer must be background or image".into())),
        };
        Ok(vec![
            Content::Json(json!({ "page": page + 1, "index": index + 1, "rect": rect_to_view(info, annotation.rect), "rotation": info.rotation,
                "image_rotation": (i64::from(info.rotation) - preview.turn).rem_euclid(360),
                "layer": layer, "opacity": preview.opacity, "dpi": dpi })),
            image,
        ])
    }

    /// Comments of a document, optionally only of one 0-based page.
    fn comments(&self, a: &Args) -> Result<Vec<Annotation>> {
        let doc = self.doc(a)?;
        Ok(doc.info.annotations.iter().filter(|x| x.subtype != "Popup").cloned().collect())
    }

    /// The (0-based page, index) a comment tool call refers to: `id`, or `page` + `index`.
    fn comment_target(&self, a: &Args) -> Result<(usize, usize)> {
        let all = self.comments(a)?;
        if let Some(id) = a.opt_str("id")? {
            let c =
                all.iter().find(|c| c.name.as_deref() == Some(id)).ok_or_else(|| failed(format!("no comment with id {id:?} (see comment_list)")))?;
            return Ok((c.page, c.index));
        }
        let (Some(page), Some(index)) = (a.opt_int("page")?, a.opt_int("index")?) else {
            return Err(ToolError::InvalidArgs("pass the comment's id, or page and index".into()));
        };
        let (page, index) = ((page.max(1) - 1) as usize, (index.max(1) - 1) as usize);
        if !all.iter().any(|c| c.page == page && c.index == index) {
            return Err(failed(format!("there is no comment {} on page {} (see comment_list)", index + 1, page + 1)));
        }
        Ok((page, index))
    }

    pub(crate) fn comment_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let only = a.opt_int("page")?.map(|p| (p.max(1) - 1) as usize);
        let all = self.comments(a)?;
        let view = |c: &Annotation| -> Value {
            let p = &doc.info.pages[c.page.min(doc.info.pages.len().saturating_sub(1))];
            json!({
                "id": c.name,
                "page": c.page + 1,
                "index": c.index + 1,
                "type": c.subtype,
                "author": c.author,
                "contents": c.contents,
                "modified": c.modified,
                "rect": rect_to_view(p, c.rect),
                "color": c.color.map(hex),
            })
        };
        let roots: Vec<Value> = all
            .iter()
            .filter(|c| c.in_reply_to.is_none() && only.is_none_or(|p| c.page == p))
            .map(|c| {
                let mut v = view(c);
                // Every reply under this comment, at any depth, in document order. A reply to a
                // reply keeps its `/IRT` on the direct parent, so following one level would drop it.
                let descends_from = |r: &Annotation, root: &str| -> bool {
                    let mut parent = r.in_reply_to.as_deref();
                    for _ in 0..64 {
                        let Some(nm) = parent else { return false };
                        if nm == root {
                            return true;
                        }
                        parent = all.iter().find(|x| x.name.as_deref() == Some(nm)).and_then(|x| x.in_reply_to.as_deref());
                    }
                    false
                };
                let thread: Vec<&Annotation> = match c.name.as_deref() {
                    Some(nm) => all.iter().filter(|r| r.in_reply_to.is_some() && descends_from(r, nm)).collect(),
                    None => Vec::new(),
                };
                let status = thread.iter().rev().filter(|r| !r.is_mark()).find_map(|r| r.state.clone());
                v["status"] = json!(status);
                v["marked"] = json!(thread.iter().rev().find(|r| r.is_mark()).is_some_and(|r| r.state.as_deref() == Some("Marked")));
                v["locked"] = json!(c.locked);
                v["replies"] = thread.iter().filter(|r| r.state.is_none()).map(|r| view(r)).collect();
                v
            })
            .collect();
        Ok(json!({ "count": roots.len(), "comments": roots }))
    }

    pub(crate) fn comment_add(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let info = self.doc(a)?.info.pages[page].clone();
        let kind = a.str("type")?;
        let endings = line_endings(a)?;
        if endings.is_some() && !matches!(kind, "line" | "arrow" | "polyline" | "callout") {
            return Err(ToolError::InvalidArgs("endings apply to a line, arrow, polyline or callout".into()));
        }
        let markup = match kind {
            "highlight" => Some(Markup::Highlight),
            "underline" => Some(Markup::Underline),
            "strikeout" | "replace" => Some(Markup::StrikeOut),
            "squiggly" => Some(Markup::Squiggly),
            _ => None,
        };
        let shape = if let Some(markup) = markup {
            let quads = match (a.opt_str("find")?, a.get("quads")) {
                (Some(needle), _) => {
                    let id = self.doc(a)?.id;
                    let text = self.page_texts(id, &[page])?.remove(0);
                    let mut hits = text.find(needle);
                    if hits.is_empty() {
                        return Err(failed(format!("{needle:?} was not found on page {}", page + 1)));
                    }
                    if !a.opt_bool("all")?.unwrap_or(false) {
                        hits.truncate(1);
                    }
                    hits.into_iter().flat_map(|r| text.line_rects(r)).map(|r| info.view_rect_to_quad(r)).collect()
                }
                (None, Some(q)) => {
                    let wrong = || ToolError::InvalidArgs("quads must be arrays of 8 numbers".into());
                    let quads = q.as_array().ok_or_else(wrong)?;
                    quads
                        .iter()
                        .map(|q| {
                            let v: Vec<f64> = q.as_array().ok_or_else(wrong)?.iter().map(|x| x.as_f64()).collect::<Option<_>>().ok_or_else(wrong)?;
                            let v: [f64; 8] = v.try_into().map_err(|_| wrong())?;
                            let mut out = [0.0; 8];
                            for i in 0..4 {
                                let [x, y] = to_user(&info, v[2 * i], v[2 * i + 1]);
                                (out[2 * i], out[2 * i + 1]) = (x, y);
                            }
                            Ok(out)
                        })
                        .collect::<Result<Vec<_>>>()?
                }
                (None, None) => return Err(ToolError::InvalidArgs(format!("{kind} needs `find` (text to mark) or `quads`"))),
            };
            Shape::TextMarkup { kind: markup, quads }
        } else {
            match kind {
                "note" => {
                    let [x, y] = a.need::<2>("at", "a note")?;
                    let icon = match a.opt_str("icon")? {
                        Some(n) => NoteIcon::from_name(n).ok_or_else(|| ToolError::InvalidArgs(format!("unknown icon {n:?}")))?,
                        None => NoteIcon::Comment,
                    };
                    Shape::Note { at: icon_anchor(&info, x, y), icon }
                }
                "stamp" => {
                    let want = a.opt_str("stamp")?.unwrap_or("approved").to_ascii_lowercase().replace([' ', '-', '_'], "");
                    let dynamic = a.opt_bool("dynamic")?.unwrap_or(false);
                    let stamp = StampKind::ALL
                        .into_iter()
                        .find(|k| k.label().to_ascii_lowercase().replace(' ', "") == want && ((k.group() == StampGroup::Dynamic) == dynamic))
                        .ok_or_else(|| ToolError::InvalidArgs(format!("unknown stamp {want:?} (see the tool description)")))?;
                    let (w, h) = stamp.size();
                    let [x, y] = a.need::<2>("at", "a stamp (its centre)")?;
                    let rect = rect_to_user(&info, [x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0]);
                    let author = a.opt_str("author")?.unwrap_or(DEFAULT_AUTHOR).to_string();
                    let by = dynamic.then(|| self.session.stamp_by_line(&author));
                    Shape::Stamp { rect, stamp, by }
                }
                "rectangle" => Shape::Rectangle { rect: rect_to_user(&info, a.need::<4>("rect", "a rectangle")?) },
                "oval" => Shape::Oval { rect: rect_to_user(&info, a.need::<4>("rect", "an oval")?) },
                "textbox" => {
                    let font_size = a.opt_num("font_size")?.unwrap_or(12.0);
                    Shape::TextBox { rect: rect_to_user(&info, a.need::<4>("rect", "a text box")?), font_size }
                }
                "line" | "arrow" => {
                    let (f, t) = (a.need::<2>("from", "a line")?, a.need::<2>("to", "a line")?);
                    let [start, end] = ending_pair(kind, endings.as_deref())?;
                    Shape::Line { from: to_user(&info, f[0], f[1]), to: to_user(&info, t[0], t[1]), start, end }
                }
                "ink" => {
                    let wrong = || ToolError::InvalidArgs("strokes must be an array of arrays of [x, y] points".into());
                    let strokes = a.get("strokes").ok_or_else(|| ToolError::InvalidArgs("ink needs `strokes`".into()))?;
                    let strokes = strokes
                        .as_array()
                        .ok_or_else(wrong)?
                        .iter()
                        .map(|s| {
                            s.as_array()
                                .ok_or_else(wrong)?
                                .iter()
                                .map(|p| match p.as_array().map(|p| p.iter().map(|v| v.as_f64()).collect::<Vec<_>>()).as_deref() {
                                    Some([Some(x), Some(y)]) => Ok(to_user(&info, *x, *y)),
                                    _ => Err(wrong()),
                                })
                                .collect::<Result<Vec<_>>>()
                        })
                        .collect::<Result<Vec<_>>>()?;
                    Shape::Ink { strokes }
                }
                "polygon" | "cloud" | "polyline" => {
                    let wrong = || ToolError::InvalidArgs(format!("{kind} needs `points`: an array of [x, y] points"));
                    let pts = a.get("points").and_then(|p| p.as_array()).ok_or_else(wrong)?;
                    let vertices = pts
                        .iter()
                        .map(|p| match p.as_array().map(|p| p.iter().map(|v| v.as_f64()).collect::<Vec<_>>()).as_deref() {
                            Some([Some(x), Some(y)]) => Ok(to_user(&info, *x, *y)),
                            _ => Err(wrong()),
                        })
                        .collect::<Result<Vec<_>>>()?;
                    match kind {
                        "polyline" => {
                            let [start, end] = ending_pair(kind, endings.as_deref())?;
                            Shape::PolyLine { vertices, start, end }
                        }
                        _ => Shape::Polygon { vertices, cloud: kind == "cloud" },
                    }
                }
                "callout" => {
                    let rect = rect_to_user(&info, a.need::<4>("rect", "a callout (its text box)")?);
                    let t = a.need::<2>("to", "a callout (the point it points at)")?;
                    let point = to_user(&info, t[0], t[1]);
                    let knee = match a.nums::<2>("knee")? {
                        Some(k) => to_user(&info, k[0], k[1]),
                        // Halfway between the point and the box, level with the box's middle.
                        None => {
                            let mid = (rect[1] + rect[3]) / 2.0;
                            let side = if point[0] < rect[0] {
                                rect[0]
                            } else if point[0] > rect[2] {
                                rect[2]
                            } else {
                                (rect[0] + rect[2]) / 2.0
                            };
                            [(point[0] + side) / 2.0, mid]
                        }
                    };
                    Shape::Callout {
                        rect,
                        knee,
                        point,
                        font_size: a.opt_num("font_size")?.unwrap_or(10.0),
                        ending: match endings.as_deref() {
                            None => LineEnding::OpenArrow,
                            Some([ending]) => *ending,
                            Some(_) => return Err(ToolError::InvalidArgs("a callout needs one ending".into())),
                        },
                    }
                }
                "attachment" => {
                    let [x, y] = a.need::<2>("at", "an attachment (its icon's top-left)")?;
                    let path = self.resolve(a.str("path")?, false)?;
                    let data = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
                    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let icon = match a.opt_str("icon")? {
                        Some(n) => pdfcraft_engine::AttachIcon::from_name(n)
                            .ok_or_else(|| ToolError::InvalidArgs(format!("unknown icon {n:?} (PushPin, Paperclip, Graph, Tag)")))?,
                        None => pdfcraft_engine::AttachIcon::PushPin,
                    };
                    Shape::Attachment { at: icon_anchor(&info, x, y), icon, file, data }
                }
                "caret" => {
                    let [x, y] = a.need::<2>("at", "a caret (the insertion point on the baseline)")?;
                    Shape::Caret { rect: rect_to_user(&info, [x - 4.0, y, x + 4.0, y + 8.0]) }
                }
                other => return Err(ToolError::InvalidArgs(format!("unknown comment type {other:?}"))),
            }
        };
        let mut style = Style::default_for(&shape);
        if let Some(c) = a.color("color")? {
            style.color = c;
        }
        style.fill = a.color("fill")?;
        if let Some(o) = a.opt_num("opacity")? {
            style.opacity = o.clamp(0.0, 1.0);
        }
        if let Some(w) = a.opt_num("width")? {
            style.width = w.max(0.0);
        }
        let contents = a.opt_str("contents")?.unwrap_or_default().to_string();
        let author = a.opt_str("author")?.unwrap_or(DEFAULT_AUTHOR).to_string();
        // Replace Text: the struck-out text and a grouped caret holding `contents`.
        if kind == "replace" {
            let Shape::TextMarkup { quads, .. } = shape else {
                return Err(ToolError::InvalidArgs("replace marks text: give it text to replace".into()));
            };
            if contents.trim().is_empty() {
                return Err(ToolError::InvalidArgs("replace needs `contents`: the replacement text".into()));
            }
            let strike = style;
            let caret = Style::default_for(&Shape::Caret { rect: [0.0; 4] });
            let mut out = self.apply(a, Edit::ReplaceText { page, quads, text: contents, author, strike, caret })?;
            out["comment"] = json!({ "page": page + 1, "type": "replace" });
            return Ok(out);
        }
        let marked = match &shape {
            Shape::TextMarkup { quads, .. } => Some(quads.len()),
            _ => None,
        };
        let before: Vec<Option<String>> = self.doc(a)?.info.annotations.iter().map(|c| c.name.clone()).collect();
        let mut out = self.apply(a, Edit::AddAnnotation(NewAnnotation { page, shape, style, contents, author }))?;
        let added = self.doc(a)?.info.annotations.iter().find(|c| c.page == page && c.subtype != "Popup" && !before.contains(&c.name));
        out["comment"] = json!({ "id": added.and_then(|c| c.name.clone()), "page": page + 1, "index": added.map(|c| c.index + 1) });
        if let Some(n) = marked {
            out["comment"]["lines"] = json!(n);
        }
        Ok(out)
    }

    /// Preferences ▸ Date format: set it with `format` and its names' `language` (`auto`
    /// follows the app's interface language, so English here), or read them.
    pub(crate) fn fill_sign_date_format(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::dates;
        let format = a.opt_str("format")?;
        let language = a.opt_str("language")?.map(|l| Some(l).filter(|l| !l.trim().eq_ignore_ascii_case("auto")));
        // Check the format before changing anything, so a bad one leaves both settings alone.
        if let Some(f) = format {
            dates::check_date_format(f).map_err(ToolError::InvalidArgs)?;
        }
        if let Some(l) = language {
            self.session.set_date_language(l).map_err(ToolError::InvalidArgs)?;
        }
        if let Some(f) = format {
            self.session.set_date_format(f).map_err(ToolError::InvalidArgs)?;
        }
        let today = self.session.today_text(None, None).map_err(ToolError::Failed)?;
        let languages: Vec<Value> = dates::DATE_LANGUAGES.iter().map(|l| json!({ "code": l.code, "name": l.name })).collect();
        Ok(json!({
            "format": self.session.date_format(),
            "language": self.session.date_language().unwrap_or("auto"),
            // Characters Fill & Sign can't write into a PDF yet; dates with them are refused.
            "unwritable": dates::unwritable(&today),
            "today": today,
            "presets": dates::DATE_FORMATS,
            "languages": languages,
        }))
    }

    pub(crate) fn fill_sign_add(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::FillMark;
        let page = self.page(a)?;
        let info = self.doc(a)?.info.pages[page].clone();
        let [x, y] = a.need::<2>("at", "Fill & Sign")?;
        let at = to_user(&info, x, y);
        let author = a.opt_str("author")?.unwrap_or(DEFAULT_AUTHOR).to_string();
        let kind = a.str("type")?;
        if let Some(path) = a.opt_str("path")? {
            if !matches!(kind, "signature" | "initials") || a.opt_str("text")?.is_some() {
                return Err(ToolError::InvalidArgs("path is only for an image signature or initials; pass either path or text".into()));
            }
            let path = self.resolve(path, false)?;
            let file = std::fs::File::open(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
            let image = pdfcraft_engine::SignatureImage::read(file).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;
            let edit = image.edit(page, &info, at, kind == "initials", &author).ok_or_else(|| ToolError::InvalidArgs("at must be finite".into()))?;
            return self.apply(a, edit);
        }
        let size = 10.0;
        let text_at = |t: &str| {
            let w = (pdfcraft_engine::annot_text::text_width(t, size) + 8.0).clamp(20.0, 600.0);
            let h = size * 1.2 + 6.0;
            Shape::Typewriter { rect: [at[0], at[1] - h, at[0] + w, at[1]], font_size: size }
        };
        let (shape, contents) = match kind {
            "text" => {
                let t = a.str("text")?.to_string();
                (text_at(&t), t)
            }
            "date" => {
                let lang = a.opt_str("language")?;
                if let Some(l) = lang.filter(|l| pdfcraft_engine::dates::date_language(l).is_none()) {
                    return Err(ToolError::InvalidArgs(format!("unknown date language {l:?} (see fill_sign_date_format)")));
                }
                let t = self.session.today_text_for_pdf(a.opt_str("format")?, lang).map_err(ToolError::InvalidArgs)?;
                (text_at(&t), t)
            }
            // A typed signature or initials in the script font, left edge at `at`, upright as displayed.
            kind @ ("signature" | "initials") => {
                let t = a.str("text")?;
                let h = if kind == "initials" { 24.0 } else { 32.0 };
                let shape = pdfcraft_engine::typed_signature_shape(at, t, h, i64::from(info.rotation))
                    .ok_or_else(|| ToolError::InvalidArgs("text has nothing to draw".into()))?;
                (shape, String::new())
            }
            kind => {
                let mark = match kind {
                    "check" => FillMark::Check,
                    "cross" => FillMark::Cross,
                    "dot" => FillMark::Dot,
                    "line" => FillMark::Line,
                    other => return Err(ToolError::InvalidArgs(format!("unknown type {other:?}"))),
                };
                let (w, h) = if mark == FillMark::Line { (36.0, 4.0) } else { (12.0, 12.0) };
                (Shape::Mark { rect: [at[0] - w / 2.0, at[1] - h / 2.0, at[0] + w / 2.0, at[1] + h / 2.0], mark }, String::new())
            }
        };
        let style = Style::default_for(&shape);
        self.apply(a, Edit::AddAnnotation(NewAnnotation { page, shape, style, contents, author }))
    }

    pub(crate) fn comment_reply(&mut self, a: &Args) -> Result<Value> {
        let (page, index) = self.comment_target(a)?;
        let author = a.opt_str("author")?.unwrap_or(DEFAULT_AUTHOR).to_string();
        self.apply(a, Edit::ReplyToAnnotation { page, index, text: a.str("text")?.to_string(), author })
    }

    pub(crate) fn comment_set_status(&mut self, a: &Args) -> Result<Value> {
        let (page, index) = self.comment_target(a)?;
        let status = a.str("status")?;
        let state = ReviewState::from_name(status).ok_or_else(|| ToolError::InvalidArgs(format!("unknown status {status:?}")))?;
        let author = a.opt_str("author")?.unwrap_or(DEFAULT_AUTHOR).to_string();
        self.apply(a, Edit::SetAnnotationStatus { page, index, state, author })
    }

    pub(crate) fn comment_mark(&mut self, a: &Args) -> Result<Value> {
        let (page, index) = self.comment_target(a)?;
        let marked = a.opt_bool("marked")?.unwrap_or(true);
        let author = a.opt_str("author")?.unwrap_or(DEFAULT_AUTHOR).to_string();
        self.apply(a, Edit::MarkAnnotation { page, index, marked, author })
    }

    pub(crate) fn comment_lock(&mut self, a: &Args) -> Result<Value> {
        let (page, index) = self.comment_target(a)?;
        let locked = a.opt_bool("locked")?.unwrap_or(true);
        self.apply(a, Edit::LockAnnotation { page, index, locked })
    }

    pub(crate) fn comments_hide(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let hidden = a.opt_bool("hidden")?.unwrap_or(true);
        self.session.set_hide_comments(id, hidden);
        Ok(json!({ "hidden": hidden }))
    }

    pub(crate) fn comments_summarize(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let (id, stem) = (doc.id, std::path::Path::new(&doc.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        let sort = match a.opt_str("sort")? {
            Some(s) => SummarySort::parse(s).ok_or_else(|| ToolError::InvalidArgs(format!("unknown sort {s:?}")))?,
            None => SummarySort::Page,
        };
        let bytes = self.session.summarize_comments(id, sort).map_err(failed)?;
        self.deliver(a, &format!("Summary of comments on {stem}.pdf"), bytes)
    }

    pub(crate) fn comment_edit(&mut self, a: &Args) -> Result<Value> {
        let (page, index) = self.comment_target(a)?;
        let info = self.doc(a)?.info.pages[page].clone();
        let mut edits = Vec::new();
        if let Some(text) = a.opt_str("contents")? {
            edits.push(Edit::SetAnnotationContents { page, index, text: text.to_string() });
        }
        let (color, opacity, width) = (a.color("color")?, a.opt_num("opacity")?, a.opt_num("width")?);
        if color.is_some() || opacity.is_some() || width.is_some() {
            edits.push(Edit::StyleAnnotation { page, index, color, opacity, width, endings: None });
        }
        if let Some(f) = a.opt_str("fill")? {
            let fill = if f.eq_ignore_ascii_case("none") { None } else { Some(parse_color(f)?) };
            edits.push(Edit::FillAnnotation { page, index, fill });
        }
        if let Some(r) = a.nums::<4>("rect")? {
            edits.push(Edit::ResizeAnnotation { page, index, rect: rect_to_user(&info, r) });
        }
        if let Some([dx, dy]) = a.nums::<2>("move")? {
            let (o, d) = (to_user(&info, 0.0, 0.0), to_user(&info, dx, dy));
            edits.push(Edit::MoveAnnotation { page, index, dx: d[0] - o[0], dy: d[1] - o[1] });
        }
        if edits.is_empty() {
            return Err(ToolError::InvalidArgs("nothing to change: pass contents, color, opacity, width, rect or move".into()));
        }
        let edit = if edits.len() == 1 { edits.remove(0) } else { Edit::Batch { label: "Edit comment".into(), edits } };
        self.apply(a, edit)
    }

    pub(crate) fn comment_delete(&mut self, a: &Args) -> Result<Value> {
        let (page, index) = self.comment_target(a)?;
        self.apply(a, Edit::DeleteAnnotation { page, index })
    }
}

impl Automation {
    /// A custom stamp: a picture file (PDF page or image) centred at `at`, at its natural size
    /// (at most 200 pt) or `width` points wide.
    pub(crate) fn stamp_custom(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let info = self.doc(a)?.info.pages[page].clone();
        let path = self.resolve(a.str("path")?, false)?;
        let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let name = a.opt_str("name")?.map(str::to_owned).unwrap_or_else(|| file.rsplit_once('.').map_or(file.clone(), |(s, _)| s.to_owned()));
        let [x, y] = a.need::<2>("at", "a stamp (its centre)")?;
        let [ux, uy] = to_user(&info, x, y);
        let edit = Edit::AddCustomStamp {
            page,
            rect: [ux, uy, ux, uy],
            name,
            file: pdfcraft_engine::MarkFile {
                name: file,
                bytes: std::sync::Arc::new(bytes),
                page: a.opt_int("file_page")?.unwrap_or(1).max(1) as usize - 1,
            },
            author: a.opt_str("author")?.unwrap_or(DEFAULT_AUTHOR).to_string(),
        };
        self.apply(a, edit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse_by_hex_and_name() {
        assert_eq!(parse_color("#FF0000").unwrap(), [1.0, 0.0, 0.0]);
        assert_eq!(parse_color("black").unwrap(), [0.0, 0.0, 0.0]);
        assert!(parse_color("#12").is_err() && parse_color("chartreuse").is_err());
        assert_eq!(hex([1.0, 0.5, 0.0]), "#FF8000");
    }
}
