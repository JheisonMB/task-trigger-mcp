#!/bin/sh
# install.sh — download and install task-trigger-mcp from GitHub Releases
# Usage: curl -fsSL https://raw.githubusercontent.com/JheisonMB/task-trigger-mcp/main/install.sh | sh
set -eu

REPO="JheisonMB/task-trigger-mcp"
BINARY="task-trigger-mcp"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

info() { printf '  \033[1;34m%s\033[0m %s\n' "$1" "$2"; }
error() { printf '  \033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }

# --- detect OS ---
OS="$(uname -s)"
case "$OS" in
  Linux*)  OS_TARGET="unknown-linux-musl" ;;
  Darwin*) OS_TARGET="apple-darwin" ;;
  *)       error "Unsupported OS: $OS (only Linux and macOS are supported)" ;;
esac

# --- detect arch ---
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)   ARCH_TARGET="x86_64" ;;
  arm64|aarch64)
    if [ "$OS" = "Darwin" ]; then
      ARCH_TARGET="aarch64"
    else
      error "aarch64 Linux builds are not available yet"
    fi
    ;;
  *)               error "Unsupported architecture: $ARCH" ;;
esac

TARGET="${ARCH_TARGET}-${OS_TARGET}"
info "platform" "$TARGET"

# --- resolve latest version ---
if [ -n "${VERSION:-}" ]; then
  TAG="v$VERSION"
  info "version" "$TAG (pinned)"
else
  TAG=$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" | rev | cut -d'/' -f1 | rev)
  [ -z "$TAG" ] && error "Could not resolve latest release tag"
  info "version" "$TAG (latest)"
fi

# --- download ---
ARCHIVE="${BINARY}-${TAG}-${TARGET}.tar.gz"
URL="https://github.com/$REPO/releases/download/${TAG}/${ARCHIVE}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

info "download" "$URL"
HTTP_CODE=$(curl -fSL -w '%{http_code}' -o "$TMPDIR/$ARCHIVE" "$URL" 2>/dev/null) || true
[ "$HTTP_CODE" = "200" ] || error "Download failed (HTTP $HTTP_CODE). Check that $TAG exists for $TARGET at:\n  $URL"

# --- extract ---
tar xzf "$TMPDIR/$ARCHIVE" -C "$TMPDIR"
[ -f "$TMPDIR/$BINARY" ] || error "Binary not found in archive"

# --- install ---
mkdir -p "$INSTALL_DIR"
mv "$TMPDIR/$BINARY" "$INSTALL_DIR/$BINARY"
chmod +x "$INSTALL_DIR/$BINARY"
info "installed" "$INSTALL_DIR/$BINARY"

# --- verify PATH ---
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    info "note" "$INSTALL_DIR is not in your PATH"
    info "" "Add this to your shell profile:"
    info "" "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac

info "done" "$($INSTALL_DIR/$BINARY --version 2>/dev/null || echo "$BINARY installed")"
