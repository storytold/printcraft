//! Editing glue: apply engine edits from the UI, undo/redo, save/save-as, and the
//! "save changes?" prompt when closing a tab or quitting with unsaved edits.

use pdfcraft_engine::Edit;
use pdfcraft_platform::staging::{StagingName, create_staging, staging_suffixes};

use crate::PdfCraftApp;

/// What the user was doing when we asked whether to save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseRequest {
    /// Close this document's tab. By id, not tab index: tabs can close and shift while a save
    /// panel is open.
    Tab(pdfcraft_engine::DocId),
    /// Quit the application once every dirty document is resolved.
    Quit,
    /// File ▸ Close all: like Quit, but the application stays open.
    All,
}

/// Where Save writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveTarget {
    /// The file's own path; asks for one if the document has none (e.g. opened from bytes).
    InPlace,
    /// Always ask for a new location.
    As,
}

impl PdfCraftApp {
    /// Apply an edit to the active document. Returns `true` on success; failures are shown.
    pub fn apply_edit(&mut self, edit: Edit) -> bool {
        let Some((i, id)) = self.active_ids() else { return false };
        let label = edit.label();
        match self.session.apply(id, edit.clone()) {
            Ok(()) => {
                if edit.applies_redactions() {
                    // A recovery snapshot taken before the redaction still holds the original.
                    self.forget_recovery(id);
                }
                let Some(doc) = self.session.get(id) else { return true };
                let info = &doc.info;
                let view = &mut self.views[i];
                view.signature_drag.committed(&edit, doc.edit_generation());
                match comment_page(&edit) {
                    // Comment edits change one page: keep every other raster.
                    Some(page) => view.page_changed(page),
                    None => view.document_changed(info),
                }
                if edit.applies_redactions() {
                    view.drop_rasters();
                }
                // Keep the pages the user acted on selected, where they now are.
                match edit {
                    Edit::MovePages { pages, to } => {
                        let n = pages.iter().collect::<std::collections::BTreeSet<_>>().len();
                        let to = to.min(info.pages.len() - n);
                        view.select_pages(&(to..to + n).collect::<Vec<_>>());
                    }
                    Edit::InsertBlankPage { at, .. } => view.select_pages(&[at.min(info.pages.len() - 1)]),
                    Edit::DeletePages { .. } => view.select_pages(&[]),
                    // A stroke drawn with the pen stays unselected, so the selection box and
                    // author popup don't sit over the next stroke (#429).
                    Edit::AddAnnotation(a)
                        if matches!(a.shape, pdfcraft_engine::Shape::Ink { .. })
                            && self.quick_tool == crate::QuickTool::Comment(crate::comments::CommentTool::Ink) =>
                    {
                        view.comments.selected = None;
                    }
                    Edit::AddAnnotation(a) => {
                        // Select the new comment (appended last among the page's comments).
                        let newest = info.annotations.iter().filter(|x| x.page == a.page && x.in_reply_to.is_none()).map(|x| x.index).max();
                        view.comments.selected = newest.map(|n| (a.page, n));
                        view.comments.reveal = true;
                        // A new highlight opens its note for typing straight away, as in Acrobat,
                        // unless a note typed into another card is still unsaved.
                        if let (pdfcraft_engine::Shape::TextMarkup { kind: pdfcraft_engine::Markup::Highlight, .. }, Some(n)) = (&a.shape, newest)
                            && a.contents.is_empty()
                            && view.comments.editing.as_ref().is_none_or(|(_, _, text)| text.is_empty())
                        {
                            view.comments.editing = Some((a.page, n, String::new()));
                        }
                    }
                    Edit::DeleteAnnotation { .. } => view.comments.selected = None,
                    _ => {}
                }
                if std::mem::take(&mut view.comments.tool_done) && !self.comment_prefs.pinned {
                    self.quick_tool = crate::QuickTool::Select;
                }
                let out = self.session.take_js_output(id);
                self.handle_js(id, out);
                true
            }
            Err(e) => {
                let message = if matches!(edit, Edit::MoveObjects { .. }) { crate::object_ui::move_error(&e) } else { e.to_string() };
                self.notify_fmt("{label} failed: {e}", &[("label", &crate::i18n::action_label(&label)), ("e", &message)]);
                false
            }
        }
    }

    /// Undo the last change: to the Combine files list while its tab shows, else the document.
    pub fn undo(&mut self) {
        if self.combine_showing() {
            self.combine_history_step(true);
        } else {
            self.history_step(true);
        }
    }

    pub fn redo(&mut self) {
        if self.combine_showing() {
            self.combine_history_step(false);
        } else {
            self.history_step(false);
        }
    }

    fn history_step(&mut self, undo: bool) {
        let Some((i, id)) = self.active_ids() else { return };
        let result = if undo { self.session.undo(id) } else { self.session.redo(id) };
        match result {
            Ok(label) => {
                if let Some(doc) = self.session.get(id) {
                    self.views[i].document_changed(&doc.info);
                }
                self.views[i].comments.selected = None;
                let label = crate::i18n::action_label(&label);
                if undo {
                    self.notify_fmt("Undid {label}", &[("label", &label)]);
                } else {
                    self.notify_fmt("Redid {label}", &[("label", &label)]);
                }
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// Apply any edit a view queued this frame (organize toolbar, keys).
    /// Returns false when a field refused the typing queued for it (its editor is open again).
    pub(crate) fn process_pending_edits(&mut self) -> bool {
        let Some(i) = self.active else { return true };
        let applied = self.apply_queued_edit(i);
        if !applied {
            // Nor may a shortcut pressed with it (⌘S right after Enter) run behind the refusal.
            self.deferred_commands.clear();
        }
        match self.views.get_mut(i).and_then(|v| v.pending_action.take()) {
            Some(crate::canvas::ViewAction::InsertFromFile) => self.insert_from_file_dialog(),
            Some(crate::canvas::ViewAction::InsertFromFileAt(at)) => self.insert_from_file_at(Some(at)),
            Some(crate::canvas::ViewAction::Save) => {
                self.save_active(SaveTarget::InPlace);
            }
            Some(crate::canvas::ViewAction::Extract) => self.open_extract_dialog(),
            Some(crate::canvas::ViewAction::Split) => self.dialog = Some(crate::Dialog::Split),
            Some(crate::canvas::ViewAction::CopyPages { cut }) => self.copy_pages(cut),
            Some(crate::canvas::ViewAction::PastePages) => self.paste_pages(),
            None => {}
        }
        applied
    }

    /// Organize ▸ Copy / Cut: remember the selected pages (the document as it is now); Cut also
    /// deletes them (one page always stays).
    pub fn copy_pages(&mut self, cut: bool) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return;
        }
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let pages = self.views[i].target_pages();
        let n = doc.info.pages.len();
        if cut && pages.len() >= n {
            self.notify_tr("A document needs at least one page: copy instead");
            return;
        }
        self.page_clipboard = Some(crate::PageClip { name: doc.name.clone(), bytes: doc.bytes.clone(), pages: pages.clone() });
        if cut {
            self.apply_edit(Edit::DeletePages { pages: pages.clone() });
        }
        let what = if pages.len() == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.len().to_string())]) };
        if cut {
            self.notify_fmt("Cut {what}", &[("what", &what)]);
        } else {
            self.notify_fmt("Copied {what}", &[("what", &what)]);
        }
    }

    /// Organize ▸ Paste: insert the copied pages after the selection (or the current page).
    pub fn paste_pages(&mut self) {
        let Some(clip) = self.page_clipboard.clone() else {
            self.notify_tr("Copy or cut pages first");
            return;
        };
        let Some(i) = self.active else { return };
        let at = self.views[i].target_pages().into_iter().max().map_or(0, |p| p + 1);
        let count = clip.pages.len();
        if self.apply_edit(Edit::InsertPagesFrom { name: clip.name.clone(), bytes: clip.bytes.clone(), pages: Some(clip.pages.clone()), at }) {
            self.views[i].select_pages(&(at..at + count).collect::<Vec<_>>());
        }
    }

    /// Make the active document say what its form shows (#166): apply an edit still queued on
    /// its tab, then commit the text in a field's open editor, which otherwise commits only on
    /// Enter, Tab or clicking away (the editor closes, as clicking away would). Run before
    /// anything reads the document: Save, Print, Export, Optimize, Sign, Extract, Split, Copy
    /// pages and button scripts.
    ///
    /// Returns false when the field rejects the value (a validation script or format; the user
    /// is told why). The editor stays open with the text, and the caller must stop: nothing is
    /// saved, printed or exported without it.
    pub(crate) fn commit_form_typing(&mut self) -> bool {
        let Some((i, id)) = self.active_ids() else { return true };
        if !self.apply_queued_edit(i) {
            return false;
        }
        let Some(draft) = self.views.get(i).and_then(|v| v.forms.focus.clone()) else { return true };
        let Some(edit) = self.session.get(id).and_then(|d| crate::forms_ui::draft_edit(&draft, &d.form)) else {
            return true; // nothing typed that the document doesn't already have
        };
        if let Some(view) = self.views.get_mut(i) {
            view.forms.focus = None;
        }
        if self.apply_edit(edit) {
            return true;
        }
        if let Some(view) = self.views.get_mut(i) {
            let mut draft = draft;
            draft.request_focus = true;
            view.forms.focus = Some(draft);
        }
        false
    }

    /// Apply the edit queued on tab `i` (the active one). Returns false only when it was a
    /// field's typing and the field refused it (#166): the editor reopens with the text, and the
    /// user has been told why, so the typing isn't lost and the caller can stop.
    fn apply_queued_edit(&mut self, i: usize) -> bool {
        let Some(edit) = self.views.get_mut(i).and_then(|v| v.pending_edit.take()) else { return true };
        let signature_page = self.views.get_mut(i).and_then(|v| v.fill_signature_page.take());
        let committed = self.views.get_mut(i).and_then(|v| v.forms.committed.take());
        let typed = match (&edit, &committed) {
            (Edit::SetFieldValue { name, .. }, Some(draft)) => *name == draft.name,
            _ => false,
        };
        if self.apply_edit(edit) {
            if let Some(page) = signature_page
                && let Some(view) = self.views.get_mut(i)
            {
                // Signature imports are a labeled batch; select their appended stamp just
                // as AddAnnotation selects a typed or drawn signature.
                let newest = self
                    .session
                    .get(view.id)
                    .and_then(|d| d.info.annotations.iter().filter(|a| a.page == page && a.in_reply_to.is_none()).map(|a| a.index).max());
                view.comments.selected = newest.map(|index| (page, index));
                view.comments.reveal = true;
                self.quick_tool = crate::QuickTool::Select;
            }
            return true;
        }
        if !typed {
            return true;
        }
        if let (Some(mut draft), Some(view)) = (committed, self.views.get_mut(i)) {
            draft.request_focus = true;
            view.forms.focus = Some(draft);
        }
        false
    }

    /// [`Self::commit_form_typing`] for document `id`, which may not be the active tab (a save
    /// panel answering later, Close All). If its tab has typing or a queued edit, the tab is
    /// brought forward first, so the commit and any field scripts act on that document.
    pub(crate) fn commit_typing_in(&mut self, id: pdfcraft_engine::DocId) -> bool {
        let Some(i) = self.views.iter().position(|v| v.id == id) else { return true };
        // Added text the standard fonts can't draw can't be saved: show it rather than drop it.
        // Its page is brought into view, so the text, the warning and Discard are on screen.
        let blocked = self.views.get_mut(i).and_then(|v| {
            let c = v.content.blocked()?;
            v.content.hold_blocked();
            if let Some(page) = v.content.draft.as_ref().map(|d| d.page) {
                v.go_to_page(page);
            }
            Some(c)
        });
        if let Some(c) = blocked {
            self.active = Some(i);
            self.notify(crate::content_ui::undrawable_message(c));
            return false;
        }
        let typing = self
            .views
            .get(i)
            .is_some_and(|v| v.pending_edit.is_some() || v.forms.focus.is_some() || v.fill_text.as_ref().is_some_and(|t| !t.text.trim().is_empty()));
        if self.active != Some(i) {
            if !typing {
                return true;
            }
            self.active = Some(i);
        }
        self.commit_form_typing() && self.commit_open_fill_text(id)
    }

    /// The Fill & Sign type box commits on click-away. Save does that first, so the text is in
    /// the file (and can be flattened) instead of being left on the screen.
    fn commit_open_fill_text(&mut self, id: pdfcraft_engine::DocId) -> bool {
        let Some(i) = self.views.iter().position(|v| v.id == id) else { return true };
        let Some(tb) = self.views[i].fill_text.clone() else { return true };
        let text = tb.text.trim().to_string();
        if text.is_empty() {
            if let Some(view) = self.views.get_mut(i) {
                view.fill_text = None;
            }
            return true;
        }
        let author = self.comment_prefs.author.clone();
        let edit = crate::fill_sign::typed(tb.page, tb.at, &text, &author);
        if self.apply_edit(edit) {
            if let Some(view) = self.views.get_mut(i) {
                view.fill_text = None;
            }
            true
        } else {
            false
        }
    }

    /// When the preference is on, bake Fill & Sign marks before the bytes are written.
    /// Returns false when the document is not allowed to change (nothing is written).
    fn flatten_fill_sign_for_save(&mut self, id: pdfcraft_engine::DocId) -> bool {
        if !self.flatten_fill_sign_on_save {
            return true;
        }
        let before = self.session.get(id).map(|d| d.edit_generation());
        if let Err(e) = self.session.apply(id, Edit::FlattenFillSign) {
            self.notify_error(e);
            return false;
        }
        let after = self.session.get(id).map(|d| d.edit_generation());
        if before != after
            && let Some(doc) = self.session.get(id)
            && let Some(view) = self.views.iter_mut().find(|v| v.id == id)
        {
            view.document_changed(&doc.info);
        }
        true
    }

    /// Whether tab `index` has work that isn't saved: edits, or form typing not committed yet.
    pub(crate) fn has_unsaved_work(&self, index: usize) -> bool {
        let Some(v) = self.views.get(index) else { return false };
        let Some(doc) = self.session.get(v.id) else { return false };
        doc.dirty
            || v.pending_edit.is_some()
            || v.content.blocked().is_some()
            || v.forms.focus.as_ref().is_some_and(|f| crate::forms_ui::draft_edit(f, &doc.form).is_some())
            || v.fill_text.as_ref().is_some_and(|t| !t.text.trim().is_empty())
            || v.line_editor.as_ref().map_or_else(|| false, |ed| ed.has_unsaved_text())
    }

    /// Save the active document. Returns `true` if it was written.
    pub fn save_active(&mut self, target: SaveTarget) -> bool {
        match self.active {
            Some(i) => self.save_view(i, target),
            None => false,
        }
    }

    /// Save the document shown in tab `index`. Returns `true` if it was written now. When it has
    /// to ask where (a new document, or Save As) it returns `false` and saves on a later frame,
    /// once the user has chosen.
    pub fn save_view(&mut self, index: usize, target: SaveTarget) -> bool {
        self.save_then(index, target, |_| {})
    }

    /// [`Self::save_view`], then `after` once the document is written: now, or on a later frame
    /// when the user had to choose where. `after` never runs when the save fails or is cancelled.
    pub(crate) fn save_then(&mut self, index: usize, target: SaveTarget, after: impl FnOnce(&mut Self) + Send + 'static) -> bool {
        // What's typed in a field is part of what's saved (#166).
        if let Some(id) = self.views.get(index).map(|v| v.id)
            && !self.commit_typing_in(id)
        {
            return false;
        }
        let Some(id) = self.views.get(index).map(|v| v.id) else { return false };
        let Some(doc) = self.session.get(id) else { return false };
        let (name, path) = (doc.name.clone(), doc.path.clone());
        let redacted = doc.has_unsaved_redaction();
        // Tests and automation choose the path, but redacted output still never replaces the
        // file the document came from.
        if redacted
            && let (Some(source), Some(p)) = (path.as_deref(), self.save_override.as_deref())
            && std::path::Path::new(source) == std::path::Path::new(p)
        {
            self.notify(format!("Couldn't save {name}: redacted output isn't saved over the original file. Choose another name."));
            return false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let destination = match (target, path, &self.save_override) {
                (_, _, Some(p)) => p.clone(),
                // Redacted output never silently replaces the original: ask where to save it (the
                // file dialog confirms before overwriting).
                (SaveTarget::InPlace, Some(p), _) if !redacted => p,
                _ => {
                    let name = if name.to_ascii_lowercase().ends_with(".pdf") { name } else { format!("{name}.pdf") };
                    let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(name);
                    // The bytes are taken once the user has chosen, so edits made meanwhile are saved too.
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, dest| {
                        if app.save_doc_to(id, &dest.to_string_lossy()) {
                            after(app);
                        }
                    });
                    return false;
                }
            };
            let saved = self.save_doc_to(id, &destination);
            if saved {
                after(self);
            }
            saved
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            if !self.flatten_fill_sign_for_save(id) {
                return false;
            }
            let bytes = match self.session.save_bytes(id) {
                Ok(b) => b,
                Err(e) => {
                    self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                    return false;
                }
            };
            // An embedding page that asked for saves (`?host=parent`) gets the bytes; otherwise
            // the browser downloads them. Either way, redacted output is checked before it
            // leaves the page: the browser has no file to protect.
            let to_host = self.host_save.is_some();
            if redacted && let Err(e) = self.session.verify_save(id, &bytes) {
                self.notify(format!("Couldn't download {name}: {e}"));
                return false;
            }
            let delivered = match &self.host_save {
                Some(save) => save(&name, &bytes, matches!(target, SaveTarget::As)),
                None => download(&name, &bytes),
            };
            match delivered {
                Ok(()) => {
                    let _ = self.session.mark_saved(id, bytes, None);
                    if let Some(doc) = self.session.get(id)
                        && let Some(view) = self.views.get_mut(index)
                    {
                        view.document_changed(&doc.info);
                    }
                    // The host reports where the file went (and any failure) itself.
                    if !to_host {
                        self.notify_fmt("Downloaded {name}", &[("name", &name)]);
                    }
                    after(self);
                    true
                }
                Err(e) if to_host => {
                    self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e)]);
                    false
                }
                Err(e) => {
                    self.notify_fmt("Couldn't download {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                    false
                }
            }
        }
    }

    /// Write document `id` to `dest` and make it the document's file. Returns `true` if written.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_doc_to(&mut self, id: pdfcraft_engine::DocId, dest: &str) -> bool {
        // Save As answers on a later frame: commit what was typed meanwhile, too (#166), even
        // if another tab is active by then.
        if !self.commit_typing_in(id) || !self.flatten_fill_sign_for_save(id) {
            return false;
        }
        let Some(name) = self.session.get(id).map(|d| d.name.clone()) else {
            self.notify_tr("The document was closed before it could be saved.");
            return false;
        };
        let bytes = match self.session.save_bytes(id) {
            Ok(b) => b,
            Err(e) => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                return false;
            }
        };
        let redacted = self.session.get(id).is_some_and(|d| d.has_unsaved_redaction());
        let written = if redacted {
            // Checked before anything is replaced; the file is owner-only until its owner decides.
            self.session
                .verify_save(id, &bytes)
                .map_err(|e| e.to_string())
                .and_then(|()| pdfcraft_engine::write_private_atomic(std::path::Path::new(dest), &bytes).map_err(|e| e.to_string()))
        } else {
            write_atomically(dest, &bytes).map_err(|e| e.to_string())
        };
        if let Err(e) = written {
            self.notify_fmt("Couldn't save {name}: {e}", &[("name", dest), ("e", &e.to_string())]);
            return false;
        }
        match self.session.mark_saved(id, bytes, Some(dest.to_string())) {
            Ok(()) => {
                self.forget_recovery(id);
                if let Some(doc) = self.session.get(id)
                    && let Some(view) = self.views.iter_mut().find(|v| v.id == id)
                {
                    view.document_changed(&doc.info);
                }
                self.notify_fmt("Saved {name}", &[("name", &short_name(dest))]);
                true
            }
            Err(e) => {
                self.notify_fmt("Saved, but reopening failed: {e}", &[("e", &e.to_string())]);
                false
            }
        }
    }

    /// Close a tab, asking first if it has unsaved changes.
    pub fn request_close_tab(&mut self, index: usize) {
        // Text typed into a field counts as an unsaved change.
        if self.has_unsaved_work(index)
            && let Some(id) = self.views.get(index).map(|v| v.id)
        {
            self.close_request = Some(CloseRequest::Tab(id));
        } else {
            self.close_tab(index);
        }
    }

    /// File ▸ Close all: clean documents close at once; each one with unsaved changes asks.
    pub fn close_all(&mut self) {
        for i in (0..self.views.len()).rev() {
            if !self.has_unsaved_work(i) {
                self.close_tab(i);
            }
        }
        if self.first_dirty().is_some() {
            self.close_request = Some(CloseRequest::All);
        }
    }

    /// File ▸ Revert (after the confirmation).
    pub fn revert_active(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        match self.session.revert(id) {
            Ok(()) => {
                if let Some(i) = self.active
                    && let Some(d) = self.session.get(id)
                {
                    let view = &mut self.views[i];
                    view.document_changed(&d.info);
                    // Revert discards this source's uncommitted typing as well as model edits.
                    // Otherwise a later Save could write a rejected draft back into the file.
                    view.forms.focus = None;
                    view.forms.committed = None;
                    view.pending_edit = None;
                }
                self.notify_tr("Reverted to the last saved version");
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The first tab with unsaved changes.
    pub fn first_dirty(&self) -> Option<usize> {
        (0..self.views.len()).find(|&i| self.has_unsaved_work(i))
    }

    /// Answer the save prompt: `Some(true)` save, `Some(false)` discard, `None` cancel.
    pub fn resolve_close(&mut self, ctx: &egui::Context, choice: Option<bool>) {
        let Some(req) = self.close_request.take() else { return };
        let index = match req {
            CloseRequest::Tab(id) => match self.views.iter().position(|v| v.id == id) {
                Some(i) => i,
                None => return, // already closed
            },
            CloseRequest::Quit | CloseRequest::All => match self.first_dirty() {
                Some(i) => i,
                None => {
                    if req == CloseRequest::Quit {
                        self.quit(ctx);
                    }
                    return;
                }
            },
        };
        // Quitting closes the unsaved tabs one by one (and brings each forward to save it):
        // remember what was open, and which tab was active, before the first one goes (#442).
        if req == CloseRequest::Quit {
            match choice {
                Some(_) => self.note_quit_session(),
                None => self.forget_quit_session(),
            }
        }
        match choice {
            None => {} // cancelled: nothing closes
            Some(false) => self.close_and_continue(ctx, index, req),
            Some(true) => {
                let Some(id) = self.views.get(index).map(|v| v.id) else { return };
                // Bring the document forward: what's typed in its form is committed and saved
                // with it, and a field that rejects the value is shown where the user can fix it.
                self.active = Some(index);
                // Close only once the save has actually been written, which may be on a later
                // frame when the user has to choose where. A failed or cancelled save keeps the
                // document open.
                let ctx = ctx.clone();
                self.save_then(index, SaveTarget::InPlace, move |app| {
                    if let Some(i) = app.views.iter().position(|v| v.id == id) {
                        app.close_and_continue(&ctx, i, req);
                    }
                });
            }
        }
    }

    /// Close tab `index` for a close request, then ask about the next dirty document, or quit.
    fn close_and_continue(&mut self, ctx: &egui::Context, index: usize, req: CloseRequest) {
        self.close_tab(index);
        // A newer prompt wins: the user may have started another close while a save was pending.
        if (req == CloseRequest::Quit || req == CloseRequest::All) && self.close_request.is_none() {
            match self.first_dirty() {
                Some(_) => self.close_request = Some(req),
                None if req == CloseRequest::Quit => self.quit(ctx),
                None => {}
            }
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.allow_quit = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Publish whether any document has unsaved work, for the browser's reload warning (#812).
    pub(crate) fn sync_unsaved_flag(&self) {
        self.unsaved_flag.store(self.first_dirty().is_some(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Intercept window close while documents have unsaved changes.
    pub(crate) fn guard_quit(&mut self, ctx: &egui::Context) {
        self.sync_unsaved_flag();
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        // `first_dirty` counts text typed into a field as an unsaved change.
        if !self.allow_quit && self.first_dirty().is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_request = Some(CloseRequest::Quit);
        } else {
            // A clean quit: nothing is left to recover.
            self.shutdown_recovery();
        }
    }
}

/// The page a comment edit changes (`None` for other edits).
fn comment_page(edit: &Edit) -> Option<usize> {
    match edit {
        Edit::AddAnnotation(a) => Some(a.page),
        Edit::DeleteAnnotation { page, .. }
        | Edit::SetAnnotationContents { page, .. }
        | Edit::ReplyToAnnotation { page, .. }
        | Edit::SetAnnotationStatus { page, .. }
        | Edit::MoveAnnotation { page, .. }
        | Edit::ResizeAnnotation { page, .. }
        | Edit::StyleAnnotation { page, .. }
        | Edit::FillAnnotation { page, .. }
        | Edit::SetAnnotationInfo { page, .. } => Some(*page),
        _ => None,
    }
}

/// Write via a temporary file in the same directory and rename over the target, so a crash or
/// full disk never leaves a half-written PDF where the original was.
pub fn write_atomically(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    write_atomically_with(path, bytes, staging_suffixes())
}

/// [`write_atomically`], trying the staging names that `suffixes` give.
fn write_atomically_with(path: &str, bytes: &[u8], suffixes: impl IntoIterator<Item = u64>) -> std::io::Result<()> {
    use std::io::Write;
    let target = std::path::Path::new(path);
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("save");
    let (tmp, f) = create_staging(dir, name, StagingName::TagThenSuffix, suffixes)?;
    let result = (|| {
        // Closed at the end of the block, before the rename.
        {
            let mut f = f;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(not(target_arch = "wasm32"))]
fn short_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// Offer bytes as a browser download.
#[cfg(target_arch = "wasm32")]
pub(crate) fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&array);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/pdf");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "anchor")?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

impl PdfCraftApp {
    /// Carry out a Bookmarks-panel action as an undoable edit.
    pub fn bookmark_action(&mut self, action: crate::panels::BmAction) {
        use crate::panels::BmAction as A;
        let Some((i, id)) = self.active_ids() else { return };
        let current = self.views[i].current;
        let parent_of = |p: &[usize]| p[..p.len() - 1].to_vec();
        let edit = match action {
            A::New => {
                let n = self.session.get(id).map_or(0, |d| d.info.outline.len());
                self.apply_edit(Edit::AddBookmark { parent: vec![], index: n, title: "Untitled".into(), page: current });
                // Like Acrobat: the new bookmark starts in rename mode.
                self.bookmark_rename = Some((vec![n], "Untitled".into()));
                self.right = Some(crate::RightPanel::Bookmarks);
                return;
            }
            A::StartRename(path) => {
                let title = self.session.get(id).and_then(|d| bookmark_at(&d.info.outline, &path)).map(|b| b.title.clone()).unwrap_or_default();
                self.bookmark_rename = Some((path, title));
                return;
            }
            A::Rename(path, title) => Edit::RenameBookmark { path, title },
            A::SetToCurrentPage(path) => Edit::SetBookmarkPage { path, page: current },
            A::Delete(path) => Edit::DeleteBookmark { path },
            A::MoveUp(path) => {
                let at = path[path.len() - 1].saturating_sub(1);
                Edit::MoveBookmark { to_parent: parent_of(&path), from: path, index: at }
            }
            A::MoveDown(path) => {
                let at = path[path.len() - 1] + 1;
                Edit::MoveBookmark { to_parent: parent_of(&path), from: path, index: at }
            }
            A::Indent(path) => {
                let mut to_parent = parent_of(&path);
                to_parent.push(path[path.len() - 1].saturating_sub(1));
                Edit::MoveBookmark { from: path, to_parent, index: usize::MAX }
            }
            A::Outdent(path) => {
                let parent = parent_of(&path);
                let grand = parent_of(&parent);
                let at = parent[parent.len() - 1] + 1;
                Edit::MoveBookmark { from: path, to_parent: grand, index: at }
            }
            A::FromStructure => {
                self.right = Some(crate::RightPanel::Bookmarks);
                Edit::BookmarksFromStructure
            }
        };
        self.apply_edit(edit);
    }
}

fn bookmark_at<'a>(items: &'a [pdfcraft_render::OutlineItem], path: &[usize]) -> Option<&'a pdfcraft_render::OutlineItem> {
    let (first, rest) = path.split_first()?;
    let item = items.get(*first)?;
    if rest.is_empty() { Some(item) } else { bookmark_at(&item.children, rest) }
}

#[cfg(test)]
mod tests {
    use super::{write_atomically, write_atomically_with};
    use pdfcraft_platform::staging::{STAGING_ATTEMPTS, staging_suffixes};
    use std::path::{Path, PathBuf};

    /// A fresh, empty folder for one staging test.
    fn staging_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ui-staging-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn staged(dir: &Path, suffix: u64) -> PathBuf {
        dir.join(format!(".out.pdf.pdfcraft-{suffix:016x}.tmp"))
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    /// A symbolic link to a file, where the system allows one (Windows needs Developer Mode or an
    /// administrator for it).
    fn file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(std::io::Error::other(format!("no symbolic links here: {} {}", target.display(), link.display())))
        }
    }

    #[test]
    fn saving_never_writes_through_a_file_planted_at_the_staging_name() {
        let dir = staging_dir("planted");
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, "PRECIOUS").unwrap();
        let target = dir.join("out.pdf");
        let path = target.to_string_lossy().into_owned();
        // A hard link needs no privileges on any system, and writing to it writes to `outside`.
        std::fs::hard_link(&outside, staged(&dir, 1)).unwrap();
        std::fs::write(staged(&dir, 2), "PLANTED").unwrap();
        write_atomically_with(&path, b"NEW", [1, 2, 3]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert_eq!(read(&outside), "PRECIOUS");
        assert_eq!(read(&staged(&dir, 1)), "PRECIOUS");
        assert_eq!(read(&staged(&dir, 2)), "PLANTED");
        assert!(!staged(&dir, 3).exists(), "the staging file was renamed into place");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_never_writes_through_a_symbolic_link_at_the_staging_name() {
        let dir = staging_dir("symlink");
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, "PRECIOUS").unwrap();
        let target = dir.join("out.pdf");
        if let Err(e) = file_symlink(&outside, &staged(&dir, 1)) {
            eprintln!("symbolic links not checked: {e}");
            return;
        }
        write_atomically_with(&target.to_string_lossy(), b"NEW", [1, 2]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert_eq!(read(&outside), "PRECIOUS");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_never_creates_a_file_through_a_dangling_link_at_the_staging_name() {
        let dir = staging_dir("dangling");
        let unborn = dir.join("created-through-a-link.txt");
        let target = dir.join("out.pdf");
        if let Err(e) = file_symlink(&unborn, &staged(&dir, 1)) {
            eprintln!("symbolic links not checked: {e}");
            return;
        }
        write_atomically_with(&target.to_string_lossy(), b"NEW", [1, 2]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert!(!unborn.exists(), "nothing was created through the dangling link");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_at_the_staging_name_is_left_alone() {
        let dir = staging_dir("folder");
        std::fs::create_dir(staged(&dir, 1)).unwrap();
        std::fs::write(staged(&dir, 1).join("inside.txt"), "PLANTED").unwrap();
        let target = dir.join("out.pdf");
        write_atomically_with(&target.to_string_lossy(), b"NEW", [1, 2]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert_eq!(read(&staged(&dir, 1).join("inside.txt")), "PLANTED");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_gives_up_rather_than_reuse_a_taken_name() {
        let dir = staging_dir("taken");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        for s in 1..=STAGING_ATTEMPTS as u64 {
            std::fs::write(staged(&dir, s), "PLANTED").unwrap();
        }
        assert!(write_atomically_with(&target.to_string_lossy(), b"NEW", 1..).is_err());
        assert_eq!(read(&target), "OLD");
        for s in 1..=STAGING_ATTEMPTS as u64 {
            assert_eq!(read(&staged(&dir, s)), "PLANTED");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The names in a folder: what a test can see was left behind.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn saving_a_long_name_leaves_only_the_saved_file() {
        assert_ne!(staging_suffixes().next(), staging_suffixes().next(), "each save draws new names");
        // 60 four-byte characters: a 244-byte name, within every system's limit. Its staging name
        // must be too (on Linux the whole name in it would be 275 bytes).
        let dir = staging_dir("clean");
        let name = format!("{}.pdf", "\u{1F600}".repeat(60));
        let path = dir.join(&name).to_string_lossy().into_owned();
        write_atomically(&path, b"NEW").unwrap();
        write_atomically(&path, b"NEWER").unwrap();
        assert_eq!(read(&dir.join(&name)), "NEWER");
        assert_eq!(listing(&dir), [name], "no staging file is left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows refuses to replace a read-only file, so the rename fails: the staging file must not
    /// be left behind.
    #[cfg(windows)]
    #[test]
    fn a_failed_save_removes_the_staging_file() {
        let dir = staging_dir("readonly");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        let mut perms = std::fs::metadata(&target).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&target, perms.clone()).unwrap();
        let e = write_atomically_with(&target.to_string_lossy(), b"NEW", [7]).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "the rename failed: {e}");
        assert_eq!(read(&target), "OLD");
        assert_eq!(listing(&dir), ["out.pdf"], "the staging file was removed");
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&target, perms).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod revert_draft_tests {
    use super::*;
    use crate::forms_ui::Focus;
    use pdfcraft_engine::FieldValue;

    fn focus(text: &str) -> Focus {
        Focus {
            name: "name".into(),
            widget: 0,
            text: text.into(),
            picked: Vec::new(),
            request_focus: false,
            select_all: false,
            calendar: None,
            calendar_rect: None,
        }
    }

    fn app() -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.open_bytes("source.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        app
    }

    fn queue_draft(app: &mut PdfCraftApp, index: usize, text: &str) {
        app.views[index].forms.focus = Some(focus(text));
        let form = app.session.get(app.views[index].id).unwrap().form.clone();
        crate::forms_ui::commit(&mut app.views[index], &form);
        assert!(app.views[index].forms.committed.is_some() && app.views[index].pending_edit.is_some());
    }

    #[test]
    fn unsaved_flag_follows_dirty_state() {
        use std::sync::atomic::Ordering;
        // `guard_quit` runs every frame, also on the web, where no close request ever arrives.
        let ctx = egui::Context::default();
        let mut app = app();
        app.guard_quit(&ctx);
        assert!(!app.unsaved_flag.load(Ordering::Relaxed));
        assert!(app.apply_edit(Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Changed".into()) }));
        app.guard_quit(&ctx);
        assert!(app.unsaved_flag.load(Ordering::Relaxed));
    }

    #[test]
    fn successful_revert_clears_focused_or_queued_drafts_only_on_its_document() {
        for queued in [false, true] {
            let mut app = app();
            let source = app.views[0].id;
            assert!(app.apply_edit(Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Committed change".into()) }));
            if queued {
                queue_draft(&mut app, 0, "Queued source draft");
            } else {
                app.views[0].forms.focus = Some(focus("Focused source draft"));
            }
            app.open_bytes("other.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            let other = app.views[1].id;
            assert!(app.apply_edit(Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Other committed value".into()) }));
            queue_draft(&mut app, 1, "Other queued draft");
            app.views[1].forms.focus = Some(focus("Other focused draft"));
            let other_bytes = app.session.get(other).unwrap().bytes.clone();
            let other_generation = app.session.get(other).unwrap().edit_generation();
            let source_generation = app.session.get(source).unwrap().edit_generation();
            app.active = Some(0);
            app.revert_active();
            let reverted = app.session.get(source).unwrap();
            assert!(!reverted.dirty);
            assert_eq!(reverted.edit_generation(), source_generation + 1);
            assert!(reverted.form.iter().find(|field| field.name == "name").unwrap().value.is_empty());
            assert!(app.views[0].forms.focus.is_none() && app.views[0].forms.committed.is_none() && app.views[0].pending_edit.is_none());
            assert!(!app.has_unsaved_work(0));
            assert_eq!(app.views[1].forms.focus.as_ref().unwrap().text, "Other focused draft");
            assert_eq!(app.views[1].forms.committed.as_ref().unwrap().text, "Other queued draft");
            assert!(
                matches!(&app.views[1].pending_edit, Some(Edit::SetFieldValue { name, value: FieldValue::Text(text) }) if name == "name" && text == "Other queued draft")
            );
            let untouched = app.session.get(other).unwrap();
            assert_eq!(untouched.bytes, other_bytes);
            assert_eq!(untouched.edit_generation(), other_generation);
            assert!(untouched.dirty && app.has_unsaved_work(1));
            assert_eq!(app.active_ids(), Some((0, source)));
        }
    }

    #[test]
    fn failed_revert_preserves_focused_and_queued_drafts() {
        let mut app = app();
        let source = app.views[0].id;
        queue_draft(&mut app, 0, "Queued draft");
        app.views[0].forms.focus = Some(focus("Focused draft"));
        // Exercise the ordinary NoDocument error boundary without a corrupt fixture.
        // This verifies UI failure preservation, not other engine refresh failures.
        app.session.close(source);
        app.revert_active();
        assert_eq!(app.views[0].forms.focus.as_ref().unwrap().text, "Focused draft");
        assert_eq!(app.views[0].forms.committed.as_ref().unwrap().text, "Queued draft");
        assert!(
            matches!(&app.views[0].pending_edit, Some(Edit::SetFieldValue { name, value: FieldValue::Text(text) }) if name == "name" && text == "Queued draft")
        );
        assert_eq!(app.active_ids(), Some((0, source)));
        assert_eq!(app.toast.as_ref().unwrap().0, pdfcraft_engine::EditError::NoDocument.to_string());
    }
}
