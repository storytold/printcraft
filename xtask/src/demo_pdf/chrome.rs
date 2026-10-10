//! Locating a Chromium-based browser and printing HTML to PDF with it.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// Find a Chrome/Chromium binary. `explicit` (from `--chrome`) wins, then the
/// `PDFCRAFT_CHROME` / `CHROME` environment variables, then well-known
/// install locations, then `PATH`.
pub fn find(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        bail!("--chrome {} does not exist", path.display());
    }
    for var in ["PDFCRAFT_CHROME", "CHROME"] {
        if let Some(path) = env::var_os(var).map(PathBuf::from)
            && path.is_file()
        {
            return Ok(path);
        }
    }

    let mut candidates: Vec<PathBuf> = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = env::var_os("HOME") {
        candidates.push(Path::new(&home).join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
        if let Some(base) = env::var_os(var) {
            let base = PathBuf::from(base);
            candidates.push(base.join(r"Google\Chrome\Application\chrome.exe"));
            candidates.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
        }
    }
    if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
        return Ok(found);
    }

    let names = ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome", "chrome.exe", "msedge"];
    let path = env::var_os("PATH").unwrap_or_default();
    for dir in env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!(
        "could not find Google Chrome or Chromium; install one or pass --chrome <path> \
         (or set PDFCRAFT_CHROME)"
    )
}

/// Print `html` to `pdf` with headless Chrome. Chrome emits a tagged PDF and,
/// with `--generate-pdf-document-outline`, bookmarks built from the headings.
///
/// Some Chrome builds finish writing the PDF but linger afterwards, so rather
/// than only waiting for exit we poll for a complete file and then stop Chrome.
pub fn print_to_pdf(chrome: &Path, html: &Path, pdf: &Path, log: &Path) -> Result<()> {
    if pdf.exists() {
        fs::remove_file(pdf).with_context(|| format!("removing stale {}", pdf.display()))?;
    }
    let mut print_arg = OsString::from("--print-to-pdf=");
    print_arg.push(pdf);
    let log_file = fs::File::create(log).with_context(|| format!("creating {}", log.display()))?;

    let mut child = Command::new(chrome)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-pdf-header-footer")
        .arg("--generate-pdf-document-outline")
        .arg(print_arg)
        .arg(super::file_url(html))
        .stdout(Stdio::null())
        .stderr(log_file)
        .spawn()
        .with_context(|| format!("running {}", chrome.display()))?;

    let started = Instant::now();
    let mut last_len = None;
    // Chrome's Skia back-end writes the PDF incrementally, and every increment ends with its own
    // `%%EOF`: a stable file that looks complete can still grow while later pages rasterise. Only
    // treat the file as finished after the size has been quiet for several consecutive polls.
    let mut quiet_polls = 0;
    loop {
        if let Some(status) = child.try_wait()? {
            if is_complete(pdf) {
                return Ok(());
            }
            bail!("Chrome exited with {status} without writing {}; see {}", pdf.display(), log.display());
        }
        let len = fs::metadata(pdf).map(|m| m.len()).ok();
        if len == last_len && len.is_some() && is_complete(pdf) {
            quiet_polls += 1;
            if quiet_polls >= 10 {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(());
            }
        } else {
            quiet_polls = 0;
        }
        last_len = len;
        if started.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            bail!("Chrome timed out printing {}; see {}", html.display(), log.display());
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// A PDF is complete once its trailer's `%%EOF` marker has been written.
fn is_complete(pdf: &Path) -> bool {
    fs::read(pdf).map(|bytes| bytes.len() > 1024 && bytes[bytes.len() - 1024..].windows(5).any(|w| w == b"%%EOF")).unwrap_or(false)
}
