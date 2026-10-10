#!/usr/bin/env bash
# Build and package PdfCraft for Linux (<arch> is x86_64 or aarch64, or riscv64 when cross-compiling):
#
#   $DIST/pdfcraft-<version>-linux-<arch>.AppImage  any distro with glibc >= the build host's
#   $DIST/pdfcraft-<version>-linux-<arch>.AppImage.zsync  delta updates (needs zsyncmake)
#   $DIST/pdfcraft-<version>-linux-<arch>.deb       Debian, Ubuntu, Mint, Pop!_OS, ...
#   $DIST/pdfcraft-<version>-linux-<arch>.rpm       Fedora, openSUSE, RHEL, ...
#   $DIST/pdfcraft-<version>-linux-<arch>.tar.gz    plain FHS-style tree (bin/, share/)
#   $DIST/pdfcraft-cli-<version>-linux-<arch>.tar.gz  the headless CLI alone (servers, CI, agents)
#
# Cross-compiling (CI: riscv64, tar only): set CROSS_ARCH (riscv64), CROSS_TARGET (the Rust target),
# CROSS_COMPILE (binutils prefix, for strip) and optionally EMULATOR (e.g. qemu-riscv64) to
# smoke-test the CLI.
#
# Usage: packaging/linux/package.sh [--skip-build] [--formats "appimage deb rpm tar cli"]
#
# Needs: cargo, curl and the network (the OCR models: cargo xtask models); nfpm for deb/rpm (https://nfpm.goreleaser.com); appimagetool for the AppImage
# (downloaded into $CARGO_TARGET_DIR if missing). Build on an old distro (CI: Ubuntu 22.04,
# glibc 2.35) so the binaries run on newer ones. Optional: desktop-file-validate, appstreamcli,
# zsyncmake (the zsync package) for the AppImage's .zsync.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"
APP_ID=ai.storyteller.pdfcraft

SKIP_BUILD=0
FORMATS="appimage deb rpm tar cli"
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="$2"; shift 2 ;;
    -h | --help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="${CROSS_ARCH:-$(uname -m)}"
case "$ARCH" in
  riscv64) DEB_ARCH=riscv64 ;;
  x86_64) DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
export PDFCRAFT_MAINTAINER="${PDFCRAFT_MAINTAINER:-PdfCraft maintainers <pdfcraft@storyteller.ai>}"
BASENAME="pdfcraft-$VERSION-linux-$ARCH"

echo "==> PdfCraft $VERSION for Linux $ARCH ($FORMATS)"

if [ "$SKIP_BUILD" = 0 ]; then
  if [ -n "${CROSS_TARGET:-}" ]; then
    (cd "$ROOT" && cargo build --release --locked -p pdfcraft -p pdfcraft-cli --target "$CROSS_TARGET")
  else
    (cd "$ROOT" && cargo build --release --locked -p pdfcraft -p pdfcraft-cli)
  fi
fi
BIN="$CARGO_TARGET_DIR/${CROSS_TARGET:+$CROSS_TARGET/}release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- stage an FHS tree (shared by every format) -------------------------------------------------
install -Dm755 "$BIN/pdfcraft" "$STAGE/usr/bin/pdfcraft"
install -Dm755 "$BIN/pdfcraft-cli" "$STAGE/usr/bin/pdfcraft-cli"
"${CROSS_COMPILE:-}strip" "$STAGE/usr/bin/pdfcraft" "$STAGE/usr/bin/pdfcraft-cli" 2>/dev/null || true
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$HERE/$APP_ID.mime.xml" "$STAGE/usr/share/mime/packages/$APP_ID.xml"
mkdir -p "$STAGE/usr/share/metainfo"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$PDFCRAFT_BUILD_DATE/g" \
  "$HERE/$APP_ID.metainfo.xml.in" >"$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
mkdir -p "$STAGE/usr/share/icons"
cp -R "$ROOT/assets/app-icon/hicolor" "$STAGE/usr/share/icons/"
mkdir -p "$STAGE/usr/share/doc/pdfcraft"
copy_docs "$STAGE/usr/share/doc/pdfcraft"
# OCR models: the app finds them at <bin>/../share/pdfcraft/models, so in every format below.
stage_models "$STAGE/usr/share/pdfcraft/models"

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi
if command -v appstreamcli >/dev/null; then
  appstreamcli validate --no-net --explain "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
fi

has() { case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# ---- .tar.gz ------------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi

# ---- CLI-only .tar.gz ---------------------------------------------------------------------------
# The stripped pdfcraft-cli (and its opt-in MCP server) with the licences, for machines that never
# open a window. Like the other formats it needs glibc >= the build host's.
if has cli; then
  CLI_NAME="pdfcraft-cli-$VERSION-linux-$ARCH"
  CLI_DIR="$WORK/cli/$CLI_NAME"
  mkdir -p "$CLI_DIR"
  cp "$STAGE/usr/bin/pdfcraft-cli" "$CLI_DIR/"
  copy_docs "$CLI_DIR"
  tar -C "$WORK/cli" -czf "$DIST/$CLI_NAME.tar.gz" "$CLI_NAME"
  echo "wrote $DIST/$CLI_NAME.tar.gz"
fi

# ---- .deb / .rpm --------------------------------------------------------------------------------
if has deb || has rpm; then
  command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/install/)" >&2; exit 1; }
  export VERSION
  export NFPM_ARCH="$DEB_ARCH"
  # nfpm expands env vars in fields like `version` and `arch`, but not in `contents[].src`.
  sed "s|\${STAGE}|$STAGE|g" "$HERE/nfpm.yaml" >"$WORK/nfpm.yaml"
  for fmt in deb rpm; do
    if has "$fmt"; then (cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt"); fi
  done
fi

# ---- AppImage -----------------------------------------------------------------------------------
if has appimage; then
  APPDIR="$WORK/PdfCraft.AppDir"
  cp -R "$STAGE" "$APPDIR"
  mv "$APPDIR/usr/share/doc" "$WORK/doc-unused"
  ln -s usr/bin/pdfcraft "$APPDIR/AppRun"
  cp "$HERE/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
  cp "$ROOT/assets/app-icon/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
  ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [ -z "$TOOL" ]; then
    TOOL="$CARGO_TARGET_DIR/appimagetool-$ARCH.AppImage"
    if [ ! -x "$TOOL" ]; then
      curl -fsSL -o "$TOOL" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
      chmod +x "$TOOL"
    fi
  fi
  # Absolute, because appimagetool runs in $DIST below (CARGO_TARGET_DIR or APPIMAGETOOL may be
  # relative, e.g. target/agent-<name>).
  OUT="$(cd "$DIST" && pwd)/$BASENAME.AppImage"
  APPDIR="$(cd "$APPDIR" && pwd)"
  TOOL="$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")"
  # A .zsync left by an earlier run would hide a missing zsyncmake and describe another file.
  rm -f "$OUT.zsync"
  # Update information: AppImageUpdate, AppImageLauncher and the like read it from the file and
  # fetch only the blocks that changed in a newer release, through the .zsync published next to
  # each AppImage on GitHub Releases. `latest` is the newest published release that is not a
  # pre-release. A fork's builds point at its own releases through GITHUB_REPOSITORY.
  REPO="${GITHUB_REPOSITORY:-storytold/pdfcraft}"
  UPDATE_INFO="gh-releases-zsync|${REPO%%/*}|${REPO#*/}|latest|pdfcraft-*-linux-$ARCH.AppImage.zsync"
  # Extract-and-run: works without FUSE (containers, CI). The output embeds the static runtime,
  # so users don't need libfuse2 either. With zsyncmake on the host (CI installs the zsync
  # package) appimagetool also writes the .zsync, into its working directory, hence the cd.
  (cd "$DIST" && ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream -u "$UPDATE_INFO" "$APPDIR" "$OUT")
  echo "wrote $OUT"
  if [ -s "$OUT.zsync" ]; then
    echo "wrote $OUT.zsync"
  else
    warn "zsyncmake not found, so $OUT.zsync was not written; AppImage delta updates need it"
  fi
fi

if [ -z "${CROSS_TARGET:-}" ]; then
  "$STAGE/usr/bin/pdfcraft-cli" --version
elif [ -n "${EMULATOR:-}" ]; then
  "$EMULATOR" "$STAGE/usr/bin/pdfcraft-cli" --version
fi
echo "==> done"
ls -lh "$DIST"
