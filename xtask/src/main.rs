//! Project automation for PdfCraft, run via `cargo xtask <command>`.

use std::process::ExitCode;

mod assets;
mod corpus;
mod demo_pdf;
mod fuzz;
mod gates;
mod layers;
mod macos_integration;
mod parity;
mod screenshots;
mod trust_lists;
mod trust_roots;
mod version;

type Command = fn(&[String]) -> anyhow::Result<()>;

/// Every subcommand: name, one-line summary, entry point.
const COMMANDS: &[(&str, &str, Command)] = &[
    ("version", "Print the workspace version, or `version set X.Y.Z[-pre]` to change it and refresh Cargo.lock", version_cmd),
    ("layers", "Enforce the crate dependency layering (plan/architecture.md §3)", gates::layers),
    ("wasm", "cargo check --target wasm32-unknown-unknown for every crate below L8", gates::wasm),
    ("deny", "Dependency licences, bans, sources and advisories (deny.toml; needs cargo-deny)", gates::deny),
    ("ci", "fmt --check, clippy -D warnings, test, layers, wasm, assets, deny, parity (stops at first failure)", gates::ci),
    ("assets", "Enforce the asset policy (AGENTS.md §1) against ATTRIBUTION.toml; --write regenerates ATTRIBUTION.md", assets::run),
    ("corpus", "Fetch the pinned pdf.js test corpus into corpus/ (git-ignored) and verify it; --update-pin re-records the pin", gates::corpus),
    ("check", "Robustness sweep over corpus/ with pdfcraft-cli; fails on crashes or regressions vs xtask/baselines", gates::check),
    ("fuzz", "Mutation fuzzing of open/render/edit/save in child processes; findings in fuzz-out/ (--time 300)", fuzz::run),
    ("parity", "Validate parity/acrobat-features.toml against the registry, tools and tests; report progress (--json, --partial)", parity::run),
    (
        "macos-integration",
        "Report macOS bundle, Quick Look, Spotlight and PDF integration diagnostics without mutating system state",
        macos_integration::run,
    ),
    ("text-oracle", "Compare text extraction with pdftotext over corpus/ (word F1; target median ≥ 0.97)", gates::text_oracle),
    ("screenshots", "Regenerate the README screenshots in docs/images/ and their ATTRIBUTION entries", screenshots::run),
    ("models", "Fetch the OCR models (ATTRIBUTION.toml kind = \"model\") into assets/models/, verified by SHA-256", assets::models),
    (
        "trust-lists",
        "Write dist/trust/eutl-qualified-ca.der (or --out FILE) from the EU Trusted Lists: a trust list to load with `sign_trust eu_trusted_list` (needs network and curl)",
        trust_lists::run,
    ),
    (
        "trust-roots",
        "Rebuild crates/sign/data/builtin-roots.der from builtin-roots.toml: fetch each root from its CA and check its pinned SHA-256 (needs network and curl)",
        trust_roots::run,
    ),
    ("demo-pdf", "Build dist/demo/pdfcraft-showcase.pdf (needs Google Chrome or Chromium)", demo_pdf::run),
];

fn version_cmd(args: &[String]) -> anyhow::Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    version::run(&gates::root(), &args).map_err(anyhow::Error::msg)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = args.first() else {
        print_help();
        return ExitCode::FAILURE;
    };
    if matches!(name.as_str(), "help" | "-h" | "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }
    let Some((_, _, command)) = COMMANDS.iter().find(|(n, _, _)| n == name) else {
        eprintln!("xtask: unknown command `{name}`\n");
        print_help();
        return ExitCode::FAILURE;
    };
    match command(&args[1..]) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask {name}: error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    eprintln!("Usage: cargo xtask <command> [args]\n\nCommands:");
    for (name, about, _) in COMMANDS {
        eprintln!("  {name:<12} {about}");
    }
    eprintln!("  {:<12} Show this list", "help");
}
