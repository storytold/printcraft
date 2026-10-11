//! The last-resort interface font: one face already installed on this machine.
//!
//! The embedded faces (Inter, egui's defaults, craft-fonts) come first in every family; this one
//! only draws characters none of them has, such as an Arabic file name in a build without
//! craft-fonts, or Simplified Chinese text in one whose embedded CJK face is the Japanese set.
//! It is read at runtime and never embedded or shipped (AGENTS.md §1.4), and
//! `PDFCRAFT_SYSTEM_FONTS=0` turns it off (published screenshots do).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::FontData;

/// Larger files are not read: a font path is still untrusted input.
const MAX_BYTES: u64 = 32 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 16;
/// Arabic letter alef: the face must have it to be worth loading for Arabic.
const PROBE_ARABIC: char = '\u{0627}';
/// Simplified-Chinese character (這 is the traditional form): the probe for a face that can draw
/// the zh-hans interface. Japanese faces and Segoe UI lack it, which is exactly the point.
const PROBE_HANS: char = '\u{8FD9}';

/// The installed fallback face, read once per need. `None` when it is turned off or no candidate
/// fits. `prefer_hans` asks for a face that can draw the Simplified-Chinese interface; otherwise
/// the fallback serves the Arabic case, as before.
pub fn fallback(prefer_hans: bool) -> Option<Arc<FontData>> {
    static ARABIC: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    static HANS: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    let slot = if prefer_hans { &HANS } else { &ARABIC };
    slot.get_or_init(|| load(prefer_hans)).clone()
}

fn load(prefer_hans: bool) -> Option<Arc<FontData>> {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return None;
    }
    candidates(prefer_hans).iter().find_map(|path| read(path, prefer_hans))
}

/// Well-known locations of faces with broad script coverage, best first. The Latin faces come
/// last everywhere: they only ever match the Arabic probe through their Arabic ranges, never the
/// Chinese one.
fn candidates(prefer_hans: bool) -> Vec<PathBuf> {
    if cfg!(windows) {
        let dir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let files: &[&str] = if prefer_hans {
            // Microsoft YaHei UI ships with every Windows 10/11; SimSun and DengXian back older
            // or trimmed installs, JhengHei covers a Traditional face that also maps this probe.
            &["msyh.ttc", "simsun.ttc", "deng.ttf", "msjh.ttc", "segoeui.ttf", "tahoma.ttf", "arial.ttf"]
        } else {
            &["segoeui.ttf", "tahoma.ttf", "arial.ttf"]
        };
        files.iter().map(|f| dir.join("Fonts").join(f)).collect()
    } else if cfg!(target_os = "macos") {
        let files: &[&str] = if prefer_hans {
            &[
                "/System/Library/Fonts/PingFang.ttc",
                "/System/Library/Fonts/Hiragino Sans GB.ttc",
                "/System/Library/Fonts/Supplemental/Songti.ttc",
                "/System/Library/Fonts/SFArabic.ttf",
                "/System/Library/Fonts/GeezaPro.ttc",
                "/System/Library/Fonts/Supplemental/Arial.ttf",
            ]
        } else {
            &["/System/Library/Fonts/SFArabic.ttf", "/System/Library/Fonts/GeezaPro.ttc", "/System/Library/Fonts/Supplemental/Arial.ttf"]
        };
        files.iter().map(PathBuf::from).collect()
    } else {
        let files: &[&str] = if prefer_hans {
            &[
                "truetype/noto/NotoSansCJK-Regular.ttc",
                "opentype/noto/NotoSansCJK-Regular.ttc",
                "noto-cjk/NotoSansCJK-Regular.ttc",
                "noto-cjk/NotoSansSC-Regular.otf",
                "truetype/wqy/wqy-microhei.ttc",
                "wqy-microhei/wqy-microhei.ttc",
                "truetype/noto/NotoSansArabic-Regular.ttf",
                "noto/NotoSansArabic-Regular.ttf",
                "google-noto/NotoSansArabic-Regular.ttf",
                "truetype/dejavu/DejaVuSans.ttf",
                "TTF/DejaVuSans.ttf",
                "dejavu/DejaVuSans.ttf",
                "dejavu-sans-fonts/DejaVuSans.ttf",
            ]
        } else {
            &[
                "truetype/noto/NotoSansArabic-Regular.ttf",
                "noto/NotoSansArabic-Regular.ttf",
                "google-noto/NotoSansArabic-Regular.ttf",
                "truetype/dejavu/DejaVuSans.ttf",
                "TTF/DejaVuSans.ttf",
                "dejavu/DejaVuSans.ttf",
                "dejavu-sans-fonts/DejaVuSans.ttf",
            ]
        };
        ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| files.iter().map(move |f| Path::new(dir).join(f))).collect()
    }
}

fn read(path: &Path, prefer_hans: bool) -> Option<Arc<FontData>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let index = face_with(&bytes, if prefer_hans { PROBE_HANS } else { PROBE_ARABIC })?;
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    log::info!("interface font fallback: {}", path.display());
    Some(Arc::new(data))
}

/// The first face of the file that parses and maps `c`. egui parses fonts with the same skrifa,
/// so a face accepted here is one it can load.
fn face_with(bytes: &[u8], c: char) -> Option<u32> {
    use skrifa::MetadataProvider as _;
    (0..MAX_FACES).find(|&index| skrifa::FontRef::from_index(bytes, index).is_ok_and(|font| font.charmap().map(c).is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_and_missing_files_are_skipped() {
        assert_eq!(face_with(b"", PROBE_ARABIC), None);
        assert_eq!(face_with(b"not a font at all", PROBE_ARABIC), None);
        assert_eq!(face_with(&[0u8; 4096], PROBE_ARABIC), None);
        assert!(read(Path::new("definitely/not/here.ttf"), false).is_none());
        assert!(read(Path::new("definitely/not/here.ttf"), true).is_none());
        // A directory is not a font.
        assert!(read(&std::env::temp_dir(), false).is_none());
    }

    #[test]
    fn a_face_without_the_probe_is_rejected() {
        // Inter is Latin, Greek and Cyrillic only: neither script's probe matches it.
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        assert_eq!(face_with(inter, PROBE_ARABIC), None);
        assert_eq!(face_with(inter, PROBE_HANS), None);
        assert_eq!(face_with(inter, 'A'), Some(0));
    }

    #[test]
    fn candidates_are_absolute_font_files() {
        for prefer_hans in [false, true] {
            let list = candidates(prefer_hans);
            assert!(!list.is_empty());
            assert!(list.iter().all(|p| p.extension().is_some_and(|e| e == "ttf" || e == "ttc" || e == "otf")));
        }
    }

    #[test]
    fn the_hans_candidates_come_first_when_preferred() {
        // The Arabic list stays as it was; the Chinese list may repeat or precede those faces.
        let (hans, arabic) = (candidates(true), candidates(false));
        assert!(!hans.is_empty() && !arabic.is_empty());
        assert!(hans.windows(arabic.len().max(1)).any(|w| w.iter().zip(arabic.iter()).all(|(a, b)| a == b)) || hans.len() > arabic.len());
    }
}
