//! pdfcraft-fonts — font metrics and encodings for generated appearances (L2).
//!
//! See the README: the metrics are approximations by character class (no vendor metrics files
//! are bundled). The full font subsystem lands in M2.2/M7.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod arabic;
mod craft;
mod encodings;
pub mod pdf;
mod script;
pub use arabic::{ShapedCluster, arabic_glyph, arabic_has, shape_arabic};
pub use craft::{
    CRAFT_FONTS, CraftFont, SHIPPORI_MINCHO, document_arabic_font, document_japanese_font, document_japanese_font_for_style,
    document_japanese_fonts_for_style, ui_arabic_fonts, ui_chinese_fonts, ui_cjk_fonts, ui_japanese_fonts, ui_telugu_fonts,
};
pub use script::{GlyphError, GlyphOutline, MAX_SIGNATURE_CHARS, ScriptOutline, japanese_glyph, japanese_glyph_from, script_outline};

/// Embedded Latin UI faces from `assets/fonts/`, also used as renderer fallbacks.
pub const INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
pub const INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
pub const JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

/// Approximate advance of `s` in Helvetica (or Arial) at `size` points.
pub fn helvetica_width(s: &str, size: f64) -> f64 {
    let units: f64 = s
        .chars()
        .map(|c| match c {
            ' ' | 'i' | 'j' | 'l' | '\'' | '!' | '|' | '.' | ',' | ':' | ';' | 'I' => 260.0,
            'f' | 't' | 'r' | '(' | ')' | '[' | ']' | '/' | '-' | '"' => 333.0,
            'm' => 833.0,
            'w' => 722.0,
            'M' => 833.0,
            'W' => 944.0,
            'J' | 'c' | 'k' | 's' | 'v' | 'x' | 'y' | 'z' => 500.0,
            '0'..='9' | 'a'..='z' | '$' | '#' | '?' | '_' => 556.0,
            'A'..='Z' => 680.0,
            '@' => 1015.0,
            _ if c.is_whitespace() => 260.0,
            _ => 584.0,
        })
        .sum();
    units * size / 1000.0
}

/// Approximate advance of `s` at `size` points in a CJK font: full-width characters take one
/// em, half-width forms half of one, and everything else is measured as Helvetica.
pub fn cjk_width(s: &str, size: f64) -> f64 {
    let mut buf = [0u8; 4];
    s.chars()
        .map(|c| match c {
            '\u{ff61}'..='\u{ffdc}' | '\u{ffe8}'..='\u{ffee}' => size * 0.5,
            _ if is_wide(c) => size,
            _ => helvetica_width(c.encode_utf8(&mut buf), size),
        })
        .sum()
}

/// East Asian wide and full-width characters (Unicode UAX #11 W and F, by block).
fn is_wide(c: char) -> bool {
    matches!(c,
        '\u{1100}'..='\u{115f}' // Hangul Jamo initials
        | '\u{2e80}'..='\u{303e}' // CJK radicals, Kangxi, ideographic description, CJK symbols
        | '\u{3041}'..='\u{33ff}' // kana, Bopomofo, Hangul compatibility, Kanbun, CJK compatibility
        | '\u{3400}'..='\u{4dbf}' // CJK extension A
        | '\u{4e00}'..='\u{9fff}' // CJK unified ideographs
        | '\u{a000}'..='\u{a4cf}' // Yi
        | '\u{ac00}'..='\u{d7a3}' // Hangul syllables
        | '\u{f900}'..='\u{faff}' // CJK compatibility ideographs
        | '\u{fe30}'..='\u{fe4f}' // CJK compatibility forms
        | '\u{ff00}'..='\u{ff60}' // full-width forms
        | '\u{ffe0}'..='\u{ffe6}'
        | '\u{20000}'..='\u{3fffd}' // supplementary ideographic planes
    )
}

/// Greedy line breaking within `width` points (paragraphs split on newlines; words longer
/// than a line are broken by character).
pub fn wrap(text: &str, size: f64, width: f64) -> Vec<String> {
    wrap_with(text, size, width, helvetica_width)
}

/// [`wrap`], measuring with `measure` (for example [`cjk_width`]).
pub fn wrap_with(text: &str, size: f64, width: f64, measure: impl Fn(&str, f64) -> f64) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split(['\n', '\r']) {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if measure(&candidate, size) <= width || line.is_empty() && measure(word, size) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            for ch in word.chars() {
                if !line.is_empty() && measure(&format!("{line}{ch}"), size) > width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        lines.push(line);
    }
    lines
}

/// The WinAnsiEncoding (ISO 32000-2 Annex D) code for `c`, if the encoding has one.
///
/// Codes 0x80–0x9F come from the same table the text extractor decodes with, so encoding and
/// decoding agree (Š š Ž ž Œ œ Ÿ ƒ † ‡ ˆ ˜ ‰ ‹ › as well as quotes, dashes, € and ™).
pub fn win_ansi_byte(c: char) -> Option<u8> {
    match c {
        '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => u8::try_from(u32::from(c)).ok(),
        _ => {
            let high = encodings::WIN_ANSI.get(0x80..0xa0)?;
            let i = high.iter().position(|&u| u != 0 && u == u32::from(c))?;
            u8::try_from(0x80 + i).ok()
        }
    }
}

/// Encode text in WinAnsiEncoding (ISO 32000-2 Annex D); tabs become spaces and unmappable
/// characters become `?`.
pub fn win_ansi(s: &str) -> Vec<u8> {
    s.chars().map(|c| if c == '\t' { b' ' } else { win_ansi_byte(c).unwrap_or(b'?') }).collect()
}

/// The character encoding of a predefined Unicode CMap (`UniJIS-UTF16-H`, `UniGB-UCS2-H`, …;
/// ISO 32000-2 §9.7.5.2), which maps Unicode text straight to codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnicodeCMap {
    Ucs2,
    Utf16,
    Utf8,
    Utf32,
}

impl UnicodeCMap {
    /// The encoding of the predefined CMap `name`, if it is a horizontal Unicode one. Vertical
    /// (`-V`) CMaps are refused: field appearances are laid out horizontally, so their glyphs
    /// would be drawn stacked down the field instead of across it.
    pub fn from_name(name: &[u8]) -> Option<Self> {
        let name = std::str::from_utf8(name).ok()?;
        if !name.starts_with("Uni") || !name.ends_with("-H") {
            return None;
        }
        name.split('-').find_map(|part| match part {
            "UCS2" => Some(Self::Ucs2),
            "UTF16" => Some(Self::Utf16),
            "UTF8" => Some(Self::Utf8),
            "UTF32" => Some(Self::Utf32),
            _ => None,
        })
    }

    /// Encode `s`; tabs become spaces, and characters UCS-2 can't hold become `?`.
    pub fn encode(self, s: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(s.len().saturating_mul(2));
        let mut units = [0u16; 2];
        let mut buf = [0u8; 4];
        for c in s.chars() {
            let c = if c == '\t' { ' ' } else { c };
            match self {
                Self::Ucs2 => {
                    let unit = u16::try_from(u32::from(c)).unwrap_or(u16::from(b'?'));
                    out.extend_from_slice(&unit.to_be_bytes());
                }
                Self::Utf16 => {
                    for unit in c.encode_utf16(&mut units) {
                        out.extend_from_slice(&unit.to_be_bytes());
                    }
                }
                Self::Utf8 => out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes()),
                Self::Utf32 => out.extend_from_slice(&u32::from(c).to_be_bytes()),
            }
        }
        out
    }
}

/// Whether [`win_ansi`] keeps `c` rather than writing `?` (a tab becomes a space).
pub fn win_ansi_encodable(c: char) -> bool {
    c == '\t' || win_ansi_byte(c).is_some()
}

/// The first character of `s` that [`win_ansi`] can't encode and would write as `?`.
pub fn first_non_win_ansi(s: &str) -> Option<char> {
    s.chars().find(|c| !win_ansi_encodable(*c))
}

/// Returns the Adobe Glyph List name for a Unicode character if known.
pub fn unicode_to_adobe_glyph_name(c: char) -> Option<&'static str> {
    match c {
        // Romanian diacritics
        'ă' => Some("abreve"),
        'Ă' => Some("Abreve"),
        'ș' => Some("scommaaccent"),
        'Ș' => Some("Scommaaccent"),
        'ț' => Some("tcommaaccent"),
        'Ț' => Some("Tcommaaccent"),
        'ş' => Some("scedilla"),
        'Ş' => Some("Scedilla"),
        'ţ' => Some("tcedilla"),
        'Ţ' => Some("Tcedilla"),

        // Central & Eastern European, Baltic, Turkish
        'ā' => Some("amacron"),
        'Ā' => Some("Amacron"),
        'ą' => Some("aogonek"),
        'Ą' => Some("Aogonek"),
        'ć' => Some("cacute"),
        'Ć' => Some("Cacute"),
        'č' => Some("ccaron"),
        'Č' => Some("Ccaron"),
        'ď' => Some("dcaron"),
        'Ď' => Some("Dcaron"),
        'đ' => Some("dcroat"),
        'Đ' => Some("Dcroat"),
        'ē' => Some("emacron"),
        'Ē' => Some("Emacron"),
        'ė' => Some("edotaccent"),
        'Ė' => Some("Edotaccent"),
        'ę' => Some("eogonek"),
        'Ę' => Some("Eogonek"),
        'ě' => Some("ecaron"),
        'Ě' => Some("Ecaron"),
        'ğ' => Some("gbreve"),
        'Ğ' => Some("Gbreve"),
        'ģ' => Some("gcommaaccent"),
        'Ģ' => Some("Gcommaaccent"),
        'ī' => Some("imacron"),
        'Ī' => Some("Imacron"),
        'ı' => Some("dotlessi"),
        'İ' => Some("Idotaccent"),
        'į' => Some("iogonek"),
        'Į' => Some("Iogonek"),
        'ķ' => Some("kcommaaccent"),
        'Ķ' => Some("Kcommaaccent"),
        'ĺ' => Some("lacute"),
        'Ĺ' => Some("Lacute"),
        'ľ' => Some("lcaron"),
        'Ľ' => Some("Lcaron"),
        'ļ' => Some("lcommaaccent"),
        'Ļ' => Some("Lcommaaccent"),
        'ł' => Some("lslash"),
        'Ł' => Some("Lslash"),
        'ń' => Some("nacute"),
        'Ń' => Some("Nacute"),
        'ň' => Some("ncaron"),
        'Ň' => Some("Ncaron"),
        'ņ' => Some("ncommaaccent"),
        'Ņ' => Some("Ncommaaccent"),
        'ō' => Some("omacron"),
        'Ō' => Some("Omacron"),
        'ő' => Some("ohungarumlaut"),
        'Ő' => Some("Ohungarumlaut"),
        'ŕ' => Some("racute"),
        'Ŕ' => Some("Racute"),
        'ř' => Some("rcaron"),
        'Ř' => Some("Rcaron"),
        'ŗ' => Some("rcommaaccent"),
        'Ŗ' => Some("Rcommaaccent"),
        'ś' => Some("sacute"),
        'Ś' => Some("Sacute"),
        'ť' => Some("tcaron"),
        'Ť' => Some("Tcaron"),
        'ū' => Some("umacron"),
        'Ū' => Some("Umacron"),
        'ů' => Some("uring"),
        'Ů' => Some("Uring"),
        'ű' => Some("uhungarumlaut"),
        'Ű' => Some("Uhungarumlaut"),
        'ų' => Some("uogonek"),
        'Ų' => Some("Uogonek"),
        'ź' => Some("zacute"),
        'Ź' => Some("Zacute"),
        'ż' => Some("zdotaccent"),
        'Ż' => Some("Zdotaccent"),
        _ => None,
    }
}

/// Returns the Unicode character for an extended Adobe Glyph List name if known.
pub fn adobe_glyph_name_to_unicode(name: &str) -> Option<char> {
    match name {
        "abreve" => Some('ă'),
        "Abreve" => Some('Ă'),
        "scommaaccent" => Some('ș'),
        "Scommaaccent" => Some('Ș'),
        "tcommaaccent" => Some('ț'),
        "Tcommaaccent" => Some('Ț'),
        "scedilla" => Some('ş'),
        "Scedilla" => Some('Ş'),
        "tcedilla" => Some('ţ'),
        "Tcedilla" => Some('Ţ'),
        "amacron" => Some('ā'),
        "Amacron" => Some('Ā'),
        "aogonek" => Some('ą'),
        "Aogonek" => Some('Ą'),
        "cacute" => Some('ć'),
        "Cacute" => Some('Ć'),
        "ccaron" => Some('č'),
        "Ccaron" => Some('Č'),
        "dcaron" => Some('ď'),
        "Dcaron" => Some('Ď'),
        "dcroat" => Some('đ'),
        "Dcroat" => Some('Đ'),
        "emacron" => Some('ē'),
        "Emacron" => Some('Ē'),
        "edotaccent" => Some('ė'),
        "Edotaccent" => Some('Ė'),
        "eogonek" => Some('ę'),
        "Eogonek" => Some('Ę'),
        "ecaron" => Some('ě'),
        "Ecaron" => Some('Ě'),
        "gbreve" => Some('ğ'),
        "Gbreve" => Some('Ğ'),
        "gcommaaccent" => Some('ģ'),
        "Gcommaaccent" => Some('Ģ'),
        "imacron" => Some('ī'),
        "Imacron" => Some('Ī'),
        "dotlessi" => Some('ı'),
        "Idotaccent" => Some('İ'),
        "iogonek" => Some('į'),
        "Iogonek" => Some('Į'),
        "kcommaaccent" => Some('ķ'),
        "Kcommaaccent" => Some('Ķ'),
        "lacute" => Some('ĺ'),
        "Lacute" => Some('Ĺ'),
        "lcaron" => Some('ľ'),
        "Lcaron" => Some('Ľ'),
        "lcommaaccent" => Some('ļ'),
        "Lcommaaccent" => Some('Ļ'),
        "lslash" => Some('ł'),
        "Lslash" => Some('Ł'),
        "nacute" => Some('ń'),
        "Nacute" => Some('Ń'),
        "ncaron" => Some('ň'),
        "Ncaron" => Some('Ň'),
        "ncommaaccent" => Some('ņ'),
        "Ncommaaccent" => Some('Ņ'),
        "omacron" => Some('ō'),
        "Omacron" => Some('Ō'),
        "ohungarumlaut" => Some('ő'),
        "Ohungarumlaut" => Some('Ő'),
        "racute" => Some('ŕ'),
        "Racute" => Some('Ŕ'),
        "rcaron" => Some('ř'),
        "Rcaron" => Some('Ř'),
        "rcommaaccent" => Some('ŗ'),
        "Rcommaaccent" => Some('Ŗ'),
        "sacute" => Some('ś'),
        "Sacute" => Some('Ś'),
        "tcaron" => Some('ť'),
        "Tcaron" => Some('Ť'),
        "umacron" => Some('ū'),
        "Umacron" => Some('Ū'),
        "uring" => Some('ů'),
        "Uring" => Some('Ů'),
        "uhungarumlaut" => Some('ű'),
        "Uhungarumlaut" => Some('Ű'),
        "uogonek" => Some('ų'),
        "Uogonek" => Some('Ų'),
        "zacute" => Some('ź'),
        "Zacute" => Some('Ź'),
        "zdotaccent" => Some('ż'),
        "Zdotaccent" => Some('Ż'),
        _ => None,
    }
}

/// Bytes as a PDF literal string, `(` … `)`, with delimiters escaped.
pub fn literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'(');
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', b]),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\n' => out.extend_from_slice(b"\\n"),
            1..=31 => {
                let mut buf = *b"\\000";
                buf[1] = b'0' + (b >> 6);
                buf[2] = b'0' + ((b >> 3) & 7);
                buf[3] = b'0' + (b & 7);
                out.extend_from_slice(&buf);
            }
            _ => out.push(b),
        }
    }
    out.push(b')');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_ansi_encodability_matches_the_encoder() {
        for c in ['a', 'é', 'ÿ', '€', '—', '™', '\t', '?'] {
            assert!(win_ansi_encodable(c), "{c:?}");
        }
        for c in ['中', 'あ', 'Ω', 'č', 'ă', '☃', '😀'] {
            assert!(!win_ansi_encodable(c), "{c:?}");
        }
        assert_eq!(first_non_win_ansi("Café — 5€"), None);
        assert_eq!(first_non_win_ansi("Dvořák"), Some('ř'));
    }

    #[test]
    fn widths_wrap_and_encode() {
        assert!(helvetica_width("MMMM", 10.0) > helvetica_width("iiii", 10.0) * 2.0);
        assert_eq!(helvetica_width("", 12.0), 0.0);
        let lines = wrap("the quick brown fox jumps over the lazy dog", 12.0, 80.0);
        assert!(lines.len() > 2 && lines.iter().all(|l| helvetica_width(l, 12.0) <= 80.0));
        assert_eq!(wrap("a\nb", 12.0, 100.0), ["a", "b"]);
        let long = wrap("Supercalifragilisticexpialidocious", 12.0, 40.0);
        assert!(long.len() > 3 && long.concat() == "Supercalifragilisticexpialidocious");
        assert_eq!(win_ansi("Café — 5€ ☃"), b"Caf\xe9 \x97 5\x80 ?");
        assert_eq!(literal(b"a(b)\\c"), b"(a\\(b\\)\\\\c)");
    }

    #[test]
    fn win_ansi_covers_the_0x80_to_0x9f_glyphs() {
        // Every character in the WinAnsiEncoding table encodes to its code and back (#347).
        for (code, &u) in encodings::WIN_ANSI.iter().enumerate() {
            let Some(c) = char::from_u32(u).filter(|_| u != 0) else { continue };
            assert_eq!(win_ansi_byte(c), Some(code as u8), "{c:?} (U+{u:04X})");
            assert_eq!(win_ansi(&c.to_string()), [code as u8]);
        }
        assert_eq!(
            win_ansi("Šta? žaba, œuvre — „ok“ ‰ ƒ † ‡ ˆ ˜ ‹›Ÿ"),
            b"\x8ata? \x9eaba, \x9cuvre \x97 \x84ok\x93 \x89 \x83 \x86 \x87 \x88 \x98 \x8b\x9b\x9f"
        );
        // Not in WinAnsi: č ć still need the fallback font; undefined codes are never produced.
        assert_eq!(win_ansi("čć\u{81}\u{8d}\t"), b"???? ");
        assert_eq!(win_ansi_byte('\0'), None);
    }

    #[test]
    fn unicode_cmaps_encode_cjk_text() {
        assert_eq!(UnicodeCMap::from_name(b"UniJIS-UTF16-H"), Some(UnicodeCMap::Utf16));
        assert_eq!(UnicodeCMap::from_name(b"UniGB-UCS2-V"), None);
        assert_eq!(UnicodeCMap::from_name(b"UniJIS-UCS2-HW-H"), Some(UnicodeCMap::Ucs2));
        assert_eq!(UnicodeCMap::from_name(b"UniKS-UTF8-H"), Some(UnicodeCMap::Utf8));
        assert_eq!(UnicodeCMap::from_name(b"UniJIS2004-UTF32-H"), Some(UnicodeCMap::Utf32));
        assert_eq!(UnicodeCMap::from_name(b"Identity-H"), None);
        assert_eq!(UnicodeCMap::from_name(b"90ms-RKSJ-H"), None);
        assert_eq!(UnicodeCMap::from_name(b"\xff\xfe"), None);
        assert_eq!(UnicodeCMap::Utf16.encode("令A"), [0x4e, 0xe4, 0x00, 0x41]);
        assert_eq!(UnicodeCMap::Utf16.encode("𠀋"), [0xd8, 0x40, 0xdc, 0x0b]);
        assert_eq!(UnicodeCMap::Ucs2.encode("𠀋\t"), [0x00, b'?', 0x00, b' ']);
        assert_eq!(UnicodeCMap::Utf8.encode("令"), "令".as_bytes());
        assert_eq!(UnicodeCMap::Utf32.encode("令"), [0, 0, 0x4e, 0xe4]);
        assert_eq!(cjk_width("令和", 10.0), 20.0);
        assert_eq!(cjk_width("ｱ", 10.0), 5.0);
        assert_eq!(cjk_width("A", 10.0), helvetica_width("A", 10.0));
        let lines = wrap_with("日本語のテキストは文字単位で折り返されます", 10.0, 45.0, cjk_width);
        assert!(lines.len() > 3 && lines.iter().all(|l| cjk_width(l, 10.0) <= 45.0), "{lines:?}");
    }
}
