//! Writing redacted output: a temporary file next to the destination, owner-only, written in
//! full and read back, then renamed over the destination. The destination is never deleted
//! first, so a failure at any step leaves it as it was.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Most bytes of the destination's name kept in the temporary name, so that the whole name stays
/// well under the 255-byte limit of common filesystems (the unique suffix and the dot are the rest).
const MAX_STEM_BYTES: usize = 120;

/// `.{stem}.pdfcraft-{pid}-{n}.tmp`, with the stem cut at a character boundary. Pid and counter
/// keep it unique; it sits in the destination's folder so the rename stays atomic.
fn temp_name(name: &str, pid: u32, n: u64) -> String {
    let mut end = name.len().min(MAX_STEM_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1; // 0 is always a boundary, so this stops
    }
    format!(".{}.pdfcraft-{pid}-{n}.tmp", name.get(..end).unwrap_or(""))
}

/// Write `bytes` to `path` atomically. The temporary file is created exclusively (an existing
/// file or link of that name is never followed) with owner-only permissions where the platform
/// has them, flushed, and compared with `bytes` before the rename.
///
/// - A new file stays owner-only: redacted output is sensitive until its owner decides otherwise.
///   Replacing an existing file keeps that file's permissions (its owner already decided), on
///   Unix. On Windows the file gets the default access rights of its folder: nothing here sets an
///   ACL, so "owner-only" is a Unix guarantee.
/// - A destination that is a symbolic link is refused: the rename would replace the link, not the
///   file it points to, and which of the two was meant is not ours to guess.
/// - On Unix the directory is flushed after the rename, so the new name survives a crash.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a file path"))?;
    let existing = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the destination is a symbolic link; save to the file it points to"));
        }
        Ok(m) => Some(m),
        Err(_) => None,
    };
    let tmp = dir.join(temp_name(&name.to_string_lossy(), std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut f = options.open(&tmp)?;
        f.write_all(bytes)?;
        if let Some(m) = existing.as_ref().filter(|m| m.is_file()) {
            f.set_permissions(m.permissions())?;
        }
        f.sync_all()?;
        drop(f);
        if std::fs::read(&tmp)? != bytes {
            return Err(std::io::Error::other("the file read back differs from what was written"));
        }
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return result;
    }
    // The file is in place; failing to flush the directory entry must not report a failed save.
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_very_long_file_name_still_saves_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-longname-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join(format!("{}.pdf", "a".repeat(246)));
        assert_eq!(target.file_name().unwrap().len(), 250);
        write_private_atomic(&target, b"%PDF-1.7 saved").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"%PDF-1.7 saved");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temp_names_are_bounded_unique_and_cut_at_a_character_boundary() {
        let long = "é".repeat(200); // 2 bytes each: 119 or 120 is not a boundary issue only if cut correctly
        let a = temp_name(&long, 7, 1);
        assert!(a.len() <= 160 && a.starts_with('.') && a.ends_with(".pdfcraft-7-1.tmp"), "{a}");
        assert_ne!(a, temp_name(&long, 7, 2));
        assert_eq!(temp_name("a.pdf", 1, 0), ".a.pdf.pdfcraft-1-0.tmp");
    }
}
