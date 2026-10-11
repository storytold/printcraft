//! text.rs — Text & Fonts domain models matching Datalogics APDFL architecture.
//! =================================================================================
//!
//! Purpose:
//!     Defines Font, Text, TextRun, TextState, Word, WordFinder, and DocTextFinder
//!     matching Datalogics APDFL Text & Fonts specifications.
//!
//! Layer:
//!     Domain Layer / Text & Fonts
//!
//! Key Types:
//!     - Font: Typeface definition, base name, encoding, and embedding flags
//!     - TextRun: Contiguous span of text rendered with uniform font and styling
//!     - TextState: Font spacing, scaling, and character geometry
//!     - Text: Composite text element consisting of multiple text runs
//!     - Word: Extracted word token with spatial bounding box and quads
//!     - DocTextFinderMatch: Match result from searching text across pages

use crate::graphics::{Color, Matrix, Quad, Rect};
use serde::{Deserialize, Serialize};

/// Font specification and metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Font {
    pub name: String,
    pub base_font: String,
    pub is_embedded: bool,
    pub is_subset: bool,
}

impl Font {
    pub fn standard(name: impl Into<String>) -> Self {
        let n = name.into();
        Self { base_font: n.clone(), name: n, is_embedded: false, is_subset: false }
    }
}

/// Text layout state parameters (spacing, horizontal scale, leading).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextState {
    pub char_spacing: f64,
    pub word_spacing: f64,
    pub horizontal_scale: f64,
    pub leading: f64,
}

impl Default for TextState {
    fn default() -> Self {
        Self { char_spacing: 0.0, word_spacing: 0.0, horizontal_scale: 100.0, leading: 0.0 }
    }
}

/// A contiguous slice of text characters sharing uniform font and styling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
    pub text: String,
    pub font: Font,
    pub font_size: f64,
    pub matrix: Matrix,
    pub color: Color,
}

impl TextRun {
    pub fn new(text: impl Into<String>, font: Font, font_size: f64) -> Self {
        Self { text: text.into(), font, font_size, matrix: Matrix::IDENTITY, color: Color::BLACK }
    }
}

/// Composite text element containing one or more text runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Text {
    pub runs: Vec<TextRun>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_run(&mut self, run: TextRun) {
        self.runs.push(run);
    }
}

/// Visual typography style used within a word, defining font, size, and color.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub color: Color,
    pub font_name: String,
    pub font_size: f64,
}

impl Style {
    pub fn new(font_name: impl Into<String>, font_size: f64, color: Color) -> Self {
        Self { font_name: font_name.into(), font_size, color }
    }
}

impl std::fmt::Display for Style {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[color={:?}, fontsize={}, fontname={}]", self.color, self.font_size, self.font_name)
    }
}

/// Represents a typographical style transition occurring at a specific character offset in a word.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StyleTransition {
    pub char_index: usize,
    pub style: Style,
}

impl StyleTransition {
    pub fn new(char_index: usize, style: Style) -> Self {
        Self { char_index, style }
    }
}

/// Extracted word token with geometrical bounding coordinates and typographical styles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub bounding_box: Rect,
    pub quads: Vec<Quad>,
    pub styles: Vec<StyleTransition>,
}

impl Word {
    pub fn new(text: impl Into<String>, bounding_box: Rect, quads: Vec<Quad>) -> Self {
        Self { text: text.into(), bounding_box, quads, styles: Vec::new() }
    }
}

/// Configuration settings for the WordFinder algorithm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WordFinderConfig {
    pub preserve_ligatures: bool,
    pub precise_quads: bool,
    pub decompose_hyphenated_words: bool,
}

/// Configuration settings for text pattern and keyword search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DocTextFinderConfig {
    pub case_sensitive: bool,
    pub whole_words_only: bool,
    pub regex: bool,
}

/// Search hit result produced by document text search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocTextFinderMatch {
    pub page_number: usize,
    pub matched_text: String,
    pub bounding_box: Rect,
    pub quads: Vec<Quad>,
}
