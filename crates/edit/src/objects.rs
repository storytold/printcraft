//! Mixed-object inventory and atomic translation in Edit a PDF. Source indexes are resolved
//! together before mutation. Added content appears once, not again as a paragraph/image.

use std::collections::{BTreeMap, BTreeSet};

use pdfcraft_content::{Matrix, Op, Pieces};
use pdfcraft_cos::{Dict, Document, Object};

use crate::{Content, EditError};

pub const MAX_MOVE_OBJECTS: usize = 1_000;
const MAX_PAGE_OBJECTS: usize = 10_000;
const MAX_COORDINATE: f64 = 1e9;
const MAX_CONTENT_BYTES: usize = 128 * 1024 * 1024;

/// The source inventory of an editable item. Indexes are zero-based within that inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectKind {
    Added,
    Text,
    Image,
}

impl ObjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Text => "text",
            Self::Image => "image",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectTarget {
    pub kind: ObjectKind,
    pub index: usize,
}

/// An item recognised by Edit a PDF. Geometry is in the page's user space; a Form is one
/// grouped artwork item. Its nested content is kept intact, not selected again separately.
#[derive(Clone, Debug, PartialEq)]
pub struct EditableObject {
    pub target: ObjectTarget,
    pub rect: [f64; 4],
    pub content_type: &'static str,
    pub text: Option<String>,
}

fn valid_rect(rect: [f64; 4]) -> bool {
    rect.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) && rect[2] > rect[0] && rect[3] > rect[1]
}

pub fn editable_objects(doc: &Document, page: usize) -> Result<Vec<EditableObject>, EditError> {
    checked_streams(doc, page)?;
    let added: Vec<_> = crate::list_added(doc).into_iter().filter(|a| a.page == page).collect();
    if added.len() > MAX_PAGE_OBJECTS {
        return Err(EditError::Invalid("this page has too many objects to select together".into()));
    }
    let geometry = added_geometry(doc, page)?.1;
    let excluded = added.iter().map(|a| a.obj).collect();
    let mut objects = Vec::new();
    for (index, a) in added.into_iter().enumerate() {
        let (content_type, text) = match &a.content {
            Content::Text(t) => ("text", Some(t.text.clone())),
            Content::Image(_) => ("image", None),
        };
        objects.push(EditableObject {
            target: ObjectTarget { kind: ObjectKind::Added, index },
            rect: geometry.get(index).ok_or_else(|| EditError::Invalid("an added item has no placement".into()))?.rect,
            content_type,
            text,
        });
    }
    for (index, b) in crate::text::selectable_blocks(doc, page, &excluded)? {
        objects.push(EditableObject {
            target: ObjectTarget { kind: ObjectKind::Text, index },
            rect: b.rect,
            content_type: "text",
            text: Some(b.text),
        });
    }
    for (index, image) in crate::images::selectable_images(doc, page, &excluded)? {
        objects.push(EditableObject {
            target: ObjectTarget { kind: ObjectKind::Image, index },
            rect: image.rect,
            content_type: if image.is_form { "form" } else { "image" },
            text: None,
        });
    }
    if objects.len() > MAX_PAGE_OBJECTS {
        return Err(EditError::Invalid("this page has too many objects to select together".into()));
    }
    if objects.iter().any(|o| !valid_rect(o.rect)) {
        return Err(EditError::Invalid("an object has invalid geometry".into()));
    }
    Ok(objects)
}

/// Move selected mixed objects by a user-space vector as one atomic operation. All targets
/// refer to the original inventories. A refusal leaves the graph, including overlay, untouched.
pub fn move_objects(doc: &mut Document, page: usize, targets: &[ObjectTarget], offset: [f64; 2]) -> Result<(), EditError> {
    if targets.is_empty() || targets.len() > MAX_MOVE_OBJECTS {
        return Err(EditError::Invalid("select between 1 and 1000 objects to move".into()));
    }
    if !offset.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) || offset == [0.0, 0.0] {
        return Err(EditError::Invalid("the move must be finite and non-zero".into()));
    }
    let inventory: BTreeMap<_, _> = editable_objects(doc, page)?.into_iter().map(|o| (o.target, o)).collect();
    let mut unique = BTreeSet::new();
    for target in targets {
        if !unique.insert(*target) {
            return Err(EditError::Invalid("an object was selected more than once".into()));
        }
        let object =
            inventory.get(target).ok_or_else(|| EditError::Invalid("a selected object no longer exists; select the objects again".into()))?;
        let r = object.rect;
        if !valid_rect([r[0] + offset[0], r[1] + offset[1], r[2] + offset[0], r[3] + offset[1]]) {
            return Err(EditError::Invalid("the object move is too large".into()));
        }
    }
    let indexes = |kind| targets.iter().filter(|t| t.kind == kind).map(|t| t.index).collect::<Vec<_>>();
    let mut work = doc.clone();
    crate::text::translate_blocks(&mut work, page, &indexes(ObjectKind::Text), offset)?;
    crate::images::translate_images(&mut work, page, &indexes(ObjectKind::Image), offset)?;
    translate_added(&mut work, page, &indexes(ObjectKind::Added), offset)?;
    // Placement operators add to size/depth/work budgets, and moving paragraphs can change
    // their grouping. Keep the resulting inventory usable; refuse before swapping otherwise.
    editable_objects(&work, page)?;
    *doc = work;
    Ok(())
}

struct AddedGeometry {
    rect: [f64; 4],
    local_rect: [f64; 4],
    origin: Matrix,
    outside: Matrix,
    current: Matrix,
    stream: usize,
    op: usize,
    params: Dict,
}

/// An added item's metadata is in the coordinate space it was written in, which can differ
/// from today's page rotation/crop/UserUnit. Read its original placement, including graphics
/// state from preceding streams. MoveOrigin retains that basis after a byte-preserving move.
fn added_geometry(doc: &Document, page: usize) -> Result<(Pieces, Vec<AddedGeometry>), EditError> {
    let added: Vec<_> = crate::list_added(doc).into_iter().filter(|a| a.page == page).collect();
    if added.is_empty() {
        return Ok((Pieces::default(), Vec::new()));
    }
    let streams = checked_streams(doc, page)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let owned: BTreeSet<_> = added.iter().map(|a| a.obj).collect();
    let mut geometry = BTreeMap::new();
    let mut first = BTreeMap::new();
    let mut matrix = Matrix::IDENTITY;
    let mut stack = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let (stream, end) = pieces.pieces_of(op);
        let object = streams.get(stream).and_then(|(r, _)| r.as_ref());
        if object.is_some_and(|r| owned.contains(&r)) {
            if let std::collections::btree_map::Entry::Vacant(entry) = first.entry(stream) {
                if stream != end || !op.is("q") {
                    return Err(EditError::Invalid("an added item has an unsupported placement".into()));
                }
                entry.insert((index, matrix));
            } else if first.get(&stream).is_some_and(|(i, _)| i.checked_add(1) == Some(index)) {
                if stream != end || !op.is("cm") {
                    return Err(EditError::Invalid("an added item has an unsupported placement".into()));
                }
                let current = op.nums::<6>().map(Matrix).ok_or_else(|| EditError::Invalid("an added item has invalid geometry".into()))?;
                let outside = first.get(&stream).map_or(Matrix::IDENTITY, |(_, m)| *m);
                let params = object
                    .and_then(|r| doc.get(r).as_dict().and_then(|d| d.get(b"PCAdded")).map(|p| doc.resolve(p)))
                    .and_then(|p| p.as_dict().cloned())
                    .ok_or_else(|| EditError::Invalid("an added item has no parameters".into()))?;
                let origin = match params.get(b"MoveOrigin") {
                    None => current,
                    Some(o) => doc
                        .resolve(o)
                        .as_array()
                        .and_then(|a| Matrix::from_operands(a))
                        .ok_or_else(|| EditError::Invalid("an added item has invalid geometry".into()))?,
                };
                let raw = params
                    .get(b"Rect")
                    .map(|r| doc.resolve(r))
                    .and_then(|r| r.as_array().cloned())
                    .ok_or_else(|| EditError::Invalid("an added item has no rectangle".into()))?;
                let local_rect = raw
                    .iter()
                    .map(Object::as_f64)
                    .collect::<Option<Vec<_>>>()
                    .and_then(|r| <[f64; 4]>::try_from(r).ok())
                    .ok_or_else(|| EditError::Invalid("an added item has invalid geometry".into()))?;
                if !current.0.iter().chain(origin.0.iter()).chain(outside.0.iter()).all(|v| v.is_finite() && v.abs() <= 1e12)
                    || !valid_rect(local_rect)
                {
                    return Err(EditError::Invalid("an added item has invalid geometry".into()));
                }
                geometry.insert(
                    stream,
                    AddedGeometry { rect: origin.then(&outside).bbox(local_rect), local_rect, origin, outside, current, stream, op: index, params },
                );
            }
        }
        match op.op.as_slice() {
            b"q" => {
                if stack.len() >= 1024 {
                    return Err(EditError::Invalid("the page graphics state is nested too deeply".into()));
                }
                stack.push(matrix);
            }
            b"Q" => matrix = stack.pop().unwrap_or(matrix),
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    matrix = Matrix(m).then(&matrix);
                }
            }
            _ => {}
        }
    }
    let geometry: Vec<_> = geometry.into_values().collect();
    if geometry.len() != added.len() {
        return Err(EditError::Invalid("an added item has no placement".into()));
    }
    Ok((pieces, geometry))
}

fn translate_added(doc: &mut Document, page: usize, indexes: &[usize], offset: [f64; 2]) -> Result<(), EditError> {
    if indexes.is_empty() {
        return Ok(());
    }
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = checked_streams(doc, page)?;
    let (pieces, geometry) = added_geometry(doc, page)?;
    let ops = pieces.parse().ops;
    let mut replacements = BTreeMap::new();
    let mut parameters = Vec::new();
    let linear = |inverse: Matrix| {
        let [a, b, c, d, _, _] = inverse.0;
        [a * offset[0] + c * offset[1], b * offset[0] + d * offset[1]]
    };
    for index in indexes {
        let g = geometry.get(*index).ok_or_else(|| EditError::Invalid("a selected added item no longer exists".into()))?;
        let delta = linear(g.origin.then(&g.outside).invert().ok_or_else(|| EditError::Invalid("an added item has no area".into()))?);
        let outer_delta = linear(g.outside.invert().ok_or_else(|| EditError::Invalid("an added item has no area".into()))?);
        let current = g.current.then(&Matrix([1.0, 0.0, 0.0, 1.0, outer_delta[0], outer_delta[1]]));
        let r = g.local_rect;
        let rect = [r[0] + delta[0], r[1] + delta[1], r[2] + delta[0], r[3] + delta[1]];
        if !valid_rect(rect) || !current.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
            return Err(EditError::Invalid("the added item move is too large".into()));
        }
        let mut params = g.params.clone();
        params.set(b"MoveOrigin".to_vec(), Object::Array(g.origin.0.map(Object::Real).to_vec()));
        params.set(b"Rect".to_vec(), Object::Array(rect.map(Object::Real).to_vec()));
        parameters.push((g.stream, params));
        replacements.insert(g.op, Op::new("cm", current.0.map(Object::Real).to_vec()));
    }
    // A placement replaces exactly one operator. Keep its existing following separator,
    // rather than adding a blank line every time the item is moved.
    let edits = replacements
        .into_iter()
        .map(|(i, op)| {
            let original = ops.get(i).ok_or_else(|| EditError::Invalid("an added item placement is missing".into()))?;
            let mut bytes = pdfcraft_content::serialize_ops(&[op]);
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            Ok((original.span.clone(), bytes))
        })
        .collect::<Result<Vec<_>, EditError>>()?;
    let data = pieces.splice(edits);
    let contents = crate::text::rewritten_contents(doc, &streams, data);
    let refs = match &contents {
        Object::Array(a) => a.clone(),
        one => vec![one.clone()],
    };
    for (stream, params) in parameters {
        let r = refs.get(stream).and_then(Object::as_ref).ok_or_else(|| EditError::Invalid("the added item stream is missing".into()))?;
        doc.update_dict(r, |d| d.set(b"PCAdded".to_vec(), Object::Dict(params)))?;
    }
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

/// A missing or undecodable preceding stream can hide graphics state that affects every
/// later item. Refuse group editing rather than inventing an inventory from partial data.
fn checked_streams(doc: &Document, page: usize) -> Result<Vec<(Object, Vec<u8>)>, EditError> {
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let contents = match p.dict.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => {
                if a.len() > MAX_PAGE_OBJECTS {
                    return Err(EditError::Invalid("this page has too many content streams to select objects together".into()));
                }
                a.clone()
            }
            _ => vec![c.clone()],
        },
    };
    let mut remaining = MAX_CONTENT_BYTES;
    let mut depth = 0usize;
    let mut operations = 0usize;
    let mut streams = Vec::with_capacity(contents.len());
    for object in contents {
        let resolved = doc.resolve(&object);
        let Object::Stream(stream) = &*resolved else {
            return Err(EditError::Invalid("a page content stream is missing".into()));
        };
        let bytes = stream
            .decoded_within(remaining.min(64 * 1024 * 1024))
            .map_err(|_| EditError::Invalid("the page content cannot be decoded within its size limit".into()))?;
        remaining =
            remaining.checked_sub(bytes.len()).ok_or_else(|| EditError::Invalid("the page content is too large for group selection".into()))?;
        let parsed = pdfcraft_content::parse(&bytes);
        operations = operations.saturating_add(parsed.ops.len());
        if operations > 100_000 {
            return Err(EditError::Invalid("this page has too many content operations for group selection".into()));
        }
        for op in parsed.ops {
            if op.is("q") {
                depth = depth.saturating_add(1);
            } else if op.is("Q") {
                depth = depth.saturating_sub(1);
            }
            if depth > 1024 {
                return Err(EditError::Invalid("the page graphics state is nested too deeply".into()));
            }
        }
        streams.push((object, bytes));
    }
    Ok(streams)
}
