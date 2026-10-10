//! The snapshot taken before applying (what is about to go) and the proof run on the output.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, write_full};

use super::extract::extract_page;
use super::geometry::{Finding, check_geometry};
use super::sweep::{Acc, Needle, add_needle, sweep_doc};
use super::util::{Ctx, count, latin1, sha256, squash, words};
use super::{Excision, Proof, ProofOptions, SURFACES, Status, Surface};
use crate::interp::{Mode, Removed, Scope, process};
use crate::limits::MIN_NEEDLE;
use crate::{Mark, RedactError, Report, annots_of, doomed_annots, page_streams, redact_appearances};

pub(super) struct RegionSnap {
    pub(super) page: usize,
    pub(super) region: [f64; 4],
    pub(super) mark: ObjRef,
    pub(super) runs: Vec<(Vec<u8>, String)>,
}

pub(super) struct PageSnap {
    pub(super) sha: Option<String>,
    pub(super) text_ops: Option<usize>,
    pub(super) undecodable_ops: usize,
}

/// What redaction is about to remove, recorded before it does.
pub struct Snapshot {
    regions: Vec<RegionSnap>,
    pages: BTreeMap<usize, PageSnap>,
    needles: Vec<Needle>,
    /// Things that made the "before" side unreadable (reported on the text surface).
    problems: Vec<String>,
    /// The document's page text before redaction (decoded view, code view), for the allowance
    /// of needles added later.
    src: (String, String),
    /// Digests of the decoded content of the streams this run created to draw the redaction
    /// boxes and labels (see [`Snapshot::set_overlay_streams`]).
    overlay: HashSet<String>,
}

impl std::fmt::Debug for Snapshot {
    // The snapshot holds what is about to be redacted: never print it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot").field("regions", &self.regions.len()).field("needles", &self.needles.len()).finish()
    }
}

impl Snapshot {
    /// Record what redacting the marks on `pages` (0-based; `None` = all) will remove. Call it
    /// before [`apply`](crate::apply); it changes nothing.
    pub fn capture(doc: &Document, pages: Option<&[usize]>) -> Result<Snapshot, RedactError> {
        let all = crate::marks(doc);
        let chosen: Vec<&Mark> = all.iter().filter(|m| pages.is_none_or(|p| p.contains(&m.page))).collect();
        if chosen.is_empty() {
            return Err(RedactError::NothingToApply);
        }
        Snapshot::capture_marks(doc, &chosen)
    }

    pub(crate) fn capture_marks(doc: &Document, chosen: &[&Mark]) -> Result<Snapshot, RedactError> {
        let mut by_page: Vec<usize> = chosen.iter().map(|m| m.page).collect();
        by_page.sort_unstable();
        by_page.dedup();
        let page_list = pdfcraft_model::pages(doc);
        let ctx = Ctx::new(HashSet::new());
        let mut regions: Vec<RegionSnap> = Vec::new();
        let mut pages = BTreeMap::new();
        for &pi in &by_page {
            let page = page_list.get(pi).ok_or(RedactError::Unreadable(pi + 1))?;
            let page_marks: Vec<&&Mark> = chosen.iter().filter(|m| m.page == pi).collect();
            let rects: Vec<[f64; 4]> = page_marks.iter().flat_map(|m| m.rects.iter().copied()).collect();
            let (_, data) = page_streams(doc, &page.dict, pi)?;
            let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
            // The interpreter's own removal path, on a scratch copy: what it takes out is what
            // has to be gone.
            let mut scratch = doc.clone();
            let mut sink = Report::default();
            let mut removed: Vec<Removed> = {
                let mut scope = Scope::new(&rects, Mode::Apply, &mut sink);
                scope.removed = Some(Vec::new());
                process(&mut scratch, &mut scope, &data, &resources, Matrix::IDENTITY);
                scope.removed.take().unwrap_or_default()
            };
            // The appearances of the annotations that survive the marks take part too (what
            // applying will cut from them, the proof looks for in the output like page text).
            let annots = annots_of(doc, &page.dict);
            let marks: Vec<&Mark> = page_marks.iter().map(|m| **m).collect();
            let (doomed, _, _) = doomed_annots(doc, &annots, &marks, &rects);
            let survivors: Vec<Object> = annots
                .iter()
                .filter(|a| {
                    // A direct dictionary has no object to interpret or repoint (applying
                    // promotes those first; the public capture skips them).
                    let Some(r) = a.as_ref() else { return false };
                    if doomed.contains(&r) {
                        return false;
                    }
                    // Pop-ups of doomed annotations go with them.
                    let parent = doc.get(r).as_dict().and_then(|d| d.get(b"Parent")).and_then(Object::as_ref);
                    !parent.is_some_and(|p| doomed.contains(&p))
                })
                .cloned()
                .collect();
            let mut sink2 = Report::default();
            let appearances = redact_appearances(&mut scratch, &resources, &survivors, &rects, Mode::Apply, &mut sink2);
            match appearances {
                Ok(changes) => removed.extend(changes.removed),
                Err(reason) => return Err(RedactError::Unsupported { page: pi + 1, reason }),
            }
            let before = extract_page(doc, &page.dict, &ctx, &[]);
            pages.insert(
                pi,
                PageSnap {
                    sha: Some(sha256(&data)),
                    text_ops: before.as_ref().ok().map(|e| e.glyph_ops),
                    undecodable_ops: before.as_ref().map_or(0, |e| e.undecodable_ops),
                },
            );
            let mut k = 0;
            for m in &page_marks {
                for r in &m.rects {
                    let runs = removed.iter().filter(|x| x.rect == k).map(|x| (x.raw.clone(), x.text.clone())).collect();
                    regions.push(RegionSnap { page: pi, region: *r, mark: m.obj, runs });
                    k += 1;
                }
            }
        }
        // The document's text as the independent extractor reads it, for the allowance of
        // strings that legitimately remain elsewhere.
        let (mut src_text, mut src_raw) = (String::new(), String::new());
        let mut problems = Vec::new();
        for (i, p) in page_list.iter().enumerate() {
            match extract_page(doc, &p.dict, &ctx, &[]) {
                Ok(e) => {
                    src_text.push_str(&e.text);
                    src_raw.push_str(&e.raw);
                }
                Err(()) => {
                    problems.push(format!("page {} of the source can't be read, so strings left on it can't be told from removed ones", i + 1))
                }
            }
        }
        let mut needles: Vec<Needle> = Vec::new();
        let (mut cut_text, mut cut_raw) = (String::new(), String::new());
        for (ri, r) in regions.iter().enumerate() {
            for (raw, text) in &r.runs {
                cut_text.push('\u{1}');
                cut_text.push_str(&squash(text));
                cut_raw.push('\u{1}');
                cut_raw.push_str(&squash(&latin1(raw)));
                let raw_s = latin1(raw);
                for (raw_view, whole) in [(false, text.trim().to_string()), (true, raw_s)] {
                    let mut shown: Vec<String> = Vec::new();
                    if !squash(&whole).is_empty() {
                        shown.push(whole.clone());
                        shown.extend(words(&whole).into_iter().filter(|w| *w != whole).map(str::to_string));
                    }
                    for s in shown {
                        add_needle(&mut needles, raw_view, s, Some(ri));
                    }
                }
            }
        }
        for n in &mut needles {
            let (src, cut) = if n.raw_view { (&src_raw, &cut_raw) } else { (&src_text, &cut_text) };
            n.allowance = count(src, &n.key).saturating_sub(count(cut, &n.key));
        }
        Ok(Snapshot { regions, pages, needles, problems, src: (src_text, src_raw), overlay: HashSet::new() })
    }

    /// Tell the proof which streams this run created to draw the redaction boxes and labels
    /// (`created`: references into `doc`, the document after `apply`). Call it before
    /// [`prove`](Snapshot::prove) / [`prove_saved`](Snapshot::prove_saved).
    ///
    /// Those streams paint over the regions on purpose and may show label text, so the proof
    /// leaves them out of the text sweep and the paint check. They are recognised by the digest
    /// of their decoded content (a save renumbers objects, so references would not survive it),
    /// and *only* those: a stream that merely carries a `/PCMark /Redaction` key in the input is
    /// not exempt. Streams that can't be read here are an error; nothing is then exempted.
    pub fn set_overlay_streams(&mut self, doc: &Document, created: &[ObjRef]) -> Result<(), RedactError> {
        let ctx = Ctx::new(HashSet::new());
        let mut digests = HashSet::new();
        for r in created {
            let obj = doc.try_get(r.num).map_err(|_| RedactError::OverlayUnreadable)?;
            let Object::Stream(s) = &*obj else { continue };
            let data = ctx.decode(s).map_err(|_| RedactError::OverlayUnreadable)?;
            digests.insert(sha256(&[data]));
        }
        self.overlay = digests;
        Ok(())
    }

    /// Also look for these strings in the output: text that left the document by another route
    /// than a page's glyphs (a removed annotation's `/Contents` or `/RC`, a form field's value).
    /// Each is searched whole, on every surface. A string that also legitimately remains as page
    /// text is counted on the page-content surfaces only (as for the removed glyph runs).
    /// Strings that are blank or shorter than the minimum needle are ignored.
    pub fn add_removed_strings(&mut self, strings: &[String]) {
        for s in strings {
            let text = s.trim();
            let key = squash(text);
            if key.chars().count() < MIN_NEEDLE {
                continue;
            }
            let known = self.needles.iter().any(|n| !n.raw_view && n.key == key);
            // No owner: a survivor fails the proof through its surface; no region carries it.
            add_needle(&mut self.needles, false, text.to_string(), None);
            if !known && let Some(n) = self.needles.last_mut() {
                n.allowance = count(&self.src.0, &n.key);
            }
        }
    }

    /// Serialize `doc` the way a save would and sweep the result.
    pub fn prove(&self, doc: &Document, opts: &ProofOptions) -> Proof {
        let save = SaveOptions { allow_signed_rewrite: opts.allow_signed, ..SaveOptions::default() };
        match write_full(doc, &save) {
            Ok(bytes) => self.prove_inner(&bytes, Some(doc), opts),
            Err(e) => {
                let mut p = self.prove_inner(&[], None, opts);
                p.failures.push(format!("the document could not be serialized for the sweep: {e}"));
                p
            }
        }
    }

    /// Sweep an already saved file (for example an incrementally saved one, whose earlier
    /// revision still holds the original content).
    pub fn prove_saved(&self, bytes: &[u8], opts: &ProofOptions) -> Proof {
        self.prove_inner(bytes, None, opts)
    }

    fn prove_inner(&self, bytes: &[u8], live: Option<&Document>, opts: &ProofOptions) -> Proof {
        let mut acc = Acc::new(Ctx::new(self.overlay.clone()));
        let mut failures: Vec<String> = Vec::new();
        for p in &self.problems {
            acc.problem(Surface::ExtractedText, p.clone());
        }
        let parsed = if bytes.is_empty() { None } else { Document::open(Arc::new(bytes.to_vec())).ok() };
        match &parsed {
            Some(out) => sweep_doc(&self.needles, out, bytes, 0, "", &mut acc),
            None => {
                let why = "the output could not be read back".to_string();
                for s in SURFACES {
                    acc.problem(s, why.clone());
                }
                failures.push(why);
            }
        }
        if live.is_some_and(|d| !d.full_save_required()) {
            acc.survivor(Surface::Revisions, "the document would be saved as an incremental update, keeping the original revision".into(), None);
        }

        // The non-text content under each region, on the output.
        let (findings, after) = match &parsed {
            Some(out) => self.check_output(out, &acc.ctx),
            None => Default::default(),
        };
        let mut entries = Vec::with_capacity(self.regions.len());
        for (i, r) in self.regions.iter().enumerate() {
            let (entry, failure) = self.entry(i, r, &findings, &after, &acc.failed_needles, opts);
            failures.extend(failure);
            entries.push(entry);
        }
        let surfaces = acc.finish();
        for s in &surfaces {
            if s.survivors > 0 {
                failures.push(format!("{:?}: {} survivor(s) at {}", s.surface, s.survivors, s.locations.join(", ")));
            }
            for p in &s.problems {
                failures.push(format!("{:?} could not be swept: {p}", s.surface));
            }
        }
        Proof { entries, surfaces, failures }
    }

    /// The non-text content under each region of the output (what still paints or shows there),
    /// and per page the digest and glyph-operator count of what `apply` left.
    fn check_output(&self, out: &Document, ctx: &Ctx) -> (HashMap<usize, Vec<Finding>>, PagesAfter) {
        let mut findings: HashMap<usize, Vec<Finding>> = HashMap::new();
        let mut after: PagesAfter = BTreeMap::new();
        let out_pages = pdfcraft_model::pages(out);
        for &pi in self.pages.keys() {
            let idx: Vec<usize> = (0..self.regions.len()).filter(|&i| self.regions.get(i).is_some_and(|r| r.page == pi)).collect();
            let Some(page) = out_pages.get(pi) else {
                for &i in &idx {
                    findings.entry(i).or_default().push(Finding::Missing);
                }
                continue;
            };
            let rects: Vec<[f64; 4]> = idx.iter().filter_map(|&i| self.regions.get(i).map(|r| r.region)).collect();
            let (sha, ops, shown) = page_after(out, &page.dict, ctx, &rects);
            after.insert(pi, (sha, ops));
            match shown {
                Some(shown) => {
                    for (k, n) in shown.into_iter().enumerate() {
                        if let (Some(&i), true) = (idx.get(k), n > 0) {
                            findings.entry(i).or_default().push(Finding::Text(n));
                        }
                    }
                }
                None => {
                    for &i in &idx {
                        findings
                            .entry(i)
                            .or_default()
                            .push(Finding::Unverifiable("the page text can't be placed, so what still shows under the region can't be checked"));
                    }
                }
            }
            for (k, f) in check_geometry(out, &page.dict, &rects, ctx) {
                if let Some(&i) = idx.get(k) {
                    findings.entry(i).or_default().push(f);
                }
            }
        }
        (findings, after)
    }

    /// The manifest entry of region `i`, and why it failed when it did.
    fn entry(
        &self,
        i: usize,
        r: &RegionSnap,
        findings: &HashMap<usize, Vec<Finding>>,
        after: &PagesAfter,
        failed_needles: &HashSet<usize>,
        opts: &ProofOptions,
    ) -> (Excision, Option<String>) {
        let mut failure = None;
        let snap = self.pages.get(&r.page);
        let (sha_after, ops_after) = after.get(&r.page).cloned().unwrap_or((None, None));
        let sha_before = snap.and_then(|s| s.sha.clone());
        let owned: Vec<&Needle> = self.needles.iter().filter(|n| n.owners.contains(&i)).collect();
        let survived = self.needles.iter().enumerate().filter(|(ni, n)| n.owners.contains(&i) && failed_needles.contains(ni)).count();
        let mut detail: Vec<String> = Vec::new();
        let mut status = if r.runs.is_empty() { Status::VerifiedNoTextInRegion } else { Status::Verified };
        let fs = findings.get(&i).map(Vec::as_slice).unwrap_or_default();
        if survived > 0 {
            status = Status::Failed;
            detail.push(format!("{survived} removed string(s) still found"));
        }
        for f in fs {
            match f {
                Finding::Text(n) => detail.push(format!("{n} glyph(s) still show inside the region (placed independently of the redaction)")),
                Finding::Image => detail.push("an image still carries pixels under the region".into()),
                Finding::Path => detail.push("a path still paints inside the region".into()),
                Finding::Shading => detail.push("a shading still paints inside the region".into()),
                Finding::Missing => detail.push("the page is missing from the output".into()),
                Finding::Unverifiable(why) => detail.push((*why).to_string()),
            }
            if !matches!(f, Finding::Unverifiable(_)) {
                status = Status::Failed;
            } else if status != Status::Failed {
                status = Status::Unverifiable;
            }
        }
        if sha_before.is_none() || sha_after.is_none() {
            status = Status::Failed;
            detail.push("the page content can't be read on one side, so it can't be compared".into());
        } else if !r.runs.is_empty() && sha_before == sha_after {
            status = Status::Failed;
            detail.push("glyphs were removed but the page content is unchanged".into());
        }
        if r.runs.is_empty() && snap.is_some_and(|s| s.undecodable_ops > 0) && status != Status::Failed {
            status = Status::Unverifiable;
            detail.push("the page draws text that can't be decoded, so \"no text in the region\" can't be checked".into());
        }
        let ambiguous = owned.iter().filter(|n| n.allowance > 0).count();
        if ambiguous > 0 {
            detail.push(format!("{ambiguous} string(s) also remain in retained content; only a surplus over those copies counts"));
        }
        if matches!(status, Status::Failed | Status::Unverifiable) {
            failure = Some(format!("page {} region {:?}: {}", r.page + 1, r.region, detail.join("; ")));
        }
        let entry = Excision {
            page: r.page,
            region: r.region,
            mark: r.mark,
            status,
            detail: detail.join("; "),
            runs_removed: r.runs.len(),
            codes_removed: r.runs.iter().map(|(raw, _)| raw.len()).sum(),
            page_sha256_after: sha_after,
            text_ops_before: snap.and_then(|s| s.text_ops),
            text_ops_after: ops_after,
            removed_text: opts.include_plaintext.then(|| r.runs.iter().map(|(_, t)| t.clone()).collect()),
        };
        (entry, failure)
    }
}

/// Per page: the digest of its content in the output and its glyph-operator count.
type PagesAfter = BTreeMap<usize, (Option<String>, Option<usize>)>;

/// The page's content digest and glyph-operator count in the output, and per region the glyphs
/// still placed inside it (`None` when the page's text can't be placed). The digest covers the
/// page's content as `apply` left it, the overlay streams included.
pub(super) fn page_after(doc: &Document, page: &Dict, ctx: &Ctx, regions: &[[f64; 4]]) -> (Option<String>, Option<usize>, Option<Vec<usize>>) {
    let sha = page_streams(doc, page, 0).ok().map(|(_, d)| sha256(&d));
    let e = extract_page(doc, page, ctx, regions).ok();
    (sha, e.as_ref().map(|e| e.glyph_ops), e.map(|e| e.shown_in))
}
