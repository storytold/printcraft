//! Reload a document when its file changes on disk (#431): a LaTeX or Typst build rewriting the
//! PDF that is open shows its new version without reopening it.
//!
//! Cheap by design. One worker thread looks at each open file's metadata (no reading) every
//! [`IDLE`], and blocks without waking at all while no open document has a file. A changed file
//! is read only once it has stopped changing ([`SETTLE`] apart) and ends like a whole PDF: its
//! last KiB is read first, so a file still being written costs one small read. The new bytes go
//! to the UI thread, which is woken for them: a clean document is reloaded in place, keeping its
//! tab, page, zoom and scroll; one with unsaved changes gets a notice instead. PdfCraft's own
//! saves are recognised by the file's stamp after writing, so they never reload.

use std::collections::HashMap;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use pdfcraft_engine::DocId;

use crate::PdfCraftApp;

/// How often an unchanged file is looked at.
pub const IDLE: Duration = Duration::from_millis(250);
/// How long a changed file must stay the same before it is read: writers pause between chunks.
pub const SETTLE: Duration = Duration::from_millis(40);
/// How much of the end of a changed file is read to see whether it is a whole PDF.
const TAIL: usize = 1024;

/// One version of a file, told apart without reading it: its length and modification time, and
/// what the platform adds (Unix: device, inode and change time; Windows: creation and last-write
/// times in 100 ns ticks). The modification time alone is coarse on FAT and some network
/// drives, and a rebuild can keep the length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    extra: [u64; 4],
}

impl Stamp {
    fn of(meta: &Metadata) -> Self {
        #[cfg(unix)]
        let extra = {
            use std::os::unix::fs::MetadataExt;
            [meta.dev(), meta.ino(), meta.ctime().cast_unsigned(), meta.ctime_nsec().cast_unsigned()]
        };
        #[cfg(windows)]
        let extra = {
            use std::os::windows::fs::MetadataExt;
            [meta.creation_time(), meta.last_write_time(), 0, 0]
        };
        #[cfg(not(any(unix, windows)))]
        let extra = [0; 4];
        Self { len: meta.len(), modified: meta.modified().ok(), extra }
    }

    /// The file at `path` now; `None` when it is missing, not a file or can't be read.
    pub fn at(path: &str) -> Option<Self> {
        std::fs::metadata(path).ok().filter(Metadata::is_file).map(|m| Self::of(&m))
    }
}

/// A file to watch, as the UI thread sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Watched {
    pub doc: DocId,
    pub path: String,
    /// The version PdfCraft last read or wrote (`None`: the first one the watcher sees).
    pub known: Option<Stamp>,
}

/// A new version of a watched file, read and ready to open.
pub struct Changed {
    pub doc: DocId,
    pub stamp: Stamp,
    pub bytes: Arc<Vec<u8>>,
}

/// The watcher's record of one file.
struct Entry {
    doc: DocId,
    path: String,
    known: Option<Stamp>,
    /// A changed version seen once, waiting to be seen unchanged again.
    pending: Option<Stamp>,
}

/// Looks at the watched files: on the worker thread, or on the UI thread in tests.
#[derive(Default)]
pub struct Watcher {
    files: Vec<Entry>,
}

impl Watcher {
    /// Watch `list` from now on. A file watched already keeps its pending change; a version the
    /// UI thread knows (it just read or saved the file) replaces the watcher's own.
    fn set(&mut self, list: Vec<Watched>) {
        let old = std::mem::take(&mut self.files);
        self.files = list
            .into_iter()
            .map(|w| {
                let prev = old.iter().find(|e| e.doc == w.doc && e.path == w.path);
                Entry { known: w.known.or(prev.and_then(|e| e.known)), pending: prev.and_then(|e| e.pending), doc: w.doc, path: w.path }
            })
            .collect();
    }

    /// Look at every file once: the new versions found, and whether a file is still changing
    /// (look again after [`SETTLE`] rather than [`IDLE`]).
    fn tick(&mut self) -> (Vec<Changed>, bool) {
        let mut found = Vec::new();
        let mut settling = false;
        for e in &mut self.files {
            // Missing (deleted, or being replaced) or unreadable: the document stays as it is.
            let Some(now) = Stamp::at(&e.path) else {
                e.pending = None;
                continue;
            };
            let known = *e.known.get_or_insert(now);
            if now == known {
                e.pending = None;
                continue;
            }
            if e.pending != Some(now) {
                // Changed since the last look: read it once it stops changing.
                e.pending = Some(now);
                settling = true;
                continue;
            }
            e.pending = None;
            match read_whole_pdf(&e.path, now) {
                Version::Ready(bytes) => {
                    e.known = Some(now);
                    found.push(Changed { doc: e.doc, stamp: now, bytes });
                }
                // Not a whole PDF in this version (a build that failed or paused): wait for the
                // next one.
                Version::Incomplete => e.known = Some(now),
                // Changed again while being read, or held by its writer: look again soon.
                Version::Busy => settling = true,
            }
        }
        (found, settling)
    }
}

/// What a changed file turned out to be.
enum Version {
    /// A whole PDF, read.
    Ready(Arc<Vec<u8>>),
    /// Not a whole PDF: a build that failed or paused.
    Incomplete,
    /// Changing, replaced or held by its writer: look again.
    Busy,
}

/// Read `path` if it is still the version `stamp` and ends like a whole PDF (`%%EOF` in its last
/// KiB, read first). The whole file is then read into a buffer of exactly its size.
fn read_whole_pdf(path: &str, stamp: Stamp) -> Version {
    // Windows refuses to open a file its writer holds exclusively: not ready yet.
    let Ok(mut file) = File::open(path) else { return Version::Busy };
    let Ok(meta) = file.metadata() else { return Version::Busy };
    if Stamp::of(&meta) != stamp {
        return Version::Busy;
    }
    let len = meta.len();
    let mut end = [0u8; TAIL];
    let tail = usize::try_from(len).map_or(TAIL, |l| l.min(TAIL));
    let (Some(end), Ok(back)) = (end.get_mut(..tail), i64::try_from(tail)) else { return Version::Incomplete };
    if file.seek(SeekFrom::End(-back)).is_err() || file.read_exact(end).is_err() {
        return Version::Busy;
    }
    if !end.windows(5).any(|w| w == b"%%EOF") {
        return Version::Incomplete;
    }
    let mut bytes = Vec::new();
    // Too big to hold: the open version stays.
    let Ok(size) = usize::try_from(len) else { return Version::Incomplete };
    if bytes.try_reserve_exact(size).is_err() {
        return Version::Incomplete;
    }
    if file.seek(SeekFrom::Start(0)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return Version::Busy;
    }
    // Still that version, under that name: a writer may have replaced the file meanwhile.
    if bytes.len() != size || Stamp::at(path) != Some(stamp) {
        return Version::Busy;
    }
    Version::Ready(Arc::new(bytes))
}

/// The worker thread: watches the newest list the UI thread sent and hands back new versions,
/// waking the UI for them. It ends when the app (its inbox's sender) is gone.
fn run(inbox: mpsc::Receiver<Vec<Watched>>, out: Arc<Mutex<Vec<Changed>>>, ctx: egui::Context) {
    let mut watcher = Watcher::default();
    // `None`: nothing to watch, so block until there is.
    let mut wait = None;
    loop {
        let message = match wait {
            None => inbox.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(wait) => inbox.recv_timeout(wait),
        };
        match message {
            Ok(list) => watcher.set(list),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(list) = inbox.try_recv() {
            watcher.set(list);
        }
        if watcher.files.is_empty() {
            wait = None;
            continue;
        }
        let (found, settling) = watcher.tick();
        if !found.is_empty() {
            out.lock().unwrap_or_else(PoisonError::into_inner).extend(found);
            ctx.request_repaint();
        }
        wait = Some(if settling { SETTLE } else { IDLE });
    }
}

/// The app's side of the watcher.
#[derive(Default)]
pub struct FileWatch {
    /// The worker's inbox (`None` until a document with a file is open).
    inbox: Option<mpsc::Sender<Vec<Watched>>>,
    found: Arc<Mutex<Vec<Changed>>>,
    /// What the watcher was last told.
    sent: Vec<Watched>,
    /// In tests ([`PdfCraftApp::run_inline`]) there is no thread: each frame looks once.
    inline: Watcher,
    /// The version of each document's file that PdfCraft last read or wrote.
    known: HashMap<DocId, Stamp>,
    /// The notice bar's Reload was pressed for this document.
    pub reload_request: Option<DocId>,
}

impl FileWatch {
    /// PdfCraft read or wrote `path` for `doc`: that version is not a change.
    pub fn known_now(&mut self, doc: DocId, path: &str) {
        if let Some(stamp) = Stamp::at(path) {
            self.known.insert(doc, stamp);
        }
    }

    /// Remember `stamp`, read before the file was, for `doc`.
    pub fn known_as(&mut self, doc: DocId, stamp: Option<Stamp>) {
        if let Some(stamp) = stamp {
            self.known.insert(doc, stamp);
        }
    }
}

impl PdfCraftApp {
    /// The documents to watch: every open one with a file, while the preference is on.
    fn watched(&self) -> impl Iterator<Item = (DocId, &str, Option<Stamp>)> {
        self.views.iter().filter(|_| self.reload_changed_files).filter_map(|v| {
            let path = self.session.get(v.id)?.path.as_deref()?;
            Some((v.id, path, self.watch.known.get(&v.id).copied()))
        })
    }

    /// Every frame: tell the watcher about files opened, closed or saved (without allocating
    /// when nothing changed), and reload what changed on disk.
    pub(crate) fn watch_files(&mut self) {
        if let Some(id) = self.watch.reload_request.take() {
            self.reload_from_disk(id);
        }
        let mut sent = self.watch.sent.iter();
        let unchanged = self.watched().all(|(doc, path, known)| sent.next().is_some_and(|w| w.doc == doc && w.path == path && w.known == known))
            && sent.next().is_none();
        if !unchanged {
            let list: Vec<Watched> = self.watched().map(|(doc, path, known)| Watched { doc, path: path.to_owned(), known }).collect();
            let open: Vec<DocId> = self.views.iter().map(|v| v.id).collect();
            self.watch.known.retain(|doc, _| open.contains(doc));
            if self.send_to_watcher(list.clone()) {
                self.watch.sent = list;
            }
        }
        if self.run_inline {
            let (found, _) = self.watch.inline.tick();
            self.watch.found.lock().unwrap_or_else(PoisonError::into_inner).extend(found);
        }
        let found = std::mem::take(&mut *self.watch.found.lock().unwrap_or_else(PoisonError::into_inner));
        for c in found {
            self.file_changed(c);
        }
    }

    /// Hand `list` to the watcher, starting it when needed. `false`: not delivered (no context
    /// yet, or the worker couldn't start or ended); the next frame tries again.
    fn send_to_watcher(&mut self, list: Vec<Watched>) -> bool {
        if self.run_inline {
            self.watch.inline.set(list);
            return true;
        }
        if self.watch.inbox.is_none() {
            if list.is_empty() {
                return true;
            }
            let Some(ctx) = self.ctx.clone() else { return false };
            let (tx, rx) = mpsc::channel();
            let out = self.watch.found.clone();
            if std::thread::Builder::new().name("pdfcraft-watch".into()).spawn(move || run(rx, out, ctx)).is_err() {
                return false;
            }
            self.watch.inbox = Some(tx);
        }
        let delivered = self.watch.inbox.as_ref().is_some_and(|tx| tx.send(list).is_ok());
        if !delivered {
            // The worker ended (it panicked): start a new one next frame.
            self.watch.inbox = None;
        }
        delivered
    }

    /// A watched file has a new version: reload its document, or ask first when it has unsaved
    /// changes.
    fn file_changed(&mut self, c: Changed) {
        let Some(i) = self.views.iter().position(|v| v.id == c.doc) else { return };
        // PdfCraft wrote this version itself (a save), or it was handled already.
        if self.watch.known.get(&c.doc) == Some(&c.stamp) {
            return;
        }
        self.watch.known.insert(c.doc, c.stamp);
        if self.has_unsaved_work(i) {
            if let Some(view) = self.views.get_mut(i) {
                view.disk_changed = true;
            }
            return;
        }
        self.reload_view(i, c.bytes);
    }

    /// The notice bar's Reload: read the file now and replace the document, unsaved changes and
    /// all.
    fn reload_from_disk(&mut self, doc: DocId) {
        let Some(i) = self.views.iter().position(|v| v.id == doc) else { return };
        let Some(path) = self.session.get(doc).and_then(|d| d.path.clone()) else { return };
        let stamp = Stamp::at(&path);
        match std::fs::read(&path) {
            Ok(bytes) => {
                self.watch.known_as(doc, stamp);
                self.reload_view(i, Arc::new(bytes));
            }
            Err(e) => {
                let name = self.session.get(doc).map(|d| d.name.clone()).unwrap_or_default();
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
            }
        }
    }

    /// Open `bytes` in place of tab `i`'s document, keeping the tab and the reader's place. A
    /// version that can't be opened leaves the document as it was, and says so.
    fn reload_view(&mut self, i: usize, bytes: Arc<Vec<u8>>) {
        let Some(id) = self.views.get(i).map(|v| v.id) else { return };
        match self.session.reload(id, bytes) {
            Ok(()) => {
                // Whatever was unsaved is gone, so there is nothing left to recover.
                self.forget_recovery(id);
                if let (Some(view), Some(doc)) = (self.views.get_mut(i), self.session.get(id)) {
                    view.document_replaced(&doc.info);
                }
                // What the form's scripts said while it opened, as on opening.
                let out = self.session.take_js_output(id);
                self.handle_js(id, out);
            }
            Err(e) => {
                let name = self.session.get(id).map(|d| d.name.clone()).unwrap_or_default();
                self.notify_fmt("Couldn't reload {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
            }
        }
    }
}
