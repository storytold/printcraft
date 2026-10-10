# Releasing PdfCraft

Every push to the `release` branch runs `.github/workflows/release.yml`. It builds installers for
macOS, Windows, Linux, FreeBSD and the web, signs the ones it has certificates for, and creates or
updates a **draft** GitHub Release named `PdfCraft v<version>`. Nobody sees a draft until a
maintainer publishes it.

The pipeline was ported from PhotoCraft's. User-facing names say **PdfCraft**; files, binaries and
ids stay lowercase (`pdfcraft-<version>-<platform>-<arch>.<ext>`, `ai.storyteller.pdfcraft`).

## Cutting a release

1. **Bump the version** on `main`. It lives only in `[workspace.package] version` in the root
   `Cargo.toml`:

   ```sh
   cargo xtask version                 # prints the current version
   cargo xtask version set 0.3.0       # or 0.3.0-rc.1; updates Cargo.toml and Cargo.lock
   ```

   Commit the change through the normal review flow (see the `Release: PdfCraft v0.2.1` PR).
2. **Merge `main` into `release`** (or fast-forward it) and push. The workflow starts by itself.
3. **Wait for the draft.** When every job has finished (notarization is the slow part), the
   Releases page has a draft `PdfCraft v0.3.0` targeting the pushed commit, with every artifact
   and `SHA256SUMS.txt`. The notes are generated from the merged PRs.
4. **Check it.** Download an installer or two and read the job summaries. A `::warning::` there
   means a signing secret was missing and that artifact is unsigned.
5. **Publish** the draft in the GitHub UI. Publishing creates the `v0.3.0` tag. Versions with a
   pre-release suffix (`-rc.1`) are marked as pre-releases.

Pushing to `release` again before you publish rebuilds the same draft and replaces its assets.
Once the draft is published, the workflow refuses to touch that version again, so bump it first.

**Test runs:** *Actions ▸ Release ▸ Run workflow* runs the whole pipeline by hand. The optional
`version` input (such as `0.3.0-rc.1`) overrides `Cargo.toml` for that run only; each job applies it
with `cargo xtask version set` before building, so the binaries report it too. The signing jobs
(macOS, Windows) and the draft-release job use the `release` environment, so pick the `release`
branch in the dialog for a full run. On any other branch the same dispatch is a **dry run**: the
Linux, Flatpak, FreeBSD and web jobs build and check everything, the signing jobs are refused by the
environment's branch rule, and the draft-release job (which needs them) is skipped, so nothing is
published (`gh workflow run release.yml --ref <branch>`).

## What gets built

| Platform | Artifacts | Built on |
|---|---|---|
| macOS 11+ (universal: Apple silicon + Intel) | `pdfcraft-<v>-macos-universal.dmg`, `pdfcraft-cli-<v>-macos-universal.zip` | `macos-15` |
| Windows 10+ x64 | `pdfcraft-<v>-windows-x64.msi`, `pdfcraft-<v>-windows-x64-portable.zip` | `windows-latest` |
| Windows 10+ x86 (32-bit) | `pdfcraft-<v>-windows-x86.msi`, `pdfcraft-<v>-windows-x86-portable.zip` | `windows-latest` |
| Windows 11 on ARM64 | `pdfcraft-<v>-windows-arm64.msi`, `pdfcraft-<v>-windows-arm64-portable.zip` | `windows-latest` (cross-compiled) |
| Linux x86_64 | `pdfcraft-<v>-linux-x86_64.{AppImage,AppImage.zsync,deb,rpm,tar.gz}`, `pdfcraft-cli-<v>-linux-x86_64.tar.gz` | `ubuntu-22.04` |
| Linux aarch64 | `pdfcraft-<v>-linux-aarch64.{AppImage,AppImage.zsync,deb,rpm,tar.gz}`, `pdfcraft-cli-<v>-linux-aarch64.tar.gz` | `ubuntu-22.04-arm` |
| Linux riscv64 | `pdfcraft-<v>-linux-riscv64.tar.gz`, `pdfcraft-cli-<v>-linux-riscv64.tar.gz` (glibc 2.39+) | `ubuntu-24.04` (cross-compiled; CLI smoke-tested under QEMU) |
| Flatpak x86_64 | `pdfcraft-<v>-linux-x86_64.flatpak` | `ubuntu-24.04` (repackages the Linux tarball) |
| Flatpak aarch64 | `pdfcraft-<v>-linux-aarch64.flatpak` | `ubuntu-24.04-arm` (repackages the Linux tarball) |
| FreeBSD 14 x86_64 | `pdfcraft-<v>-freebsd-x86_64.tar.gz` | FreeBSD VM on `ubuntu-latest` |
| Web | `pdfcraft-web-<v>.zip` (a static site; see [`packaging/web/README.md`](../packaging/web/README.md)) | `ubuntu-latest` |

The ARM64 Windows build is cross-compiled on the x64 runner, so signing and WiX work as for the
other Windows builds; `.github/workflows/windows-arm64.yml` installs and runs it on ARM64 hardware.

Every binary reports its version (`pdfcraft --version`, `pdfcraft-cli --version`, *Help ▸ About*).
The workflow sets `PDFCRAFT_BUILD_SHA` and `PDFCRAFT_BUILD_DATE` (`packaging/env.sh` fills them in
for local builds): the commit is recorded in the macOS `Info.plist` (`PdfCraftBuildCommit`) and the
date in the AppStream metadata. The binaries don't embed the commit yet.

**Fonts:** every job checks out [craft-fonts](https://github.com/storytold/craft-fonts) at the commit
pinned in `release.yml` and builds with `CRAFT_FONTS_DIR` and `CRAFT_FONTS_REQUIRED=1`, so releases
embed its Japanese fonts and fail rather than ship without them (`AGENTS.md` §1.4). Bump the pin
deliberately.

**OCR models:** every desktop package ships the Scan & OCR models (#103). The packaging scripts call
`stage_models` (`packaging/env.sh`; `package.ps1` on Windows), which runs `cargo xtask models` to
fetch every `kind = "model"` file in `ATTRIBUTION.toml` (verified by SHA-256) with its licence text
and an `ATTRIBUTION.txt`, straight into the package, so packaging needs the network. They go where
`pdfcraft_ocr::Models::dirs_beside_exe` looks: `models\` beside `pdfcraft.exe` (MSI, portable zip),
`PdfCraft.app/Contents/Resources/models` (macOS), and `share/pdfcraft/models` beside `bin/`
(deb, rpm, tar.gz, AppImage, Flatpak, FreeBSD). The web build doesn't include them yet.

### macOS

`packaging/macos/package.sh` builds both architectures, joins them with `lipo`, and assembles
`PdfCraft.app` from `Info.plist.in` (bundle id `ai.storyteller.pdfcraft`, macOS 11+, PDF declared
as a document type with rank Alternate, so PdfCraft is offered under Open With without taking over
from Preview). Files opened from Finder arrive as Apple events, which
`apps/pdfcraft/src/apple_events.rs` receives.

- **Signing** uses the hardened runtime and a secure timestamp, executable first, then the bundle.
  `packaging/macos/import-cert.sh` puts the certificate in a temporary keychain, deleted at the end
  of the job.
- **Notarization:** the app is zipped and sent with `xcrun notarytool submit --wait`, the ticket is
  stapled, and the result is checked with `codesign --verify`, `stapler validate` and `spctl`. The
  app ships on a drag-to-Applications DMG, which is signed and notarized too.
  Its Finder window (background, icon size and positions) comes from
  [`packaging/macos/dmg/`](../packaging/macos/dmg/README.md), and its volume is named `PdfCraft`
  without the version, which the window's background needs; the DMG file name keeps the version.
- **CLI:** `pdfcraft-cli` is signed and notarized as a zip. A bare executable can't hold a stapled
  ticket, so Gatekeeper looks it up online the first time a downloaded copy runs.

Without certificates (locally) the script signs ad-hoc and skips notarization:

```sh
packaging/macos/package.sh --arch aarch64     # quicker, host-only; --arch universal needs both targets
```

### Windows

`packaging/windows/package.ps1 -Arch x64|x86|arm64` builds with `-C target-feature=+crt-static`, so
neither the MSI nor the portable zip needs the Visual C++ redistributable.

- Before packaging, it reads both executables' PE headers: the machine type must match `-Arch`, and
  `pdfcraft.exe` must be a GUI-subsystem program (no console window, #57) while `pdfcraft-cli.exe`
  stays a console program.
- `pdfcraft.wxs` (WiX v5) installs per machine into Program Files with a Start Menu shortcut, a
  desktop shortcut (on by default) and an App Paths entry. Both shortcuts are plain links to
  `pdfcraft.exe`, not advertised MSI shortcuts (#107, #143). The MSI version is the numeric `X.Y.Z`
  (MSI has no pre-release field), and same-version upgrades are allowed so release candidates replace
  each other. Icon ids end in `.ico` or `.exe` (Windows Installer requires it); packaging-lint checks
  this.
- `installer-ui.wxs` supplies native welcome, maintenance, progress, files-in-use and outcome
  dialogs. Full UI confirms success with Finish; failures and cancellations have distinct messages.
  The welcome dialog has a "Create a desktop shortcut" checkbox, ticked by default. `/qn` and `/qb`
  stay unattended and create the desktop shortcut unless `INSTALLDESKTOPSHORTCUT=0` is passed.
- `test-msi.ps1 <file.msi>` checks the compiled shortcut and dialog tables. Packaging runs it in a
  child process before signing, so the MSI isn't held open when signtool runs. The ARM64 install
  smoke test checks that both all-users shortcuts point at the installed `pdfcraft.exe` and are
  removed on uninstall, and that `INSTALLDESKTOPSHORTCUT=0` skips the desktop one.
- The portable zip holds both executables, the README, the licences, the OFL licence of each
  embedded craft-fonts family, and `portable.txt`. That marker beside `pdfcraft.exe` keeps the
  settings, logs, crash recovery and new digital IDs in `PdfCraftData\` next to the exe instead of
  `%APPDATA%` and `%LOCALAPPDATA%` (`crates/ui-egui/src/portable.rs`).
- **Signing:** `packaging/windows/sign.ps1` signs both executables and the MSI with `signtool`
  (SHA-256, RFC 3161 timestamp), from a `.pfx` (`WINDOWS_CERTIFICATE`) or Azure Trusted Signing
  (`AZURE_*`), whichever is configured. It is the one place to change when signing changes.

Locally: `dotnet tool install -g wix --version 5.0.2`, then `pwsh packaging/windows/package.ps1 -Arch x64`.

### Linux

`packaging/linux/package.sh` stages one FHS tree (both binaries, the desktop entry, hicolor icons,
AppStream metainfo) and makes every format from it: an **AppImage** (any distribution, nothing to
install), a **.deb** and an **.rpm** (built with [nfpm](https://nfpm.goreleaser.com) from
`nfpm.yaml`; they integrate with the menu, MIME and icon caches), a **.tar.gz**, and a CLI-only
**`pdfcraft-cli-<v>-linux-<arch>.tar.gz`** (the stripped `pdfcraft-cli` plus the licences and
README, for servers, CI and agent sandboxes). `--formats` picks a subset (`appimage deb rpm tar cli`).

The binaries are built on Ubuntu 22.04, the oldest GitHub-hosted image, so they need only
**glibc ≥ 2.35**: Ubuntu 22.04+, Debian 12+, Fedora 36+, RHEL 10. Windowing (X11, Wayland,
xkbcommon) and the GPU (Vulkan, EGL) are loaded at runtime from the system; the .deb and .rpm declare
them as dependencies (see `nfpm.yaml`).

Each AppImage embeds update information
(`gh-releases-zsync|storytold|pdfcraft|latest|pdfcraft-*-linux-<arch>.AppImage.zsync`), and its
`.zsync` is published beside it, so AppImageUpdate and AppImageLauncher can update it in place,
downloading only the changed blocks. `latest` is the newest published, non-pre-release version.
`package.sh` writes the `.zsync` when `zsyncmake` (the `zsync` package) is installed and warns
otherwise; the release job checks both.

**Flatpak:** the `flatpak` job turns each arch's tarball into a single-file bundle with
`packaging/linux/flatpak-bundle.sh` and `flatpak/ai.storyteller.pdfcraft.bundle.yml` (no Rust build;
the same binaries), then installs it and runs `pdfcraft-cli --version` in the sandbox.
`flatpak/ai.storyteller.pdfcraft.yml` is the from-source manifest for a later Flathub submission;
packaging-lint keeps the runtime and sandbox permissions (`finish-args`) of the two identical.
Printing is not available in the Flatpak yet: it runs `lp`, which the freedesktop runtime lacks
(the job logs a note); it needs the print portal. Users install the bundle with `flatpak install --user pdfcraft-<v>-linux-<arch>.flatpak`; the
freedesktop runtime comes from Flathub.

Locally (on Linux, with nfpm): `packaging/linux/package.sh` or `--formats "deb tar"`; then
`packaging/linux/flatpak-bundle.sh` for the Flatpak.

### FreeBSD

GitHub has no FreeBSD runners, so the `freebsd` job runs `packaging/freebsd/package.sh` in a FreeBSD
VM with the packages from `.github/workflows/freebsd.yml`. The tarball is a `/usr/local`-style tree
with the same desktop entry, metainfo and icons as Linux. FreeBSD has no code signing for loose
binaries; check the tarball against `SHA256SUMS.txt`.

### Web

`packaging/web/package.sh` runs `trunk build --release` in `apps/pdfcraft-web` and zips the site
with sample `_headers` and `.htaccess` files. Hosting (MIME types, compression, caching, iframes) is
covered in [`packaging/web/README.md`](../packaging/web/README.md).

## Secrets

The signing secrets live in the repository's **`release` environment** (*Settings ▸ Environments ▸
release*), restricted to the `release` branch; every job in `release.yml` declares
`environment: release`. Each secret is optional: if one is missing, that platform's artifacts are
unsigned and the run shows a warning.

| Secret | Used for |
|---|---|
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` | base64 Developer ID Application `.p12` and its password |
| `KEYCHAIN_PASSWORD` | the temporary CI keychain (random if unset) |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | `notarytool` (an app-specific password) |
| `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` | base64 `.pfx` code-signing certificate and its password |
| `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` | service principal for Azure Trusted Signing (instead of a `.pfx`) |
| `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE` | Trusted Signing endpoint, account and certificate profile |

`GITHUB_TOKEN` creates the release; only the final job gets `contents: write`.

## Checks

`.github/workflows/packaging-lint.yml` runs on changes to `packaging/` and the workflows: actionlint, shellcheck,
a PowerShell parse, xmllint (WiX, plist, MIME), `desktop-file-validate`, `appstreamcli validate`, and
the MSI icon-id check. The FreeBSD and Windows ARM64 workflows also exercise their packaging before a
release does.

## Scanner detections

Antivirus and reputation scanners sometimes flag a released file. Check the facts before you change
code or answer publicly. The first report was [issue #747](https://github.com/storytold/pdfcraft/issues/747),
a match of the `CobaltStrikeBeacon` family rule.

1. **Confirm the file is the release.** Compare its SHA-256 with `SHA256SUMS.txt` on the GitHub
   Release. Only a file whose hash matches the release says anything about that release.
2. **Record the signature.** `packaging/windows/security-audit.ps1` prints the Authenticode status:
   - `Valid`: the signature verifies.
   - `NotSigned`: no signature, as in developer and test builds. Not evidence of compromise by itself.
   - `HashMismatch`: the file changed after it was signed. Investigate.
   - `NotTrusted` or `UnknownError`: a signature exists but did not validate on this machine. Compare
     the signer and the hash with the release.
3. **Extract the payloads without running them.** Scan the executables inside the MSI and the
   portable zip, not only the outer installer. `7z x -oDIR pdfcraft-<version>-windows-x64.msi`
   extracts them. The payload names are WiX-internal, so tell the executables apart by their file
   type. Don't install or run the MSI or the extracted files.
4. **Scan with YARA and keep every match.** Use your own copies of YARA and the rule. PdfCraft
   bundles neither, and the helper never runs in the app, the installer or CI:

   ```powershell
   pwsh packaging/windows/security-audit.ps1 -Path .\pdfcraft.exe, .\pdfcraft-cli.exe -YaraExe C:\tools\yara64.exe -YaraRule C:\rules\CobaltStrikeBeacon.yar
   ```

   The output lists each matched identifier with its offset, and the SHA-256 of the rule file.
   Record where the rule came from and its revision. For CAPEv2's `CobaltStrikeBeacon` rule, that is
   the commit that last changed `data/yara/CAPE/CobaltStrikeBeacon.yar`.
5. **Find the condition branch.** Family rules usually have alternative branches, such as
   `all of ($x*) or 2 of ($a*) or ...`. Copy the rule, make one rule per branch with only the strings
   it uses, and scan again to see which branch fired.
6. **Interpret with care.** A match on generic repeated-byte patterns says much less than a match on
   distinctive code or strings, and it does not show that the software belongs to the family. Rules
   change too: CAPEv2 commit `9b599185` (2024-12-05) lengthened this rule's two repeated-byte patterns
   from 4 to 16 bytes to reduce false positives, so an older rule revision can match files that the
   current one does not. Don't alter PdfCraft's binaries to change a scanner's result. That changes
   nothing about the software.
7. **Report.** To the scanner, send the file's SHA-256, the release URL, the rule name and revision,
   and the matched identifiers and offsets. Report PdfCraft problems at
   <https://github.com/storytold/pdfcraft/issues> with the same facts.
