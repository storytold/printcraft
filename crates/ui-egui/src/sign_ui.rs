//! Use a certificate (execution plan M9.6): Digitally sign and Certify (drag a rectangle, or
//! click an empty signature field), the Sign with a Digital ID / Configure New Digital ID /
//! Sign as dialogs, the Signatures panel, and the signature message bar.

use std::path::PathBuf;

use egui::{Align, Color32, CornerRadius, Layout, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::sign::{self, Appearance, Certificate, DigitalId, Modification, Name, PrivateKey};
use pdfcraft_engine::{SignOptions, SignatureInfo, SignatureReport, SignatureStatus};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons, widgets};

/// A digital ID the app knows about (Acrobat: Digital ID files). The file stays where it is;
/// its password is asked for at each signing.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DigitalIdEntry {
    pub path: String,
    pub name: String,
    pub issuer: String,
    pub email: String,
    pub expires: String,
    /// Why the ID can't sign (a Windows store certificate whose key PdfCraft can't use): shown
    /// greyed with the reason, and never selectable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unusable: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignStep {
    /// Sign with a Digital ID: pick one.
    Choose,
    /// Configure a Digital ID for signing: from a file or a new self-signed one.
    Configure,
    /// Sign as "Name": appearance, password, Sign.
    SignAs,
}

/// The key algorithms of a new digital ID (Acrobat offers RSA; P-256 also works in browsers).
pub const KEY_ALGORITHMS: [(&str, &str); 4] =
    [("2048-bit RSA", "rsa2048"), ("3072-bit RSA", "rsa3072"), ("4096-bit RSA", "rsa4096"), ("256-bit ECDSA (P-256)", "p256")];

#[derive(Clone, Debug, PartialEq)]
pub struct NewIdDraft {
    /// `true`: create a self-signed ID; `false`: use an ID from a file.
    pub create: bool,
    pub name: String,
    pub unit: String,
    pub organization: String,
    pub email: String,
    pub country: String,
    pub key: usize,
    pub password: String,
    pub confirm: String,
    /// Use a Digital ID from a file: its path and password.
    pub file: String,
    pub file_password: String,
}

impl Default for NewIdDraft {
    fn default() -> Self {
        Self {
            create: true,
            name: String::new(),
            unit: String::new(),
            organization: String::new(),
            email: String::new(),
            country: String::new(),
            key: 0,
            password: String::new(),
            confirm: String::new(),
            file: String::new(),
            file_password: String::new(),
        }
    }
}

/// The signing dialogs' state.
#[derive(Clone, Debug, PartialEq)]
pub struct SignDraft {
    pub page: usize,
    /// The signature rectangle (user space); `None`: invisible.
    pub rect: Option<[f64; 4]>,
    /// Sign this empty signature field instead.
    pub field: Option<String>,
    /// Certify (DocMDP) with these permissions: 1 none, 2 form fill and signing, 3 also comments.
    pub certify: Option<u8>,
    pub step: SignStep,
    pub selected: Option<usize>,
    pub password: String,
    pub reason: String,
    pub location: String,
    pub appearance: Appearance,
    pub new_id: NewIdDraft,
    pub error: Option<String>,
}

impl SignDraft {
    /// Starts at Choose when there are IDs to show (even only unusable ones, so their reasons
    /// are seen), with the first usable one selected.
    fn new(page: usize, rect: Option<[f64; 4]>, field: Option<String>, certify: Option<u8>, ids: &[DigitalIdEntry]) -> Self {
        Self {
            page,
            rect,
            field,
            certify,
            step: if ids.is_empty() { SignStep::Configure } else { SignStep::Choose },
            selected: ids.iter().position(|e| e.unusable.is_none()),
            password: String::new(),
            reason: String::new(),
            location: String::new(),
            appearance: Appearance::default(),
            new_id: NewIdDraft::default(),
            error: None,
        }
    }
}

/// What a page asks of the signing flow.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignView {
    /// A signature rectangle being dragged: (page, start on screen).
    pub drag: Option<(usize, Pos2)>,
    /// A rectangle was drawn: (page, user-space rect).
    pub drawn: Option<(usize, [f64; 4])>,
    /// An empty signature field was clicked (its name).
    pub field: Option<String>,
}

/// Drag a signature rectangle on one page (Digitally sign, Certify (visible signature)).
pub(crate) fn page_input(ui: &egui::Ui, resp: &egui::Response, xf: &PageXform, page: usize, info: &pdfcraft_render::DocInfo, view: &mut DocView) {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let origin = ui.input(|i| i.pointer.press_origin());
    if pointer.is_some_and(|p| xf.rect.contains(p)) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if resp.drag_started()
        && let Some(o) = origin.filter(|o| xf.rect.contains(*o))
    {
        view.sign.drag = Some((page, o));
    }
    if let Some((p, start)) = view.sign.drag
        && p == page
        && let Some(end) = pointer
    {
        let end = Pos2::new(end.x.clamp(xf.rect.left(), xf.rect.right()), end.y.clamp(xf.rect.top(), xf.rect.bottom()));
        let r = Rect::from_two_pos(start, end);
        ui.painter().rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(0x14, 0x73, 0xE6, 28));
        ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_rgb(0x14, 0x73, 0xE6)), egui::StrokeKind::Middle);
        if resp.drag_stopped() {
            view.sign.drag = None;
            if r.width() >= 8.0 && r.height() >= 8.0 {
                let user = |q: Pos2| {
                    let (vx, vy) = xf.screen_to_view(q);
                    info.pages[page].view_to_user(vx, vy)
                };
                let (a, b) = (user(r.left_top()), user(r.right_bottom()));
                let rect = [a[0].min(b[0]) as f64, a[1].min(b[1]) as f64, a[0].max(b[0]) as f64, a[1].max(b[1]) as f64];
                view.sign.drawn = Some((page, rect));
            }
        }
    }
}

/// Where new digital IDs are saved: next to the recovery folder (`…/PdfCraft/Digital IDs`, or
/// `PdfCraftData/Digital IDs` in portable mode).
fn id_dir() -> Option<PathBuf> {
    crate::recovery::RecoveryStore::default_dir().and_then(|d| d.parent().map(|p| p.join("Digital IDs")))
}

/// Save a new digital ID as `<stem>.p12`, or `<stem> 2.p12`, … when the name is taken. The file
/// is always created fresh, never opened through a file or link already at the name (checking
/// first and then writing would let one be planted in between), and on Unix only its owner can
/// read it: it holds the private key.
fn save_new_id_file(dir: &std::path::Path, stem: &str, p12: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::{ErrorKind, Write};
    for i in 1..=10_000u32 {
        let path = dir.join(if i == 1 { format!("{stem}.p12") } else { format!("{stem} {i}.p12") });
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let file = match opts.open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            // Windows refuses `create_new` on a folder with "access denied"; the name is taken all
            // the same. A folder we can't write to, with nothing at the name, still fails here.
            Err(e) if e.kind() == ErrorKind::PermissionDenied && path.symlink_metadata().is_ok() => continue,
            Err(e) => return Err(e),
        };
        // The block closes the file before a failed one is removed (Windows can't remove an open file).
        let written = {
            let mut file = file;
            file.write_all(p12).and_then(|()| file.sync_all())
        };
        if let Err(e) = written {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        return Ok(path);
    }
    Err(std::io::Error::new(ErrorKind::AlreadyExists, "no free file name for the digital ID"))
}

/// A Keychain identity by its `keychain:` reference.
fn keychain_id(reference: &str) -> Result<DigitalId, String> {
    #[cfg(target_os = "macos")]
    return sign::keychain::find(reference).map_err(|e| e.to_string());
    #[cfg(not(target_os = "macos"))]
    Err(format!("{reference}: Keychain identities are only available on macOS"))
}

/// A Windows store identity by its `windows:` reference.
fn windows_id(reference: &str) -> Result<DigitalId, String> {
    #[cfg(target_os = "windows")]
    return sign::windows::find(reference).map_err(|e| e.to_string());
    #[cfg(not(target_os = "windows"))]
    Err(format!("{reference}: Windows certificate store identities are only available on Windows"))
}

pub fn entry_for(path: &str, c: &Certificate) -> DigitalIdEntry {
    DigitalIdEntry {
        path: path.to_string(),
        name: c.display_name(),
        issuer: c.issuer.common_name().map(str::to_string).unwrap_or_else(|| c.issuer.display()),
        email: c.subject.email().unwrap_or("").to_string(),
        expires: format!("{:04}.{:02}.{:02}", c.not_after.year, c.not_after.month, c.not_after.day),
        unusable: None,
    }
}

/// A greyed entry for a Windows store certificate that can't sign, with the reason.
#[cfg(target_os = "windows")]
fn entry_unusable(u: &sign::windows::Unusable) -> DigitalIdEntry {
    DigitalIdEntry {
        path: format!("windows:{}", u.fingerprint.split(' ').collect::<String>().to_ascii_lowercase()),
        name: u.subject.clone(),
        issuer: String::new(),
        email: String::new(),
        expires: String::new(),
        unusable: Some(u.reason.clone()),
    }
}

impl PdfCraftApp {
    /// Start signing: the rectangle (or field) is known; show Sign with a Digital ID.
    pub fn start_signing(&mut self, page: usize, rect: Option<[f64; 4]>, field: Option<String>, certify: Option<u8>) {
        self.refresh_os_key_store_ids();
        self.sign_draft = Some(SignDraft::new(page, rect, field, certify, &self.digital_ids));
        self.dialog = Some(crate::Dialog::Sign);
    }

    /// List OS key store signing identities (after the file-based IDs).
    fn refresh_os_key_store_ids(&mut self) {
        self.digital_ids.retain(|e| !e.path.starts_with("keychain:") && !e.path.starts_with("windows:"));
        #[cfg(target_os = "macos")]
        if self.os_key_store_ids {
            match sign::keychain::identities(None) {
                Ok(ids) => {
                    for id in ids {
                        self.digital_ids.push(entry_for(&sign::keychain::reference(&id.certificate), &id.certificate));
                    }
                }
                Err(e) => self.notify_fmt("The Keychain's digital IDs couldn't be listed: {e}", &[("e", &e.to_string())]),
            }
        }
        #[cfg(target_os = "windows")]
        if self.os_key_store_ids {
            match sign::windows::list() {
                Ok(listing) => {
                    for id in &listing.ids {
                        self.digital_ids.push(entry_for(&sign::windows::reference(&id.certificate), &id.certificate));
                    }
                    // Certificates without a private key aren't identities; the rest are shown
                    // greyed with why they can't sign (issue #179).
                    for u in listing.unusable.iter().filter(|u| !u.no_private_key) {
                        self.digital_ids.push(entry_unusable(u));
                    }
                }
                Err(e) => self.notify_fmt("The Windows store's digital IDs couldn't be listed: {e}", &[("e", &e.to_string())]),
            }
        }
    }

    /// Add a digital ID file to the list (or select it if it's there).
    pub fn add_digital_id(&mut self, path: &str, cert: &Certificate) -> usize {
        if let Some(i) = self.digital_ids.iter().position(|e| e.path == path) {
            self.digital_ids[i] = entry_for(path, cert);
            return i;
        }
        self.digital_ids.push(entry_for(path, cert));
        self.digital_ids.len() - 1
    }

    /// Configure New Digital ID ▸ Create: a self-signed ID saved as a .p12 next to the app data
    /// (or in `export_dir_override`). Returns its list index.
    fn create_digital_id(&mut self) -> Result<usize, String> {
        let Some(d) = self.sign_draft.as_ref().map(|d| d.new_id.clone()) else { return Err("nothing to create".into()) };
        if d.name.trim().is_empty() {
            return Err("Enter a name.".into());
        }
        if d.password.chars().count() < 6 {
            return Err("The password must have at least 6 characters.".into());
        }
        if d.password != d.confirm {
            return Err("The passwords do not match.".into());
        }
        let key = match KEY_ALGORITHMS[d.key.min(KEY_ALGORITHMS.len() - 1)].1 {
            "rsa3072" => PrivateKey::generate_rsa(3072),
            "rsa4096" => PrivateKey::generate_rsa(4096),
            "p256" => PrivateKey::generate_p256(),
            _ => PrivateKey::generate_rsa(2048),
        }
        .map_err(|e| e.to_string())?;
        let now = self.session.now_secs();
        let dn = Name::build(&d.name, &d.unit, &d.organization, &d.email, &d.country);
        let serial = sign::keys::DigestAlg::Sha256.digest(&[d.name.as_bytes(), &now.to_be_bytes()])[..8].to_vec();
        let cert = Certificate::self_signed(&dn, &key, sign::Time::from_unix(now), 5, &serial).map_err(|e| e.to_string())?;
        let id = DigitalId { key, certificate: cert.clone(), chain: Vec::new(), friendly_name: Some(d.name.trim().to_string()) };
        let p12 = sign::pkcs12::write(&id, &d.password).map_err(|e| e.to_string())?;
        let dir = match &self.export_dir_override {
            Some(p) => Some(PathBuf::from(p)),
            None => id_dir(),
        }
        .ok_or("There is no folder to save the digital ID in.")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let stem: String = d.name.trim().chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
        let path = save_new_id_file(&dir, &stem, &p12).map_err(|e| e.to_string())?;
        Ok(self.add_digital_id(&path.to_string_lossy(), &cert))
    }

    /// Configure New Digital ID ▸ Use a Digital ID from a file.
    fn import_digital_id(&mut self) -> Result<usize, String> {
        let Some(d) = self.sign_draft.as_ref().map(|d| d.new_id.clone()) else { return Err("nothing to import".into()) };
        let bytes = std::fs::read(&d.file).map_err(|e| format!("{}: {e}", d.file))?;
        let id = sign::pkcs12::open(&bytes, &d.file_password).map_err(|e| match e {
            sign::SignError::WrongPassword => "The password is incorrect.".to_string(),
            e => e.to_string(),
        })?;
        Ok(self.add_digital_id(&d.file, &id.certificate))
    }

    /// Sign as …: open the ID, sign, save the signed file (Save As), and show it.
    fn finish_signing(&mut self) -> Result<(), String> {
        // What's typed in a form field is signed with the document (#166). A rejected value keeps
        // the dialog open; the notice says why.
        if !self.commit_form_typing() {
            return Err(String::new());
        }
        let Some((_, doc_id)) = self.active_ids() else { return Err("no document".into()) };
        let d = self.sign_draft.clone().ok_or("nothing to sign")?;
        let entry = d.selected.and_then(|i| self.digital_ids.get(i)).filter(|e| e.unusable.is_none()).cloned().ok_or("Choose a digital ID.")?;
        let id = if entry.path.starts_with("keychain:") {
            keychain_id(&entry.path)?
        } else if entry.path.starts_with("windows:") {
            windows_id(&entry.path)?
        } else {
            let bytes = std::fs::read(&entry.path).map_err(|e| format!("{}: {e}", entry.path))?;
            sign::pkcs12::open(&bytes, &d.password).map_err(|e| match e {
                sign::SignError::WrongPassword => "The password is incorrect.".to_string(),
                e => e.to_string(),
            })?
        };
        let some = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        let opts = SignOptions {
            field: d.field.clone(),
            page: d.page,
            rect: d.rect,
            reason: some(&d.reason),
            location: some(&d.location),
            certify: d.certify,
            appearance: d.appearance.clone(),
            ..SignOptions::default()
        };
        // Signing saves, as in Acrobat: choose where (a cancelled save cancels signing). The
        // document is signed once the user has chosen.
        let name = self.session.get(doc_id).map(|d| d.name.clone()).unwrap_or_default();
        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let sign_and_save = move |app: &mut Self, path: PathBuf| -> Result<(), String> {
            // Sign what was typed while the save panel was open, too (#166); a refused value
            // has said why already.
            if !app.commit_typing_in(doc_id) {
                return Err(String::new());
            }
            let signed = app.session.sign(doc_id, &id, opts).map_err(|e| e.to_string())?;
            crate::editing::write_atomically(&path.to_string_lossy(), signed.as_slice()).map_err(|e| format!("Could not save: {e}"))?;
            app.session.mark_signed(doc_id, signed, Some(path.to_string_lossy().into_owned())).map_err(|e| e.to_string())?;
            if let Some(view) = app.views.iter_mut().find(|v| v.id == doc_id) {
                view.invalidate_content();
            }
            app.right = Some(crate::RightPanel::Signatures);
            app.notify_fmt("Signed and saved to {path}", &[("path", &path.display().to_string())]);
            Ok(())
        };
        match self.save_override.clone() {
            Some(p) => sign_and_save(self, PathBuf::from(p)),
            #[cfg(not(target_arch = "wasm32"))]
            None => {
                let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(format!("{stem}_signed.pdf"));
                // The signature's page, rectangle and field refer to the document as it is now:
                // the pick is dropped (with a notice) if it is edited or switched away from
                // meanwhile. Once the picker shows, the dialog closes; an error from here on
                // arrives as a notice. If no picker could show, the dialog stays open.
                let asked = self.ask_one(crate::pickers::Ask::Save(dialog), Some(doc_id), move |app, path| {
                    if let Err(e) = sign_and_save(app, path)
                        && !e.is_empty()
                    {
                        app.notify(e);
                    }
                });
                if asked { Ok(()) } else { Err(String::new()) }
            }
            #[cfg(target_arch = "wasm32")]
            None => {
                let _ = (stem, sign_and_save);
                Err(String::new())
            }
        }
    }

    /// Trust a certificate (Signatures panel ▸ Add to trusted certificates), revalidating.
    pub fn trust_certificate(&mut self, cert: Certificate) {
        let mut certs = self.session.trusted_certificates().to_vec();
        if !certs.iter().any(|c| c.raw == cert.raw) {
            certs.push(cert);
        }
        self.session.set_trusted_certificates(certs);
    }

    /// Trust the bundled CCA India roots (Signatures panel ▸ Verify with CCA India
    /// roots) and revalidate every open document.
    pub fn trust_cca_roots(&mut self) {
        match self.session.trust_cca_roots() {
            Ok(0) => self.notify_tr("The CCA India roots are already trusted."),
            Ok(_) => {
                self.right = Some(crate::RightPanel::Signatures);
                self.notify_tr("The CCA India roots are now trusted. Signatures were revalidated.");
            }
            Err(e) => self.notify(e.to_string()),
        }
    }

    /// Open saved revision `n` (1 = the oldest) of the active document as a new document.
    pub fn open_revision(&mut self, n: usize) {
        let Some((_, id)) = self.active_ids() else { return };
        match self.session.open_revision(id, n) {
            Ok(new) => {
                let Some(doc) = self.session.get(new) else { return };
                self.views.push(DocView::new(new, &doc.info, self.view_defaults));
                self.active = Some(self.views.len() - 1);
            }
            Err(e) => self.notify_fmt("Couldn't open revision {n}: {e}", &[("n", &n.to_string()), ("e", &e.to_string())]),
        }
    }

    /// Open the signed version of a signature as a new document (View signed version).
    pub fn view_signed_version(&mut self, len: usize) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let bytes = doc.bytes[..len.min(doc.bytes.len())].to_vec();
        let name = format!("{} (signed version)", doc.name.trim_end_matches(".pdf"));
        match self.session.open(format!("{name}.pdf"), None, std::sync::Arc::new(bytes), doc.password.clone().as_deref()) {
            Ok(new) => {
                let Some(doc) = self.session.get(new) else { return };
                self.views.push(DocView::new(new, &doc.info, self.view_defaults));
                self.active = Some(self.views.len() - 1);
            }
            Err(e) => self.notify_fmt("Couldn't open the signed version: {e}", &[("e", &e.to_string())]),
        }
    }
}

/// Draw the signing dialogs; returns `true` when the dialog should close.
pub(crate) fn dialog(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let Some(step) = app.sign_draft.as_ref().map(|d| d.step) else { return true };
    match step {
        SignStep::Choose => choose(ui, app, t),
        SignStep::Configure => configure(ui, app, t),
        SignStep::SignAs => sign_as(ui, app, t),
    }
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(tl!(text)).font(theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(8.0);
}

fn error(ui: &mut egui::Ui, err: &Option<String>) {
    if let Some(e) = err.as_ref().filter(|e| !e.is_empty()) {
        ui.colored_label(Color32::from_rgb(0xD7, 0x37, 0x3F), e);
        ui.add_space(4.0);
    }
}

fn choose(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    title(ui, "Sign with a Digital ID");
    ui.label(tl!("Choose the digital ID that you want to use for signing:"));
    ui.add_space(6.0);
    let ids = app.digital_ids.clone();
    let Some(d) = app.sign_draft.as_mut() else { return true };
    let mut close = false;
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        for (i, e) in ids.iter().enumerate() {
            let selected = d.selected == Some(i);
            let usable = e.unusable.is_none();
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), egui::Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, usable, selected, &e.name));
            let fill = if selected {
                t.accent_soft
            } else if resp.hovered() && usable {
                t.hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
            // An unusable ID is greyed, with the reason where the issuer would be.
            let (icon, name_color) = if usable { (t.accent, t.text) } else { (t.text_muted, t.text_muted) };
            icons::paint(ui, Rect::from_min_size(rect.min + vec2(10.0, 15.0), vec2(20.0, 20.0)), "badge-check", 18.0, icon);
            // Certificate names can be any length: cut both lines with "…" at the row's edge
            // and show them whole on hover.
            let line = |text: &str, font: egui::FontId, color: Color32| {
                let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
                job.wrap = egui::text::TextWrapping::truncate_at_width((rect.width() - 50.0).max(0.0));
                ui.painter().layout_job(job)
            };
            let name = line(&e.name, theme::semibold(13.0), name_color);
            let store = if e.path.starts_with("keychain:") {
                tl!("Keychain  ·  ").to_string()
            } else if e.path.starts_with("windows:") {
                tl!("Windows store  ·  ").to_string()
            } else {
                String::new()
            };
            let sub = match &e.unusable {
                Some(reason) => format!("{store}{reason}"),
                None => format!(
                    "{store}{email}{issued}{issuer}{expires}{date}",
                    email = if e.email.is_empty() { String::new() } else { format!("{}  ·  ", e.email) },
                    issued = tl!("Issued by: "),
                    issuer = e.issuer,
                    expires = tl!(", Expires: "),
                    date = e.expires,
                ),
            };
            let details = line(&sub, theme::regular(11.5), t.text_muted);
            let elided = name.elided || details.elided;
            ui.painter().galley(rect.min + vec2(40.0, 9.0), name, name_color);
            ui.painter().galley(rect.min + vec2(40.0, 28.0), details, t.text_muted);
            let resp = if elided { resp.on_hover_text(format!("{}\n{sub}", e.name)) } else { resp };
            if !usable {
                continue;
            }
            if resp.clicked() {
                d.selected = Some(i);
            }
            if resp.double_clicked() {
                d.step = SignStep::SignAs;
            }
        }
        if ids.is_empty() {
            ui.label(egui::RichText::new(tl!("No digital IDs yet.")).color(t.text_muted));
        }
    });
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Configure New Digital ID"), false).clicked() {
            d.step = SignStep::Configure;
            d.error = None;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(d.selected.is_some(), |ui| widgets::pill_button(ui, tl!("Continue"), true)).inner.clicked() {
                d.step = SignStep::SignAs;
                d.error = None;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                close = true;
            }
        });
    });
    close
}

fn configure(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    title(ui, "Configure a Digital ID for Signing");
    let Some(d) = app.sign_draft.as_mut() else { return true };
    let mut close = false;
    let mut go = false;
    // Browse… is desktop-only.
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut browse = false;
    ui.radio_value(&mut d.new_id.create, false, tl!("Use a Digital ID from a file"));
    ui.radio_value(&mut d.new_id.create, true, tl!("Create a new Digital ID (self-signed, saved to a password-protected file)"));
    ui.add_space(8.0);
    let n = &mut d.new_id;
    if n.create {
        egui::Grid::new("new-id").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            for (label, value) in [
                (tl!("Name"), &mut n.name),
                (tl!("Organizational Unit"), &mut n.unit),
                (tl!("Organization Name"), &mut n.organization),
                (tl!("Email Address"), &mut n.email),
            ] {
                let l = ui.label(label);
                ui.add(egui::TextEdit::singleline(value).desired_width(260.0)).labelled_by(l.id);
                ui.end_row();
            }
            let l = ui.label(tl!("Country/Region"));
            ui.add(egui::TextEdit::singleline(&mut n.country).hint_text("US").char_limit(2).desired_width(60.0)).labelled_by(l.id);
            ui.end_row();
            ui.label(tl!("Key Algorithm"));
            egui::ComboBox::from_id_salt("key-alg").selected_text(KEY_ALGORITHMS[n.key].0).show_ui(ui, |ui| {
                for (i, (label, _)) in KEY_ALGORITHMS.iter().enumerate() {
                    ui.selectable_value(&mut n.key, i, *label);
                }
            });
            ui.end_row();
            let l = ui.label(tl!("Password"));
            ui.add(egui::TextEdit::singleline(&mut n.password).password(true).desired_width(200.0)).labelled_by(l.id);
            ui.end_row();
            let l = ui.label(tl!("Confirm Password"));
            ui.add(egui::TextEdit::singleline(&mut n.confirm).password(true).desired_width(200.0)).labelled_by(l.id);
            ui.end_row();
        });
        ui.label(
            egui::RichText::new(tl!("Valid for 5 years, for digital signatures. Self-signed IDs are usually not trusted by others."))
                .small()
                .color(t.text_muted),
        );
    } else {
        egui::Grid::new("id-file").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            let l = ui.label(tl!("File (.p12, .pfx)"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut n.file).desired_width(220.0)).labelled_by(l.id);
                #[cfg(not(target_arch = "wasm32"))]
                if ui.button(tl!("Browse…")).clicked() {
                    browse = true;
                }
            });
            ui.end_row();
            let l = ui.label(tl!("Password"));
            ui.add(egui::TextEdit::singleline(&mut n.file_password).password(true).desired_width(200.0)).labelled_by(l.id);
            ui.end_row();
        });
    }
    ui.add_space(6.0);
    error(ui, &d.error);
    ui.horizontal(|ui| {
        if !app.digital_ids.is_empty() && widgets::pill_button(ui, tl!("Back"), false).clicked() {
            d.step = SignStep::Choose;
            d.error = None;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, if d.new_id.create { tl!("Save") } else { tl!("Continue") }, true).clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                close = true;
            }
        });
    });
    if go {
        let create = app.sign_draft.as_ref().is_some_and(|d| d.new_id.create);
        let result = if create { app.create_digital_id() } else { app.import_digital_id() };
        let Some(d) = app.sign_draft.as_mut() else { return true };
        match result {
            Ok(i) => {
                d.selected = Some(i);
                d.step = SignStep::Choose;
                d.error = None;
                d.new_id = NewIdDraft::default();
            }
            Err(e) => d.error = Some(e),
        }
    }
    // The picker answers on a later frame, into the draft if the same dialog is still open.
    #[cfg(not(target_arch = "wasm32"))]
    if browse {
        let dialog = rfd::AsyncFileDialog::new().add_filter(tl!("Digital ID"), &["p12", "pfx"]);
        let epoch = app.dialog_epoch();
        app.ask_one(crate::pickers::Ask::File(dialog), None, move |app, p| {
            // Not one closed meanwhile, or opened again since.
            if app.dialog_epoch() == epoch
                && let Some(d) = app.sign_draft.as_mut()
            {
                d.new_id.file = p.to_string_lossy().into_owned();
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = browse;
    close
}

/// The appearance preview: Acrobat's standard layout (name left, details right).
fn preview(ui: &mut egui::Ui, t: &Tokens, name: &str, d: &SignDraft) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), egui::Sense::hover());
    let bg = if t.dark() { Color32::from_gray(0xF4) } else { Color32::WHITE };
    ui.painter().rect(rect, CornerRadius::same(4), bg, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let inner = rect.shrink(10.0);
    let a = &d.appearance;
    if a.name {
        let half = Rect::from_min_size(inner.min, vec2(inner.width() * 0.5 - 6.0, inner.height()));
        let galley = ui.painter().layout(name.to_string(), theme::regular(22.0), Color32::BLACK, half.width());
        ui.painter().galley(half.left_center() - vec2(0.0, galley.size().y / 2.0), galley, Color32::BLACK);
    }
    let mut lines = Vec::new();
    if a.name {
        lines.push(if a.labels { format!("Digitally signed by {name}") } else { name.to_string() });
    }
    if a.reason && !d.reason.trim().is_empty() {
        lines.push(format!("{}{}", if a.labels { "Reason: " } else { "" }, d.reason.trim()));
    }
    if a.location && !d.location.trim().is_empty() {
        lines.push(format!("{}{}", if a.labels { "Location: " } else { "" }, d.location.trim()));
    }
    if a.date {
        lines.push(format!("{}{}", if a.labels { "Date: " } else { "" }, "(the signing time)"));
    }
    let x = if a.name { inner.center().x + 6.0 } else { inner.left() };
    let galley = ui.painter().layout(lines.join("\n"), theme::regular(11.0), Color32::from_gray(0x20), inner.right() - x);
    ui.painter().galley(egui::pos2(x, inner.center().y - galley.size().y / 2.0), galley, Color32::BLACK);
}

fn sign_as(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let ids = app.digital_ids.clone();
    let Some(d) = app.sign_draft.as_mut() else { return true };
    let Some(entry) = d.selected.and_then(|i| ids.get(i)).cloned() else {
        d.step = SignStep::Choose;
        return false;
    };
    title(
        ui,
        &crate::i18n::fmt(
            tl!("{verb} as \"{name}\""),
            &[("verb", if d.certify.is_some() { tl!("Certify") } else { tl!("Sign") }), ("name", &entry.name)],
        ),
    );
    let in_os_key_store = entry.path.starts_with("keychain:") || entry.path.starts_with("windows:");
    let mut close = false;
    let mut go = false;
    if d.rect.is_some() || d.field.is_some() {
        ui.label(egui::RichText::new(tl!("Appearance")).font(theme::semibold(12.5)));
        preview(ui, t, &entry.name, d);
        ui.horizontal_wrapped(|ui| {
            let a = &mut d.appearance;
            ui.checkbox(&mut a.name, tl!("Name"));
            ui.checkbox(&mut a.date, tl!("Date"));
            ui.checkbox(&mut a.reason, tl!("Reason"));
            ui.checkbox(&mut a.location, tl!("Location"));
            ui.checkbox(&mut a.distinguished_name, tl!("Distinguished name"));
            ui.checkbox(&mut a.labels, tl!("Labels"));
        });
        ui.add_space(6.0);
    } else {
        ui.label(egui::RichText::new(tl!("The signature will be invisible.")).color(t.text_muted));
    }
    egui::Grid::new("sign-as").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        if d.certify.is_some() {
            ui.label(tl!("Permitted actions after certifying"));
            let mut p = d.certify.unwrap_or(2);
            let label = |p: u8| match p {
                1 => tl!("No changes allowed"),
                3 => tl!("Annotations, form fill-in, and digital signatures"),
                _ => tl!("Form fill-in and digital signatures"),
            };
            egui::ComboBox::from_id_salt("certify-p").selected_text(label(p)).width(300.0).show_ui(ui, |ui| {
                for v in [1u8, 2, 3] {
                    ui.selectable_value(&mut p, v, label(v));
                }
            });
            d.certify = Some(p);
            ui.end_row();
        }
        let l = ui.label(tl!("Reason"));
        ui.add(egui::TextEdit::singleline(&mut d.reason).hint_text(tl!("Optional")).desired_width(260.0)).labelled_by(l.id);
        ui.end_row();
        let l = ui.label(tl!("Location"));
        ui.add(egui::TextEdit::singleline(&mut d.location).hint_text(tl!("Optional")).desired_width(260.0)).labelled_by(l.id);
        ui.end_row();
        if in_os_key_store {
            ui.label("");
            ui.label(
                egui::RichText::new(if entry.path.starts_with("windows:") {
                    tl!("The key is in the Windows certificate store, which may ask to allow PdfCraft to use it.")
                } else {
                    tl!("The key is in the macOS Keychain, which may ask to allow PdfCraft to use it.")
                })
                .small()
                .color(t.text_muted),
            );
        } else {
            let l = ui.label(tl!("Digital ID password"));
            let r = ui.add(egui::TextEdit::singleline(&mut d.password).password(true).desired_width(200.0)).labelled_by(l.id);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = true;
            }
        }
        ui.end_row();
    });
    ui.add_space(6.0);
    error(ui, &d.error);
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Back"), false).clicked() {
            d.step = SignStep::Choose;
            d.error = None;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Sign"), true).clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                close = true;
            }
        });
    });
    if go {
        match app.finish_signing() {
            Ok(()) => {
                app.sign_draft = None;
                return true;
            }
            Err(e) => {
                if let Some(d) = app.sign_draft.as_mut() {
                    d.error = Some(e);
                }
            }
        }
    }
    close
}

/// The message bar for signed documents: (icon, colour, template, argument).
/// The template renders through [`crate::i18n::tr_fmt`] at the call site, so the bar follows
/// the UI language; names inside `argument` stay as they are.
pub(crate) fn banner(sigs: &[SignatureInfo]) -> Option<(&'static str, Color32, &'static str, String)> {
    let signed: Vec<&SignatureInfo> = sigs.iter().filter(|s| s.signed).collect();
    if signed.is_empty() {
        return None;
    }
    if let Some(c) = signed.iter().find(|s| s.certify.is_some())
        && c.status == SignatureStatus::Valid
    {
        let by = c.certificate.as_ref().map(|x| {
            let org = x.subject.organization().map(|o| format!(", {o}")).unwrap_or_default();
            format!("{}{org}, certificate issued by {}.", x.display_name(), x.issuer.common_name().unwrap_or("an unknown issuer"))
        });
        return Some(("badge-check", Color32::from_rgb(0x14, 0x73, 0xE6), "Certified by {by}", by.unwrap_or_default()));
    }
    Some(if signed.iter().any(|s| s.status == SignatureStatus::Invalid) {
        ("circle-x", Color32::from_rgb(0xD7, 0x37, 0x3F), "At least one signature is invalid.", String::new())
    } else if signed.iter().all(|s| s.status == SignatureStatus::Valid) {
        ("circle-check", Color32::from_rgb(0x2D, 0x9D, 0x78), "Signed and all signatures are valid.", String::new())
    } else {
        ("triangle-alert", Color32::from_rgb(0xE6, 0x86, 0x19), "At least one signature has problems.", String::new())
    })
}

fn status_icon(s: &SignatureInfo) -> (&'static str, Color32) {
    if !s.signed {
        return ("pen-line", Color32::from_gray(0x80));
    }
    match (s.status, &s.modification) {
        (SignatureStatus::Valid, Modification::None) => ("circle-check", Color32::from_rgb(0x2D, 0x9D, 0x78)),
        (SignatureStatus::Valid, _) => ("circle-check", Color32::from_rgb(0x2D, 0x9D, 0x78)),
        (SignatureStatus::Unknown, _) => ("triangle-alert", Color32::from_rgb(0xE6, 0x86, 0x19)),
        (SignatureStatus::Invalid, _) => ("circle-x", Color32::from_rgb(0xD7, 0x37, 0x3F)),
    }
}

/// Viewer-only verification badge for a signed signature widget on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WidgetBadge {
    pub icon: &'static str,
    pub color: Color32,
    /// Tooltip text: the icon never stands alone.
    pub label: String,
}

/// Map a signature to its canvas badge. `None` for empty fields: nothing to verify.
/// This mirrors the engine verdict without re-checking it — invalid stays invalid and
/// unknown stays unknown — so the widget reflects verification instead of forcing green.
pub(crate) fn widget_badge(s: &SignatureInfo) -> Option<WidgetBadge> {
    if !s.signed {
        return None;
    }
    const GREEN: Color32 = Color32::from_rgb(0x2D, 0x9D, 0x78);
    const AMBER: Color32 = Color32::from_rgb(0xE6, 0x86, 0x19);
    const RED: Color32 = Color32::from_rgb(0xD7, 0x37, 0x3F);
    const GRAY: Color32 = Color32::from_gray(0x6E);
    if matches!(s.modification, Modification::Disallowed(_)) {
        return Some(WidgetBadge {
            icon: "circle-x",
            color: RED,
            label: tl!("Signature is invalid: the document was modified after signing.").to_string(),
        });
    }
    match s.status {
        SignatureStatus::Valid => Some(WidgetBadge { icon: "circle-check", color: GREEN, label: tl!("Signature valid").to_string() }),
        SignatureStatus::Invalid => Some(WidgetBadge { icon: "circle-x", color: RED, label: tl!("Signature is invalid.").to_string() }),
        SignatureStatus::Unknown if s.details.iter().any(|d| d.to_lowercase().contains("can't check")) => Some(WidgetBadge {
            icon: "circle-help",
            color: GRAY,
            label: tl!("Signature cannot be checked yet: unsupported algorithm or structure.").to_string(),
        }),
        SignatureStatus::Unknown => Some(WidgetBadge {
            icon: "triangle-alert",
            color: AMBER,
            label: tl!("Signature validity is unknown: the signer is not trusted.").to_string(),
        }),
    }
}

/// A coherent on-canvas presentation of one signed widget: an opaque plate covering
/// the widget's baked appearance plus the live verification verdict. The baked
/// appearance (e.g. the signer's own "not verified" look) is document content and is
/// never edited — it is hidden behind viewer-only paint so it cannot contradict the
/// verdict beside a green tick.
pub(crate) struct WidgetPlate {
    /// The widget's screen rect, painted opaquely.
    pub cover: Rect,
    pub badge: WidgetBadge,
    pub signer: Option<String>,
    /// Signing time for the details line, where available.
    pub date: Option<String>,
    /// Present only when validation justifies the valid state: painted as the
    /// reference-style stamp instead of the generic plate.
    pub stamp: Option<crate::stamp::ValidStamp>,
}

/// Screen-space plates for the signed, visible widgets on `page`. Viewer-only: the
/// document bytes are untouched.
pub(crate) fn widget_plates(sigs: &[SignatureInfo], page: usize, xf: &PageXform, info: &DocInfo) -> Vec<WidgetPlate> {
    let mut out = Vec::new();
    // A corrupt file can name a page that does not exist: show nothing, never panic.
    if info.pages.get(page).is_none() {
        return out;
    }
    for s in sigs {
        if s.page != Some(page) || !s.visible {
            continue;
        }
        let (Some(rect), Some(badge)) = (s.rect, widget_badge(s)) else { continue };
        if rect[2] <= rect[0] || rect[3] <= rect[1] {
            continue;
        }
        let cover = xf.user_rect(info, page, [rect[0] as f32, rect[1] as f32, rect[2] as f32, rect[3] as f32]);
        let date = s.signing_time.map(|t| t.to_string()).or_else(|| s.date.clone());
        let stamp = crate::stamp::valid_stamp_for(s);
        out.push(WidgetPlate { cover, badge, signer: s.signer.clone(), date, stamp });
    }
    out
}

/// Design box the plate typography is laid out for, in screen points: the reference
/// viewer scales its 50x30pt stamp form to the widget box the same way.
const PLATE_DESIGN_W: f32 = 220.0;
const PLATE_DESIGN_H: f32 = 64.0;

/// Scale + geometry for one plate's content. Pure so the complete visual state is
/// unit-testable: the status line must survive (scaled to fit), never be dropped.
#[derive(Clone, Debug)]
pub(crate) struct PlateLayout {
    pub icon_rect: Rect,
    pub icon_size: f32,
    /// Absolute x where text starts, and how wide the text column is.
    pub text_x: f32,
    pub text_w: f32,
    pub status_size: f32,
    pub sub_size: f32,
    pub line_gap: f32,
}

/// Compute the plate content layout for `cover`. `status_w_at_base` is the measured
/// width of the status label at 12pt. Returns `None` for degenerate covers (paint the
/// cover only). The status line always fits: the layout shrinks uniformly to fit it,
///
/// mirroring how the reference viewer scales its stamp to the widget box.
pub(crate) fn plate_layout(cover: Rect, status_w_at_base: f32) -> Option<PlateLayout> {
    let pad = 4.0;
    if cover.width() < pad * 2.0 + 8.0 || cover.height() < pad * 2.0 + 8.0 {
        return None;
    }
    // Uniform scale from the box, like the reference stampScale from the widget box.
    let mut k = ((cover.width() - pad * 2.0) / PLATE_DESIGN_W).min((cover.height() - pad * 2.0) / PLATE_DESIGN_H).clamp(0.3, 1.25);
    let scaled = |k: f32| {
        let icon = (22.0 * k).clamp(8.0, 24.0);
        let text_x = cover.left() + pad * 2.0 + icon;
        (icon, text_x, cover.right() - pad - text_x)
    };
    let (mut icon, mut text_x, mut available) = scaled(k);
    // Shrink-to-fit so the required status line is always drawn, never dropped.
    let need = status_w_at_base * k;
    if need > available && available > 0.0 {
        k = (k * available / need).max(0.25);
        (icon, text_x, available) = scaled(k);
    }
    let status_size = 12.0 * k;
    let sub_size = 10.5 * k;
    Some(PlateLayout {
        icon_rect: Rect::from_center_size(pos2(cover.left() + pad + icon / 2.0, cover.center().y), vec2(icon, icon)),
        icon_size: icon,
        text_x,
        text_w: available.max(0.0),
        status_size,
        sub_size,
        line_gap: 1.0,
    })
}
/// Paint one plate. A plate carrying a valid stamp is painted exactly as the
/// reference valid stamp (opaque cover, tick, title, body — no outer border, which
/// the reference stamp does not have). Anything else keeps the warning-plate
/// presentation so untrusted, invalid and modified states stay distinct.
pub(crate) fn paint_widget_plate(ui: &egui::Ui, painter: &egui::Painter, plate: &WidgetPlate) {
    if let Some(stamp) = &plate.stamp {
        crate::stamp::paint_valid_stamp(ui, painter, plate.cover, stamp);
        return;
    }
    let r = plate.cover;
    painter.rect_filled(r, CornerRadius::same(2), Color32::WHITE);
    painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, plate.badge.color), egui::StrokeKind::Inside);
    let ink = Color32::from_gray(0x20);
    let status_w = painter.layout_no_wrap(plate.badge.label.clone(), theme::semibold(12.0), ink).size().x;
    let Some(l) = plate_layout(r, status_w) else {
        return; // degenerate on screen: the cover alone hides the stale appearance
    };
    let painter = painter.with_clip_rect(r);
    icons::paint(ui, l.icon_rect, plate.badge.icon, l.icon_size, plate.badge.color);
    // Status line first (always fits by construction), then signer and date while the
    // measured widths and the vertical room allow.
    let mut lines = vec![(plate.badge.label.clone(), l.status_size, true)];
    for (text, size) in [plate.signer.clone().map(|s| (s, l.sub_size)), plate.date.clone().map(|s| (s, l.sub_size))].into_iter().flatten() {
        if text.trim().is_empty() {
            continue;
        }
        let w = painter.layout_no_wrap(text.clone(), theme::regular(size), Color32::from_gray(0x5A)).size().x;
        if w <= l.text_w {
            lines.push((text, size, false));
        }
    }
    let heights: Vec<f32> = lines.iter().map(|(_, s, _)| s * 1.25).collect();
    let mut shown = lines.len();
    while shown > 1 && heights[..shown].iter().sum::<f32>() + (shown as f32 - 1.0) * l.line_gap + 6.0 > r.height() {
        shown -= 1;
    }
    let total: f32 = heights[..shown].iter().sum::<f32>() + (shown as f32 - 1.0) * l.line_gap;
    let mut y = r.center().y - total / 2.0;
    for (text, size, strong) in &lines[..shown] {
        let font = if *strong { theme::semibold(*size) } else { theme::regular(*size) };
        let g = painter.layout_no_wrap(text.clone(), font, ink);
        painter.galley(pos2(l.text_x, y), g.clone(), ink);
        y += g.size().y + l.line_gap;
    }
}

/// What the Signatures panel asks the app to do.
#[derive(Clone, Debug)]
pub enum PanelAction {
    Validate,
    GoTo(usize),
    Trust(Box<Certificate>),
    /// Trust the bundled CCA India roots (e-Aadhaar) and revalidate.
    TrustCca,
    ViewSigned(usize),
    Sign(String),
    ExportCertificate(Box<Certificate>),
    /// Certificate Viewer for the signer's chain (signer first).
    ViewCertificate(Vec<Certificate>),
}

fn detail_row(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.add(egui::Label::new(egui::RichText::new(format!("{}: {value}", tl!(key))).font(theme::regular(11.5))).wrap());
}

/// The separated verification verdicts for one signed signature: integrity, trust,
/// certificate validity, revocation, signing-time source, later changes and next steps.
fn verification_rows(ui: &mut egui::Ui, t: &Tokens, r: &SignatureReport) {
    use pdfcraft_engine::sign::verify::{CertValidity, Integrity, Revocation, TimeSource, Trust};
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Verification")).font(theme::semibold(12.0)));
    detail_row(
        ui,
        "Integrity",
        &match &r.integrity {
            Integrity::Intact => tl!("Intact: the digest and the signature value verify.").to_string(),
            Integrity::Altered { reason } => format!("{} {reason}", tl!("Altered:")),
            Integrity::Unknown { reason } => format!("{} {reason}", tl!("Unknown:")),
            Integrity::NotApplicable => tl!("No signature to check.").to_string(),
        },
    );
    detail_row(
        ui,
        "Trust",
        &match &r.trust {
            Trust::Trusted { anchor, via_cca: true } => format!("{} ({anchor})", tl!("Trusted through the bundled CCA India roots")),
            Trust::Trusted { anchor, .. } => format!("{} ({anchor})", tl!("Trusted")),
            Trust::Untrusted => tl!("Not trusted: the signer is unknown.").to_string(),
        },
    );
    detail_row(
        ui,
        "Certificate",
        &match &r.cert_validity {
            CertValidity::Valid => tl!("Valid at the signing time.").to_string(),
            CertValidity::NotValidAtSigning { detail } => detail.clone(),
            CertValidity::NoSigningTime => tl!("No signing time recorded, so validity then cannot be judged.").to_string(),
            CertValidity::NoCertificate => tl!("No signer certificate in the signature.").to_string(),
        },
    );
    detail_row(
        ui,
        "Revocation",
        &match &r.revocation {
            Revocation::Good => tl!("Not revoked: embedded evidence was checked.").to_string(),
            Revocation::Revoked { detail } => detail.clone(),
            Revocation::Unknown => tl!("Unknown: offline verification only checks evidence embedded in the document.").to_string(),
        },
    );
    detail_row(
        ui,
        "Signing time",
        &match (&r.signing_time, &r.time_source) {
            (Some(when), TimeSource::TrustedTimestamp) => format!("{when} ({})", tl!("from a validated timestamp of a trusted authority")),
            (Some(when), _) => format!("{when} ({})", tl!("from the signer's computer clock, not independently validated")),
            (None, _) => tl!("Not recorded.").to_string(),
        },
    );
    detail_row(
        ui,
        "Changes after signing",
        &match &r.modification {
            Modification::None => tl!("None.").to_string(),
            Modification::Allowed(kinds) => format!("{} ({})", tl!("Permitted"), kinds.join(", ")),
            Modification::Disallowed(kinds) => format!("{} ({})", tl!("Not permitted"), kinds.join(", ")),
        },
    );
    if !r.next_steps.is_empty() {
        ui.label(egui::RichText::new(tl!("Recommended next steps")).font(theme::semibold(12.0)));
        for step in &r.next_steps {
            ui.add(egui::Label::new(egui::RichText::new(format!("• {step}")).font(theme::regular(11.5)).color(t.text_muted)).wrap());
        }
    }
}

/// The Signatures panel body. `reports` holds one [`SignatureReport`] per field in
/// `sigs` (see [`pdfcraft_engine::sign::verify`]); `cca_trusted` says whether the
/// bundled CCA India roots are in the session trust store.
pub(crate) fn panel(
    ui: &mut egui::Ui,
    t: &Tokens,
    sigs: &[SignatureInfo],
    reports: &[SignatureReport],
    cca_trusted: bool,
    expanded: &mut Vec<String>,
) -> Option<PanelAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        if widgets::pill_button(ui, tl!("Validate all"), false).clicked() {
            action = Some(PanelAction::Validate);
        }
        if cca_trusted {
            ui.label(egui::RichText::new(tl!("CCA India roots trusted")).small().color(t.text_muted));
        } else if widgets::pill_button(ui, tl!("Verify with CCA India roots"), false).clicked() {
            action = Some(PanelAction::TrustCca);
        }
    });
    ui.label(
        egui::RichText::new(tl!("e-Aadhaar PDFs chain to India's CCA roots, bundled with PdfCraft and checked on this device. Nothing is uploaded."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(6.0);
    if sigs.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.add(icons::image("signature", 32.0, t.text_faint));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tl!("This document has no signatures.")).color(t.text_muted));
        });
        return action;
    }
    let mut ordered: Vec<&SignatureInfo> = sigs.iter().collect();
    ordered.sort_by_key(|s| (!s.signed, s.revision));
    for s in ordered {
        let open = expanded.contains(&s.field);
        let (icon, color) = status_icon(s);
        let head = if s.signed {
            format!(
                "Rev. {}: {} by {}",
                s.revision,
                if s.certify.is_some() { "Certified" } else { "Signed" },
                s.signer.as_deref().unwrap_or("an unknown signer")
            )
        } else {
            format!("Unsigned signature field: {}", s.field)
        };
        let resp = ui
            .horizontal(|ui| {
                let chevron = if open { "chevron-down" } else { "chevron-right" };
                let toggle = icons::button(ui, chevron, 20.0, false, if open { "Collapse" } else { "Expand" }).clicked();
                ui.add(icons::image(icon, 16.0, color));
                let l = ui.add(egui::Label::new(egui::RichText::new(&head).font(theme::semibold(12.5)).color(t.text)).sense(egui::Sense::click()));
                toggle || l.clicked()
            })
            .inner;
        if resp {
            if open {
                expanded.retain(|f| f != &s.field);
            } else {
                expanded.push(s.field.clone());
            }
        }
        if !open {
            continue;
        }
        egui::Frame::NONE.inner_margin(egui::Margin { left: 44, right: 4, top: 2, bottom: 8 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            if !s.signed {
                if widgets::pill_button(ui, tl!("Sign this field…"), true).clicked() {
                    action = Some(PanelAction::Sign(s.field.clone()));
                }
                return;
            }
            ui.label(
                egui::RichText::new(match s.status {
                    SignatureStatus::Valid => tl!("Signature is valid:"),
                    SignatureStatus::Unknown => tl!("Signature validity is UNKNOWN:"),
                    SignatureStatus::Invalid => tl!("Signature is INVALID:"),
                })
                .font(theme::semibold(12.0)),
            );
            for line in &s.details {
                ui.add(egui::Label::new(egui::RichText::new(format!("• {line}")).font(theme::regular(11.5)).color(t.text_muted)).wrap());
            }
            if let Some(r) = reports.iter().find(|r| r.field == s.field) {
                verification_rows(ui, t, r);
            }
            ui.add_space(4.0);
            ui.label(egui::RichText::new(tl!("Signature Details")).font(theme::semibold(12.0)));
            let row = |ui: &mut egui::Ui, k: &str, v: &str| {
                ui.add(egui::Label::new(egui::RichText::new(format!("{}: {v}", tl!(k))).font(theme::regular(11.5))).wrap());
            };
            if let Some(r) = &s.reason {
                row(ui, "Reason", r);
            }
            if let Some(l) = &s.location {
                row(ui, "Location", l);
            }
            if let Some(d) = &s.date {
                row(ui, "Signing time", &sign::pdf::display_date(d));
            }
            if let Some(a) = &s.algorithm {
                row(ui, "Algorithm", a);
            }
            if let Some(c) = &s.certificate {
                row(ui, "Signer", &c.subject.display());
                row(ui, "Issued by", &c.issuer.display());
                row(ui, "Valid", &format!("{} to {}", c.not_before, c.not_after));
            }
            match s.page {
                Some(p) => {
                    let link = ui.add(
                        egui::Label::new(
                            egui::RichText::new(crate::i18n::fmt(
                                tl!("Field: {field} on page {p}"),
                                &[("field", &s.field), ("p", &(p + 1).to_string())],
                            ))
                            .font(theme::regular(11.5))
                            .color(t.accent_text)
                            .underline(),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if link.clicked() {
                        action = Some(PanelAction::GoTo(p));
                    }
                }
                None => row(ui, "Field", &s.field),
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(c) = s.chain.last().or(s.certificate.as_ref())
                    && s.status == SignatureStatus::Unknown
                    && ui.button(tl!("Add to trusted certificates")).clicked()
                {
                    action = Some(PanelAction::Trust(Box::new(c.clone())));
                }
                if ui.button(tl!("View signed version")).clicked() {
                    action = Some(PanelAction::ViewSigned(s.signed_len));
                }
                if let Some(c) = &s.certificate
                    && ui.button(tl!("Show certificate…")).clicked()
                {
                    let mut chain = vec![c.clone()];
                    chain.extend(s.chain.iter().filter(|x| x.raw != c.raw).cloned());
                    action = Some(PanelAction::ViewCertificate(chain));
                }
                if let Some(c) = &s.certificate
                    && ui.button(tl!("Export certificate…")).clicked()
                {
                    action = Some(PanelAction::ExportCertificate(Box::new(c.clone())));
                }
            });
        });
    }
    action
}

/// Certificate Viewer: a chain (the end certificate first) and the tab shown.
#[derive(Clone, Debug)]
pub struct CertViewer {
    pub chain: Vec<Certificate>,
    pub selected: usize,
    pub tab: CertTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertTab {
    Summary,
    Details,
    Trust,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn key_usage(bits: u16) -> String {
    const NAMES: [&str; 9] = [
        "Digital Signature",
        "Non-Repudiation",
        "Key Encipherment",
        "Data Encipherment",
        "Key Agreement",
        "Certificate Signing",
        "CRL Signing",
        "Encipher Only",
        "Decipher Only",
    ];
    let used: Vec<&str> = NAMES.iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).map(|(_, n)| *n).collect();
    if used.is_empty() { "None".into() } else { used.join(", ") }
}

/// What the viewer asks for.
pub(crate) enum CertAction {
    Trust(Box<Certificate>),
    Export(Box<Certificate>),
}

/// The Certificate Viewer dialog: the chain on the left, the selected certificate's tabs.
pub(crate) fn cert_viewer(ui: &mut egui::Ui, v: &mut CertViewer, trusted: &[Certificate], t: &Tokens) -> (bool, Option<CertAction>) {
    title(ui, "Certificate Viewer");
    ui.label(egui::RichText::new(tl!("This dialog shows the details of a certificate and its chain.")).color(t.text_muted));
    ui.add_space(8.0);
    let mut action = None;
    v.selected = v.selected.min(v.chain.len().saturating_sub(1));
    ui.horizontal_top(|ui| {
        // The chain, root at the top (as Acrobat shows it).
        ui.vertical(|ui| {
            ui.set_width(200.0);
            for (depth, i) in (0..v.chain.len()).rev().enumerate() {
                let c = &v.chain[i];
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 12.0);
                    if ui.selectable_label(v.selected == i, c.display_name()).clicked() {
                        v.selected = i;
                    }
                });
            }
        });
        ui.add_space(16.0);
        ui.vertical(|ui| {
            ui.set_width(440.0);
            let Some(c) = v.chain.get(v.selected).cloned() else { return };
            ui.horizontal(|ui| {
                for (tab, label) in [(CertTab::Summary, tl!("Summary")), (CertTab::Details, tl!("Details")), (CertTab::Trust, tl!("Trust"))] {
                    if widgets::pill_button(ui, label, v.tab == tab).clicked() {
                        v.tab = tab;
                    }
                }
            });
            ui.add_space(8.0);
            let grid = |ui: &mut egui::Ui, rows: Vec<(&str, String)>| {
                egui::Grid::new(("cert-rows", v.tab as u8)).num_columns(2).spacing([12.0, 5.0]).show(ui, |ui| {
                    for (k, val) in rows {
                        ui.label(egui::RichText::new(tl_ctx!("certificate", k)).color(t.text_muted));
                        ui.add(egui::Label::new(val).wrap());
                        ui.end_row();
                    }
                });
            };
            match v.tab {
                CertTab::Summary => grid(
                    ui,
                    vec![
                        ("Issued to", c.subject.display()),
                        ("Issued by", c.issuer.display()),
                        ("Valid from", c.not_before.to_string()),
                        ("Valid to", c.not_after.to_string()),
                        ("Intended usage", c.key_usage.map(key_usage).unwrap_or_else(|| tl!("Any").to_string())),
                    ],
                ),
                CertTab::Details => grid(
                    ui,
                    vec![
                        ("Version", "3".into()),
                        ("Serial number", c.serial_hex()),
                        ("Issuer", c.issuer.display()),
                        ("Subject", c.subject.display()),
                        ("Validity starts", c.not_before.to_string()),
                        ("Validity ends", c.not_after.to_string()),
                        ("Public key", c.public_key.describe()),
                        ("Basic constraints", tl!(if c.is_ca { "Certificate authority" } else { "End entity" }).to_string()),
                        ("Key usage", c.key_usage.map(key_usage).unwrap_or_else(|| tl!("Not present").to_string())),
                        ("Self-signed", if c.is_self_signed() { tl!("Yes").to_string() } else { tl!("No").to_string() }),
                        ("SHA-1 digest", hex(&sign::keys::DigestAlg::Sha1.digest(&[&c.raw]))),
                        ("SHA-256 digest", hex(&sign::keys::DigestAlg::Sha256.digest(&[&c.raw]))),
                    ],
                ),
                CertTab::Trust => {
                    let is_trusted = trusted.iter().any(|x| x.raw == c.raw);
                    let anchored = v.chain.iter().any(|x| trusted.iter().any(|y| y.raw == x.raw));
                    ui.label(if is_trusted {
                        tl!("This certificate is in your list of trusted certificates.")
                    } else if anchored {
                        tl!("This certificate is trusted through a certificate above it in the chain.")
                    } else {
                        tl!("This certificate is not trusted. Signatures made with it show an unknown identity.")
                    });
                    ui.add_space(8.0);
                    if !is_trusted && widgets::pill_button(ui, tl!("Add to Trusted Certificates"), false).clicked() {
                        action = Some(CertAction::Trust(Box::new(c.clone())));
                    }
                }
            }
            ui.add_space(8.0);
            if widgets::pill_button(ui, tl!("Export…"), false).clicked() {
                action = Some(CertAction::Export(Box::new(c.clone())));
            }
        });
    });
    ui.add_space(10.0);
    let mut close = false;
    ui.horizontal(|ui| ui.with_layout(Layout::right_to_left(Align::Center), |ui| close = widgets::pill_button(ui, tl!("OK"), true).clicked()));
    (close, action)
}

/// The text of an exported certificate (`.cer`, PEM).
pub fn certificate_pem(c: &Certificate) -> String {
    sign::x509::to_pem(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder per call: tests run in parallel.
    fn scratch() -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("pdfcraft-sign-ui-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Create a digital ID named "Grace Hopper" in `dir`, as Configure New Digital ID ▸ Create
    /// does. Returns where it was saved.
    fn create_in(dir: &std::path::Path) -> Result<PathBuf, String> {
        let mut app = PdfCraftApp::new();
        app.export_dir_override = Some(dir.to_string_lossy().into_owned());
        let mut draft = SignDraft::new(0, None, None, None, &[]);
        // P-256 keeps the test fast.
        let key = KEY_ALGORITHMS.iter().position(|k| k.1 == "p256").unwrap();
        draft.new_id =
            NewIdDraft { name: "Grace Hopper".into(), key, password: "secret1".into(), confirm: "secret1".into(), ..NewIdDraft::default() };
        app.sign_draft = Some(draft);
        let i = app.create_digital_id()?;
        Ok(PathBuf::from(&app.digital_ids[i].path))
    }

    #[test]
    fn a_new_digital_id_skips_names_already_taken_without_writing_through_them() {
        let dir = scratch();
        // Another file's hard link at the first name, and a folder at the second (Windows refuses
        // `create_new` on a folder with "access denied" rather than "already exists").
        std::fs::write(dir.join("victim.txt"), "keep me").unwrap();
        std::fs::hard_link(dir.join("victim.txt"), dir.join("Grace Hopper.p12")).unwrap();
        std::fs::create_dir(dir.join("Grace Hopper 2.p12")).unwrap();
        let saved = create_in(&dir).unwrap();
        assert_eq!(saved, dir.join("Grace Hopper 3.p12"));
        assert_eq!(std::fs::read_to_string(dir.join("victim.txt")).unwrap(), "keep me");
        assert!(sign::pkcs12::open(&std::fs::read(&saved).unwrap(), "secret1").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_digital_id_is_not_written_through_a_dangling_link() {
        // `exists()` follows links, so a link to a file that doesn't exist yet looked free, and
        // `fs::write` then created the private key wherever the link pointed.
        let dir = scratch();
        let (link, target) = (dir.join("Grace Hopper.p12"), dir.join("elsewhere.p12"));
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link);
        #[cfg(not(any(unix, windows)))]
        let made: std::io::Result<()> = {
            let _ = (&target, &link);
            Err(std::io::ErrorKind::Unsupported.into())
        };
        if let Err(e) = made {
            // Windows needs Developer Mode (or admin) for symlinks.
            eprintln!("skipped: can't create a symlink here: {e}");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let saved = create_in(&dir).unwrap();
        assert!(!target.exists(), "nothing written through the link");
        assert_eq!(saved, dir.join("Grace Hopper 2.p12"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_digital_id_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        let saved = create_in(&dir).unwrap();
        let mode = std::fs::metadata(&saved).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the private key is not readable by others: {mode:o}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A signed signature widget for badge tests: status and modification vary.
    fn widget_info(status: SignatureStatus, modification: pdfcraft_engine::sign::Modification, details: &[&str]) -> SignatureInfo {
        SignatureInfo {
            field: "Sig1".to_string(),
            signed: true,
            page: Some(0),
            rect: Some([20.0, 20.0, 180.0, 60.0]),
            visible: true,
            signer: None,
            certificate: None,
            chain: Vec::new(),
            date: None,
            signing_time: None,
            reason: None,
            location: None,
            contact: None,
            sub_filter: None,
            doc_timestamp: false,
            certify: None,
            revision: 2,
            signed_len: 1000,
            digest: None,
            algorithm: None,
            timestamp: false,
            timestamp_time: None,
            status,
            modification,
            details: details.iter().map(|d| d.to_string()).collect(),
        }
    }

    #[test]
    fn widget_badges_mirror_the_engine_verdict() {
        use pdfcraft_engine::sign::Modification;
        // Valid and trusted: green check, "valid" wording.
        let b = widget_badge(&widget_info(SignatureStatus::Valid, Modification::None, &["The signer's identity is valid."])).unwrap();
        assert_eq!((b.icon, b.color), ("circle-check", Color32::from_rgb(0x2D, 0x9D, 0x78)));
        assert!(b.label.contains("valid") && !b.label.contains("unknown"), "{}", b.label);
        // Intact but untrusted: yellow, explicitly unknown — never green.
        let b = widget_badge(&widget_info(SignatureStatus::Unknown, Modification::None, &["The signer's identity is unknown."])).unwrap();
        assert_eq!((b.icon, b.color), ("triangle-alert", Color32::from_rgb(0xE6, 0x86, 0x19)));
        assert!(b.label.contains("unknown"), "{}", b.label);
        // Unsupported: distinct indeterminate state, not red.
        let b = widget_badge(&widget_info(SignatureStatus::Unknown, Modification::None, &["PdfCraft can't check this signature yet: test seam."]))
            .unwrap();
        assert_eq!((b.icon, b.color), ("circle-help", Color32::from_gray(0x6E)));
        // Invalid: red, "invalid" wording.
        let b = widget_badge(&widget_info(SignatureStatus::Invalid, Modification::None, &["The signature value does not match."])).unwrap();
        assert_eq!((b.icon, b.color), ("circle-x", Color32::from_rgb(0xD7, 0x37, 0x3F)));
        assert!(b.label.contains("invalid"), "{}", b.label);
        // Modified after signing: red with a modified-document warning.
        let b = widget_badge(&widget_info(SignatureStatus::Invalid, Modification::Disallowed(vec!["page content".into()]), &["altered"])).unwrap();
        assert_eq!(b.icon, "circle-x");
        assert!(b.label.contains("modified"), "{}", b.label);
    }

    #[test]
    fn empty_fields_get_no_badge() {
        let mut unsigned = widget_info(SignatureStatus::Unknown, pdfcraft_engine::sign::Modification::None, &[]);
        unsigned.signed = false;
        assert!(widget_badge(&unsigned).is_none());
    }

    #[test]
    fn plates_cover_only_signed_visible_widgets_on_the_page() {
        let xf = crate::canvas::PageXform { rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(600.0, 800.0)), rot: 0, pw: 600.0, ph: 800.0 };
        let info = badge_doc_info();
        let mut other_page = widget_info(SignatureStatus::Valid, Modification::None, &[]);
        other_page.page = Some(1);
        let mut hidden = widget_info(SignatureStatus::Valid, Modification::None, &[]);
        hidden.visible = false;
        let mut no_rect = widget_info(SignatureStatus::Valid, Modification::None, &[]);
        no_rect.rect = None;
        let mut unsigned = widget_info(SignatureStatus::Unknown, Modification::None, &[]);
        unsigned.signed = false;
        let sigs = vec![
            widget_info(SignatureStatus::Valid, Modification::None, &[]),
            widget_info(SignatureStatus::Unknown, Modification::None, &[]),
            other_page,
            hidden,
            no_rect,
            unsigned,
        ];
        let plates = widget_plates(&sigs, 0, &xf, &info);
        assert_eq!(plates.len(), 2, "only the two signed, visible widgets on page 0");
        assert_eq!((plates[0].badge.icon, plates[1].badge.icon), ("circle-check", "triangle-alert"));
        // Geometry follows the widget through the page transform, and the plate covers
        // the whole widget: the baked appearance underneath cannot show through.
        let sr = xf.user_rect(&info, 0, [20.0, 20.0, 180.0, 60.0]);
        assert_eq!(plates[0].cover, sr);
        assert!(widget_plates(&sigs, 1, &xf, &info).len() == 1);
        assert!(widget_plates(&[], 0, &xf, &info).is_empty());
        assert!(widget_plates(&sigs, 9, &xf, &info).is_empty(), "a page that does not exist shows nothing");
    }

    #[test]
    fn valid_plate_shows_no_contradictory_indicator() {
        // Regression test for the green-tick-over-question-mark bug: a valid, trusted
        // signature yields exactly one plate, carrying the green verdict and covering the
        // widget — never an unknown/invalid badge alongside it.
        let xf = crate::canvas::PageXform { rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(600.0, 800.0)), rot: 0, pw: 600.0, ph: 800.0 };
        let info = badge_doc_info();
        let sig = widget_info(SignatureStatus::Valid, Modification::None, &["The signer's identity is valid."]);
        let plates = widget_plates(std::slice::from_ref(&sig), 0, &xf, &info);
        assert_eq!(plates.len(), 1);
        let plate = &plates[0];
        assert_eq!(plate.badge.icon, "circle-check");
        assert!(plate.badge.label.contains("valid") && !plate.badge.label.contains("unknown"), "{}", plate.badge.label);
        assert!(!plate.badge.label.contains("Not Verified"), "{}", plate.badge.label);
        let sr = xf.user_rect(&info, 0, [20.0, 20.0, 180.0, 60.0]);
        assert!(plate.cover.contains_rect(sr), "the opaque cover hides the whole baked appearance");
    }

    #[test]
    fn plate_layout_draws_status_text_for_eaadhaar_sizes() {
        // A typical e-Aadhaar widget on screen: wide but short. The old fixed-12pt
        // code dropped the status line here, leaving a tick-only rectangle.
        let cover = Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 53.0));
        let l = plate_layout(cover, 105.0).expect("content for a normal widget");
        // The status line fits the text column at the laid-out size.
        assert!(105.0 * (l.status_size / 12.0) <= l.text_w + 0.01, "{l:?}");
        assert!((3.0..=15.0).contains(&l.status_size), "{l:?}");
        assert!(l.text_x > cover.left() && l.text_x < cover.right() - 40.0, "{l:?}");
        assert!(cover.contains_rect(l.icon_rect), "{l:?}");
    }

    #[test]
    fn plate_layout_never_drops_status_on_narrow_widgets() {
        let cover = Rect::from_min_max(pos2(0.0, 0.0), pos2(70.0, 44.0));
        let l = plate_layout(cover, 105.0).expect("content even when narrow");
        // Shrunk uniformly to fit instead of dropped.
        assert!(l.status_size < 12.0, "{l:?}");
        assert!(105.0 * (l.status_size / 12.0) <= l.text_w + 0.01, "{l:?}");
    }

    #[test]
    fn plate_layout_degenerate_cover_has_no_content() {
        let tiny = Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0));
        assert!(plate_layout(tiny, 105.0).is_none(), "cover paint alone hides the stale look");
        let empty = Rect::from_min_max(pos2(0.0, 0.0), pos2(0.0, 0.0));
        assert!(plate_layout(empty, 105.0).is_none());
    }

    fn badge_doc_info() -> pdfcraft_render::DocInfo {
        let page = |label: &str| pdfcraft_render::PageInfo {
            width: 600.0,
            height: 800.0,
            label: label.to_string(),
            crop: [0.0, 0.0, 600.0, 800.0],
            rotation: 0,
        };
        pdfcraft_render::DocInfo { pages: vec![page("1"), page("2")], ..Default::default() }
    }

    /// Render the harness frame and count (green-tick, dark-text) pixels inside the
    /// screen-space `cover` rect. Green ~= Acrobat #00a651 tick fill.
    fn stamp_pixels(h: &mut egui_kittest::Harness<'static, PdfCraftApp>, cover: Rect) -> (u32, u32) {
        let image = h.render().expect("a rendered frame");
        // The builder below uses 1200x800 logical points.
        let (sx, sy) = (image.width() as f32 / 1200.0, image.height() as f32 / 800.0);
        let (left, top, right, bottom) =
            ((cover.left() * sx) as u32, (cover.top() * sy) as u32, (cover.right() * sx) as u32, (cover.bottom() * sy) as u32);
        let mut green = 0u32;
        let mut dark = 0u32;
        for y in top..bottom.min(image.height()) {
            for x in left..right.min(image.width()) {
                let p = image.get_pixel(x, y).0;
                let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
                if g - r > 60 && g > 100 && b < 160 {
                    green += 1;
                }
                if r < 100 && g < 100 && b < 100 {
                    dark += 1;
                }
            }
        }
        (green, dark)
    }

    /// Plate data follows real validation through the app: yellow while the signer is
    /// untrusted, green once trusted — recomputed, never stored.
    #[test]
    fn widget_badges_follow_trust_through_the_app() {
        use egui_kittest::Harness;
        const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 45 >> stream
BT /F1 14 Tf 20 150 Td (Please sign below) Tj ET
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app
        });
        h.run_steps(1);
        // Pin the clock before opening anything: a deterministic signing time.
        {
            let st = h.state_mut();
            let s = std::mem::replace(&mut st.session, pdfcraft_engine::Session::new()).with_clock(|| 1_800_000_000);
            st.session = s;
            st.open_bytes("contract.pdf", None, FIXTURE.to_vec()).expect("opens");
        }
        h.run_steps(6);
        let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
        let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
        let signer = digital_id.certificate.clone();
        let doc_id = h.state().views[0].id;
        let opts = SignOptions { page: 0, rect: Some([20.0, 20.0, 180.0, 60.0]), ..SignOptions::default() };
        let signed = h.state().session.sign(doc_id, &digital_id, opts).unwrap();
        h.state_mut().session.mark_signed(doc_id, signed, None).unwrap();
        h.run_steps(6);
        // Intact but untrusted: the canvas badge is yellow, like the panel and banner.
        {
            let s = h.state();
            let doc = s.session.get(s.views[0].id).unwrap();
            assert_eq!(doc.signatures.len(), 1);
            let sig = &doc.signatures[0];
            assert!(sig.signed && sig.visible && sig.page == Some(0));
            let badge = widget_badge(sig).expect("a badge for the signed widget");
            assert_eq!((badge.icon, badge.color), ("triangle-alert", Color32::from_rgb(0xE6, 0x86, 0x19)));
            let view = &s.views[0];
            let xf = view.page_xform(0).expect("a painted page transform");
            let plates = widget_plates(&doc.signatures, 0, &xf, &doc.info);
            assert_eq!(plates.len(), 1);
            let r = sig.rect.unwrap();
            let sr = xf.user_rect(&doc.info, 0, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]);
            assert_eq!(plates[0].cover, sr, "the opaque plate hides the whole baked appearance");
            assert_eq!(plates[0].badge.icon, "triangle-alert");
            // No green tick pixels while untrusted: the yellow warning plate paints no green.
            assert_eq!(stamp_pixels(&mut h, sr).0, 0, "no green tick before trust");
        }
        // Trusting the signer turns the badge green without touching the document.
        let before = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
        h.state_mut().trust_certificate(signer);
        h.run_steps(3);
        {
            let s = h.state();
            let doc = s.session.get(s.views[0].id).unwrap();
            assert_eq!(*before, *doc.bytes, "revalidation must not rewrite the file");
            let sig = &doc.signatures[0];
            let badge = widget_badge(sig).expect("a badge for the signed widget");
            assert_eq!((badge.icon, badge.color), ("circle-check", Color32::from_rgb(0x2D, 0x9D, 0x78)));
            assert_eq!(banner(&doc.signatures).map(|b| b.0), Some("circle-check"));
            // The plate is unchanged geometrically — only the verdict flipped — and it
            // still covers the widget, so no stale "not verified" look can leak through.
            let view = &s.views[0];
            let xf = view.page_xform(0).expect("a painted page transform");
            let plates = widget_plates(&doc.signatures, 0, &xf, &doc.info);
            assert_eq!(plates.len(), 1);
            let r = sig.rect.unwrap();
            let sr = xf.user_rect(&doc.info, 0, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]);
            assert_eq!(plates[0].cover, sr);
            assert!(plates[0].badge.label.contains("valid") && !plates[0].badge.label.contains("unknown"));
            // The complete visual state travels with the plate: tick + valid text +
            // signer details, with a signing date for the details line.
            assert!(plates[0].signer.as_deref().is_some_and(|s| !s.trim().is_empty()));
            assert!(plates[0].date.as_deref().is_some_and(|s| !s.trim().is_empty()));
            let status_w = plates[0].badge.label.len() as f32 * 7.0;
            let layout = plate_layout(sr, status_w).expect("a layout for the real widget");
            assert!(layout.status_size > 0.0 && layout.text_x < sr.right());
        }
        // Actual pixels: the stamp paints green tick pixels and dark title/body text
        // inside the widget cover — the rendering regression test.
        let cover = h.state().session.get(h.state().views[0].id).unwrap().signatures[0].rect.map(|r| {
            let s = h.state();
            let doc = s.session.get(s.views[0].id).unwrap();
            let xf = s.views[0].page_xform(0).expect("a painted page transform");
            xf.user_rect(&doc.info, 0, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32])
        });
        let cover = cover.expect("a widget rect");
        let (green, dark) = stamp_pixels(&mut h, cover);
        assert!(green >= 40, "the green tick painted only {green} pixels in {cover:?}");
        assert!(dark >= 15, "the stamp title/body painted only {dark} pixels in {cover:?}");
        if let Ok(path) = std::env::var("PDFCRAFT_STAMP_SHOT") {
            let image = h.render().expect("a rendered frame");
            image.save(&path).unwrap();
        }
    }
}
