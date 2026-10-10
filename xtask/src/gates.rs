//! Quality gates: layering, wasm, the CI bundle, corpora and the robustness sweep.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, anyhow, bail};

use crate::layers;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask has a parent dir").to_path_buf()
}

/// Cargo's target directory: `CARGO_TARGET_DIR` when it is set (parallel agents use their own),
/// else `target/`.
pub fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), |d| root().join(d))
}

/// The release `pdfcraft-cli` that `cargo build --release -p pdfcraft-cli` produced.
pub fn release_cli() -> PathBuf {
    target_dir().join("release").join(format!("pdfcraft-cli{}", std::env::consts::EXE_SUFFIX))
}

fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(mut cmd: Command, what: &str) -> anyhow::Result<()> {
    eprintln!("$ {what}");
    let status = cmd.status().with_context(|| format!("{what}: failed to spawn"))?;
    if !status.success() {
        bail!("{what}: exited with {status}");
    }
    Ok(())
}

fn metadata() -> anyhow::Result<serde_json::Value> {
    let out = cargo().args(["metadata", "--format-version", "1", "--no-deps"]).output().context("cargo metadata")?;
    if !out.status.success() {
        bail!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

pub fn layers(_: &[String]) -> anyhow::Result<()> {
    let crates = layers::from_metadata(&metadata()?).map_err(|e| anyhow!(e))?;
    println!("Dependency layering (plan/architecture.md §3)\n");
    println!("{:<24} {:<14} workspace deps", "crate", "layer");
    for c in &crates {
        let ws: Vec<String> = c
            .deps
            .iter()
            .filter(|d| d.workspace)
            .map(|d| format!("{}{}", layers::short_name(&d.name), if d.kind == layers::DepKind::Dev { " (dev)" } else { "" }))
            .collect();
        println!("{:<24} {:<14} {}", c.name, layers::describe(layers::classify(&c.name)), ws.join(", "));
    }
    let violations = layers::check(&crates);
    println!();
    if violations.is_empty() {
        println!("OK: {} crates, no layering violations.", crates.len());
        return Ok(());
    }
    for v in &violations {
        println!("  - {v}");
    }
    bail!("{} layering violation(s)", violations.len())
}

pub fn wasm(_: &[String]) -> anyhow::Result<()> {
    let crates = layers::from_metadata(&metadata()?).map_err(|e| anyhow!(e))?;
    let set: Vec<String> = crates
        .into_iter()
        .filter(|c| matches!(layers::classify(&c.name), Some(layers::Class::Layer(_) | layers::Class::Standalone(_))))
        .map(|c| c.name)
        .collect();
    let mut failed = Vec::new();
    for pkg in &set {
        let mut c = cargo();
        c.args(["check", "--target", "wasm32-unknown-unknown", "-p", pkg]);
        if run(c, &format!("cargo check --target wasm32-unknown-unknown -p {pkg}")).is_err() {
            failed.push(pkg.clone());
        }
    }
    if failed.is_empty() {
        println!("wasm32: {} crates ok", set.len());
        Ok(())
    } else {
        bail!("wasm32 check failed for: {}", failed.join(", "))
    }
}

pub fn ci(_: &[String]) -> anyhow::Result<()> {
    type Step = (&'static str, Box<dyn Fn() -> anyhow::Result<()>>);
    let steps: Vec<Step> = vec![
        ("fmt", Box::new(|| run_args(&["fmt", "--all", "--", "--check"]))),
        ("clippy", Box::new(|| run_args(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]))),
        ("test", Box::new(|| run_args(&["test", "--workspace"]))),
        ("layers", Box::new(|| layers(&[]))),
        ("wasm", Box::new(|| wasm(&[]))),
        ("assets", Box::new(|| crate::assets::run(&[]))),
        ("deny", Box::new(|| deny(&[]))),
        ("parity", Box::new(|| crate::parity::run(&[]))),
    ];
    for (i, (name, f)) in steps.iter().enumerate() {
        eprintln!("\n=== ci: {name} ===");
        if let Err(e) = f() {
            println!("\nCI: {} passed, FAILED at `{name}`: {e:#}", i);
            bail!("ci failed at `{name}`");
        }
    }
    println!("\nCI: all {} steps passed", steps.len());
    Ok(())
}

/// Dependency licences, bans, sources and advisories (`deny.toml`). Skipped with a hint when
/// cargo-deny is not installed, except under CI (`CI` set), where it is required.
pub fn deny(_: &[String]) -> anyhow::Result<()> {
    let installed = Command::new("cargo-deny").arg("--version").output().is_ok_and(|o| o.status.success());
    if !installed {
        if std::env::var_os("CI").is_some() {
            bail!("cargo-deny is required in CI: cargo install cargo-deny --locked");
        }
        eprintln!("deny: SKIPPED, cargo-deny is not installed (cargo install cargo-deny --locked)");
        return Ok(());
    }
    run_args(&["deny", "--log-level", "error", "check"])
}

fn run_args(args: &[&str]) -> anyhow::Result<()> {
    let mut c = cargo();
    c.args(args);
    run(c, &format!("cargo {}", args.join(" ")))
}

pub fn corpus(args: &[String]) -> anyhow::Result<()> {
    crate::corpus::run_cmd(args)
}

pub fn check(args: &[String]) -> anyhow::Result<()> {
    let update = args.iter().any(|a| a == "--update-baseline");
    let corpus = crate::corpus::pdfs();
    // The baseline in xtask/baselines/pdfjs.json describes one exact tree. Sweeping a different
    // one silently moves the numbers it is compared against, so verify the pin first.
    match crate::corpus::state()? {
        crate::corpus::State::Absent => bail!("corpus missing: run `cargo xtask corpus` first"),
        crate::corpus::State::Mismatch { detail } => {
            bail!("corpus does not match the pin ({detail}); run `cargo xtask corpus` to repair it")
        }
        crate::corpus::State::Verified { files } => println!("corpus verified: {files} files at {}", crate::corpus::PDFJS_COMMIT),
    }
    run_args(&["build", "--release", "-p", "pdfcraft-cli"])?;
    let report = target_dir().join("check-pdfjs.json");
    let mut c = Command::new(release_cli());
    c.args(["check"]).arg(&corpus).args(["--timeout", "20", "--dpi", "36", "--json"]).arg(&report);
    run(c, "pdfcraft-cli check corpus/pdfjs")?;
    let results: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(&report)?)?;
    let name =
        |v: &serde_json::Value| Path::new(v["file"].as_str().unwrap_or("")).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let status = |v: &serde_json::Value| v["status"].as_str().unwrap_or("?").split(':').next().unwrap_or("?").to_string();
    let current: BTreeMap<String, String> = results.iter().filter(|v| status(v) != "ok").map(|v| (name(v), status(v))).collect();
    let crashes: Vec<&String> = current.iter().filter(|(_, s)| s.as_str() == "crash").map(|(f, _)| f).collect();
    let baseline_path = root().join("xtask/baselines/pdfjs.json");
    if update {
        std::fs::create_dir_all(baseline_path.parent().expect("has parent"))?;
        std::fs::write(&baseline_path, serde_json::to_string_pretty(&current)? + "\n")?;
        println!("baseline updated: {} known non-ok files", current.len());
    }
    let baseline: BTreeMap<String, String> =
        std::fs::read_to_string(&baseline_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let regressions: Vec<String> = current.iter().filter(|(f, _)| !baseline.contains_key(*f)).map(|(f, s)| format!("{f} ({s})")).collect();
    let fixed: Vec<&String> = baseline.keys().filter(|f| !current.contains_key(*f)).collect();
    println!(
        "\n{} files: {} ok, {} known issues, {} regressions, {} newly fixed, {} crashes",
        results.len(),
        results.len() - current.len(),
        current.len() - regressions.len(),
        regressions.len(),
        fixed.len(),
        crashes.len()
    );
    if !fixed.is_empty() {
        println!("newly fixed (run with --update-baseline): {fixed:?}");
    }
    if !crashes.is_empty() {
        bail!("crashes: {crashes:?}");
    }
    if !regressions.is_empty() {
        bail!("regressions: {regressions:?}");
    }
    Ok(())
}

/// `cargo xtask text-oracle [--limit N]`: compare `pdfcraft-cli text` with poppler's `pdftotext`
/// (external oracle process, never linked) over the corpus. Reports word-level F1 per file and the
/// median. Plan target (M2): median ≥ 0.97.
pub fn text_oracle(args: &[String]) -> anyhow::Result<()> {
    let limit: usize = args.iter().position(|a| a == "--limit").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    if Command::new("pdftotext").arg("-v").output().is_err() {
        bail!("pdftotext (poppler) not found; install it to run the text oracle (brew install poppler / apt install poppler-utils)");
    }
    run_args(&["build", "--release", "-p", "pdfcraft-cli"])?;
    let corpus = root().join("corpus/pdfjs/test/pdfs");
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(&corpus)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "pdf")).collect();
    files.sort();
    let cli = release_cli();
    let mut scores = Vec::new();
    let mut worst: Vec<(f64, String)> = Vec::new();
    for f in files.iter().take(limit) {
        let ours = run_capture(Command::new(&cli).arg("text").arg(f), 20);
        let theirs = run_capture(Command::new("pdftotext").arg("-q").arg(f).arg("-"), 20);
        let (Some(ours), Some(theirs)) = (ours, theirs) else { continue };
        let (a, b) = (words(&ours), words(&theirs));
        if b.len() < 20 {
            continue; // image-only or near-empty per the oracle
        }
        let score = f1(&a, &b);
        scores.push(score);
        worst.push((score, f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    }
    if scores.is_empty() {
        bail!("no comparable files");
    }
    scores.sort_by(f64::total_cmp);
    worst.sort_by(|a, b| a.0.total_cmp(&b.0));
    let median = scores[scores.len() / 2];
    let mean = scores.iter().sum::<f64>() / scores.len() as f64;
    println!("text oracle: {} files with text; median F1 {:.3}, mean {:.3}, p10 {:.3}", scores.len(), median, mean, scores[scores.len() / 10]);
    println!("lowest:");
    for (s, n) in worst.iter().take(12) {
        println!("  {s:.3}  {n}");
    }
    Ok(())
}

fn run_capture(cmd: &mut Command, timeout_s: u64) -> Option<String> {
    let mut child = cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let start = std::time::Instant::now();
    let mut out = String::new();
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed().as_secs() > timeout_s => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(_) => return None,
        }
    }
    out.push_str(&reader.join().ok()?);
    Some(out)
}

fn words(s: &str) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for w in s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        *m.entry(w.to_lowercase()).or_insert(0) += 1;
    }
    m
}

fn f1(a: &BTreeMap<String, usize>, b: &BTreeMap<String, usize>) -> f64 {
    let common: usize = a.iter().map(|(w, n)| (*n).min(*b.get(w).unwrap_or(&0))).sum();
    let (na, nb): (usize, usize) = (a.values().sum(), b.values().sum());
    if na == 0 || nb == 0 {
        return 0.0;
    }
    let (p, r) = (common as f64 / na as f64, common as f64 / nb as f64);
    if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
}

#[cfg(test)]
mod tests {
    use super::root;

    // Issue #211: Finder lists the app under Open With > Recommended only when the
    // bundle claims PDFs by UTI (#176).
    #[test]
    fn macos_bundle_claims_pdf_by_uti() {
        let pdf = pdf_document_type();
        assert!(pdf.contains("<string>com.adobe.pdf</string>"), "claim PDFs by UTI, not just extension/MIME");
        assert!(pdf.contains("<string>Editor</string>"), "PDF role stays Editor");
        assert!(pdf.contains("<string>Alternate</string>"), "rank stays Alternate: offered without taking over Preview");
        assert!(pdf.contains("<key>CFBundleTypeIconFile</key>"), "declares document icon file for Finder");
    }

    // The bundle id must match the app id the binary uses; renames change both (#174).
    #[test]
    fn macos_bundle_id_matches_app_id() {
        assert!(read("packaging/macos/Info.plist.in").contains("<string>ai.storyteller.pdfcraft</string>"), "bundle id");
        let main = read("apps/pdfcraft/src/main.rs");
        assert!(main.contains(r#"const APP_ID: &str = "ai.storyteller.pdfcraft""#), "APP_ID drifted from the bundle id");
    }

    // Committed file with `<!-- -->` comments stripped, so a commented-out claim cannot pass.
    fn read(rel: &str) -> String {
        let text = std::fs::read_to_string(root().join(rel)).expect("committed file");
        let mut out = String::with_capacity(text.len());
        let mut rest = text.as_str();
        while let Some(i) = rest.find("<!--") {
            out.push_str(&rest[..i]);
            rest = rest[i..].find("-->").map_or("", |j| &rest[i + j + 3..]);
        }
        out.push_str(rest);
        out
    }

    // The `<dict>` inside `CFBundleDocumentTypes` that mentions the PDF UTI: scoped so a
    // UTI string anywhere else cannot satisfy the asserts above.
    fn pdf_document_type() -> String {
        let plist = read("packaging/macos/Info.plist.in");
        let docs = plist.split("<key>CFBundleDocumentTypes</key>").nth(1).expect("CFBundleDocumentTypes");
        docs.split("<dict>")
            .map(|chunk| chunk.split("</dict>").next().unwrap_or(""))
            .find(|chunk| chunk.contains("com.adobe.pdf"))
            .expect("a document type for com.adobe.pdf")
            .to_string()
    }
}
