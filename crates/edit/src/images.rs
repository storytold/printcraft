//! Edit a PDF ▸ edit existing images and grouped artwork (Image/Form XObjects painted by `Do`
//! in its own content streams), and changing one: move/resize/rotate/flip (the `Do` is wrapped in
//! `q … cm … Q` with the extra transform), replace (it draws another XObject in the same place),
//! or delete. Only the stream that draws it is rewritten, as a new object.

use pdfcraft_content::{Matrix, Op, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::EditError;

/// An image drawn on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct PageImage {
    /// Its box in user space.
    pub rect: [f64; 4],
    /// The placement: image unit square or Form coordinates → user space.
    pub matrix: [f64; 6],
    /// The XObject resource name and object.
    pub name: String,
    pub object: Option<ObjRef>,
    /// Whether this is grouped Form artwork, edited as a whole (including its nested content).
    pub is_form: bool,
    /// Pixel size; zero for vector/grouped Form artwork.
    pub width: u32,
    pub height: u32,
    stream: usize,
    op: usize,
    /// CTM before `Do`, excluding a Form's own Matrix.
    draw_matrix: [f64; 6],
}

fn streams(doc: &Document, page: &Dict) -> Vec<(Object, Vec<u8>)> {
    let list: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    list.into_iter()
        .map(|o| {
            let data = match &*doc.resolve(&o) {
                Object::Stream(s) => s.decoded().unwrap_or_default(),
                _ => Vec::new(),
            };
            (o, data)
        })
        .collect()
}

fn page_of(doc: &Document, page: usize) -> Result<pdfcraft_model::Page, EditError> {
    pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))
}

fn xobjects(doc: &Document, page: &Dict) -> Dict {
    page.get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|d| d.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())))
        .unwrap_or_default()
}

/// The images and Form artwork page `page` (0-based) draws, in drawing order.
/// Forms stay intact: no traversal or rewriting of their internal objects is needed.
pub fn page_images(doc: &Document, page: usize) -> Result<Vec<PageImage>, EditError> {
    let p = page_of(doc, page)?;
    let xo = xobjects(doc, &p.dict);
    let mut out = Vec::new();
    // The graphics state carries over from one stream to the next.
    let mut ctm = Matrix::IDENTITY;
    let mut stack = Vec::new();
    for (si, (_, data)) in streams(doc, &p.dict).into_iter().enumerate() {
        for (i, op) in parse(&data).ops.iter().enumerate() {
            match op.op.as_slice() {
                b"q" => stack.push(ctm),
                b"Q" => ctm = stack.pop().unwrap_or(ctm),
                b"cm" => {
                    if let Some(m) = op.nums::<6>() {
                        ctm = Matrix(m).then(&ctm);
                    }
                }
                b"Do" => {
                    let Some(name) = op.name(0) else { continue };
                    let Some(resource) = xo.get(name) else { continue };
                    let obj = doc.resolve(resource);
                    let Object::Stream(s) = &*obj else { continue };
                    let is_form = s.dict.name(b"Subtype") == Some(b"Form");
                    let (matrix, bounds) = if is_form {
                        let Some(bounds) = s.dict.get(b"BBox").map(|v| doc.resolve(v)).and_then(|v| {
                            v.as_array().and_then(|a| {
                                if a.len() != 4 {
                                    return None;
                                }
                                let mut b = [0.0; 4];
                                for (n, v) in b.iter_mut().zip(a) {
                                    *n = v.as_f64()?;
                                }
                                Some(b)
                            })
                        }) else {
                            continue;
                        };
                        let matrix = match s.dict.get(b"Matrix") {
                            Some(v) => {
                                let v = doc.resolve(v);
                                let Some(m) = v.as_array().and_then(|a| Matrix::from_operands(a)) else { continue };
                                m
                            }
                            None => Matrix::IDENTITY,
                        };
                        (matrix.then(&ctm), bounds)
                    } else if s.dict.name(b"Subtype") == Some(b"Image") {
                        (ctm, [0.0, 0.0, 1.0, 1.0])
                    } else {
                        continue;
                    };
                    let rect = matrix.bbox(bounds);
                    if !bounds.iter().chain(matrix.0.iter()).chain(rect.iter()).all(|v| v.is_finite())
                        || bounds[2] <= bounds[0]
                        || bounds[3] <= bounds[1]
                        || rect[2] <= rect[0]
                        || rect[3] <= rect[1]
                    {
                        continue;
                    }
                    out.push(PageImage {
                        rect,
                        matrix: matrix.0,
                        name: String::from_utf8_lossy(name).into_owned(),
                        object: resource.as_ref(),
                        is_form,
                        width: if is_form { 0 } else { s.dict.int(b"Width").unwrap_or(0).clamp(0, u32::MAX as i64) as u32 },
                        height: if is_form { 0 } else { s.dict.int(b"Height").unwrap_or(0).clamp(0, u32::MAX as i64) as u32 },
                        stream: si,
                        op: i,
                        draw_matrix: ctm.0,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

/// The images page `page` (0-based) shows, in drawing order, including those drawn inside form
/// XObjects (Export to Word, HTML and RTF). Read-only: indexes into this list are not
/// [`page_images`] indexes, so they can't be passed to [`change_image`].
pub fn reading_images(doc: &Document, page: usize) -> Result<Vec<PageImage>, EditError> {
    /// The CTM and `q` stack carry from one page stream to the next (§7.8.2); a form starts
    /// its own.
    /// The forms being read (cycle guard) and how many were entered (the page's budget).
    struct Forms {
        path: Vec<ObjRef>,
        visits: usize,
    }
    struct State {
        ctm: Matrix,
        stack: Vec<Matrix>,
    }
    fn walk(doc: &Document, data: &[u8], resources: &Dict, st: &mut State, stream: usize, forms: &mut Forms, out: &mut Vec<PageImage>) {
        let xo = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
        let (ctm, stack) = (&mut st.ctm, &mut st.stack);
        for (i, op) in parse(data).ops.iter().enumerate() {
            match op.op.as_slice() {
                b"q" => stack.push(*ctm),
                b"Q" => *ctm = stack.pop().unwrap_or(*ctm),
                b"cm" => {
                    if let Some(m) = op.nums::<6>() {
                        *ctm = Matrix(m).then(ctm);
                    }
                }
                b"Do" => {
                    let Some(name) = op.name(0) else { continue };
                    if let Some(form) = crate::text::form_call(doc, resources, name, *ctm, &forms.path, &mut forms.visits) {
                        if let Some(r) = form.obj {
                            forms.path.push(r);
                        }
                        let mut inner = State { ctm: form.ctm, stack: Vec::new() };
                        walk(doc, &form.data, &form.resources, &mut inner, stream, forms, out);
                        if form.obj.is_some() {
                            forms.path.pop();
                        }
                        continue;
                    }
                    let Some(r) = xo.get(name).and_then(Object::as_ref) else { continue };
                    let obj = doc.get(r);
                    let Object::Stream(s) = &*obj else { continue };
                    if s.dict.name(b"Subtype") != Some(b"Image") {
                        continue;
                    }
                    out.push(PageImage {
                        rect: ctm.bbox([0.0, 0.0, 1.0, 1.0]),
                        matrix: ctm.0,
                        name: String::from_utf8_lossy(name).into_owned(),
                        object: Some(r),
                        is_form: false,
                        width: s.dict.int(b"Width").unwrap_or(0).max(0) as u32,
                        height: s.dict.int(b"Height").unwrap_or(0).max(0) as u32,
                        stream,
                        op: i,
                        draw_matrix: ctm.0,
                    });
                }
                _ => {}
            }
        }
    }
    let p = page_of(doc, page)?;
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut out = Vec::new();
    let mut st = State { ctm: Matrix::IDENTITY, stack: Vec::new() };
    let mut forms = Forms { path: Vec::new(), visits: 0 };
    for (si, (_, data)) in streams(doc, &p.dict).into_iter().enumerate() {
        walk(doc, &data, &res, &mut st, si, &mut forms, &mut out);
    }
    Ok(out)
}

/// What to do with an image.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageChange {
    /// Apply a user-space transform after its placement (move, resize, rotate, flip).
    Transform([f64; 6]),
    /// Draw this image XObject instead, in the same place.
    Replace(ObjRef),
    Delete,
}

/// The user-space transform that maps box `from` onto box `to` (move and resize).
pub fn rect_to_rect(from: [f64; 4], to: [f64; 4]) -> [f64; 6] {
    let (fw, fh) = ((from[2] - from[0]).max(1e-6), (from[3] - from[1]).max(1e-6));
    let (sx, sy) = ((to[2] - to[0]) / fw, (to[3] - to[1]) / fh);
    [sx, 0.0, 0.0, sy, to[0] - from[0] * sx, to[1] - from[1] * sy]
}

/// Turn the image a quarter turn clockwise (`quarters` = 1, 2, 3) about its centre, or flip it.
pub fn turn_about_centre(rect: [f64; 4], quarters: i32, flip_h: bool, flip_v: bool) -> [f64; 6] {
    let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    let (a, b, c, d) = match quarters.rem_euclid(4) {
        1 => (0.0, -1.0, 1.0, 0.0),
        2 => (-1.0, 0.0, 0.0, -1.0),
        3 => (0.0, 1.0, -1.0, 0.0),
        _ => (1.0, 0.0, 0.0, 1.0),
    };
    let (fx, fy) = (if flip_h { -1.0 } else { 1.0 }, if flip_v { -1.0 } else { 1.0 });
    let (a, b, c, d) = (a * fx, b * fy, c * fx, d * fy);
    // Translate the centre to the origin, turn, translate back.
    [a, b, c, d, cx - a * cx - c * cy, cy - b * cx - d * cy]
}

/// Change image `index` (from [`page_images`]) on `page`.
pub fn change_image(doc: &mut Document, page: usize, index: usize, change: &ImageChange) -> Result<(), EditError> {
    let images = page_images(doc, page)?;
    let img = images.get(index).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no image {}", page + 1, index + 1)))?;
    let p = page_of(doc, page)?;
    let all = streams(doc, &p.dict);
    let (obj, data) = all.get(img.stream).cloned().ok_or_else(|| EditError::Invalid("the image stream is missing".into()))?;
    let mut ops = parse(&data).ops;
    let original = ops.get(img.op).cloned().ok_or_else(|| EditError::Invalid("the image operation is missing".into()))?;
    let mut resources = None;
    let replacement: Vec<Op> = match change {
        ImageChange::Delete => Vec::new(),
        ImageChange::Transform(t) => {
            if !t.iter().all(|v| v.is_finite()) {
                return Err(EditError::Invalid("the image transform must be finite".into()));
            }
            // New CTM = placement × T; as a `cm` before the Do: M = P · T · P⁻¹.
            let pm = Matrix(img.draw_matrix);
            let inv = pm.invert().ok_or_else(|| EditError::Invalid("the image has no area".into()))?;
            let m = pm.then(&Matrix(*t)).then(&inv);
            if !m.0.iter().all(|v| v.is_finite()) {
                return Err(EditError::Invalid("the image transform is too large".into()));
            }
            vec![Op::new("q", vec![]), Op::new("cm", m.0.iter().map(|v| pdfcraft_content::num(*v)).collect()), original, Op::new("Q", vec![])]
        }
        ImageChange::Replace(r) => {
            if img.is_form {
                return Err(EditError::Invalid(
                    "grouped Form artwork can be moved, resized, rotated, flipped or deleted; replacing it with a bitmap is not supported".into(),
                ));
            }
            let name = format!("PCImg{}", r.num);
            let mut res = p.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
            let mut xo = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
            xo.set(name.clone().into_bytes(), Object::Ref(*r));
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            resources = Some(res);
            vec![Op::new("Do", vec![Object::name(&name)])]
        }
    };
    ops.splice(img.op..=img.op, replacement);
    let mut dict = match &*doc.resolve(&obj) {
        Object::Stream(s) => s.dict.clone(),
        _ => Dict::new(),
    };
    dict.remove(b"Length");
    let new = doc.add(Object::Stream(Stream::flate(dict, &serialize_ops(&ops))));
    let contents: Vec<Object> = all.iter().enumerate().map(|(i, (o, _))| if i == img.stream { Object::Ref(new) } else { o.clone() }).collect();
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), if let [one] = contents.as_slice() { one.clone() } else { Object::Array(contents) });
        if let Some(res) = resources {
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
    })?;
    Ok(())
}

/// Inventory indexes exclude the streams that belong to added content.
pub(crate) fn selectable_images(
    doc: &Document,
    page: usize,
    excluded: &std::collections::BTreeSet<ObjRef>,
) -> Result<Vec<(usize, PageImage)>, EditError> {
    let p = page_of(doc, page)?;
    let all = streams(doc, &p.dict);
    Ok(page_images(doc, page)?
        .into_iter()
        .enumerate()
        .filter(|(_, image)| !all.get(image.stream).and_then(|(o, _)| o.as_ref()).is_some_and(|r| excluded.contains(&r)))
        .collect())
}

/// Plan all transforms before changing a stream; indexes refer to the original inventory.
/// Splice only the chosen Do operators and preserve all other bytes and stream dictionaries.
pub(crate) fn translate_images(doc: &mut Document, page: usize, indexes: &[usize], offset: [f64; 2]) -> Result<(), EditError> {
    if indexes.is_empty() {
        return Ok(());
    }
    let images = page_images(doc, page)?;
    let p = page_of(doc, page)?;
    let all = streams(doc, &p.dict);
    let mut plans = std::collections::BTreeMap::new();
    for index in indexes {
        let image = images.get(*index).ok_or_else(|| EditError::Invalid("a selected image no longer exists".into()))?;
        let placement = Matrix(image.draw_matrix);
        let inverse = placement.invert().ok_or_else(|| EditError::Invalid("a selected image has no area".into()))?;
        let [a, b, c, d, _, _] = inverse.0;
        let movement = Matrix([1.0, 0.0, 0.0, 1.0, a * offset[0] + c * offset[1], b * offset[0] + d * offset[1]]);
        if !movement.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
            return Err(EditError::Invalid("the image move is too large".into()));
        }
        plans.insert((image.stream, image.op), movement);
    }
    let mut data = Vec::with_capacity(all.len());
    let mut inside_text = false;
    for (stream, (_, bytes)) in all.iter().enumerate() {
        let pieces = pdfcraft_content::Pieces::join(&[bytes.as_slice()]);
        let ops = pieces.parse().ops;
        for (index, op) in ops.iter().enumerate() {
            if op.is("BT") {
                inside_text = true;
            } else if op.is("ET") {
                inside_text = false;
            } else if inside_text && plans.contains_key(&(stream, index)) {
                return Err(EditError::Invalid("an image inside a text object cannot be moved in a group".into()));
            }
        }
        let changed = crate::text::splice(&pieces, &ops, |index| {
            let Some(matrix) = plans.remove(&(stream, index)) else { return (Vec::new(), true) };
            let original = ops.get(index).cloned().into_iter();
            // Local translations can be tiny under a large CTM. Preserve their precision
            // rather than rounding them to zero or amplifying an error on the page.
            let mut out = vec![Op::new("q", vec![]), Op::new("cm", matrix.0.map(Object::Real).to_vec())];
            out.extend(original);
            out.push(Op::new("Q", vec![]));
            (out, false)
        });
        data.push(changed.into_iter().next().unwrap_or_default());
    }
    if !plans.is_empty() {
        return Err(EditError::Invalid("a selected image operation is missing".into()));
    }
    let contents = crate::text::rewritten_contents(doc, &all, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}
