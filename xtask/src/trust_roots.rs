//! `cargo xtask trust-roots`: rebuild `crates/sign/data/builtin-roots.der` from the manifest
//! `crates/sign/data/builtin-roots.toml`.
//!
//! Every root is fetched from the CA's own repository (the manifest has the URL) and must hash to
//! the SHA-256 pinned there; a difference stops the run and changes nothing, so a CA replacing a
//! file, or a hijacked download, is noticed instead of shipped. The manifest names, per root, the
//! independent source its pin was compared with. The file is the DER certificates one after the
//! other, in manifest order.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// A root certificate is a kilobyte or two; anything bigger is not what the pin is for.
const MAX_CERT_BYTES: &str = "100000";

#[derive(Deserialize)]
struct Manifest {
    root: Vec<Root>,
}

#[derive(Deserialize)]
struct Root {
    name: String,
    url: String,
    sha256: String,
    checked_against: String,
}

/// DER from a downloaded file that is either DER or one PEM `CERTIFICATE` block.
fn der_of(bytes: &[u8]) -> Result<Vec<u8>> {
    let text = String::from_utf8_lossy(bytes);
    if let Some(start) = text.find("-----BEGIN CERTIFICATE-----") {
        let body = &text[start + "-----BEGIN CERTIFICATE-----".len()..];
        let body = body.split("-----END CERTIFICATE-----").next().context("no PEM end")?;
        return super::trust_lists::base64(body).context("bad PEM base64");
    }
    if bytes.first() != Some(&0x30) {
        bail!("neither DER nor PEM");
    }
    Ok(bytes.to_vec())
}

pub fn run(_args: &[String]) -> Result<()> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/sign/data");
    let manifest: Manifest = toml::from_str(&std::fs::read_to_string(dir.join("builtin-roots.toml"))?).context("builtin-roots.toml")?;
    let mut bundle = Vec::new();
    let mut problems = Vec::new();
    for r in &manifest.root {
        let fetched = Command::new("curl")
            .args(["-sSfL", "-m", "60", "--max-filesize", MAX_CERT_BYTES, "--retry", "3", "--retry-all-errors", &r.url])
            .output()
            .context("running curl")?;
        if !fetched.status.success() {
            problems.push(format!("{}: {}: {}", r.name, r.url, String::from_utf8_lossy(&fetched.stderr).trim()));
            continue;
        }
        match der_of(&fetched.stdout) {
            Ok(der) => {
                let sum: String = Sha256::digest(&der).iter().map(|b| format!("{b:02x}")).collect();
                if sum == r.sha256 {
                    println!("  ok  {} ({})", r.name, r.checked_against);
                    bundle.extend(der);
                } else {
                    problems.push(format!("{}: {} now hashes to {sum}, the manifest pins {}", r.name, r.url, r.sha256));
                }
            }
            Err(e) => problems.push(format!("{}: {}: {e:#}", r.name, r.url)),
        }
    }
    if !problems.is_empty() {
        bail!("{} root(s) did not match; builtin-roots.der was not changed:\n  {}", problems.len(), problems.join("\n  "));
    }
    std::fs::write(dir.join("builtin-roots.der"), &bundle)?;
    let sum: String = Sha256::digest(&bundle).iter().map(|b| format!("{b:02x}")).collect();
    println!(
        "{} roots, {} bytes -> crates/sign/data/builtin-roots.der\nsha256 = \"{sum}\"  (put this in ATTRIBUTION.toml)",
        manifest.root.len(),
        bundle.len()
    );
    Ok(())
}
