//! `cargo xtask demo-pdf`: build the PdfCraft showcase PDF.
//!
//! 1. Render a procedural PNG and fill the placeholders in `assets/demo/showcase.html`.
//! 2. Print the HTML with headless Chrome (tagged PDF, outline from headings).
//! 3. Post-process with `lopdf`: turn marker links into annotations, append an
//!    AcroForm page, add layers, page labels, attachments, metadata and outline.

mod annots;
mod chrome;
mod form;
mod pdf;
mod postprocess;
mod raster;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const USAGE: &str = "Usage: cargo xtask demo-pdf [--chrome <path>] [--out <file.pdf>]";

pub fn run(args: &[String]) -> Result<()> {
    let mut chrome_override = None;
    let mut out_override = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--chrome" => chrome_override = Some(PathBuf::from(it.next().context(USAGE)?)),
            "--out" => out_override = Some(PathBuf::from(it.next().context(USAGE)?)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => bail!("unexpected argument `{other}`\n{USAGE}"),
        }
    }

    let root = repo_root();
    let out = out_override.unwrap_or_else(|| root.join("dist/demo/pdfcraft-showcase.pdf"));
    let build = root.join("dist/demo/build");
    fs::create_dir_all(&build).with_context(|| format!("creating {}", build.display()))?;
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }

    // Fonts: only the pinned, openly licensed files listed in ATTRIBUTION.toml (AGENTS.md §1).
    let manifest = crate::assets::load(&root)?;
    let fonts = root.join("target/demo-fonts");
    crate::assets::fetch_all(&manifest, &fonts, "font")?;

    println!("demo-pdf: rendering raster");
    let png = raster::mandelbrot_png(640)?;
    fs::write(build.join("mandelbrot.png"), &png)?;

    let source = root.join("assets/demo/showcase.html");
    let html = fs::read_to_string(&source).with_context(|| format!("reading {}", source.display()))?;
    let html = html
        .replace("{{ASSETS}}", &file_url(&root.join("assets")))
        .replace("{{FONTS}}", &file_url(&fonts))
        .replace("{{MANDELBROT_PNG}}", &format!("data:image/png;base64,{}", raster::base64(&png)));
    let html_path = build.join("showcase.html");
    fs::write(&html_path, html)?;

    let chrome = chrome::find(chrome_override.as_deref())?;
    println!("demo-pdf: printing with {}", chrome.display());
    let raw = build.join("chrome.pdf");
    chrome::print_to_pdf(&chrome, &html_path, &raw, &build.join("chrome.log"))?;

    println!("demo-pdf: post-processing");
    let mut doc = lopdf::Document::load(&raw).with_context(|| format!("loading {}", raw.display()))?;
    let summary = postprocess::enhance(&mut doc, pdf::Stamp::now())?;
    doc.compress();
    let foreign = foreign_fonts(&doc);
    if !foreign.is_empty() {
        bail!("the PDF embeds fonts that are not in ATTRIBUTION.toml (Chrome fell back to system fonts; AGENTS.md §1): {}", foreign.join(", "));
    }
    doc.save(&out).with_context(|| format!("writing {}", out.display()))?;

    println!(
        "demo-pdf: wrote {} ({} pages, {} annotations, {} form fields, outline: {})",
        out.display(),
        summary.pages,
        summary.annotations,
        summary.fields,
        summary.outline
    );
    Ok(())
}

/// The workspace root (the parent of this crate's manifest directory).
fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = manifest.canonicalize().unwrap_or_else(|_| manifest.to_path_buf());
    manifest.parent().map(Path::to_path_buf).unwrap_or(manifest)
}

/// A `file://` URL for an absolute path, percent-encoding unsafe bytes.
pub(crate) fn file_url(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut s = abs.to_string_lossy().replace('\\', "/");
    // Windows `canonicalize` returns a verbatim `\\?\C:\...` path; after the slash swap that is
    // `//?/C:/...`, which must not be mistaken for an absolute Unix path, or the URL Chrome is
    // handed becomes `file:////?/C:/...` and it prints one blank page instead of the document.
    if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    let mut url = String::from(if s.starts_with('/') { "file://" } else { "file:///" });
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => url.push(byte as char),
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

/// PostScript-name prefixes of the fonts the showcase may embed: the OFL fonts in
/// ATTRIBUTION.toml. Standard-14 names are allowed only as non-embedded references.
const ALLOWED_FONT_PREFIXES: &[&str] = &[
    "Inter",
    "JetBrainsMono",
    "EBGaramond",
    "PlayfairDisplay",
    "GreatVibes",
    "STIXTwoMath",
    "Amiri",
    "NotoSansHebrew",
    "NotoSansDevanagari",
    "NotoSansThai",
    "NotoSansArmenian",
    "NotoSansGeorgian",
    "NotoSansTamil",
    "CharisSIL",
    "ZCOOLXiaoWei",
    "ShipporiMincho",
    "GowunBatang",
    "NotoColorEmoji",
];
const STANDARD_14: &[&str] = &[
    "Helvetica",
    "Helvetica-Bold",
    "Helvetica-Oblique",
    "Helvetica-BoldOblique",
    "Times-Roman",
    "Times-Bold",
    "Times-Italic",
    "Times-BoldItalic",
    "Courier",
    "Courier-Bold",
    "Courier-Oblique",
    "Courier-BoldOblique",
    "Symbol",
    "ZapfDingbats",
];

/// Fonts in the document that are not allowed, as "name (embedded|referenced)".
fn foreign_fonts(doc: &lopdf::Document) -> Vec<String> {
    let mut bad = std::collections::BTreeSet::new();
    for obj in doc.objects.values() {
        let Ok(d) = obj.as_dict() else { continue };
        if d.get(b"Type").and_then(|t| t.as_name()).ok() != Some(b"Font") {
            continue;
        }
        let subtype = d.get(b"Subtype").and_then(|t| t.as_name()).unwrap_or(b"");
        if subtype == b"Type0" {
            continue; // checked through its descendant CIDFont
        }
        let name = d.get(b"BaseFont").and_then(|n| n.as_name()).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
        // Strip the subset tag ("ABCDEF+Name").
        let plain = match name.split_once('+') {
            Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest.to_string(),
            _ => name.clone(),
        };
        let embedded = d
            .get(b"FontDescriptor")
            .ok()
            .and_then(|r| r.as_reference().ok())
            .and_then(|r| doc.get_dictionary(r).ok())
            .is_some_and(|fd| fd.has(b"FontFile") || fd.has(b"FontFile2") || fd.has(b"FontFile3"));
        let allowed = if subtype == b"Type3" {
            // Chrome emits colour-emoji glyphs as Type 3 fonts named after the source font.
            plain.is_empty() || ALLOWED_FONT_PREFIXES.iter().any(|p| plain.starts_with(p))
        } else if embedded {
            ALLOWED_FONT_PREFIXES.iter().any(|p| plain.starts_with(p))
        } else {
            STANDARD_14.contains(&plain.as_str())
        };
        if !allowed {
            bad.insert(format!("{plain} ({})", if embedded { "embedded" } else { "referenced" }));
        }
    }
    bad.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Windows `canonicalize` returns a verbatim `\?\C:\...` path; the URL must not carry the
    /// `//?/` prefix over (regression test for `file:////?/C:/...`, which Chrome prints as one
    /// blank page).
    #[test]
    fn file_url_has_no_verbatim_prefix() {
        let url = file_url(&std::env::temp_dir());
        assert!(!url.contains("//?/"), "{url}");
        assert!(url.starts_with("file:///"), "{url}");
    }
}
