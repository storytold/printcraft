//! Serialization and saving (ISO 32000-2 §7.5).
//!
//! * `write_incremental` appends the edited objects, a cross-reference section in the same style
//!   as the file's last one (table or stream) and a trailer with `/Prev`. The original bytes are
//!   never modified, which keeps digital signatures over earlier revisions valid.
//! * `write_full` writes only objects reachable from the trailer, renumbered densely, with a
//!   cross-reference table or stream — the "Save As" / garbage-collecting path.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write;

use crate::CosError;
use crate::document::{Document, ObjectReader};
use crate::object::{Dict, ObjRef, Object, PdfString, Stream};

/// Serialize one object. Deterministic: same object → same bytes.
pub fn serialize(o: &Object, out: &mut Vec<u8>) {
    match o {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Object::Real(r) => write_real(*r, out),
        Object::String(s) => write_string(s, out),
        Object::Name(n) => write_name(n, out),
        Object::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                serialize(x, out);
            }
            out.push(b']');
        }
        Object::Dict(d) => write_dict(d, out),
        Object::Stream(s) => write_stream(s, out),
        Object::Ref(r) => {
            let _ = write!(out, "{} {} R", r.num, r.generation);
        }
    }
}

fn write_real(r: f64, out: &mut Vec<u8>) {
    if !r.is_finite() {
        out.push(b'0');
        return;
    }
    if r.fract() == 0.0 && r.abs() < 1e15 {
        let _ = write!(out, "{}", r as i64);
        return;
    }
    // Plain decimal notation (§7.3.3 has no exponents). Rust's `Display` for f64 prints the
    // shortest string that parses back to the same value, never in exponent form, so values
    // round-trip exactly (rounding to a fixed number of places visibly shifted shading colours).
    let s = format!("{r}");
    out.extend_from_slice(if s == "-0" { b"0" } else { s.as_bytes() });
}

fn write_name(n: &[u8], out: &mut Vec<u8>) {
    out.push(b'/');
    for &b in n {
        let regular = (b'!'..=b'~').contains(&b) && !matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' | b'#');
        if regular {
            out.push(b);
        } else {
            let _ = write!(out, "#{b:02X}");
        }
    }
}

fn write_string(s: &PdfString, out: &mut Vec<u8>) {
    let binary = s.bytes.iter().filter(|b| !(b' '..=b'~').contains(*b) && !matches!(b, b'\n' | b'\r' | b'\t')).count();
    if s.hex || binary * 4 > s.bytes.len().max(1) {
        out.push(b'<');
        for b in &s.bytes {
            let _ = write!(out, "{b:02X}");
        }
        out.push(b'>');
        return;
    }
    out.push(b'(');
    for &b in &s.bytes {
        match b {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(b);
            }
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0C => out.extend_from_slice(b"\\f"),
            b' '..=b'~' => out.push(b),
            _ => {
                let _ = write!(out, "\\{b:03o}");
            }
        }
    }
    out.push(b')');
}

fn write_dict(d: &Dict, out: &mut Vec<u8>) {
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        write_name(k, out);
        out.push(b' ');
        serialize(v, out);
    }
    out.extend_from_slice(b">>");
}

fn write_stream(s: &Stream, out: &mut Vec<u8>) {
    let mut d = s.dict.clone();
    d.set(b"Length".to_vec(), Object::Int(s.raw.len() as i64));
    write_dict(&d, out);
    out.extend_from_slice(b"\nstream\n");
    out.extend_from_slice(&s.raw);
    out.extend_from_slice(b"\nendstream");
}

/// The object as written: encrypted with its (output) number when the document is encrypted,
/// except the `/Encrypt` dictionary itself (`source_num` is its number in the source document).
fn prepared<'a>(doc: &Document, source_num: u32, num: u32, generation: u16, o: &'a Object) -> Cow<'a, Object> {
    match doc.output_security() {
        (Some(h), encrypt_num) if Some(source_num) != encrypt_num => Cow::Owned(crate::security::transform(h, o, num, generation, false)),
        _ => Cow::Borrowed(o),
    }
}

fn write_indirect(num: u32, generation: u16, o: &Object, out: &mut Vec<u8>) {
    let _ = writeln!(out, "{num} {generation} obj");
    serialize(o, out);
    out.extend_from_slice(b"\nendobj\n");
}

/// Options for saving.
#[derive(Clone, Debug)]
pub struct SaveOptions {
    /// Replace or add `/Info /ModDate` with this PDF date string (e.g. from `pdf_date`).
    pub mod_date: Option<String>,
    /// Seed for a new file identifier when the document has none (deterministic in tests).
    pub id_seed: u64,
    /// Full saves: pack non-stream objects into compressed object streams with a cross-reference
    /// stream (PDF 1.5+, §7.5.7–7.5.8). Off writes a classic table that any reader accepts.
    pub object_streams: bool,
    /// Entropy for a regenerated file identifier (see `Document::require_full_save_with_new_id`),
    /// mixed with process randomness. Supply it where the standard library has none (WebAssembly)
    /// or to make tests deterministic in combination with `id_seed`.
    pub id_entropy: Option<[u8; 16]>,
    /// Allow a full rewrite of a signed document. Off by default: a rewrite invalidates every
    /// signature, so `write_full` (and `write_incremental` when it has to fall back to one)
    /// returns `CosError::SignedDocument` instead.
    pub allow_signed_rewrite: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self { mod_date: None, id_seed: 0x5052_494E_5443_5241, object_streams: true, id_entropy: None, allow_signed_rewrite: false }
    }
}

/// Objects per object stream (Acrobat and qpdf use 100–200).
const OBJSTM_SIZE: usize = 100;
/// Keep a batch's serialized body small as well as limiting its object count. One object
/// may exceed this target; it is emitted in its own object stream without retaining a batch.
const OBJSTM_BYTES: usize = 8 * 1024 * 1024;

/// Where an object lives, for the cross-reference section.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    /// Free, with the generation the number gets when reused.
    Free(u16),
    /// At a byte offset.
    InFile(u64, u16),
    /// Inside object stream `stream`, at position `index`.
    InStream(u32, u32),
}

/// Append the document's edits to its original bytes (incremental update, §7.5.6).
///
/// A document that had to be reconstructed (no usable cross-reference chain) cannot be updated
/// incrementally — other readers would follow the broken chain — so it is rewritten in full,
/// which also repairs the file (what users expect after "the file was damaged and repaired").
pub fn write_incremental(doc: &Document, opts: &SaveOptions) -> Result<Vec<u8>, CosError> {
    if doc.stream_limited() {
        return Err(CosError::ReadOnlyLimit);
    }
    // Reconstructed files have no chain to append to; added or removed encryption must
    // rewrite every object; redaction must not leave the old revision behind. All need a full save.
    let rewrite_requested = doc.encryption_changed() || doc.full_save_required();
    if doc.revisions().is_empty() && !rewrite_requested {
        // Repair of a file with no usable chain (e.g. signing one): nothing to append to, and not
        // a rewrite anybody requested, so the signed-document guard does not apply.
        return write_full_unguarded(doc, opts);
    }
    if rewrite_requested {
        return write_full(doc, opts);
    }
    let mut doc = doc.clone();
    stamp_mod_date(&mut doc, opts);
    let original = doc.bytes().clone();
    // The revision is written on its own and joined to the original at the end, so the result is
    // allocated once at its exact size: growing a buffer that holds a large original past a
    // guessed capacity would copy the whole file again and keep up to as much again unused.
    let newline = !matches!(original.last(), Some(b'\n' | b'\r'));
    let base = (original.len() + usize::from(newline)) as u64;
    let mut out = Vec::new();
    let mut offsets: BTreeMap<u32, (u64, u16, bool)> = BTreeMap::new(); // num → (offset, gen, in use)
    for (num, generation, obj) in doc.overlay_entries() {
        match obj {
            Some(o) => {
                offsets.insert(num, (base + out.len() as u64, generation, true));
                write_indirect(num, generation, &prepared(&doc, num, num, generation, &o), &mut out);
            }
            None => {
                offsets.insert(num, (0, generation, false));
            }
        }
    }
    let prev = doc.revisions().last().map(|r| r.xref_offset);
    let as_stream = doc.revisions().last().is_some_and(|r| r.is_stream);
    let mut trailer = doc.trailer().clone();
    if doc.output_security().0.is_none() {
        ensure_id(&mut trailer, opts, &original); // an encrypted file's ID is part of its key
    }
    if let Some(p) = prev {
        trailer.set(b"Prev".to_vec(), Object::Int(p as i64));
    }
    let size = doc.next_num().max(trailer.int(b"Size").unwrap_or(0) as u32);
    if as_stream {
        let rows = offsets.iter().map(|(n, (o, g, used))| (*n, if *used { Row::InFile(*o, *g) } else { Row::Free(*g) })).collect();
        write_xref_stream(&mut out, base, &mut trailer, rows, size);
    } else {
        trailer.set(b"Size".to_vec(), Object::Int(size as i64));
        let xref_at = base + out.len() as u64;
        write_xref_table(&mut out, &offsets, prev.is_none());
        out.extend_from_slice(b"trailer\n");
        write_dict(&trailer, &mut out);
        let _ = write!(out, "\nstartxref\n{xref_at}\n%%EOF\n");
    }
    let mut file = Vec::with_capacity(original.len() + usize::from(newline) + out.len());
    file.extend_from_slice(&original);
    if newline {
        file.push(b'\n');
    }
    file.extend_from_slice(&out);
    Ok(file)
}

/// Write reachable objects in full; signed rewrites require explicit permission.
pub fn write_full(doc: &Document, opts: &SaveOptions) -> Result<Vec<u8>, CosError> {
    if doc.stream_limited() {
        return Err(CosError::ReadOnlyLimit);
    }
    if !opts.allow_signed_rewrite {
        let signed = if doc.full_save_required() { doc.is_signed() } else { doc.is_signed_definitely() };
        if signed {
            return Err(CosError::SignedDocument);
        }
    }
    write_full_unguarded(doc, opts)
}

fn write_full_unguarded(doc: &Document, opts: &SaveOptions) -> Result<Vec<u8>, CosError> {
    if doc.stream_limited() {
        return Err(CosError::ReadOnlyLimit);
    }
    let mut reader = doc.object_reader();
    stamp_mod_date(&mut reader.document, opts);
    let trailer_in = reader.document.trailer().clone();
    // Breadth-first walk from the trailer, assigning new numbers in visit order.
    let mut map: HashMap<ObjRef, u32> = HashMap::new();
    // Record the type while traversing so classifying packed objects needs no second parse.
    let mut order: Vec<(ObjRef, bool)> = Vec::new();
    let mut queue: VecDeque<ObjRef> = VecDeque::new();
    let visit_refs = |o: &Object, queue: &mut VecDeque<ObjRef>| collect_refs(o, &mut |r| queue.push_back(r));
    for key in [&b"Root"[..], b"Info", b"Encrypt"] {
        if let Some(r) = trailer_in.reference(key) {
            queue.push_back(r);
        }
    }
    while let Some(r) = queue.pop_front() {
        if map.contains_key(&r) {
            continue;
        }
        let o = reader.get(r);
        if matches!(*o, Object::Null) {
            continue; // dangling reference: written as null by renumbering below
        }
        map.insert(r, output_number(order.len())?);
        order.push((r, matches!(*o, Object::Stream(_))));
        visit_refs(&o, &mut queue);
    }
    let version = reader.document.version().chars().take(3).collect::<String>();
    let version = if opts.object_streams && version.as_str() < "1.5" { "1.5".to_string() } else { version };
    let mut out = Vec::new();
    let _ = writeln!(out, "%PDF-{version}");
    // A binary comment (raw high bytes) marks the file as binary for transfer tools.
    out.extend_from_slice(&[b'%', 0xE2, 0xE3, 0xCF, 0xD3, b'\n']);
    // The /Encrypt dictionary and everything it references (e.g. indirect /CF crypt-filter
    // dictionaries) are needed before anything can be decrypted: never inside object streams.
    let mut encryption_objects: std::collections::HashSet<ObjRef> = std::collections::HashSet::new();
    let mut pending: Vec<ObjRef> = trailer_in.reference(b"Encrypt").into_iter().collect();
    while let Some(r) = pending.pop() {
        if encryption_objects.insert(r) {
            collect_refs(&reader.get(r), &mut |x| pending.push(x));
        }
    }
    // Acrobat refuses an encrypted file whose catalog sits in an object stream ("the file is
    // damaged and could not be repaired", before it even asks for the password), while other
    // readers accept it. qpdf keeps the catalog out of object streams too. Verified with
    // Acrobat DC on macOS (#774).
    let root_outside = if reader.document.output_security().0.is_some() { trailer_in.reference(b"Root") } else { None };
    let mut rows: BTreeMap<u32, Row> = BTreeMap::new();
    // Keep only references, not renamed copies of the entire object graph. Standalone
    // objects still precede object streams, preserving the established output order.
    let mut packed: Vec<(u32, ObjRef)> = Vec::new();
    for (i, (r, is_stream)) in order.iter().enumerate() {
        let num = output_number(i)?;
        let in_stream = opts.object_streams && !is_stream && !encryption_objects.contains(r) && root_outside != Some(*r);
        if in_stream {
            packed.push((num, *r));
        } else {
            let o = renumber(&*reread(&mut reader, *r, *is_stream)?, &map);
            rows.insert(num, Row::InFile(out.len() as u64, 0));
            write_indirect(num, 0, &prepared(&reader.document, r.num, num, 0, &o), &mut out);
        }
    }
    // Object streams take the numbers after the last object. Their contents are not encrypted
    // individually: the whole stream is (§7.5.7, §7.6.2).
    let mut next = output_number(order.len())?;
    let mut batch = ObjectStreamBatch::default();
    for (num, reference) in packed {
        let mut encoded = Vec::new();
        serialize(&renumber(&*reread(&mut reader, reference, false)?, &map), &mut encoded);
        if batch.count > 0 && batch.body.len().saturating_add(encoded.len()).saturating_add(1) > OBJSTM_BYTES {
            batch.flush(&reader.document, &mut next, &mut rows, &mut out)?;
        }
        let _ = write!(batch.header, "{} {} ", num, batch.body.len());
        batch.body.extend_from_slice(&encoded);
        batch.body.push(b'\n');
        drop(encoded);
        rows.insert(num, Row::InStream(next, batch.count as u32));
        batch.count += 1;
        if batch.count == OBJSTM_SIZE || batch.body.len() >= OBJSTM_BYTES {
            batch.flush(&reader.document, &mut next, &mut rows, &mut out)?;
        }
    }
    batch.flush(&reader.document, &mut next, &mut rows, &mut out)?;
    let mut trailer = Dict::new();
    for key in [&b"Root"[..], b"Info", b"Encrypt"] {
        if let Some(r) = trailer_in.reference(key).and_then(|r| map.get(&r)) {
            trailer.set(key.to_vec(), Object::Ref(ObjRef::new(*r, 0)));
        }
    }
    if let Some(Object::Dict(d)) = trailer_in.get(b"Encrypt") {
        trailer.set(b"Encrypt".to_vec(), Object::Dict(d.clone()));
    }
    if let Some(id) = trailer_in.get(b"ID") {
        trailer.set(b"ID".to_vec(), id.clone());
    }
    if reader.document.output_security().0.is_none() {
        ensure_id(&mut trailer, opts, reader.document.bytes());
    }
    if doc.new_id_required() {
        // An encrypted file's first string is hashed into its key: keep it, change the second.
        regenerate_id(&mut trailer, opts, &out, doc.output_security().0.is_some());
    }
    if opts.object_streams {
        write_xref_stream(&mut out, 0, &mut trailer, rows, next);
    } else {
        trailer.set(b"Size".to_vec(), Object::Int(next as i64));
        let offsets: BTreeMap<u32, (u64, u16, bool)> =
            rows.iter().filter_map(|(n, r)| if let Row::InFile(o, g) = r { Some((*n, (*o, *g, true))) } else { None }).collect();
        let xref_at = out.len();
        write_xref_table(&mut out, &offsets, true);
        out.extend_from_slice(b"trailer\n");
        write_dict(&trailer, &mut out);
        let _ = write!(out, "\nstartxref\n{xref_at}\n%%EOF\n");
    }
    // The size isn't known until the end, and the buffer grew by doubling. The result is often
    // kept as a document's working bytes for as long as it's open: drop the unused capacity.
    out.shrink_to_fit();
    Ok(out)
}

/// Object `r` read again for writing. The traversal read it once and only kept its number, so
/// a second read that fails, finds nothing or finds a different kind of object must fail the
/// save rather than write `null` in its place.
fn reread(reader: &mut ObjectReader, r: ObjRef, is_stream: bool) -> Result<std::sync::Arc<Object>, CosError> {
    let o = reader.try_get(r)?;
    if matches!(*o, Object::Null) || matches!(*o, Object::Stream(_)) != is_stream {
        return Err(CosError::Syntax { offset: 0, detail: format!("object {} read differently while saving; nothing was written", r.num) });
    }
    Ok(o)
}

#[derive(Default)]
struct ObjectStreamBatch {
    header: Vec<u8>,
    body: Vec<u8>,
    count: usize,
}

impl ObjectStreamBatch {
    fn flush(&mut self, doc: &Document, next: &mut u32, rows: &mut BTreeMap<u32, Row>, out: &mut Vec<u8>) -> Result<(), CosError> {
        if self.count == 0 {
            return Ok(());
        }
        let following = next.checked_add(1).ok_or_else(|| CosError::Syntax { offset: 0, detail: "too many objects to save".into() })?;
        // Release capacity after each batch too: an unusually large object must not leave
        // a large scratch buffer alive for the rest of the save.
        let Self { mut header, body, count } = std::mem::take(self);
        let first = header.len();
        header.extend_from_slice(&body);
        drop(body);
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("ObjStm"));
        d.set(b"N".to_vec(), Object::Int(count as i64));
        d.set(b"First".to_vec(), Object::Int(first as i64));
        let stm = Object::Stream(Stream::flate(d, &header));
        rows.insert(*next, Row::InFile(out.len() as u64, 0));
        write_indirect(*next, 0, &prepared(doc, u32::MAX, *next, 0, &stm), out);
        *next = following;
        Ok(())
    }
}

fn output_number(index: usize) -> Result<u32, CosError> {
    u32::try_from(index).ok().and_then(|n| n.checked_add(1)).ok_or_else(|| CosError::Syntax { offset: 0, detail: "too many objects to save".into() })
}

fn collect_refs(o: &Object, f: &mut impl FnMut(ObjRef)) {
    match o {
        Object::Ref(r) => f(*r),
        Object::Array(a) => a.iter().for_each(|x| collect_refs(x, f)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| collect_refs(v, f)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| collect_refs(v, f)),
        _ => {}
    }
}

fn renumber(o: &Object, map: &HashMap<ObjRef, u32>) -> Object {
    match o {
        Object::Ref(r) => map.get(r).map(|n| Object::Ref(ObjRef::new(*n, 0))).unwrap_or(Object::Null),
        Object::Array(a) => Object::Array(a.iter().map(|x| renumber(x, map)).collect()),
        Object::Dict(d) => Object::Dict(d.iter().map(|(k, v)| (k.clone(), renumber(v, map))).collect()),
        Object::Stream(s) => Object::Stream(Stream { dict: s.dict.iter().map(|(k, v)| (k.clone(), renumber(v, map))).collect(), raw: s.raw.clone() }),
        other => other.clone(),
    }
}

/// Classic xref table with one subsection per run of consecutive numbers (§7.5.4).
fn write_xref_table(out: &mut Vec<u8>, offsets: &BTreeMap<u32, (u64, u16, bool)>, include_zero: bool) {
    out.extend_from_slice(b"xref\n");
    let mut all: Vec<(u32, (u64, u16, bool))> = offsets.iter().map(|(k, v)| (*k, *v)).collect();
    if include_zero {
        all.insert(0, (0, (0, 65535, false)));
    }
    let mut i = 0;
    while i < all.len() {
        let start = all[i].0;
        let mut j = i;
        while j + 1 < all.len() && all[j + 1].0 == all[j].0 + 1 {
            j += 1;
        }
        let _ = writeln!(out, "{} {}", start, j - i + 1);
        for (_, (off, generation, used)) in &all[i..=j] {
            let _ = writeln!(out, "{:010} {:05} {} ", off, generation, if *used { 'n' } else { 'f' });
        }
        i = j + 1;
    }
}

/// A cross-reference stream section (§7.5.8) that takes object number `num`, ending the file.
/// `out` holds the file from byte offset `base` on. Rows are Flate-compressed with the PNG Up
/// predictor, and field widths fit the largest value.
fn write_xref_stream(out: &mut Vec<u8>, base: u64, trailer: &mut Dict, mut rows: BTreeMap<u32, Row>, num: u32) {
    let xref_at = base + out.len() as u64;
    rows.insert(num, Row::InFile(xref_at, 0));
    if trailer.get(b"Prev").is_none() {
        rows.entry(0).or_insert(Row::Free(65535));
    }
    let fields = |r: &Row| match *r {
        Row::Free(g) => (0u64, 0u64, u64::from(g)),
        Row::InFile(o, g) => (1, o, u64::from(g)),
        Row::InStream(s, i) => (2, u64::from(s), u64::from(i)),
    };
    let width = |v: u64| ((64 - v.leading_zeros()).div_ceil(8)).max(1) as usize;
    let (w2, w3) = rows.values().map(fields).fold((1, 1), |(a, b), (_, x, y)| (width(x).max(a), width(y).max(b)));
    let cols = 1 + w2 + w3;
    let mut index = Vec::new();
    let mut numbers = rows.keys().copied().peekable();
    while let Some(start) = numbers.next() {
        let mut last = start;
        let mut count = 1i64;
        while numbers.peek().copied().is_some_and(|next| last.checked_add(1) == Some(next)) {
            if let Some(next) = numbers.next() {
                last = next;
                count += 1;
            }
        }
        index.push(Object::Int(i64::from(start)));
        index.push(Object::Int(count));
    }
    let mut data = Vec::with_capacity(rows.len().saturating_mul(cols + 1));
    // Type is one byte, and each remaining field is at most one u64. Reuse the
    // predictor row on the stack instead of allocating a Vec for every xref entry.
    let mut prev = [0u8; 17];
    for r in rows.values() {
        let (t, a, b) = fields(r);
        let a = a.to_be_bytes();
        let b = b.to_be_bytes();
        let fields = std::iter::once(t as u8).chain(a.into_iter().skip(8 - w2)).chain(b.into_iter().skip(8 - w3));
        data.push(2); // PNG "Up"
        for (value, previous) in fields.zip(&mut prev) {
            data.push(value.wrapping_sub(*previous));
            *previous = value;
        }
    }
    let mut d = trailer.clone();
    d.set(b"Type".to_vec(), Object::name("XRef"));
    d.set(b"Size".to_vec(), Object::Int(num as i64 + 1));
    d.set(b"W".to_vec(), Object::Array(vec![Object::Int(1), Object::Int(w2 as i64), Object::Int(w3 as i64)]));
    d.set(b"Index".to_vec(), Object::Array(index));
    let mut stream = Stream::flate(d, &data);
    let mut parms = Dict::new();
    parms.set(b"Predictor".to_vec(), Object::Int(12));
    parms.set(b"Columns".to_vec(), Object::Int(cols as i64));
    stream.dict.set(b"DecodeParms".to_vec(), Object::Dict(parms));
    write_indirect(num, 0, &Object::Stream(stream), out);
    let _ = write!(out, "startxref\n{xref_at}\n%%EOF\n");
    trailer.set(b"Size".to_vec(), Object::Int(num as i64 + 1));
}

fn stamp_mod_date(doc: &mut Document, opts: &SaveOptions) {
    let Some(date) = &opts.mod_date else { return };
    let date = Object::String(PdfString::literal(date.as_bytes().to_vec()));
    match doc.trailer().get(b"Info").cloned() {
        Some(Object::Ref(r)) => {
            let _ = doc.update_dict(r, |d| d.set(b"ModDate".to_vec(), date.clone()));
        }
        _ => {
            let mut info = Dict::new();
            info.set(b"ModDate".to_vec(), date);
            let r = doc.add(info);
            doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(r));
        }
    }
}

/// Keep an existing `/ID`, or create one (two equal 16-byte strings, §14.4).
fn ensure_id(trailer: &mut Dict, opts: &SaveOptions, content: &[u8]) {
    if matches!(trailer.get(b"ID"), Some(Object::Array(a)) if a.len() == 2) {
        return;
    }
    // FNV-1a over a sample of the content and the seed: stable, dependency-free.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ opts.id_seed;
    for b in content.iter().step_by((content.len() / 4096).max(1)).chain(content.iter().rev().take(64)) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let mut id = Vec::with_capacity(16);
    id.extend_from_slice(&h.to_be_bytes());
    id.extend_from_slice(&h.rotate_left(29).wrapping_mul(0x9E37_79B9_7F4A_7C15).to_be_bytes());
    let s = Object::String(PdfString { bytes: id, hex: true });
    trailer.set(b"ID".to_vec(), Object::Array(vec![s.clone(), s]));
}

/// Give the file a new identity (§14.4): `/ID` becomes a fresh pair, except that the first
/// string is kept when `keep_first` (encryption keys depend on it).
///
/// The standard library offers randomness only through `RandomState`, which is seeded from the
/// operating system on native targets (no RNG dependency is needed) but not on wasm32, where the
/// seed is fixed. It is mixed with a process-wide counter (so two calls in one process never
/// agree), the optional caller entropy and a hash of what was written, which is what makes the
/// result differ from the source file's identifier. Where the seed is fixed (wasm32) two
/// processes saving the same bytes get the same identifier unless the caller supplies
/// [`SaveOptions::id_entropy`]; hosts that need identifiers unique across sessions must do so.
/// The identifier only has to be unique, not unpredictable.
fn regenerate_id(trailer: &mut Dict, opts: &SaveOptions, written: &[u8], keep_first: bool) {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let state = std::collections::hash_map::RandomState::new();
    let mut id = Vec::with_capacity(16);
    for lane in 0u8..2 {
        let mut h = state.build_hasher();
        h.write_u64(count);
        h.write_u64(opts.id_seed);
        h.write_u8(lane);
        if let Some(e) = &opts.id_entropy {
            h.write(e);
        }
        h.write_usize(written.len());
        h.write(written.get(..written.len().min(1 << 16)).unwrap_or_default());
        id.extend_from_slice(&h.finish().to_be_bytes());
    }
    let new = Object::String(PdfString { bytes: id, hex: true });
    let first = match trailer.get(b"ID") {
        Some(Object::Array(a)) if keep_first && a.len() == 2 => a.first().cloned().unwrap_or_else(|| new.clone()),
        _ => new.clone(),
    };
    trailer.set(b"ID".to_vec(), Object::Array(vec![first, new]));
}

/// A PDF date string for "now" given seconds since the Unix epoch (UTC).
pub fn pdf_date(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs = unix_secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::Lexer;

    fn roundtrip(o: &Object) -> Object {
        let mut out = Vec::new();
        serialize(o, &mut out);
        Lexer::new(&out, 0).object().expect("parses")
    }

    #[test]
    fn a_second_read_that_differs_fails_the_save_instead_of_writing_null() {
        let mut doc = Document::new_empty();
        let dict = doc.add(Object::Dict(Dict::new()));
        let mut reader = doc.object_reader();
        assert!(reread(&mut reader, dict, false).is_ok());
        assert!(reread(&mut reader, dict, true).is_err(), "a dictionary where a stream was traversed");
        assert!(reread(&mut reader, ObjRef::new(9999, 0), false).is_err(), "nothing where an object was traversed");
    }

    #[test]
    fn xref_predictor_rows_preserve_variable_width_fields_and_sparse_runs() {
        for offset in [0, 255, 256, 65535, 65536, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
            for index in [0, 255, 256, u32::MAX] {
                for incremental in [false, true] {
                    let mut trailer = Dict::new();
                    if incremental {
                        trailer.set(b"Prev".to_vec(), Object::Int(17));
                    }
                    let rows = BTreeMap::from([(1, Row::InFile(offset, 32768)), (2, Row::Free(65535)), (4, Row::InStream(65536, index))]);
                    let mut bytes = Vec::new();
                    write_xref_stream(&mut bytes, 0, &mut trailer, rows, 5);
                    let (_, object) = crate::parser::parse_indirect(&bytes, 0, &|_| None).unwrap();
                    let Object::Stream(stream) = object else { panic!("xref stream") };
                    let widths: Vec<_> = stream.dict.get(b"W").unwrap().as_array().unwrap().iter().map(|o| o.as_int().unwrap() as usize).collect();
                    assert_eq!(widths[0], 1);
                    assert!((3..=8).contains(&widths[1]));
                    assert!((2..=4).contains(&widths[2]));
                    let runs: Vec<_> = stream.dict.get(b"Index").unwrap().as_array().unwrap().iter().map(|o| o.as_int().unwrap()).collect();
                    assert_eq!(runs, if incremental { vec![1, 2, 4, 2] } else { vec![0, 3, 4, 2] });
                    let mut expected = vec![(1, offset, 32768), (0, 0, 65535), (2, 65536, u64::from(index)), (1, 0, 0)];
                    if !incremental {
                        expected.insert(0, (0, 0, 65535));
                    }
                    let decoded = stream.decoded().unwrap();
                    let columns: usize = widths.iter().sum();
                    assert_eq!(decoded.len(), columns * expected.len());
                    for (row, (kind, first, second)) in decoded.chunks_exact(columns).zip(expected) {
                        let values: Vec<_> = [0..1, 1..1 + widths[1], 1 + widths[1]..columns]
                            .into_iter()
                            .map(|range| row[range].iter().fold(0u64, |value, byte| value * 256 + u64::from(*byte)))
                            .collect();
                        assert_eq!(values, vec![kind, first, second]);
                    }
                }
            }
        }
    }

    #[test]
    fn reals_keep_full_precision_without_exponents() {
        for r in [0.1, -0.000_000_123_456_789, 1e-12, 123_456.789_012_345, 1.0 / 3.0, -2.5e7 + 0.25] {
            let mut out = Vec::new();
            serialize(&Object::Real(r), &mut out);
            assert!(!out.contains(&b'e') && !out.contains(&b'E'), "{}", String::from_utf8_lossy(&out));
            assert_eq!(roundtrip(&Object::Real(r)), Object::Real(r));
        }
    }

    proptest! {
        #[test]
        fn any_finite_real_round_trips(r in proptest::num::f64::NORMAL | proptest::num::f64::SUBNORMAL | proptest::num::f64::ZERO) {
            prop_assume!(r.abs() < 1e15 || r.fract() == 0.0);
            let back = roundtrip(&Object::Real(r));
            match back {
                Object::Real(b) => prop_assert_eq!(b, r),
                Object::Int(i) => prop_assert_eq!(i as f64, r),
                other => prop_assert!(false, "unexpected {other:?}"),
            }
        }

        #[test]
        fn strings_and_names_round_trip(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let s = Object::String(PdfString { bytes: bytes.clone(), hex: false });
            prop_assert_eq!(roundtrip(&s).as_string().map(|s| s.bytes.clone()), Some(bytes.clone()));
            if !bytes.is_empty() && !bytes.contains(&0) {
                prop_assert_eq!(roundtrip(&Object::Name(bytes.clone())), Object::Name(bytes));
            }
        }
    }
}
