#!/usr/bin/env sh
set -e

REPO="Yatsuiii/api--causality-engine"
BIN="ace"

# Detect OS and arch
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  TARGET="ace-linux-x86_64.tar.gz" ;;
      aarch64) TARGET="ace-linux-aarch64.tar.gz" ;;
      *) echo "Unsupported architecture: $ARCH" && exit 1 ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64)  TARGET="ace-macos-x86_64.tar.gz" ;;
      arm64)   TARGET="ace-macos-aarch64.tar.gz" ;;
      *) echo "Unsupported architecture: $ARCH" && exit 1 ;;
    esac
    ;;
  *) echo "Unsupported OS: $OS" && exit 1 ;;
esac

# Get latest release tag
TAG="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | cut -d'"' -f4)"
if [ -z "$TAG" ]; then
  echo "Could not fetch latest release tag" && exit 1
fi

URL="https://github.com/$REPO/releases/download/$TAG/$TARGET"

echo "Installing ace $TAG for $OS/$ARCH..."
curl -fsSL "$URL" | tar -xz
chmod +x "$BIN"

INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
if [ -w "$INSTALL_DIR" ]; then
  mv "$BIN" "$INSTALL_DIR/$BIN"
else
  sudo mv "$BIN" "$INSTALL_DIR/$BIN"
fi

echo "ace $TAG installed to $INSTALL_DIR/$BIN"
echo "Run: ace --help"
