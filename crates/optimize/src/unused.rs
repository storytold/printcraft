//! Clean Up: drop image and form XObjects that a page lists in its resources but nothing draws,
//! such as an image deleted with Edit a PDF (its `Do` goes, its resource entry stays). A full
//! rewrite keeps every object that is still referenced, so without this the image would stay in
//! the file.
//!
//! Removing a name something still draws would make content disappear, so the rule is simple
//! and errs on the side of keeping:
//! - only a page's own resources are cleaned: dictionaries nothing else refers to (resources
//!   shared with other pages, forms or appearances, and inherited ones, are left alone);
//! - a name goes only if it is written nowhere in any stream the document still uses, found by
//!   scanning the bytes wherever the name appears. Nothing can draw a resource by name without
//!   writing the name in a stream, whichever resources a viewer then looks it up in;
//! - if any such stream can't be read exactly the way a viewer would read it, nothing is
//!   removed. Only pictures (images in an image codec, used only as images) are skipped.
//!
//! So a name another page writes for its own image keeps this page's image of that name too.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::{OptimizeError, Stage};

/// Image codecs: a stream encoded with one holds picture data.
const IMAGE_CODECS: [&[u8]; 6] = [b"DCTDecode", b"DCT", b"JPXDecode", b"JBIG2Decode", b"CCITTFaxDecode", b"CCF"];

/// Most data decoded while looking for names, in all; past it nothing is removed.
const DECODE_BUDGET: usize = 1 << 31;

/// How much data is decoded between two checks for a cancel.
const CANCEL_CHECK: usize = 64 << 20;

/// Where a page's own `/XObject` dictionary is.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Place {
    /// An object of its own (`/XObject n 0 R`).
    Own(ObjRef),
    /// Directly in the page's resources object (`/Resources n 0 R`).
    InResources(ObjRef),
    /// Directly in resources written directly in the page.
    InPage(ObjRef),
}

/// How an object is used where a reference to it appears.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    /// As a picture: an `/XObject` entry, `/SMask`, `/Mask`, `/Thumb` or an alternate `/Image`.
    Picture,
    /// As an `/XObject` dictionary (its entries are pictures or forms).
    Entries,
    Other,
}

/// The objects the document uses (everything a full rewrite keeps): how many references point
/// at each, and which are referenced other than as a picture.
struct Graph {
    refs: HashMap<u32, usize>,
    not_picture: HashSet<u32>,
}

fn graph(doc: &Document) -> Graph {
    let child = |key: &[u8], parent: Use| match (parent, key) {
        (Use::Entries, _) => Use::Picture,
        (_, b"XObject") => Use::Entries,
        (_, b"SMask" | b"Mask" | b"Thumb" | b"Image") => Use::Picture,
        _ => Use::Other,
    };
    let mut g = Graph { refs: HashMap::new(), not_picture: HashSet::new() };
    let mut stack: Vec<(Object, Use)> = doc.trailer().iter().map(|(_, v)| (v.clone(), Use::Other)).collect();
    while let Some((obj, how)) = stack.pop() {
        let expand = |d: &Dict, stack: &mut Vec<(Object, Use)>| stack.extend(d.iter().map(|(k, v)| (v.clone(), child(k, how))));
        match obj {
            Object::Ref(r) => {
                if how != Use::Picture {
                    g.not_picture.insert(r.num);
                }
                let n = g.refs.entry(r.num).or_default();
                *n += 1;
                if *n > 1 {
                    continue;
                }
                match &*doc.get(r) {
                    Object::Dict(d) => expand(d, &mut stack),
                    Object::Stream(s) => expand(&s.dict, &mut stack),
                    Object::Array(a) => stack.extend(a.iter().map(|v| (v.clone(), how))),
                    _ => {}
                }
            }
            Object::Dict(d) => expand(&d, &mut stack),
            Object::Array(a) => stack.extend(a.into_iter().map(|v| (v, how))),
            _ => {}
        }
    }
    g
}

/// `page`'s own `/XObject` dictionary, if nothing else refers to it.
fn own_xobjects(doc: &Document, page: ObjRef, refs: &HashMap<u32, usize>) -> Option<(Place, Dict)> {
    let only_once = |r: &ObjRef| refs.get(&r.num) == Some(&1);
    let p = doc.get(page);
    let p = p.as_dict()?;
    // A node with kids passes its resources on.
    if p.name(b"Type") != Some(b"Page") || p.contains(b"Kids") {
        return None;
    }
    let (res, place) = match p.get(b"Resources")? {
        Object::Ref(r) if only_once(r) => (doc.get(*r).as_dict()?.clone(), Place::InResources(*r)),
        Object::Dict(d) => (d.clone(), Place::InPage(page)),
        _ => return None,
    };
    match res.get(b"XObject")? {
        Object::Ref(x) if only_once(x) => Some((Place::Own(*x), doc.get(*x).as_dict()?.clone())),
        Object::Dict(x) => Some((place, x.clone())),
        _ => None,
    }
}

fn hex(b: &u8) -> Option<u8> {
    (*b as char).to_digit(16).map(|d| d as u8)
}

/// Record which of `wanted` (none longer than `longest`) are written in `data` (`/Name`, `#xx`
/// escapes decoded), wherever they appear: operands, strings, comments or inline image data.
fn find_names(data: &[u8], wanted: &HashSet<Vec<u8>>, longest: usize, found: &mut HashSet<Vec<u8>>) {
    let regular =
        |b: u8| !matches!(b, b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%');
    let mut i = 0;
    while let Some(slash) = data.get(i..).and_then(|rest| rest.iter().position(|&b| b == b'/')) {
        let mut j = i + slash + 1;
        let mut name = Vec::new();
        let mut too_long = false;
        while let Some(&b) = data.get(j) {
            if !regular(b) {
                break;
            }
            let (byte, step) = match (b, data.get(j + 1).and_then(hex), data.get(j + 2).and_then(hex)) {
                (b'#', Some(h), Some(l)) => (h << 4 | l, 3),
                _ => (b, 1),
            };
            if name.len() < longest {
                name.push(byte);
            } else {
                too_long = true;
            }
            j += step;
        }
        if !too_long && wanted.contains(&name) {
            found.insert(name);
        }
        i = j;
    }
}

/// What a stream's data is, for finding names in it.
enum Data {
    /// A picture: image data in an image codec, used only as an image.
    Picture,
    Decoded(Vec<u8>),
    /// Not readable exactly the way a viewer would read it.
    Unknown,
}

/// Whether the document's security handler leaves metadata streams unencrypted.
fn plain_metadata(doc: &Document) -> bool {
    doc.trailer()
        .get(b"Encrypt")
        .map(|e| doc.resolve(e))
        .is_some_and(|e| matches!(e.as_dict().and_then(|e| e.get(b"EncryptMetadata")), Some(Object::Bool(false))))
}

fn data(s: &Stream, num: u32, g: &Graph, encrypted: bool, plain_metadata: bool) -> Data {
    let d = &s.dict;
    // Streams the security handler leaves as stored, which a viewer may still decrypt.
    let ty = d.name(b"Type");
    if encrypted && (ty == Some(b"XRef") || (ty == Some(b"Metadata") && plain_metadata)) {
        return Data::Unknown;
    }
    // External data, or a filter description that is abbreviated, indirect or doesn't line up:
    // a viewer might decode it differently from this check.
    if d.contains(b"F")
        || d.contains(b"DP")
        || matches!(d.get(b"Filter"), Some(Object::Ref(_)))
        || matches!(d.get(b"DecodeParms"), Some(Object::Ref(_)))
    {
        return Data::Unknown;
    }
    let filters: Vec<&[u8]> = match d.get(b"Filter") {
        None => Vec::new(),
        Some(Object::Name(n)) => vec![n.as_slice()],
        Some(Object::Array(a)) => {
            let names: Vec<&[u8]> = a.iter().filter_map(Object::as_name).collect();
            if names.len() != a.len() {
                return Data::Unknown;
            }
            names
        }
        Some(_) => return Data::Unknown,
    };
    if filters.contains(&&b"Crypt"[..]) {
        return Data::Unknown;
    }
    if filters.iter().any(|f| IMAGE_CODECS.contains(f)) {
        // Viewers decode any stream's codec, so picture data skipped here must be a picture.
        let picture = d.name(b"Subtype") == Some(b"Image") && !g.not_picture.contains(&num);
        return if picture { Data::Picture } else { Data::Unknown };
    }
    // Parameters only as direct integers, which this check and viewers read alike.
    let direct = |p: &Object| match p {
        Object::Dict(p) => p.iter().all(|(_, v)| matches!(v, Object::Int(_))),
        Object::Null => true,
        _ => false,
    };
    let parms_line_up = match d.get(b"DecodeParms") {
        None => true,
        Some(p @ (Object::Dict(_) | Object::Null)) => filters.len() == 1 && direct(p),
        Some(Object::Array(a)) => a.len() == filters.len() && a.iter().all(direct),
        Some(_) => false,
    };
    if !parms_line_up {
        return Data::Unknown;
    }
    s.decoded_strict().map_or(Data::Unknown, Data::Decoded)
}

/// Drop the image and form XObjects that `pages` list in their own resources but no stream
/// names. Returns how many went. `progress` is asked again every [`CANCEL_CHECK`] bytes
/// decoded; `false` cancels.
pub(crate) fn remove_unused(doc: &mut Document, pages: &[ObjRef], progress: &mut dyn FnMut(Stage) -> bool) -> Result<usize, OptimizeError> {
    let g = graph(doc);
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for page in pages {
        let Some((place, xobjects)) = own_xobjects(doc, *page, &g.refs) else { continue };
        if !seen.insert(place) {
            continue;
        }
        let names: Vec<Vec<u8>> = xobjects
            .iter()
            .filter(|(_, v)| matches!(&*doc.resolve(v), Object::Stream(s) if matches!(s.dict.name(b"Subtype"), Some(b"Image" | b"Form"))))
            .map(|(name, _)| name.clone())
            .collect();
        if !names.is_empty() {
            candidates.push((place, names));
        }
    }
    let wanted: HashSet<Vec<u8>> = candidates.iter().flat_map(|(_, names)| names.iter().cloned()).collect();
    let longest = wanted.iter().map(Vec::len).max().unwrap_or(0);
    let (encrypted, plain_metadata) = (doc.trailer().contains(b"Encrypt"), plain_metadata(doc));
    let mut written = HashSet::new();
    let (mut decoded, mut next_check) = (0usize, CANCEL_CHECK);
    for num in g.refs.keys() {
        if written.len() == wanted.len() {
            return Ok(0);
        }
        let obj = doc.get(ObjRef::new(*num, doc.generation(*num)));
        let Object::Stream(s) = &*obj else { continue };
        match data(s, *num, &g, encrypted, plain_metadata) {
            Data::Picture => {}
            Data::Decoded(bytes) => {
                decoded = decoded.saturating_add(bytes.len());
                if decoded > DECODE_BUDGET {
                    return Ok(0);
                }
                if decoded >= next_check {
                    next_check = decoded.saturating_add(CANCEL_CHECK);
                    if !progress(Stage::CleanUp) {
                        return Err(OptimizeError::Cancelled);
                    }
                }
                find_names(&bytes, &wanted, longest, &mut written);
            }
            Data::Unknown => return Ok(0),
        }
    }
    let mut removed = 0;
    for (place, names) in candidates {
        let gone: Vec<Vec<u8>> = names.into_iter().filter(|n| !written.contains(n)).collect();
        if gone.is_empty() {
            continue;
        }
        let strip = |x: &mut Dict| {
            for n in &gone {
                x.remove(n);
            }
        };
        match place {
            Place::Own(x) => doc.update_dict(x, strip)?,
            Place::InResources(r) => doc.update_dict(r, |res| {
                if let Some(Object::Dict(x)) = res.get_mut(b"XObject") {
                    strip(x);
                }
            })?,
            Place::InPage(p) => doc.update_dict(p, |d| {
                if let Some(Object::Dict(res)) = d.get_mut(b"Resources")
                    && let Some(Object::Dict(x)) = res.get_mut(b"XObject")
                {
                    strip(x);
                }
            })?,
        }
        removed += gone.len();
    }
    Ok(removed)
}
