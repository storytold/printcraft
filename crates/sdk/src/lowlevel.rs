//! lowlevel.rs — Low-level PDF Objects (COS) domain models matching Datalogics APDFL.
//! =====================================================================================
//!
//! Purpose:
//!     Defines PDFObject, PDFDict, PDFArray, PDFStream, PDFString, PDFName, PDFInteger,
//!     PDFReal, PDFBoolean, NameTree, and NumberTree matching Datalogics APDFL Low-level Objects.
//!
//! Layer:
//!     Domain Layer / Low-level PDF Objects
//!
//! Key Types:
//!     - PDFObject: Strongly typed enum representing any COS object
//!     - PDFDict / PDFArray / PDFStream: Composite COS data structures
//!     - NameTree / NumberTree: Hierarchical indexing structures for PDF resources

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Indirect object reference identifying an object in a PDF document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PDFReference {
    pub obj_number: u32,
    pub gen_number: u16,
}

impl PDFReference {
    pub fn new(obj_number: u32, gen_number: u16) -> Self {
        Self { obj_number, gen_number }
    }
}

/// Primitive and composite low-level COS PDF Objects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PDFObject {
    Null,
    Boolean(bool),
    Integer(i64),
    Real(f64),
    String(String),
    Name(String),
    Array(Vec<PDFObject>),
    Dict(BTreeMap<String, PDFObject>),
    Stream { dict: BTreeMap<String, PDFObject>, data: Vec<u8> },
    Reference(PDFReference),
}

impl PDFObject {
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Real(r) => Some(*r),
            Self::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) | Self::Name(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<String, PDFObject>> {
        match self {
            Self::Dict(d) | Self::Stream { dict: d, .. } => Some(d),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[PDFObject]> {
        match self {
            Self::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }
}

/// Hierarchical NameTree structure used for named destinations, embedded files, etc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NameTree {
    pub entries: BTreeMap<String, PDFObject>,
}

impl NameTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lookup(&self, key: &str) -> Option<&PDFObject> {
        self.entries.get(key)
    }

    pub fn insert(&mut self, key: impl Into<String>, value: PDFObject) {
        self.entries.insert(key.into(), value);
    }
}

/// Hierarchical NumberTree structure used for page labeling, structure tree, etc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NumberTree {
    pub entries: BTreeMap<i64, PDFObject>,
}

impl NumberTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lookup(&self, key: i64) -> Option<&PDFObject> {
        self.entries.get(&key)
    }

    pub fn insert(&mut self, key: i64, value: PDFObject) {
        self.entries.insert(key, value);
    }
}
