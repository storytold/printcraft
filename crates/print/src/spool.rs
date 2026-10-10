//! The system print spooler. On macOS and Linux this is CUPS: printers come from `lpstat` — its
//! queues, and the driverless destinations it can print to without one — and jobs are piped to
//! `lp` with the job options (copies, collation, duplex, colour). On Windows the printers come
//! from .NET's `PrinterSettings` and jobs are rendered with the in-box `Windows.Data.Pdf` and spooled
//! with the driver's own settings — both through `powershell`, so no Win32 API is called here.
//! Other platforms report that printing isn't available yet; the print-ready PDF can still be
//! saved.

use crate::PrintError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Printer {
    pub name: String,
    pub default: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Duplex {
    #[default]
    Off,
    LongEdge,
    ShortEdge,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// `None` = the system default printer.
    pub printer: Option<String>,
    pub copies: u32,
    pub collate: bool,
    pub duplex: Duplex,
    pub grayscale: bool,
    pub title: String,
    /// The printer driver's own job options (a PPD keyword and the chosen value), such as the
    /// paper tray or paper type; see [`printer_options`]. CUPS only.
    pub options: Vec<(String, String)>,
}

impl Default for Job {
    fn default() -> Self {
        Job { printer: None, copies: 1, collate: true, duplex: Duplex::Off, grayscale: false, title: "PdfCraft".into(), options: Vec::new() }
    }
}

/// The `lpstat -d` line's destination name, when the spooler has a default.
fn default_destination(out: &str) -> Option<&str> {
    out.lines().find_map(|l| l.strip_prefix("system default destination:")).map(str::trim)
}

/// Parse `lpstat -p -d` output.
pub fn parse_lpstat(out: &str) -> Vec<Printer> {
    let default = default_destination(out);
    out.lines()
        .filter_map(|l| l.strip_prefix("printer "))
        .filter_map(|l| l.split_whitespace().next())
        .map(|n| Printer { name: n.to_string(), default: default == Some(n) })
        .collect()
}

/// Parse `lpstat -e` output: destination names, one per line.
pub fn parse_lpstat_e(out: &str) -> Vec<String> {
    out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// The `lp` arguments for a job. There is no file argument: `lp` reads the job from stdin.
pub fn lp_args(job: &Job) -> Vec<String> {
    let mut a = Vec::new();
    if let Some(p) = &job.printer {
        a.extend(["-d".to_string(), p.clone()]);
    }
    a.extend(["-n".to_string(), job.copies.clamp(1, 999).to_string()]);
    a.extend(["-t".to_string(), job.title.clone()]);
    let mut opt = |o: &str| a.extend(["-o".to_string(), o.to_string()]);
    opt(if job.collate { "collate=true" } else { "collate=false" });
    opt(match job.duplex {
        Duplex::Off => "sides=one-sided",
        Duplex::LongEdge => "sides=two-sided-long-edge",
        Duplex::ShortEdge => "sides=two-sided-short-edge",
    });
    if job.grayscale {
        opt("print-color-mode=monochrome");
    }
    // The sheets are already laid out at their final size.
    opt("fit-to-page=false");
    for (key, value) in &job.options {
        // Driver options are PPD keywords; anything else is not something lp can be given.
        if is_ppd_keyword(key) && is_ppd_keyword(value) && !HANDLED_OPTIONS.contains(&key.as_str()) {
            a.extend(["-o".to_string(), format!("{key}={value}")]);
        }
    }
    a
}

/// One of the printer driver's job options (a PPD `OpenUI` entry), such as the paper tray.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrinterOption {
    /// The PPD keyword (`InputSlot`).
    pub key: String,
    /// What the driver calls it (`Paper tray`); the keyword when it gives no name.
    pub label: String,
    /// The driver's group (`Paper Handling`); empty outside a group.
    pub group: String,
    /// The choices: keyword and label (`Tray2`, `Tray 2`).
    pub choices: Vec<(String, String)>,
    /// The choice the printer uses when a job doesn't say.
    pub default: String,
}

/// Options PdfCraft's Print dialog sets itself (paper, two-sided, collation), so the driver's
/// copies of them are neither shown nor sent.
pub const HANDLED_OPTIONS: &[&str] = &["PageSize", "PageRegion", "Duplex", "Collate", "ImageableArea", "PaperDimension"];

/// Caps on what a PPD may make the dialog hold (PPDs are text files from the printer vendor).
const MAX_OPTIONS: usize = 400;
const MAX_CHOICES: usize = 400;

/// A PPD keyword: printable ASCII without spaces, `/`, `:` or `=`.
fn is_ppd_keyword(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'/' | b':' | b'=' | b'"'))
}

/// `Name/Label` → (`Name`, `Label`), the label falling back to the name.
fn name_label(s: &str) -> (&str, &str) {
    match s.split_once('/') {
        Some((n, l)) if !l.trim().is_empty() => (n.trim(), l.trim()),
        Some((n, _)) => (n.trim(), n.trim()),
        None => (s.trim(), s.trim()),
    }
}

/// The job options a PPD offers, in file order, without the installable-hardware group and the
/// options PdfCraft sets itself ([`HANDLED_OPTIONS`]). Lenient: malformed lines are skipped.
pub fn parse_ppd(ppd: &str) -> Vec<PrinterOption> {
    let mut out: Vec<PrinterOption> = Vec::new();
    let (mut group, mut group_key) = (String::new(), String::new());
    let mut open: Option<PrinterOption> = None;
    for line in ppd.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("*OpenGroup:") {
            let (k, l) = name_label(rest);
            (group_key, group) = (k.to_string(), l.to_string());
        } else if line.starts_with("*CloseGroup") {
            (group_key, group) = (String::new(), String::new());
        } else if let Some(rest) = line.strip_prefix("*OpenUI") {
            let Some((head, _kind)) = rest.trim().rsplit_once(':') else { continue };
            let (key, label) = name_label(head.trim().trim_start_matches('*'));
            open = is_ppd_keyword(key).then(|| PrinterOption {
                key: key.to_string(),
                label: label.to_string(),
                group: group.clone(),
                choices: Vec::new(),
                default: String::new(),
            });
        } else if line.starts_with("*CloseUI") {
            if let Some(o) = open.take()
                && !o.choices.is_empty()
                && group_key != "InstallableOptions"
                && !HANDLED_OPTIONS.contains(&o.key.as_str())
                && out.len() < MAX_OPTIONS
            {
                out.push(o);
            }
        } else if let Some(o) = open.as_mut() {
            let Some(rest) = line.strip_prefix('*') else { continue };
            if let Some(value) = rest.strip_prefix("Default").and_then(|r| r.strip_prefix(o.key.as_str())).and_then(|r| r.strip_prefix(':')) {
                o.default = value.trim().to_string();
            } else if let Some(rest) = rest.strip_prefix(o.key.as_str()).and_then(|r| r.strip_prefix(' ')) {
                // `*InputSlot Tray2/Tray 2: "<< ... >>"`; the code after the colon is the driver's.
                let head = rest.split_once(':').map_or(rest, |(h, _)| h);
                let (choice, label) = name_label(head);
                if is_ppd_keyword(choice) && o.choices.len() < MAX_CHOICES && o.choices.iter().all(|(c, _)| c != choice) {
                    o.choices.push((choice.to_string(), label.to_string()));
                }
            }
        }
    }
    for o in &mut out {
        if !o.choices.iter().any(|(c, _)| *c == o.default) {
            o.default = o.choices.first().map(|(c, _)| c.clone()).unwrap_or_default();
        }
    }
    out
}

/// The current choice of each option in `lpoptions -p NAME -l` output (`Key/Label: a *b c`): the
/// queue's defaults, including the user's own (`~/.cups/lpoptions`).
pub fn parse_lpoptions(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let (head, choices) = l.split_once(':')?;
            let (key, _) = name_label(head);
            let current = choices.split_whitespace().find_map(|c| c.strip_prefix('*'))?;
            Some((key.to_string(), current.to_string()))
        })
        .collect()
}

/// A PPD's text: UTF-8, or ISO Latin-1 (the PPD default) when it isn't valid UTF-8.
pub fn ppd_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

/// The driver's job options for a printer (CUPS: its PPD, with the queue's current defaults);
/// empty when it has none or on other platforms. Reads files and runs `lpoptions`, so call it
/// when the user asks, not every frame.
pub fn printer_options(printer: &str) -> Vec<PrinterOption> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        // Queue names never contain `/`; anything else is not a queue and not a path to follow.
        if !is_ppd_keyword(printer) {
            return Vec::new();
        }
        const PPD_CAP: u64 = 16 << 20;
        let ppd = std::fs::File::open(format!("/etc/cups/ppd/{printer}.ppd")).ok().and_then(|f| {
            use std::io::Read as _;
            let mut bytes = Vec::new();
            f.take(PPD_CAP).read_to_end(&mut bytes).ok().map(|_| bytes)
        });
        let mut options = ppd.map(|b| parse_ppd(&ppd_text(&b))).unwrap_or_default();
        let current = std::process::Command::new("lpoptions")
            .args(["-p", printer, "-l"])
            .output()
            .map(|o| parse_lpoptions(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default();
        for (key, value) in current {
            if let Some(o) = options.iter_mut().find(|o| o.key == key)
                && o.choices.iter().any(|(c, _)| *c == value)
            {
                o.default = value;
            }
        }
        options
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        let _ = printer;
        Vec::new()
    }
}

/// Whether [`open_printer_preferences`] can show the driver's own settings window (Windows).
pub const HAS_PRINTER_PREFERENCES: bool = cfg!(windows);

/// Open the printer driver's Printing Preferences window (Windows: `printui.dll`). What the user
/// sets there is saved as that printer's defaults for this Windows user, which every job starts
/// from; the Print dialog's copies, two-sided, grayscale and paper still apply on top.
pub fn open_printer_preferences(printer: &str) -> Result<(), PrintError> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // printui takes the name in quotes; Windows printer names can't contain one anyway.
        if printer.is_empty() || printer.contains('"') {
            return Err(PrintError::Spool("not a printer name".into()));
        }
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let rundll = std::path::Path::new(&root).join("System32").join("rundll32.exe");
        std::process::Command::new(rundll)
            .raw_arg("printui.dll,PrintUIEntry")
            .raw_arg("/e")
            .raw_arg("/n")
            .raw_arg(format!("\"{printer}\""))
            .spawn()
            .map(|_| ())
            .map_err(|e| PrintError::Spool(format!("the printer's preferences could not be opened: {e}")))
    }
    #[cfg(not(windows))]
    {
        let _ = printer;
        Err(PrintError::Spool("printer preferences open only on Windows; use the printer's options instead".into()))
    }
}

/// `lpstat`, forced to print untranslated messages so [`parse_lpstat`] and [`parse_lpstat_e`] can
/// read them.
///
/// `LC_ALL`/`LANG=C` is enough on Linux. macOS CUPS ignores them and follows the user's
/// interface language (`AppleLanguages`) unless `SOFTWARE` is set, in which case it uses `LANG`.
fn lpstat_with(args: &[&str]) -> std::process::Command {
    let mut c = std::process::Command::new("lpstat");
    c.args(args).env("LC_ALL", "C").env("LANG", "C").env("SOFTWARE", "PdfCraft");
    c
}

/// `lpstat -p -d`: the spooler's queues and the default destination.
pub fn lpstat_command() -> std::process::Command {
    lpstat_with(&["-p", "-d"])
}

/// `lpstat -e`: every destination CUPS can print to, including driverless network printers no
/// permanent queue exists for (a queue is built only when a job needs one). GTK's and macOS's
/// print dialogs list them the same way. CUPS ≥ 1.7 (2013).
///
/// Gated like its only caller, `printers`' unix block: off unix this is dead code, and the
/// Windows clippy gate builds with `-D warnings`.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn lpstat_e_command() -> std::process::Command {
    lpstat_with(&["-e"])
}

/// The printers the system knows (empty when there are none or no spooler): its queues, then the
/// driverless destinations CUPS can print to without one.
pub fn printers() -> Vec<Printer> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let Some(queues) = lpstat_command().output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()) else {
            return Vec::new();
        };
        // A spooler without `-e`, or one whose discovery times out, only loses the driverless names.
        let available = lpstat_e_command().output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        printers_parsed(&queues, available.as_deref())
    }
    #[cfg(windows)]
    {
        // The spooler's printers through .NET's `PrinterSettings`, the API the print script
        // spools with, so every name listed is one it can print to (and several times quicker
        // than a CIM query). A machine whose PowerShell is locked down lists no printers and
        // the dialog offers only Save as PDF, as with CUPS that isn't running.
        match powershell_command(POWERSHELL_PRINTERS).output() {
            Ok(o) if o.status.success() => parse_windows_printers(&String::from_utf8_lossy(&o.stdout)),
            _ => Vec::new(),
        }
    }
    #[cfg(not(any(windows, all(unix, not(target_arch = "wasm32")))))]
    {
        Vec::new()
    }
}

/// The Print dialog's printers from lpstat's two answers: the spooler's queues (`-p -d`), then
/// the driverless destinations of `available` (`-e`) that have no queue — CUPS builds a temporary
/// queue when `lp` sends them a job. Each destination once, queues first; the default still comes
/// from `-d`, so a driverless default keeps its marker. `None` when the spooler doesn't know `-e`
/// (CUPS < 1.7, 2013), which loses only the driverless names.
pub fn printers_parsed(queues: &str, available: Option<&str>) -> Vec<Printer> {
    let mut printers = parse_lpstat(queues);
    let default = default_destination(queues);
    if let Some(names) = available {
        for name in parse_lpstat_e(names) {
            if !printers.iter().any(|p| p.name == name) {
                printers.push(Printer { default: default == Some(name.as_str()), name });
            }
        }
    }
    printers
}

/// The `powershell` command that runs a script. `-NoProfile` keeps a user's profile from
/// writing banners into what is parsed, `-NonInteractive` from waiting for input.
#[cfg(windows)]
fn powershell_command(script: &str) -> std::process::Command {
    let mut c = std::process::Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    c
}

/// The Windows printer list: one line per printer, `*` on the system default (the printer a
/// fresh `PrinterSettings` names). UTF-8, through the output encoding the script sets first.
#[cfg(windows)]
const POWERSHELL_PRINTERS: &str = "[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false); Add-Type -AssemblyName System.Drawing; $d = (New-Object System.Drawing.Printing.PrinterSettings).PrinterName; foreach ($n in [System.Drawing.Printing.PrinterSettings]::InstalledPrinters) { if ($n -eq $d) { '*' + $n } else { $n } }";

/// The Print dialog's printers from the Windows printer list: one per line, `*` before the
/// system default (a Windows queue name may not contain `*`). The first `*` wins when a broken
/// spooler marks more than one, so the dialog still has a single default to preselect.
pub fn parse_windows_printers(out: &str) -> Vec<Printer> {
    let mut printers: Vec<Printer> = out
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.is_empty())
        .map(|l| match l.strip_prefix('*') {
            Some(name) => Printer { name: name.to_string(), default: true },
            None => Printer { name: l.to_string(), default: false },
        })
        .collect();
    let mut seen_default = false;
    for p in &mut printers {
        if p.default {
            p.default = !seen_default;
            seen_default = true;
        }
    }
    printers
}

/// Send a print-ready PDF to the spooler. Returns the spooler's message (the job id).
pub fn submit(pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    submit_with_progress(pdf, job, &mut |_, _| {})
}

/// [`submit`], told how far the job has got: `progress(sent, sheets)` after each sheet reaches
/// the spooler. Windows renders every sheet before the spooler has it, which takes seconds, so it
/// reports each one; CUPS takes the whole PDF at once and reports nothing.
pub fn submit_with_progress(pdf: &[u8], job: &Job, progress: &mut dyn FnMut(usize, usize)) -> Result<String, PrintError> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let _ = progress;
        submit_via(std::process::Command::new("lp"), pdf, job)
    }
    #[cfg(windows)]
    {
        windows_submit(pdf, job, &[], progress)
    }
    #[cfg(not(any(windows, all(unix, not(target_arch = "wasm32")))))]
    {
        let _ = (pdf, job, progress);
        Err(PrintError::Spool("printing to a printer isn't available on this platform yet; save the print-ready PDF instead".into()))
    }
}

/// A progress line from the print script: `progress SENT SHEETS`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_progress(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.trim().strip_prefix("progress ")?.split_whitespace();
    let sent = parts.next()?.parse().ok()?;
    let sheets = parts.next()?.parse().ok()?;
    (parts.next().is_none() && sent <= sheets).then_some((sent, sheets))
}

/// [`submit`] on Windows: the print-ready PDF goes to a job file of its own (WinRT reads from
/// a path), the embedded [`WINDOWS_PRINT_SCRIPT`] spools it, and the file is gone again however
/// the job ended. `extra_envs` reaches the script too — the tests use it for
/// `PDFCRAFT_PRINT_DRYRUN`, which renders without a printer.
#[cfg(windows)]
pub(crate) fn windows_submit(
    pdf: &[u8],
    job: &Job,
    extra_envs: &[(&str, &str)],
    progress: &mut dyn FnMut(usize, usize),
) -> Result<String, PrintError> {
    let path = windows_job_file(pdf)?;
    let ran = run_print_script(&path, job, extra_envs, progress);
    // Remove the job file before reporting: a spooler that refused the job must not leave the
    // document behind.
    let _ = std::fs::remove_file(&path);
    ran
}

/// Run [`WINDOWS_PRINT_SCRIPT`] on a job file, passing its progress lines on as they come and
/// returning the rest of what it said.
#[cfg(windows)]
fn run_print_script(
    path: &std::path::Path,
    job: &Job,
    extra_envs: &[(&str, &str)],
    progress: &mut dyn FnMut(usize, usize),
) -> Result<String, PrintError> {
    use std::io::{BufRead as _, Read as _};
    use std::process::Stdio;
    let mut ps = powershell_command(WINDOWS_PRINT_SCRIPT);
    for (key, value) in windows_env_vars(path, job).into_iter().chain(extra_envs.iter().map(|(k, v)| (k.to_string(), v.to_string()))) {
        ps.env(key, value);
    }
    let mut child = ps
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (said, errors) = std::thread::scope(|s| {
        // Errors are read on a thread of their own, so a script that writes many can't stall
        // behind a full pipe while its progress is read here.
        let errors = std::thread::Builder::new().name("print errors".into()).spawn_scoped(s, move || {
            let mut buf = Vec::new();
            if let Some(mut e) = stderr {
                let _ = e.read_to_end(&mut buf);
            }
            buf
        });
        let mut said = Vec::new();
        if let Some(out) = stdout {
            // Bytes, not `lines()`: a line that isn't UTF-8 must not stop the reading, or the
            // script would block on a pipe no one empties.
            for line in std::io::BufReader::new(out).split(b'\n') {
                let Ok(line) = line else { break };
                let line = String::from_utf8_lossy(&line).trim_end().to_string();
                match parse_progress(&line) {
                    Some((sent, sheets)) => progress(sent, sheets),
                    None => said.push(line),
                }
            }
        }
        let errors = errors.ok().and_then(|t| t.join().ok()).unwrap_or_default();
        (said.join("\n").trim().to_string(), errors)
    });
    let status = child.wait().map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    if !status.success() {
        let err = String::from_utf8_lossy(&errors).trim().to_string();
        let err = if err.is_empty() { said } else { err };
        return Err(PrintError::Spool(if err.is_empty() { "the print job was refused".into() } else { err }));
    }
    Ok(said)
}

/// The job file [`windows_submit`] hands to `powershell`, in this user's own temporary folder:
/// a fresh name no other local user can predict (`create_new`, with the process id and the
/// time), written before anything is spawned and removed after. WinRT reads a path, so unlike
/// the job piped to `lp` this one touches disk for the seconds the spooler reads it.
#[cfg(windows)]
fn windows_job_file(pdf: &[u8]) -> Result<std::path::PathBuf, PrintError> {
    use std::io::Write as _;
    let written = |e: std::io::Error| PrintError::Spool(format!("the print job could not be written: {e}"));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    for attempt in 0..64u32 {
        let path = std::env::temp_dir().join(format!("pdfcraft-print-{}-{nanos}-{attempt}.pdf", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(pdf).map_err(written)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(written(e)),
        }
    }
    Err(PrintError::Spool("the print job could not be written: no free name in the temporary folder".into()))
}

/// The print script's environment: no arguments, so a printer name with quotes or spaces
/// survives the command line and the document's path is not visible in the process list while
/// it prints. The driver options in [`Job::options`] are CUPS' PPD keywords; on Windows the
/// driver's own per-printer preferences apply (Print ▸ Properties…), so they are not sent.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn windows_env_vars(path: &std::path::Path, job: &Job) -> Vec<(String, String)> {
    vec![
        ("PDFCRAFT_PRINT_FILE".into(), path.to_string_lossy().into_owned()),
        ("PDFCRAFT_PRINT_PRINTER".into(), job.printer.clone().unwrap_or_default()),
        ("PDFCRAFT_PRINT_COPIES".into(), job.copies.to_string()),
        ("PDFCRAFT_PRINT_COLLATE".into(), if job.collate { "1" } else { "0" }.into()),
        (
            "PDFCRAFT_PRINT_DUPLEX".into(),
            match job.duplex {
                Duplex::Off => "off",
                Duplex::LongEdge => "long-edge",
                Duplex::ShortEdge => "short-edge",
            }
            .into(),
        ),
        ("PDFCRAFT_PRINT_GRAYSCALE".into(), if job.grayscale { "1" } else { "0" }.into()),
        ("PDFCRAFT_PRINT_TITLE".into(), job.title.clone()),
    ]
}

/// The embedded print script (see [`windows_submit`]).
#[cfg(windows)]
pub(crate) const WINDOWS_PRINT_SCRIPT: &str = include_str!("windows_print.ps1");

/// [`submit`] with the spooler command given, so tests can stand in for `lp`.
///
/// The job goes to `lp` on stdin, never through a file: a predictable job folder in a shared
/// temp directory lets another local user read printed documents or swap the file before `lp`
/// reads it.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
pub(crate) fn submit_via(mut lp: std::process::Command, pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = lp
        .args(lp_args(job))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    let stdin = child.stdin.take();
    // Feed the job on its own thread while collecting the output, so neither side can fill a
    // pipe and wait on the other. Dropping `stdin` at the end closes it: the end of the job.
    let fed_and_out = std::thread::scope(|s| {
        let feeder = std::thread::Builder::new().name("print job".into()).spawn_scoped(s, move || match stdin {
            Some(mut stdin) => stdin.write_all(pdf),
            None => Err(std::io::Error::other("no pipe to the spooler")),
        });
        match feeder {
            Ok(feeder) => {
                let out = child.wait_with_output();
                Ok((feeder.join(), out))
            }
            Err(e) => {
                // No thread to feed it: stop `lp` rather than leave it with an empty job.
                let _ = child.kill();
                let _ = child.wait();
                Err(PrintError::Spool(format!("the print job could not be sent to the spooler: {e}")))
            }
        }
    });
    let (fed, out) = fed_and_out?;
    let out = out.map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    if !out.status.success() {
        // `lp` may refuse before reading the job (an unknown printer); its message says why,
        // and the broken pipe that leaves behind does not.
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(PrintError::Spool(if err.is_empty() { "the print job was refused".into() } else { err }));
    }
    match fed {
        Ok(Ok(())) => Ok(job_message(&out.stdout)),
        Ok(Err(e)) => Err(PrintError::Spool(format!("the print job could not be sent to the spooler: {e}"))),
        Err(_) => Err(PrintError::Spool("the print job could not be sent to the spooler".into())),
    }
}

/// `lp`'s reply ("request id is Office-12"), without the "(0 file(s))" CUPS adds when the job came
/// on stdin: it reads as if nothing was sent.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
pub(crate) fn job_message(stdout: &[u8]) -> String {
    let s = String::from_utf8_lossy(stdout);
    let s = s.trim();
    s.strip_suffix("(0 file(s))").map_or(s, str::trim_end).to_string()
}
