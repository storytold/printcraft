//! Read-only structure inspection backed by the lazy COS reader. Keep the existing
//! inspector's lopdf object vocabulary while loading only the objects its queries use.

use std::collections::{BTreeMap, HashSet};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

use lopdf::{Dictionary, Error, Object, ObjectId, Result};
use pdfcraft_cos::{Document, Object as CosObject};

pub(crate) trait Structure {
    fn trailer(&self) -> &Dictionary;
    fn get_object(&self, id: ObjectId) -> Result<&Object>;
    fn encrypted(&self) -> bool;

    fn dereference<'a>(&'a self, mut object: &'a Object) -> Result<(Option<ObjectId>, &'a Object)> {
        let mut id = None;
        for _ in 0..128 {
            match object {
                Object::Reference(reference) => {
                    id = Some(*reference);
                    object = self.get_object(*reference)?;
                }
                _ => return Ok((id, object)),
            }
        }
        Err(Error::ReferenceLimit)
    }

    fn get_dictionary(&self, id: ObjectId) -> Result<&Dictionary> {
        self.get_object(id).and_then(|o| self.dereference(o)).and_then(|(_, o)| o.as_dict())
    }

    fn catalog(&self) -> Result<&Dictionary> {
        self.trailer().get(b"Root").and_then(Object::as_reference).and_then(|id| self.get_dictionary(id))
    }

    fn get_pages(&self) -> BTreeMap<u32, ObjectId> {
        let mut pages = BTreeMap::new();
        let Ok(root) = self.catalog().and_then(|c| c.get(b"Pages")).and_then(Object::as_reference) else { return pages };
        let mut pending = vec![(root, 0usize)];
        let mut seen = HashSet::new();
        while let Some((id, depth)) = pending.pop() {
            if depth > 256 || !seen.insert(id) {
                continue;
            }
            let Ok(dict) = self.get_dictionary(id) else { continue };
            match dict.get_type() {
                Ok(b"Page") => {
                    // Page numbers are 1-based u32s; a tree past u32::MAX pages stops there.
                    let Some(number) = u32::try_from(pages.len()).ok().and_then(|n| n.checked_add(1)) else { return pages };
                    pages.insert(number, id);
                }
                Ok(b"Pages") => {
                    if let Ok(kids) = dict.get(b"Kids").and_then(|o| self.dereference(o)).and_then(|(_, o)| o.as_array()) {
                        pending.extend(kids.iter().rev().filter_map(|k| k.as_reference().ok()).map(|id| (id, depth + 1)));
                    }
                }
                _ => {}
            }
        }
        pages
    }

    // Match lopdf's font-resource precedence: a page's direct resources, then its
    // indirect resources and ancestors. Stop cycles and excessive inheritance depth.
    fn get_page_fonts(&self, page: ObjectId) -> Result<BTreeMap<Vec<u8>, &Dictionary>> {
        let mut resources = Vec::new();
        let mut current = Some(page);
        let mut seen = HashSet::new();
        while let Some(id) = current {
            if !seen.insert(id) {
                return Err(Error::ReferenceCycle(id));
            }
            if seen.len() > 100 {
                return Err(Error::RecursionLimit);
            }
            let dict = self.get_dictionary(id)?;
            if let Ok(resource) = dict.get(b"Resources") {
                match resource {
                    Object::Dictionary(dict) if id == page => resources.push(dict),
                    Object::Reference(id) => {
                        if let Ok(dict) = self.get_dictionary(*id) {
                            resources.push(dict);
                        }
                    }
                    _ => {}
                }
            }
            current = dict.get(b"Parent").and_then(Object::as_reference).ok();
        }
        let mut fonts = BTreeMap::new();
        for resources in resources {
            if let Ok(dict) = resources.get(b"Font").and_then(|o| self.dereference(o)).and_then(|(_, o)| o.as_dict()) {
                for (name, font) in dict.iter() {
                    if let Ok(font) = self.dereference(font).and_then(|(_, o)| o.as_dict()) {
                        fonts.entry(name.clone()).or_insert(font);
                    }
                }
            }
        }
        Ok(fonts)
    }
}

impl Structure for lopdf::Document {
    fn trailer(&self) -> &Dictionary {
        &self.trailer
    }
    fn get_object(&self, id: ObjectId) -> Result<&Object> {
        self.get_object(id)
    }
    fn encrypted(&self) -> bool {
        self.is_encrypted() || self.was_encrypted() || self.trailer.has(b"Encrypt")
    }
    // `get_pages` is not overridden: lopdf's own page-tree walk trusts /Count and pre-allocates
    // by it, so a hostile /Count (a fuzz finding: 4294967295) aborted the process with a 48 GiB
    // allocation. The trait's bounded default walk counts the real pages instead.
    fn get_page_fonts(&self, page: ObjectId) -> Result<BTreeMap<Vec<u8>, &Dictionary>> {
        self.get_page_fonts(page)
    }
}

type Slot = (u32, OnceLock<Option<Box<Object>>>);

pub(crate) struct LazyStructure {
    source: Document,
    trailer: Dictionary,
    // Sorted slots keep returned references stable, without allocating by the largest
    // input object number (which can be sparse or hostile). Only queried objects fill slots.
    objects: Vec<Slot>,
    failed: AtomicBool,
}

impl LazyStructure {
    pub(crate) fn new(bytes: Arc<Vec<u8>>, password: Option<&str>) -> std::result::Result<Self, String> {
        let source = Document::open_with_stream_limit(bytes, password, 256 << 20).map_err(|e| e.to_string())?;
        if !source.repair_log().is_empty() {
            return Err("the structure requires compatibility repair".into());
        }
        let trailer = convert_dict(source.trailer(), 0).map_err(|e| e.to_string())?;
        let objects = source.object_numbers().into_iter().map(|n| (n, OnceLock::new())).collect();
        Ok(Self { source, trailer, objects, failed: AtomicBool::new(false) })
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

impl Structure for LazyStructure {
    fn trailer(&self) -> &Dictionary {
        &self.trailer
    }
    fn encrypted(&self) -> bool {
        self.source.security().is_some() || self.trailer.has(b"Encrypt")
    }
    fn get_object(&self, id: ObjectId) -> Result<&Object> {
        let missing = || Error::ObjectNotFound(id);
        let slot = self.objects.binary_search_by_key(&id.0, |(n, _)| *n).ok().and_then(|i| self.objects.get(i)).ok_or_else(missing)?;
        if self.source.generation(id.0) != id.1 {
            return Err(missing());
        }
        slot.1
            .get_or_init(|| {
                let result = self.source.try_get(id.0).ok().and_then(|o| convert(&o, 0).ok()).map(Box::new);
                if result.is_none() {
                    self.failed.store(true, Ordering::Relaxed);
                }
                result
            })
            .as_deref()
            .ok_or_else(missing)
    }
}

fn convert_dict(dict: &pdfcraft_cos::Dict, depth: usize) -> Result<Dictionary> {
    if depth > 100 {
        return Err(Error::RecursionLimit);
    }
    dict.iter().map(|(key, value)| Ok((key.clone(), convert(value, depth + 1)?))).collect::<Result<Vec<_>>>().map(|items| items.into_iter().collect())
}

fn convert(object: &CosObject, depth: usize) -> Result<Object> {
    if depth > 100 {
        return Err(Error::RecursionLimit);
    }
    Ok(match object {
        CosObject::Null => Object::Null,
        CosObject::Bool(value) => Object::Boolean(*value),
        CosObject::Int(value) => Object::Integer(*value),
        CosObject::Real(value) => Object::Real(*value as f32),
        CosObject::Name(value) => Object::Name(value.clone()),
        CosObject::String(value) => {
            Object::String(value.bytes.clone(), if value.hex { lopdf::StringFormat::Hexadecimal } else { lopdf::StringFormat::Literal })
        }
        CosObject::Ref(value) => Object::Reference((value.num, value.generation)),
        CosObject::Array(value) => Object::Array(value.iter().map(|o| convert(o, depth + 1)).collect::<Result<_>>()?),
        CosObject::Dict(value) => Object::Dictionary(convert_dict(value, depth + 1)?),
        CosObject::Stream(value) => Object::Stream(lopdf::Stream::new(convert_dict(&value.dict, depth + 1)?, value.raw.to_vec())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hostile page-tree /Count must not abort the process: lopdf's own `get_pages`
    /// pre-allocates by it (fuzz finding: /Count 4294967295 -> a 48 GiB allocation),
    /// the bounded default walk counts the real pages instead.
    #[test]
    fn a_hostile_page_count_does_not_abort_inspection() {
        let pdf = b"%PDF-1.7
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 4294967295 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>
endobj
trailer
<< /Size 4 /Root 1 0 R >>
%%EOF";
        let doc = lopdf::Document::load_mem(&pdf[..]).unwrap();
        let pages = Structure::get_pages(&doc);
        assert_eq!(pages.len(), 1, "the walk counts the one real page, not /Count");
    }

    use super::*;

    #[test]
    fn inspection_loads_only_requested_objects_in_a_compressed_document() {
        let mut source = lopdf::Document::with_version("1.7");
        let catalog = source.add_object(lopdf::dictionary! { "Type" => "Catalog", "Pages" => (2, 0) });
        source.add_object(lopdf::dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference((3, 0))], "Count" => 1 });
        source
            .add_object(lopdf::dictionary! { "Type" => "Page", "Parent" => (2, 0), "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()] });
        source.trailer.set("Root", catalog);
        let unused: Vec<_> = (0..300).map(|i| Object::Reference(source.add_object(lopdf::dictionary! { "Unused" => i }))).collect();
        // Keep these objects reachable when the full writer removes orphan objects,
        // through a catalog extension that the panel inspector has no reason to read.
        source.get_dictionary_mut(catalog).unwrap().set("Uninspected", unused);
        let mut bytes = Vec::new();
        source.save_to(&mut bytes).unwrap();
        let cos = Document::open(Arc::new(bytes)).unwrap();
        let compressed = pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap();
        let expected = lopdf::Document::load_mem(&compressed).unwrap().get_pages();
        let view = LazyStructure::new(Arc::new(compressed), None).unwrap();
        assert_eq!(view.get_pages().len(), 1);
        assert_eq!(view.get_pages(), expected);
        assert!(!view.failed());
        assert!(
            view.objects.iter().filter(|(_, slot)| slot.get().is_some()).count() <= 3,
            "unused objects must not be materialized to inspect a page tree"
        );
        let unused = view.catalog().unwrap().get(b"Uninspected").unwrap().as_array().unwrap();
        let last = unused.last().unwrap().as_reference().unwrap();
        assert!(matches!(view.get_dictionary(last).unwrap().get(b"Unused"), Ok(Object::Integer(299))));
    }
}
