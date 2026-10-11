# Rust SDK examples

## Prerequisites

Rust **1.90+** via `rustup`. Nothing else is required: previews are rendered by the PdfCraft
engine itself through the SDK (`LocalClient::render_page`), not an external tool.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal -c clippy -c rustfmt
echo '. "$HOME/.cargo/env"' >> ~/.zshrc
source "$HOME/.cargo/env"
cargo --version
```

`--no-modify-path` avoids the installer failing when it cannot write `~/.bash_profile`
(`could not amend shell profile ... Permission denied`).

Broken install (`error: missing manifest in toolchain` or `no default is configured`)?

```bash
rustup toolchain uninstall stable-aarch64-apple-darwin   # name may differ on your machine
rm -rf ~/.rustup/toolchains/stable-*
rustup toolchain install stable --profile minimal -c clippy -c rustfmt
rustup default stable
```

## Run

From the repository root. The first build compiles the whole engine and takes several minutes;
later runs take seconds.

```bash
cargo run -p pdfcraft-sdk --example sdk_capability_tour
cargo run -p pdfcraft-sdk --example sdk_capability_tour -- --render-previews
cargo run -p pdfcraft-sdk --example sdk_capability_tour -- --output sample-docs/outputs/my.pdf
open sample-docs/outputs/sdk_capability_tour_rs.pdf          # macOS
```

The example is registered in `crates/sdk/Cargo.toml` (`[[example]]`), which points at
`docs/sdk/examples/rust/sdk_capability_tour.rs`.

## What it shows

A 5-page PDF built from the SDK's domain types: Bezier `Path`s and `Matrix` transforms, CMYK
and RGB `Color`, `BlendMode` transparency, `Annotation`/`Action`, `Field`, `Bookmark`. The SDK has
no document writer, so the example includes a small PDF serializer. Page 5 reports **measured**
`LocalClient` results (`render_page`, `split`, `merge`, `SdkError` codes) gathered by running the
document through the engine.

## Checks

```bash
cargo test -p pdfcraft-sdk
cargo clippy -p pdfcraft-sdk --all-targets -- -D warnings
cargo fmt -p pdfcraft-sdk --check
```
